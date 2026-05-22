use runfile_parser::CommandSpec;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

// ANSI escape codes — these work in all modern terminals including
// bash, zsh, fish, PowerShell 5.1+, PowerShell 7+, Windows Terminal,
// and cmd.exe on Windows 10+.
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";

// Windows console mode bits we require for our log output. Defined locally
// (rather than imported from `windows-sys`) so the pure mode-update helper
// below compiles and is unit-testable on every platform, not just Windows.
// `allow(dead_code)` on non-Windows because only `console_mode_update` (and
// the tests) reference them there.
#[cfg_attr(not(windows), allow(dead_code))]
const ENABLE_PROCESSED_OUTPUT: u32 = 0x0001;
#[cfg_attr(not(windows), allow(dead_code))]
const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

/// Given the current Windows console mode, return the mode that should be set,
/// or `None` if the bits we need are already on (so the caller can skip the
/// `SetConsoleMode` syscall).
///
/// We require BOTH:
/// - `ENABLE_PROCESSED_OUTPUT` — so `\n` line terminators act as newlines
///   instead of being rendered as the CP437 glyph `◙`.
/// - `ENABLE_VIRTUAL_TERMINAL_PROCESSING` — so ANSI color/style escapes render
///   as colors instead of as literal `←[..m` text.
///
/// We only ever OR these two bits in — unrelated mode flags are preserved.
/// Pure bit math, extracted from [`enable_ansi_support`] so the behaviour is
/// unit-testable without touching a real console handle.
#[cfg_attr(not(windows), allow(dead_code))]
fn console_mode_update(current: u32) -> Option<u32> {
	let desired = current | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING;
	if desired == current {
		None
	} else {
		Some(desired)
	}
}

/// Tracks the global step number across an entire run, so that nested
/// `@target` invocations and `when:` blocks share one continuous
/// `(N/total)` counter instead of restarting per call.
///
/// The total is computed up front by walking the dependency tree (see
/// `count_target_leaves` in `runner.rs`). Conditional `when: failure` /
/// `when: always` blocks inflate the total — actual execution may stop
/// before reaching them, in which case the last shown step number will
/// be lower than the total. That trade-off is acceptable.
///
/// Uses atomics + `Arc` so the counter can be shared across threads when
/// `parallel: true` targets spawn dependency invocations on worker threads.
/// `Clone` is shallow (cloning shares the same atomics) so all clones
/// observe the same `(N, total)` state.
#[derive(Debug, Clone)]
pub struct StepCounter {
	current: Arc<AtomicUsize>,
	total: Arc<AtomicUsize>,
}

impl StepCounter {
	pub fn new(total: usize) -> Self {
		Self {
			current: Arc::new(AtomicUsize::new(0)),
			total: Arc::new(AtomicUsize::new(total)),
		}
	}

	/// Advance the counter and return the (1-indexed step, total) pair
	/// to use in log output. Thread-safe.
	pub fn next_step(&self) -> (usize, usize) {
		let n = self.current.fetch_add(1, Ordering::SeqCst) + 1;
		let t = self.total.load(Ordering::SeqCst);
		(n, t)
	}

	pub fn total(&self) -> usize {
		self.total.load(Ordering::SeqCst)
	}

	/// Bump the total step count. Used at runtime when a `for glob` /
	/// `for shell` iterator expands to more iterations than the planning
	/// pass estimated, or when nested control flow inflates the total
	/// beyond the static estimate. Counts are always monotonically
	/// non-decreasing — this never shrinks the total.
	pub fn add_to_total(&self, n: usize) {
		self.total.fetch_add(n, Ordering::SeqCst);
	}

	/// Reduce the total step count. Called when a shell template was
	/// counted by the static [`crate::control_flow::count_leaves`] pass
	/// but turns out to be a runtime no-op (typically a line that
	/// resolves to whitespace — e.g. one consisting only of
	/// `{{ define(...) }}` calls — which is dropped from execution).
	/// Without this, the visible `(N/total)` ratio would drift because
	/// the current counter never advances for the skipped step while
	/// the total still includes it. Saturating: never underflows.
	pub fn subtract_from_total(&self, n: usize) {
		let _ = self
			.total
			.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |t| Some(t.saturating_sub(n)));
	}
}

/// Determine whether logging is enabled for a command.
/// Defaults to false if not set.
pub fn is_logging_enabled(spec: &CommandSpec) -> bool {
	spec.logging.unwrap_or(false)
}

