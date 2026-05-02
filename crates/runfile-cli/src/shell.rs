use runfile_settings::Settings;
use runfile_shell::{detect_default_shell, resolve_shell, resolve_shell_from_path, ResolvedShell, ShellKind};
use std::path::PathBuf;
use std::process;

/// Resolve a shell from the --shell CLI flag.
/// Accepts either a shell name ("bash", "powershell") or a direct path to an executable.
pub fn resolve_cli_shell_override(value: &str, settings: &Settings) -> ResolvedShell {
	if let Some(kind) = ShellKind::from_name(value) {
		if let Some(custom_path) = settings.get_shell_path(value) {
			if let Ok(shell) = resolve_shell_from_path(value, custom_path.clone()) {
				return shell;
			}
		}
		if let Ok(shell) = resolve_shell(kind.name()) {
			return shell;
		}
	}

	let path = PathBuf::from(value);
	if path.exists() {
		let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
		let kind = ShellKind::from_name(name).unwrap_or(ShellKind::Sh);
		return ResolvedShell { kind, path };
	}

	eprintln!("Error: shell \"{value}\" not found. Provide a shell name (bash, powershell, ...) or a path to a shell executable.");
	process::exit(1);
}

pub fn resolve_shell_for_runfile(command_shell: Option<&str>, settings: &Settings) -> ResolvedShell {
	let force_shell = command_shell;

	if let Some(shell_name) = force_shell {
		if let Some(custom_path) = settings.get_shell_path(shell_name) {
			match resolve_shell_from_path(shell_name, custom_path.clone()) {
				Ok(shell) => return shell,
				Err(e) => {
					eprintln!("Warning: custom shell path failed ({e}), trying default locations...");
				}
			}
		}

		match resolve_shell(shell_name) {
			Ok(shell) => return shell,
			Err(e) => {
				eprintln!("Error: {e}");
				eprintln!("Use `run :config shell set {shell_name} /path/to/shell` to configure the shell path.");
				process::exit(1);
			}
		}
	}

	match detect_default_shell() {
		Ok(shell) => shell,
		Err(e) => {
			eprintln!("Error: {e}");
			process::exit(1);
		}
	}
}
