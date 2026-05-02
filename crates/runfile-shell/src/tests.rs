use crate::*;

#[test]
fn shell_kind_from_valid_names() {
	assert_eq!(ShellKind::from_name("bash"), Some(ShellKind::Bash));
	assert_eq!(ShellKind::from_name("BASH"), Some(ShellKind::Bash));
	assert_eq!(ShellKind::from_name("zsh"), Some(ShellKind::Zsh));
	assert_eq!(ShellKind::from_name("sh"), Some(ShellKind::Sh));
	assert_eq!(ShellKind::from_name("fish"), Some(ShellKind::Fish));
	assert_eq!(ShellKind::from_name("powershell"), Some(ShellKind::PowerShell));
	assert_eq!(ShellKind::from_name("pwsh"), Some(ShellKind::PowerShell));
	assert_eq!(ShellKind::from_name("cmd"), Some(ShellKind::Cmd));
	assert_eq!(ShellKind::from_name("cmd.exe"), Some(ShellKind::Cmd));
}

#[test]
fn shell_kind_from_invalid_name() {
	assert_eq!(ShellKind::from_name("nonexistent"), None);
	assert_eq!(ShellKind::from_name(""), None);
}

#[test]
fn shell_kind_names() {
	assert_eq!(ShellKind::Bash.name(), "bash");
	assert_eq!(ShellKind::Zsh.name(), "zsh");
	assert_eq!(ShellKind::Sh.name(), "sh");
	assert_eq!(ShellKind::Fish.name(), "fish");
	assert_eq!(ShellKind::PowerShell.name(), "powershell");
	assert_eq!(ShellKind::Cmd.name(), "cmd");
}

#[test]
fn shell_command_flags() {
	assert_eq!(ShellKind::Bash.command_flag(), "-c");
	assert_eq!(ShellKind::Zsh.command_flag(), "-c");
	assert_eq!(ShellKind::Sh.command_flag(), "-c");
	assert_eq!(ShellKind::Fish.command_flag(), "-c");
	assert_eq!(ShellKind::PowerShell.command_flag(), "-Command");
	assert_eq!(ShellKind::Cmd.command_flag(), "/C");
}

#[test]
fn resolved_shell_exec_args() {
	let shell = ResolvedShell {
		kind: ShellKind::Bash,
		path: "/bin/bash".into(),
	};
	let args = shell.exec_args("echo hello");
	assert_eq!(args, vec!["-c", "echo hello"]);
}

#[test]
fn resolved_shell_exec_args_powershell() {
	let shell = ResolvedShell {
		kind: ShellKind::PowerShell,
		path: "powershell.exe".into(),
	};
	let args = shell.exec_args("Write-Host hello");
	assert_eq!(args, vec!["-Command", "Write-Host hello"]);
}

#[test]
fn resolved_shell_exec_args_cmd() {
	let shell = ResolvedShell {
		kind: ShellKind::Cmd,
		path: "cmd.exe".into(),
	};
	let args = shell.exec_args("echo hello");
	assert_eq!(args, vec!["/C", "echo hello"]);
}

#[test]
fn detect_default_shell_succeeds() {
	// This test assumes at least one shell is available on the test machine.
	let result = detect_default_shell();
	assert!(result.is_ok(), "Should find at least one shell: {:?}", result.err());
	let shell = result.unwrap();
	assert!(shell.path.exists());
}

#[test]
fn resolve_unknown_shell_returns_error() {
	let result = resolve_shell("not_a_shell");
	assert!(result.is_err());
	assert!(matches!(result.unwrap_err(), ShellResolveError::UnknownShell(_)));
}

#[test]
fn resolve_shell_from_nonexistent_path() {
	let result = resolve_shell_from_path("bash", "/no/such/path/bash".into());
	assert!(result.is_err());
}

// ── Additional coverage tests ──────────────────────────────────────

#[test]
fn resolve_shell_from_path_unknown_shell_name() {
	let result = resolve_shell_from_path("not_a_shell", "/bin/bash".into());
	assert!(result.is_err());
	assert!(matches!(result.unwrap_err(), ShellResolveError::UnknownShell(_)));
}

#[test]
fn shell_kind_from_name_pwsh_variant() {
	assert_eq!(ShellKind::from_name("PWSH"), Some(ShellKind::PowerShell));
	assert_eq!(ShellKind::from_name("Pwsh"), Some(ShellKind::PowerShell));
}

#[test]
fn shell_kind_from_name_cmd_exe_variant() {
	assert_eq!(ShellKind::from_name("CMD.EXE"), Some(ShellKind::Cmd));
	assert_eq!(ShellKind::from_name("Cmd.Exe"), Some(ShellKind::Cmd));
}

