use crate::types::{ResolvedShell, ShellKind};
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ShellResolveError {
	#[error("Unknown shell: \"{0}\". Supported shells: bash, zsh, sh, fish, powershell, cmd")]
	UnknownShell(String),

	#[error("Shell \"{0}\" not found at any known location. Please set the path in your Runfile settings.")]
	ShellNotFound(String),
}

/// Resolve a shell by name. Tries well-known paths, then falls back to
/// searching PATH via `which`. As a last resort, if the requested shell is
/// `sh` (commonly missing on Windows and minimal containers), falls back to
/// other sh-compatible shells in order: bash → zsh → fish. The returned
/// `kind` reflects the shell that will actually run, so `$(RUN.shell)` is
/// accurate.
pub fn resolve_shell(name: &str) -> Result<ResolvedShell, ShellResolveError> {
	let kind = ShellKind::from_name(name).ok_or_else(|| ShellResolveError::UnknownShell(name.to_string()))?;

	// Try well-known paths first
	for path_str in known_paths(&kind) {
		let path = PathBuf::from(path_str);
		if path.exists() {
			return Ok(ResolvedShell { kind, path });
		}
	}

	// Fall back to `which`
	if let Ok(path) = which::which(name) {
		return Ok(ResolvedShell { kind, path });
	}

	if matches!(kind, ShellKind::Sh) {
		for fallback in [ShellKind::Bash, ShellKind::Zsh, ShellKind::Fish] {
			if let Ok(shell) = resolve_shell(fallback.name()) {
				return Ok(shell);
			}
		}
	}

	Err(ShellResolveError::ShellNotFound(name.to_string()))
}

/// Resolve a shell from an explicit path. Validates that the file exists.
pub fn resolve_shell_from_path(name: &str, path: PathBuf) -> Result<ResolvedShell, ShellResolveError> {
	let kind = ShellKind::from_name(name).ok_or_else(|| ShellResolveError::UnknownShell(name.to_string()))?;

	if path.exists() {
		Ok(ResolvedShell { kind, path })
	} else {
		Err(ShellResolveError::ShellNotFound(name.to_string()))
	}
}

pub fn known_paths(kind: &ShellKind) -> Vec<String> {
	match kind {
		ShellKind::Bash => {
			if cfg!(windows) {
				git_bash_known_paths()
			} else {
				vec!["/bin/bash".into(), "/usr/bin/bash".into(), "/usr/local/bin/bash".into()]
			}
		}
		ShellKind::Zsh => vec!["/bin/zsh".into(), "/usr/bin/zsh".into(), "/usr/local/bin/zsh".into()],
		ShellKind::Sh => vec!["/bin/sh".into(), "/usr/bin/sh".into()],
		ShellKind::Fish => vec!["/usr/bin/fish".into(), "/usr/local/bin/fish".into()],
		ShellKind::PowerShell => {
			if cfg!(windows) {
				windows_powershell_known_paths()
			} else {
				vec!["/usr/bin/pwsh".into(), "/usr/local/bin/pwsh".into()]
			}
		}
		ShellKind::Cmd => {
			if cfg!(windows) {
				let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
				vec![format!(r"{}\System32\cmd.exe", system_root)]
			} else {
				vec![]
			}
		}
	}
}

/// Git Bash known locations on Windows.
/// Intentionally does NOT include `System32\bash.exe` — that is WSL's bash,
/// which runs inside a separate Linux environment and cannot see Windows
/// programs in PATH.
#[cfg(windows)]
fn git_bash_known_paths() -> Vec<String> {
	let program_files = std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into());
	let program_files_x86 = std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into());
	let local_app_data = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| r"C:\Users\default\AppData\Local".into());

	vec![
		format!(r"{}\Git\bin\bash.exe", program_files),
		format!(r"{}\Git\bin\bash.exe", program_files_x86),
		format!(r"{}\Programs\Git\bin\bash.exe", local_app_data),
		r"C:\Git\bin\bash.exe".into(),
	]
}

#[cfg(not(windows))]
fn git_bash_known_paths() -> Vec<String> {
	vec![]
}

/// PowerShell known locations on Windows.
#[cfg(windows)]
fn windows_powershell_known_paths() -> Vec<String> {
	let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
	vec![format!(
		r"{}\System32\WindowsPowerShell\v1.0\powershell.exe",
		system_root
	)]
}

#[cfg(not(windows))]
fn windows_powershell_known_paths() -> Vec<String> {
	vec![]
}
