use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Whether a force-kill guard is currently active.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// RAII guard that sets up a CTRL+C / SIGINT handler which forcefully kills
/// all tracked child processes (and their descendants) when triggered.
///
/// On Windows: uses a Job Object — all children and grandchildren are assigned
/// to the job. CTRL+C handler calls `TerminateJobObject` to kill the entire tree.
///
/// On Unix: tracks child PIDs and sends SIGKILL on SIGINT.
pub(crate) struct ForceKillGuard {
	_private: (),
}

impl ForceKillGuard {
	/// Create a new guard and install the signal handler.
	pub fn new() -> Self {
		platform::setup();
		ACTIVE.store(true, Ordering::Relaxed);
		platform::install_handler();
		Self { _private: () }
	}

	/// Register a spawned child so it will be killed on SIGINT.
	pub fn add_child(&self, child: &Child) {
		platform::add_child(child);
	}
}

impl Drop for ForceKillGuard {
	fn drop(&mut self) {
		ACTIVE.store(false, Ordering::Relaxed);
		platform::uninstall_handler();
		platform::teardown();
	}
}

// ──── Windows implementation ────

#[cfg(windows)]
mod platform {
	use super::{Mutex, ACTIVE};
	use std::process::Child;
	use std::sync::atomic::Ordering;

	/// Wrapper to make raw HANDLE Send+Sync for use in a static Mutex.
	/// Safety: Job Object handles are safe to use from any thread.
	struct SendHandle(windows_sys::Win32::Foundation::HANDLE);
	unsafe impl Send for SendHandle {}

	static JOB_HANDLE: Mutex<Option<SendHandle>> = Mutex::new(None);

	pub(super) fn setup() {
		let handle =
			unsafe { windows_sys::Win32::System::JobObjects::CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
		if !handle.is_null() {
			*JOB_HANDLE.lock().unwrap() = Some(SendHandle(handle));
		}
	}

	pub(super) fn teardown() {
		if let Some(SendHandle(handle)) = JOB_HANDLE.lock().unwrap().take() {
			unsafe {
				windows_sys::Win32::Foundation::CloseHandle(handle);
			}
		}
	}

	pub(super) fn add_child(child: &Child) {
		if let Some(SendHandle(handle)) = &*JOB_HANDLE.lock().unwrap() {
			use std::os::windows::io::AsRawHandle;
			unsafe {
				windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(
					*handle,
					child.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE,
				);
			}
		}
	}

	pub(super) fn install_handler() {
		unsafe {
			windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(ctrl_handler), 1);
		}
	}

	pub(super) fn uninstall_handler() {
		unsafe {
			windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(ctrl_handler), 0);
		}
	}

	unsafe extern "system" fn ctrl_handler(_ctrl_type: u32) -> i32 {
		if !ACTIVE.load(Ordering::Relaxed) {
			return 0; // Not active, pass to next handler
		}

		// Terminate all processes in the job
		if let Ok(guard) = JOB_HANDLE.lock() {
			if let Some(SendHandle(handle)) = &*guard {
				windows_sys::Win32::System::JobObjects::TerminateJobObject(*handle, 1);
			}
		}

		// Return TRUE to suppress the default handler (which would kill us
		// before we can reap the children and report the result).
		1
	}
}

// ──── Unix implementation ────

#[cfg(unix)]
mod platform {
	use super::{Mutex, ACTIVE};
	use std::process::Child;
	use std::sync::atomic::Ordering;

	static CHILD_PIDS: Mutex<Vec<u32>> = Mutex::new(Vec::new());
	static PREV_HANDLER: Mutex<Option<libc::sighandler_t>> = Mutex::new(None);

	pub(super) fn setup() {
		CHILD_PIDS.lock().unwrap().clear();
	}

	pub(super) fn teardown() {
		CHILD_PIDS.lock().unwrap().clear();
	}

	pub(super) fn add_child(child: &Child) {
		CHILD_PIDS.lock().unwrap().push(child.id());
	}

	pub(super) fn install_handler() {
		let prev = unsafe { libc::signal(libc::SIGINT, sigint_handler as *const () as libc::sighandler_t) };
		*PREV_HANDLER.lock().unwrap() = Some(prev);
	}

	pub(super) fn uninstall_handler() {
		if let Some(prev) = PREV_HANDLER.lock().unwrap().take() {
			unsafe {
				libc::signal(libc::SIGINT, prev);
			}
		}
	}

	extern "C" fn sigint_handler(_sig: libc::c_int) {
		if !ACTIVE.load(Ordering::Relaxed) {
			return;
		}

		// Send SIGKILL to each tracked child process
		if let Ok(pids) = CHILD_PIDS.lock() {
			for &pid in pids.iter() {
				unsafe {
					// Kill the process group (negative PID) to catch grandchildren
					libc::kill(-(pid as i32), libc::SIGKILL);
					// Also kill the process directly in case it's not a group leader
					libc::kill(pid as i32, libc::SIGKILL);
				}
			}
		}
	}
}