/// Print a command that is about to be executed, in a formatted style.
/// `step` is the 1-indexed global step number; `total` is the total
/// step count for the entire run.
pub fn log_command(command: &str, step: usize, total: usize) {
	// Enable ANSI support on Windows (needed for older cmd.exe / PowerShell 5)
	#[cfg(windows)]
	enable_ansi_support();

	if total > 1 {
		eprintln!("{BOLD}{CYAN}[runfile]{RESET} {DIM}({step}/{total}){RESET} {BOLD}{command}{RESET}");
	} else {
		eprintln!("{BOLD}{CYAN}[runfile]{RESET} {BOLD}{command}{RESET}");
	}
}

/// Print a command that is about to be executed in parallel mode.
/// `step` is the 1-indexed global step number; `total` is the total
/// step count for the entire run.
pub fn log_parallel_command(command: &str, step: usize, total: usize) {
	#[cfg(windows)]
	enable_ansi_support();

	if total > 1 {
		eprintln!("{BOLD}{CYAN}[runfile]{RESET} {DIM}({step}/{total}) [parallel]{RESET} {BOLD}{command}{RESET}");
	} else {
		eprintln!("{BOLD}{CYAN}[runfile]{RESET} {DIM}[parallel]{RESET} {BOLD}{command}{RESET}");
	}
}

/// Print a summary of which leaves failed in a parallel batch and with what
/// exit code. Called at the end of `run_parallel_batch` whenever at least
/// one leaf failed — even when `ignoreErrors` is set, because the whole
/// point is to surface failures that would otherwise get swallowed by the
/// interleaved parallel output.
///
/// Each entry is `(label, detail)` where `label` is the leaf identity
/// (raw shell template, or `@target args...` for dispatched targets) and
/// `detail` is a short human-readable phrase describing how it failed
/// (e.g. `exit code 1`, `terminated by signal`, `error: ...`).
pub fn log_parallel_failure_summary(failures: &[(String, String)]) {
	if failures.is_empty() {
		return;
	}
	#[cfg(windows)]
	enable_ansi_support();
	let n = failures.len();
	let plural = if n == 1 { "" } else { "s" };
	eprintln!("{BOLD}{CYAN}[runfile]{RESET} {BOLD}{RED}[parallel] {n} command{plural} failed:{RESET}");
	for (label, detail) in failures {
		eprintln!("  {BOLD}{RED}-{RESET} {BOLD}{label}{RESET} {DIM}—{RESET} {RED}{detail}{RESET}");
	}
}

/// Format a duration for human display.
fn format_duration(d: Duration) -> String {
	let secs = d.as_secs_f64();
	if secs < 1.0 {
		format!("{:.0}ms", d.as_millis())
	} else if secs < 60.0 {
		format!("{secs:.1}s")
	} else {
		let mins = secs as u64 / 60;
		let remaining = secs - (mins as f64 * 60.0);
		format!("{mins}m {remaining:.1}s")
	}
}

/// Print timing information for a single command.
pub fn log_command_timing(duration: Duration) {
	#[cfg(windows)]
	enable_ansi_support();
	eprintln!(
		"{BOLD}{CYAN}[runfile]{RESET} {DIM}completed in {}{RESET}",
		format_duration(duration),
	);
}

/// Print timing information for a target.
pub fn log_target_timing(target_name: &str, duration: Duration) {
	#[cfg(windows)]
	enable_ansi_support();
	eprintln!(
		"{BOLD}{CYAN}[runfile]{RESET} target \"{BOLD}{target_name}{RESET}\" completed in {BOLD}{}{RESET}",
		format_duration(duration),
	);
}

/// Print total timing information.
pub fn log_total_timing(duration: Duration) {
	#[cfg(windows)]
	enable_ansi_support();
	eprintln!(
		"{BOLD}{CYAN}[runfile]{RESET} total: {BOLD}{}{RESET}",
		format_duration(duration),
	);
}

