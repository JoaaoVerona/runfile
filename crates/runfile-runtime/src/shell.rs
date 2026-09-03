//! Default shell resolution for `$`.
//!
//! bash first, then Git Bash at the locations Git for Windows installs to,
//! then `sh`. Preferring bash is what makes the Windows story tractable: Git
//! Bash supplies bash *and* the coreutils real targets lean on, so one lookup
//! covers what used to be two problems. Measured on Windows 11: bash 5.3,
//! pipefail available, 19 of 19 tools present.
//!
//! `System32\bash.exe` is deliberately never a candidate. It is WSL's launcher,
//! which runs in a separate Linux environment and cannot see Windows programs
//! on PATH -- resolving to it would silently run targets against the wrong
//! filesystem.

use std::path::PathBuf;

pub fn default_shell() -> Option<PathBuf> {
	if let Ok(p) = which::which("bash")
		&& !is_wsl_launcher(&p)
	{
		return Some(p);
	}
	for c in git_bash_paths() {
		let p = PathBuf::from(c);
		if p.is_file() {
			return Some(p);
		}
	}
	which::which("sh").ok()
}

fn is_wsl_launcher(p: &std::path::Path) -> bool {
	if !cfg!(windows) {
		return false;
	}
	let s = p.to_string_lossy().to_lowercase();
	s.contains("\\system32\\") || s.contains("/system32/")
}

#[cfg(windows)]
fn git_bash_paths() -> Vec<String> {
	let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into());
	let pf86 = std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into());
	let lad = std::env::var("LOCALAPPDATA").unwrap_or_default();
	vec![
		format!(r"{pf}\Git\bin\bash.exe"),
		format!(r"{pf86}\Git\bin\bash.exe"),
		format!(r"{lad}\Programs\Git\bin\bash.exe"),
		r"C:\Git\bin\bash.exe".into(),
	]
}

#[cfg(not(windows))]
fn git_bash_paths() -> Vec<String> {
	Vec::new()
}
