//! Ctrl+C.
//!
//! A terminal sends SIGINT to the whole foreground process group, so a running
//! child already receives it and dies on its own. What the runner has to add is
//! the three things that do not happen by themselves: stop walking rather than
//! treating the child's death as an ordinary failure, delete the temp files the
//! run created, and exit 130 so a caller can tell an interrupt from a failure.
//!
//! The flag is process-global because a signal handler has nowhere else to
//! write, and because `.parallel` branches all need to see it.

use std::sync::atomic::{AtomicBool, Ordering};

/// What a shell reports for a process killed by SIGINT, and what the old
/// implementation used.
pub const EXIT_CODE: i32 = 130;

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Whether Ctrl+C has been pressed since the process started.
pub fn interrupted() -> bool {
	INTERRUPTED.load(Ordering::SeqCst)
}

/// Set the flag. Public so tests can exercise everything downstream of it
/// without raising a real signal, which would take the test runner with it.
pub fn set_interrupted() {
	INTERRUPTED.store(true, Ordering::SeqCst);
}

/// Clear it, for a watch-mode iteration and for tests.
pub fn clear() {
	INTERRUPTED.store(false, Ordering::SeqCst);
}

/// Ask the OS to tell us about Ctrl+C.
///
/// Best effort: if the handler cannot be installed the runner still works, it
/// just exits the way it would have anyway. Idempotent.
pub fn install() {
	static DONE: AtomicBool = AtomicBool::new(false);
	if DONE.swap(true, Ordering::SeqCst) {
		return;
	}
	install_inner();
}

#[cfg(unix)]
fn install_inner() {
	// Only an atomic store, which is async-signal-safe. Anything more -- a
	// print, an allocation -- is not, and this handler runs on whatever thread
	// the signal lands on.
	extern "C" fn on_sigint(_: libc::c_int) {
		INTERRUPTED.store(true, Ordering::SeqCst);
	}
	// SAFETY: `signal` with a plain extern "C" fn is the documented use, and
	// the handler touches nothing but one atomic.
	unsafe {
		libc::signal(libc::SIGINT, on_sigint as *const () as libc::sighandler_t);
	}
}

#[cfg(windows)]
fn install_inner() {
	use windows_sys::Win32::Foundation::{BOOL, FALSE, TRUE};
	use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler};

	unsafe extern "system" fn on_ctrl(kind: u32) -> BOOL {
		if kind == CTRL_C_EVENT || kind == CTRL_BREAK_EVENT {
			INTERRUPTED.store(true, Ordering::SeqCst);
			// Handled: Windows would otherwise terminate the process outright,
			// leaving the temp files behind.
			return TRUE;
		}
		FALSE
	}
	// SAFETY: the callback matches the documented signature and touches only
	// one atomic.
	unsafe {
		SetConsoleCtrlHandler(Some(on_ctrl), TRUE);
	}
}

#[cfg(not(any(unix, windows)))]
fn install_inner() {}

#[cfg(test)]
mod tests {
	use super::*;

	/// Serialised, because the flag is process-global.
	pub(crate) static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

	#[test]
	fn the_flag_round_trips() {
		let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
		clear();
		assert!(!interrupted());
		set_interrupted();
		assert!(interrupted());
		clear();
		assert!(!interrupted());
	}

	#[test]
	fn installing_twice_is_harmless() {
		install();
		install();
	}
}
