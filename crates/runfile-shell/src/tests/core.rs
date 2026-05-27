use super::*;

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
