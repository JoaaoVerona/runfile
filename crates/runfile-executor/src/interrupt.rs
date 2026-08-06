//! Ctrl+C / SIGINT handling for the lifetime of a target run.
//!
//! Ctrl+C in a terminal delivers SIGINT to the whole *foreground process
//! group* — the shell child we spawned AND the `run` process itself. With the
//! default disposition, `run` dies on the spot, which means `when: failure` /
//! `when: always` cleanup blocks never get a chance to execute. That is the
//! bug this module fixes.
//!
//! [`InterruptGuard`] installs a handler that only *records* the interrupt.
//! The child still dies (a handler is reset to the default disposition across
//! `exec`, so children are unaffected), the executor observes its
//! signal-terminated exit status through the normal path, and the walker keeps
//! going: remaining `when: success` work is abandoned while `when: failure` /
//! `when: always` blocks still run.
//!
//! Two properties matter for the semantics to feel right:
//!
//! - **`ignoreErrors` must not swallow an interrupt.** A failed command can be
//!   forgiven; "the user asked to stop" cannot. So the abort signal lives here,
//!   as process state, rather than being folded into `WalkState::failed`.
//! - **Cleanup blocks must be exempt from their own abort.** While a
//!   `when: failure` / `when: always` body runs we push a [`CleanupScope`]; the
//!   gate consults [`should_abort`], which reads `false` inside that scope so
//!   the cleanup's own default-`when: success` children actually execute. The
//!   scope is thread-local so it flows into `@target` dependencies dispatched
//!   from inside a cleanup block without threading a flag through the whole
//!   [`crate::DependencyResolver`] signature.
//!
//! A **second** Ctrl+C exits the process immediately (code 130), so a hanging
//! cleanup block is always escapable.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Exit code for "terminated by SIGINT" (128 + 2), the shell convention.
pub const INTERRUPTED_EXIT_CODE: i32 = 130;

/// Set by the signal handler once the user has pressed Ctrl+C.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Live [`InterruptGuard`] count. Nested runs (and parallel batches, which
/// create their own guard so direct `execute_parallel` callers stay protected)
/// share a single installed handler: only the first guard installs, only the
/// last uninstalls. Mirrors the refcounting in [`crate::force_kill`].
static GUARD_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Latch so the "Interrupted" notice is printed exactly once per process.
static NOTICE_PRINTED: AtomicBool = AtomicBool::new(false);