#[test]
fn shell_kind_from_name_mixed_case() {
	assert_eq!(ShellKind::from_name("Bash"), Some(ShellKind::Bash));
	assert_eq!(ShellKind::from_name("ZSH"), Some(ShellKind::Zsh));
	assert_eq!(ShellKind::from_name("FISH"), Some(ShellKind::Fish));
	assert_eq!(ShellKind::from_name("SH"), Some(ShellKind::Sh));
}

#[test]
fn known_paths_returns_paths_for_all_shell_kinds() {
	// All shell kinds should return a list (possibly empty on non-matching platforms)
	let _ = known_paths(&ShellKind::Bash);
	let _ = known_paths(&ShellKind::Zsh);
	let _ = known_paths(&ShellKind::Sh);
	let _ = known_paths(&ShellKind::Fish);
	let _ = known_paths(&ShellKind::PowerShell);
	let _ = known_paths(&ShellKind::Cmd);

	// On Unix, Bash/Zsh/Sh/Fish should return non-empty lists
	#[cfg(unix)]
	{
		assert!(!known_paths(&ShellKind::Bash).is_empty());
		assert!(!known_paths(&ShellKind::Zsh).is_empty());
		assert!(!known_paths(&ShellKind::Sh).is_empty());
		assert!(!known_paths(&ShellKind::Fish).is_empty());
		// Cmd should return empty on Unix
		assert!(known_paths(&ShellKind::Cmd).is_empty());
	}
}

#[test]
fn resolve_shell_success_for_available_shells() {
	// At least one of bash/zsh/sh should be resolvable on any Unix system
	#[cfg(unix)]
	{
		let found = resolve_shell("bash")
			.or_else(|_| resolve_shell("zsh"))
			.or_else(|_| resolve_shell("sh"));
		assert!(found.is_ok(), "Should resolve at least one common shell");
		let shell = found.unwrap();
		assert!(shell.path.exists());
	}
}

#[test]
fn resolve_shell_not_found_error() {
	// Try resolving a valid shell name that doesn't exist on this system
	// cmd should not exist on Unix
	#[cfg(unix)]
	{
		let result = resolve_shell("cmd");
		assert!(result.is_err());
		assert!(matches!(result.unwrap_err(), ShellResolveError::ShellNotFound(_)));
	}
}

#[test]
fn resolved_shell_exec_args_fish() {
	let shell = ResolvedShell {
		kind: ShellKind::Fish,
		path: "/usr/bin/fish".into(),
	};
	let args = shell.exec_args("echo hello");
	assert_eq!(args, vec!["-c", "echo hello"]);
}

#[test]
fn resolved_shell_exec_args_zsh() {
	let shell = ResolvedShell {
		kind: ShellKind::Zsh,
		path: "/bin/zsh".into(),
	};
	let args = shell.exec_args("echo hello");
	assert_eq!(args, vec!["-c", "echo hello"]);
}

#[test]
fn resolved_shell_exec_args_sh() {
	let shell = ResolvedShell {
		kind: ShellKind::Sh,
		path: "/bin/sh".into(),
	};
	let args = shell.exec_args("echo hello");
	assert_eq!(args, vec!["-c", "echo hello"]);
}

#[test]
fn shell_kind_equality() {
	assert_eq!(ShellKind::Bash, ShellKind::Bash);
	assert_ne!(ShellKind::Bash, ShellKind::Zsh);
	assert_ne!(ShellKind::PowerShell, ShellKind::Cmd);
}

#[test]
fn resolved_shell_equality() {
	let a = ResolvedShell {
		kind: ShellKind::Bash,
		path: "/bin/bash".into(),
	};
	let b = ResolvedShell {
		kind: ShellKind::Bash,
		path: "/bin/bash".into(),
	};
	let c = ResolvedShell {
		kind: ShellKind::Bash,
		path: "/usr/bin/bash".into(),
	};
	assert_eq!(a, b);
	assert_ne!(a, c);
}

#[test]
fn detect_default_shell_returns_valid_shell_kind() {
	let shell = detect_default_shell().expect("Should detect a shell");
	// The kind should match the file name
	let file_name = shell.path.file_name().unwrap().to_str().unwrap();
	// The kind should be parseable from the file name
	let parsed_kind = ShellKind::from_name(file_name);
	assert!(
		parsed_kind.is_some(),
		"Detected shell file name '{}' should be a recognized shell kind",
		file_name
	);
	assert_eq!(parsed_kind.unwrap(), shell.kind, "Detected kind should match file name");
}