/// On Windows, (re-)assert the console mode our log output depends on:
/// `ENABLE_PROCESSED_OUTPUT` + `ENABLE_VIRTUAL_TERMINAL_PROCESSING`, on BOTH
/// the stdout and stderr console handles.
///
/// This MUST run on every log call, not once. Child processes — notably
/// `wsl.exe` and tools running inside it — clear these flags on the shared
/// console mid-run. After that, our ANSI escapes render as literal `←[..m`
/// and our `\n` line terminators render as the CP437 glyph `◙` until the
/// flags are restored. A `Once`-gated, set-only-VT, stderr-only version (the
/// previous implementation) could never recover from this: the first log line
/// armed it, then WSL disarmed it, and every later call was a no-op.
///
/// Both handles need it: our `[runfile]` log lines go to stderr, but the
/// inherited stdout of sequential commands (e.g. a trailing `echo`) shares
/// the same console and is also affected. Re-asserting per call is cheap (a
/// couple of syscalls) and mirrors the strategy the parallel output writer
/// uses in `parallel_output::write_to_stream_windows`.
#[cfg(windows)]
fn enable_ansi_support() {
	use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
	use windows_sys::Win32::System::Console::{
		GetConsoleMode, GetStdHandle, SetConsoleMode, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
	};

	for handle_id in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
		unsafe {
			let handle = GetStdHandle(handle_id);
			if handle.is_null() || handle == INVALID_HANDLE_VALUE {
				continue;
			}
			let mut mode: u32 = 0;
			// GetConsoleMode fails for non-console handles (pipes, files, NUL).
			// Those need no fix — the bytes pass through verbatim.
			if GetConsoleMode(handle, &mut mode) == 0 {
				continue;
			}
			if let Some(desired) = console_mode_update(mode) {
				let _ = SetConsoleMode(handle, desired);
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::{console_mode_update, ENABLE_PROCESSED_OUTPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING};

	/// Both managed bits OR'd together — the end state we always converge to.
	const BOTH: u32 = ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING;

	// A handful of plausible "unrelated" Windows console mode bits that our
	// helper must never touch (ENABLE_WRAP_AT_EOL_OUTPUT, ENABLE_LVB_GRID, an
	// arbitrary high bit).
	const UNRELATED: u32 = 0x0002 | 0x0008 | 0x8000_0000;

	#[test]
	fn flag_values_match_win32_constants() {
		// Guard the magic numbers against accidental edits — these are the
		// documented Win32 console mode bits.
		assert_eq!(ENABLE_PROCESSED_OUTPUT, 0x0001);
		assert_eq!(ENABLE_VIRTUAL_TERMINAL_PROCESSING, 0x0004);
	}

	#[test]
	fn enables_both_flags_from_zero() {
		// The exact state after `wsl.exe` clears the console mode: neither flag
		// set. We must turn both back on. This is the core regression case —
		// the bug manifested as both ANSI escapes (`←[..m`) and `\n` (`◙`)
		// rendering literally because both bits were off and never restored.
		assert_eq!(console_mode_update(0), Some(BOTH));
	}

	#[test]
	fn adds_processed_output_when_only_vt_present() {
		// The previous implementation set ONLY VT, leaving `\n` rendering as
		// `◙`. Ensure the missing ENABLE_PROCESSED_OUTPUT bit is added.
		assert_eq!(console_mode_update(ENABLE_VIRTUAL_TERMINAL_PROCESSING), Some(BOTH));
	}

	#[test]
	fn adds_vt_when_only_processed_present() {
		assert_eq!(console_mode_update(ENABLE_PROCESSED_OUTPUT), Some(BOTH));
	}

	#[test]
	fn no_update_when_both_already_set() {
		// Already correct → no SetConsoleMode syscall needed.
		assert_eq!(console_mode_update(BOTH), None);
	}

	#[test]
	fn no_update_when_both_set_alongside_unrelated_bits() {
		assert_eq!(console_mode_update(BOTH | UNRELATED), None);
	}

	#[test]
	fn no_update_when_all_bits_set() {
		// All bits set already includes our two → nothing to do.
		assert_eq!(console_mode_update(u32::MAX), None);
	}

	#[test]
	fn preserves_unrelated_bits_while_adding_ours() {
		// We only OR our two bits in — every other mode flag must survive.
		let got = console_mode_update(UNRELATED).expect("should need an update");
		assert_eq!(got, UNRELATED | BOTH);
		assert_eq!(got & UNRELATED, UNRELATED, "unrelated bits were cleared");
	}

	#[test]
	fn is_idempotent() {
		// Applying the computed mode again must report "no change" — proves the
		// per-call re-assert converges and won't thrash SetConsoleMode.
		let first = console_mode_update(0).expect("first update");
		assert_eq!(console_mode_update(first), None);

		let from_partial = console_mode_update(UNRELATED).expect("update from partial");
		assert_eq!(console_mode_update(from_partial), None);
	}

	#[test]
	fn only_ever_sets_our_two_bits_and_clears_nothing() {
		// Exhaustive-ish sweep: for any input, the delta must be a subset of
		// our two managed bits, and no existing bit may be cleared.
		for current in [
			0u32,
			0x1,
			0x2,
			0x4,
			0x5,
			0x7,
			0xFF,
			0xFF00,
			0x8000_0000,
			UNRELATED,
			u32::MAX,
		] {
			if let Some(desired) = console_mode_update(current) {
				let changed = desired ^ current;
				assert_eq!(changed & !BOTH, 0, "unexpected bits changed for {current:#x}");
				assert_eq!(desired & current, current, "cleared bits for {current:#x}");
				// Whenever an update is returned, both target bits end up set.
				assert_eq!(desired & BOTH, BOTH, "both flags not set for {current:#x}");
			} else {
				// `None` is only valid when both bits were already present.
				assert_eq!(current & BOTH, BOTH, "spurious None for {current:#x}");
			}
		}
	}
}