thread_local! {
	/// Depth of nested `when: failure` / `when: always` bodies on this thread.
	static CLEANUP_DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// Whether the user has interrupted this run (Ctrl+C).
///
/// This stays `true` for the rest of the process — the CLI reads it after the
/// run returns to exit with [`INTERRUPTED_EXIT_CODE`] instead of the target's
/// own status.
pub fn interrupted() -> bool {
	INTERRUPTED.load(Ordering::SeqCst)
}

/// Whether the walker should stop starting new non-cleanup work.
///
/// Identical to [`interrupted`] except inside a [`CleanupScope`], where it
/// reads `false` so `when: failure` / `when: always` bodies run to completion.
pub(crate) fn should_abort() -> bool {
	abort_gate(interrupted(), in_cleanup())
}

/// Pure form of [`should_abort`], split out so the rule is unit-testable
/// without mutating the process-global interrupt flag (which sibling tests in
/// the same binary would observe).
fn abort_gate(interrupted: bool, in_cleanup: bool) -> bool {
	interrupted && !in_cleanup
}

/// Whether this thread is currently executing a cleanup block.
pub(crate) fn in_cleanup() -> bool {
	CLEANUP_DEPTH.with(|d| d.get() > 0)
}

/// Record an interrupt. Returns `true` when one had already been recorded
/// (i.e. this is a repeat Ctrl+C).
///
/// Async-signal-safe: a single atomic swap, nothing else. Also called by the
/// `forceKillOnSigInt` handler in [`crate::force_kill`], which replaces our
/// handler for the duration of such a target and would otherwise leave the
/// abort unrecorded.
pub(crate) fn mark_interrupted() -> bool {
	INTERRUPTED.swap(true, Ordering::SeqCst)
}

/// Print the one-line "interrupted" notice, at most once per process.
///
/// Called from the points that first *act* on the interrupt (a child coming
/// back signal-terminated, and the CLI's exit path) rather than from the signal
/// handler, where printing would not be async-signal-safe.
pub fn announce_interrupt() {
	if !NOTICE_PRINTED.swap(true, Ordering::SeqCst) {
		crate::logging::log_interrupted();
	}
}

/// RAII marker for "this thread is inside a `when: failure` / `when: always`
/// body". While one is alive [`should_abort`] reads `false`, so cleanup steps
/// run even though the run as a whole is aborting.
pub(crate) struct CleanupScope {
	_private: (),
}

impl CleanupScope {
	pub(crate) fn enter() -> Self {
		CLEANUP_DEPTH.with(|d| d.set(d.get() + 1));
		Self { _private: () }
	}

	/// Enter only when `cond` holds — the shape every call site wants, since
	/// blocks are gated on `when != success`.
	pub(crate) fn enter_if(cond: bool) -> Option<Self> {
		cond.then(Self::enter)
	}
}

impl Drop for CleanupScope {
	fn drop(&mut self) {
		CLEANUP_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
	}
}

/// RAII guard that keeps a Ctrl+C from killing this process outright.
///
/// Installed for the duration of a target run (see
/// [`crate::run_target_with_cwd`]) and around a parallel batch. Refcounted, so
/// nesting composes. On drop the previous disposition is restored — which is
/// what lets watch mode's idle "Ctrl+C to stop" keep working: outside a run
/// there is no guard, so the default handling applies.
pub struct InterruptGuard {
	_private: (),
}

impl Default for InterruptGuard {
	fn default() -> Self {
		Self::new()
	}
}

impl InterruptGuard {
	pub fn new() -> Self {
		if GUARD_COUNT.fetch_add(1, Ordering::SeqCst) == 0 {
			platform::install();
		}
		Self { _private: () }
	}
}

impl Drop for InterruptGuard {
	fn drop(&mut self) {
		if GUARD_COUNT.fetch_sub(1, Ordering::SeqCst) == 1 {
			platform::uninstall();
		}
	}
}

// ──── Unix implementation ────

#[cfg(unix)]
mod platform {
	use super::{INTERRUPTED_EXIT_CODE, mark_interrupted};
	use std::sync::Mutex;

	static PREV_HANDLER: Mutex<Option<libc::sighandler_t>> = Mutex::new(None);

	pub(super) fn install() {
		let prev = unsafe { libc::signal(libc::SIGINT, sigint_handler as *const () as libc::sighandler_t) };
		*PREV_HANDLER.lock().unwrap() = Some(prev);
	}

	pub(super) fn uninstall() {
		if let Some(prev) = PREV_HANDLER.lock().unwrap().take() {
			unsafe {
				libc::signal(libc::SIGINT, prev);
			}
		}
	}

	/// Async-signal-safe body: one atomic swap, plus `_exit` on a repeat.
	/// No locking, no allocation, no I/O.
	extern "C" fn sigint_handler(_sig: libc::c_int) {
		if mark_interrupted() {
			// Second Ctrl+C — the user is done waiting. `_exit` (not `exit`)
			// because atexit handlers are not async-signal-safe.
			unsafe { libc::_exit(INTERRUPTED_EXIT_CODE) };
		}
	}
}

// ──── Windows implementation ────

#[cfg(windows)]
mod platform {
	use super::{INTERRUPTED_EXIT_CODE, mark_interrupted};
	use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler};

	pub(super) fn install() {
		unsafe {
			SetConsoleCtrlHandler(Some(ctrl_handler), 1);
		}
	}

	pub(super) fn uninstall() {
		unsafe {
			SetConsoleCtrlHandler(Some(ctrl_handler), 0);
		}
	}

	unsafe extern "system" fn ctrl_handler(ctrl_type: u32) -> i32 {
		// Only Ctrl+C / Ctrl+Break are "the user asked to stop". Close / logoff
		// / shutdown events get passed through (return FALSE) so the OS default
		// teardown still happens — we must not stall a shutdown to run cleanup.
		if ctrl_type != CTRL_C_EVENT && ctrl_type != CTRL_BREAK_EVENT {
			return 0;
		}
		if mark_interrupted() {
			unsafe {
				windows_sys::Win32::System::Threading::ExitProcess(INTERRUPTED_EXIT_CODE as u32);
			}
		}
		// TRUE = handled: suppress the default terminate-the-process behaviour
		// so the walker can reach its `when: failure` / `when: always` blocks.
		1
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The cleanup scope must nest and fully unwind — an inner block leaving
	/// the depth non-zero would permanently disable abort gating for the rest
	/// of the run.
	#[test]
	fn cleanup_scope_nests_and_restores() {
		assert!(!in_cleanup());
		{
			let _outer = CleanupScope::enter();
			assert!(in_cleanup());
			{
				let _inner = CleanupScope::enter();
				assert!(in_cleanup());
			}
			assert!(in_cleanup(), "inner scope must not clear the outer one");
		}
		assert!(!in_cleanup());
	}

	#[test]
	fn cleanup_scope_enter_if_is_conditional() {
		assert!(CleanupScope::enter_if(false).is_none());
		assert!(!in_cleanup());
		let guard = CleanupScope::enter_if(true);
		assert!(guard.is_some());
		assert!(in_cleanup());
		drop(guard);
		assert!(!in_cleanup());
	}

	/// The gate the walker consults: an interrupt aborts pending work, except
	/// inside a cleanup block, where it must not (otherwise `when: always`
	/// bodies would skip their own default-`when: success` children — the very
	/// steps we interrupted the run to reach).
	#[test]
	fn abort_gate_is_suppressed_inside_cleanup() {
		assert!(!abort_gate(false, false));
		assert!(!abort_gate(false, true));
		assert!(abort_gate(true, false));
		assert!(!abort_gate(true, true));
	}
}
