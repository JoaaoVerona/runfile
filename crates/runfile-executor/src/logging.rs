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

/// On Windows, enable virtual terminal processing so ANSI codes work
/// in cmd.exe and older PowerShell.
#[cfg(windows)]
fn enable_ansi_support() {
	use std::sync::Once;
	static INIT: Once = Once::new();
	INIT.call_once(|| {
		unsafe {
			let handle = windows_sys::Win32::System::Console::GetStdHandle(
				windows_sys::Win32::System::Console::STD_ERROR_HANDLE,
			);
			if handle != windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
				let mut mode: u32 = 0;
				if windows_sys::Win32::System::Console::GetConsoleMode(handle, &mut mode) != 0 {
					// ENABLE_VIRTUAL_TERMINAL_PROCESSING = 0x0004
					let _ = windows_sys::Win32::System::Console::SetConsoleMode(handle, mode | 0x0004);
				}
			}
		}
	});
}
