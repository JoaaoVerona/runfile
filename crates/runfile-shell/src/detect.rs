use crate::types::{ResolvedShell, ShellKind};
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ShellDetectError {
	#[error("No suitable shell found on this system")]
	NoShellFound,
}

/// Detect the default shell for the current platform.
pub fn detect_default_shell() -> Result<ResolvedShell, ShellDetectError> {
	#[cfg(unix)]
	{
		// Try $SHELL first
		if let Ok(shell_env) = std::env::var("SHELL") {
			let path = PathBuf::from(&shell_env);
			if path.exists() {
				let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
				if let Some(kind) = ShellKind::from_name(name) {
					return Ok(ResolvedShell { kind, path });
				}
			}
		}

		// Fallback: try common Unix shells in order
		for (kind, paths) in unix_shell_candidates() {
			for p in paths {
				let path = PathBuf::from(p);
				if path.exists() {
					return Ok(ResolvedShell {
						kind: kind.clone(),
						path,
					});
				}
			}
		}
	}

	#[cfg(windows)]
	{
		// Try common shell locations on Windows
		for (kind, paths) in windows_shell_candidates() {
			for p in paths {
				let path = PathBuf::from(p);
				if path.exists() {
					return Ok(ResolvedShell {
						kind: kind.clone(),
						path,
					});
				}
			}
		}

		// Try which/where for shells in PATH
		for kind in &[ShellKind::Bash, ShellKind::PowerShell, ShellKind::Cmd] {
			if let Ok(path) = which::which(kind.name()) {
				return Ok(ResolvedShell {
					kind: kind.clone(),
					path,
				});
			}
		}
	}

	Err(ShellDetectError::NoShellFound)
}

#[cfg(unix)]
fn unix_shell_candidates() -> Vec<(ShellKind, Vec<&'static str>)> {
	vec![
		(
			ShellKind::Bash,
			vec!["/bin/bash", "/usr/bin/bash", "/usr/local/bin/bash"],
		),
		(ShellKind::Zsh, vec!["/bin/zsh", "/usr/bin/zsh", "/usr/local/bin/zsh"]),
		(ShellKind::Sh, vec!["/bin/sh", "/usr/bin/sh"]),
		(ShellKind::Fish, vec!["/usr/bin/fish", "/usr/local/bin/fish"]),
	]
}

#[cfg(windows)]
fn windows_shell_candidates() -> Vec<(ShellKind, Vec<String>)> {
	let mut candidates = Vec::new();

	// Git Bash locations
	let program_files = std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into());
	let program_files_x86 = std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into());
	let local_app_data = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| r"C:\Users\default\AppData\Local".into());

	candidates.push((
		ShellKind::Bash,
		vec![
			format!(r"{}\Git\bin\bash.exe", program_files),
			format!(r"{}\Git\bin\bash.exe", program_files_x86),
			format!(r"{}\Programs\Git\bin\bash.exe", local_app_data),
			r"C:\Git\bin\bash.exe".into(),
		],
	));

	// PowerShell (pwsh = PowerShell 7+, then Windows PowerShell)
	let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
	candidates.push((
		ShellKind::PowerShell,
		vec![format!(
			r"{}\System32\WindowsPowerShell\v1.0\powershell.exe",
			system_root
		)],
	));

	// cmd.exe
	candidates.push((ShellKind::Cmd, vec![format!(r"{}\System32\cmd.exe", system_root)]));

	candidates
}
