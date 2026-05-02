use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Supported shell types.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShellKind {
	Bash,
	Zsh,
	Sh,
	Fish,
	PowerShell,
	Cmd,
}

impl ShellKind {
	/// Parse a shell name string into a ShellKind.
	pub fn from_name(name: &str) -> Option<ShellKind> {
		let lower = name.to_lowercase();
		let base = lower.strip_suffix(".exe").unwrap_or(&lower);
		match base {
			"bash" => Some(ShellKind::Bash),
			"zsh" => Some(ShellKind::Zsh),
			"sh" => Some(ShellKind::Sh),
			"fish" => Some(ShellKind::Fish),
			"powershell" | "pwsh" => Some(ShellKind::PowerShell),
			"cmd" => Some(ShellKind::Cmd),
			_ => None,
		}
	}

	/// The canonical name used in settings and display.
	pub fn name(&self) -> &'static str {
		match self {
			ShellKind::Bash => "bash",
			ShellKind::Zsh => "zsh",
			ShellKind::Sh => "sh",
			ShellKind::Fish => "fish",
			ShellKind::PowerShell => "powershell",
			ShellKind::Cmd => "cmd",
		}
	}

	/// The flag used to pass a command string to this shell.
	pub fn command_flag(&self) -> &'static str {
		match self {
			ShellKind::Bash | ShellKind::Zsh | ShellKind::Sh | ShellKind::Fish => "-c",
			ShellKind::PowerShell => "-Command",
			ShellKind::Cmd => "/C",
		}
	}
}

/// A resolved shell — its type and the path to its executable.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedShell {
	pub kind: ShellKind,
	pub path: PathBuf,
}

impl ResolvedShell {
	/// Get the command-line arguments to execute a command string in this shell.
	pub fn exec_args(&self, command: &str) -> Vec<String> {
		vec![self.kind.command_flag().to_string(), command.to_string()]
	}
}
