//! Default shell resolution for `$`, and where a shell the file names is.
//!
//! bash first, then Git Bash at the locations Git for Windows installs to,
//! then `sh`. Preferring bash is what makes the Windows story tractable: Git
//! Bash supplies bash *and* the coreutils real targets lean on, so one lookup
//! covers what used to be two problems. Measured on Windows 11: bash 5.3,
//! pipefail available, 19 of 19 tools present.
//!
//! WSL's launcher is deliberately never a candidate. It runs in a separate
//! Linux environment and cannot see Windows programs on PATH -- resolving to it
//! would silently run targets against the wrong filesystem, and on a machine
//! with no distribution installed it runs nothing at all. That holds for a
//! shell the file names too: `.shell = "bash"` and `exec bash` are found by the
//! same [`locate`], or they would reach the launcher first, as the standard
//! library's own search does.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

pub fn default_shell() -> Option<PathBuf> {
	locate(OsStr::new("bash"), None)
		.or_else(|_| locate(OsStr::new("sh"), None))
		.ok()
}

/// Where the shell called `name` is on this machine: the first on `path` that
/// is not WSL's launcher, then where Git for Windows installs it.
///
/// `path` is the PATH to search, the runner's own when `None`. The default
/// shell is the runner's choice and searches the runner's; a shell the file
/// names is looked for on the PATH its command runs with, which is where the
/// standard library looked before this did, so `.add-path` still decides which
/// one runs.
pub fn locate(name: &OsStr, path: Option<&OsStr>) -> std::io::Result<PathBuf> {
	let path = path.map(OsStr::to_os_string).or_else(|| std::env::var_os("PATH"));
	which::which_in_global(name, path)
		.ok()
		.and_then(|mut found| found.find(|p| !(cfg!(windows) && is_wsl_launcher(p))))
		.or_else(|| git_for_windows(name))
		.ok_or_else(|| {
			std::io::Error::new(
				std::io::ErrorKind::NotFound,
				if cfg!(windows) {
					"not on PATH, nor where Git for Windows installs it (WSL's launcher is never used)"
				} else {
					"not on PATH"
				},
			)
		})
}

/// Whether `p` is WSL's launcher rather than a shell.
///
/// WSL puts a `bash.exe` in two places: `System32`, where the Windows feature
/// installs it, and `WindowsApps`, where the Store package registers it as an
/// App Execution Alias. That directory is on the user PATH by default, so a
/// check for `System32` alone let `$` lines reach WSL wherever the alias was
/// the first bash on PATH. Decided by the directory the file is in, which is
/// the same whichever separator PATH spelled it with and however it was cased;
/// a `system32` further up is somebody else's directory.
///
/// Asked only on Windows, where those two are the system's. The question is
/// about names, so it is answered the same everywhere, which is what lets a
/// test ask it on any platform.
fn is_wsl_launcher(p: &Path) -> bool {
	p.parent()
		.and_then(Path::file_name)
		.is_some_and(|dir| dir.eq_ignore_ascii_case("system32") || dir.eq_ignore_ascii_case("windowsapps"))
}

/// `name` where Git for Windows installs it, for a machine whose PATH has only
/// its `cmd` directory -- which is all its installer puts there by default.
#[cfg(windows)]
fn git_for_windows(name: &OsStr) -> Option<PathBuf> {
	let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into());
	let pf86 = std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into());
	let lad = std::env::var("LOCALAPPDATA").unwrap_or_default();
	let mut file = name.to_os_string();
	if Path::new(name).extension().is_none() {
		file.push(".exe");
	}
	[
		format!(r"{pf}\Git\bin"),
		format!(r"{pf86}\Git\bin"),
		format!(r"{lad}\Programs\Git\bin"),
		r"C:\Git\bin".into(),
	]
	.into_iter()
	.map(|dir| Path::new(&dir).join(&file))
	.find(|p| p.is_file())
}

#[cfg(not(windows))]
fn git_for_windows(_: &OsStr) -> Option<PathBuf> {
	None
}

#[cfg(test)]
mod tests {
	use super::{is_wsl_launcher, locate};
	use std::ffi::OsStr;
	use std::path::{Path, PathBuf};

	#[test]
	fn wsl_s_launcher_is_known_by_the_directory_it_is_in() {
		// Both places WSL puts a `bash.exe`, however they are cased.
		for p in [
			"/c/Windows/System32/bash.exe",
			"/c/WINDOWS/system32/bash.exe",
			"/c/Users/me/AppData/Local/Microsoft/WindowsApps/bash.exe",
			"/c/Users/me/AppData/Local/Microsoft/windowsapps/bash.exe",
		] {
			assert!(is_wsl_launcher(Path::new(p)), "{p}");
		}
		// Git for Windows' bash in both of its directories, and a `system32`
		// that is not the directory the file is in -- which a substring test
		// took for the launcher.
		for p in [
			"/c/Program Files/Git/bin/bash.exe",
			"/c/Program Files/Git/usr/bin/bash.exe",
			"/c/Users/me/system32/tools/bash.exe",
			"/usr/bin/bash",
		] {
			assert!(!is_wsl_launcher(Path::new(p)), "{p}");
		}
	}

	#[cfg(windows)]
	#[test]
	fn wsl_s_launcher_is_known_however_path_spelled_the_directory() {
		// A PATH entry may use either separator; `which` joins the name on
		// with a backslash either way.
		for p in [
			r"C:\Windows\System32\bash.exe",
			r"C:/Windows/System32\bash.exe",
			r"C:/Windows/System32/bash.exe",
			r"C:\Users\me\AppData\Local\Microsoft\WindowsApps\bash.exe",
		] {
			assert!(is_wsl_launcher(Path::new(p)), "{p}");
		}
		assert!(!is_wsl_launcher(Path::new(r"C:\Program Files\Git\bin\bash.exe")));
		assert!(!is_wsl_launcher(Path::new(r"C:\Users\me\system32\tools\bash.exe")));
	}

	/// A file `locate` will take for the program `name` in `dir`.
	fn program(dir: &Path, name: &str) -> PathBuf {
		std::fs::create_dir_all(dir).unwrap();
		let file = dir.join(if cfg!(windows) {
			format!("{name}.exe")
		} else {
			name.into()
		});
		std::fs::write(&file, "#!/bin/sh\n").unwrap();
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;
			std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
		}
		file
	}

	#[test]
	fn a_shell_is_located_on_the_path_it_is_given() {
		// Not the runner's own: this PATH names nothing else, so finding the
		// file means it was the one searched.
		let dir = tempfile::TempDir::new().unwrap();
		let bin = dir.path().join("bin");
		let bash = program(&bin, "bash");
		assert_eq!(locate(OsStr::new("bash"), Some(bin.as_os_str())).unwrap(), bash);
	}

	#[cfg(windows)]
	#[test]
	fn wsl_s_launcher_is_passed_over_for_the_next_bash_on_path() {
		// Where PATH has the launcher first -- the default, since `System32`
		// is on the system PATH and `WindowsApps` on the user's -- the bash
		// after it is the one found, rather than the launcher or nothing.
		let dir = tempfile::TempDir::new().unwrap();
		let (system32, apps, git) = (
			dir.path().join("System32"),
			dir.path().join("WindowsApps"),
			dir.path().join("Git").join("bin"),
		);
		program(&system32, "bash");
		program(&apps, "bash");
		let bash = program(&git, "bash");
		let path = std::env::join_paths([system32, apps, git]).unwrap();
		assert_eq!(locate(OsStr::new("bash"), Some(&path)).unwrap(), bash);
	}
}
