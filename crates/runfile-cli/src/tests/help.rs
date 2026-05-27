use super::*;

// ── help-text tests ──────────────────────────────────────────────

#[test]
fn top_level_help_does_not_panic() {
	let mut cmd = Cli::command();
	let mut buf = Vec::new();
	cmd.write_help(&mut buf).unwrap();
	let help = String::from_utf8(buf).unwrap();
	assert!(help.contains(":config"));
	assert!(help.contains(":mcp"));
	assert!(help.contains(":completions"));
	assert!(help.contains(":generate"));
	assert!(help.contains(":convert"));
	assert!(!help.contains(":utilities"));
}

#[test]
fn mcp_help_shows_subcommands() {
	let cmd = Cli::command();
	let mcp = find_subcommand(&cmd, ":mcp");
	let mut buf = Vec::new();
	mcp.clone().write_help(&mut buf).unwrap();
	let help = String::from_utf8(buf).unwrap();
	assert!(help.contains("server"), "mcp help missing 'server'");
	assert!(help.contains("inspect"), "mcp help missing 'inspect'");
	assert!(help.contains("install"), "mcp help missing 'install'");
}

#[test]
fn completions_help_shows_subcommands() {
	let cmd = Cli::command();
	let completions = find_subcommand(&cmd, ":completions");
	let mut buf = Vec::new();
	completions.clone().write_help(&mut buf).unwrap();
	let help = String::from_utf8(buf).unwrap();
	assert!(help.contains("install"), "completions help missing 'install'");
	assert!(help.contains("uninstall"), "completions help missing 'uninstall'");
	assert!(help.contains("output"), "completions help missing 'output'");
}

#[test]
fn generate_help_shows_subcommands() {
	let cmd = Cli::command();
	let generate = find_subcommand(&cmd, ":generate");
	let mut buf = Vec::new();
	generate.clone().write_help(&mut buf).unwrap();
	let help = String::from_utf8(buf).unwrap();
	assert!(help.contains("zed-tasks"), "generate help missing 'zed-tasks'");
	assert!(
		help.contains("jetbrains-run-configurations"),
		"generate help missing 'jetbrains-run-configurations'"
	);
}

#[test]
fn convert_help_shows_subcommands() {
	let cmd = Cli::command();
	let convert = find_subcommand(&cmd, ":convert");
	let mut buf = Vec::new();
	convert.clone().write_help(&mut buf).unwrap();
	let help = String::from_utf8(buf).unwrap();
	assert!(help.contains("makefile"), "convert help missing 'makefile'");
	assert!(help.contains("package-json"), "convert help missing 'package-json'");
}
