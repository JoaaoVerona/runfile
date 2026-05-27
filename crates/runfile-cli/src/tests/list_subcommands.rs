use super::*;

// ── list-subcommands tests ───────────────────────────────────────
// These test the tree navigation used by shell completion scripts

#[test]
fn list_subcommands_navigation_finds_mcp_subcommands() {
	let cmd = Cli::command();
	let mcp = cmd.find_subcommand(":mcp").unwrap();
	let names: Vec<&str> = mcp
		.get_subcommands()
		.filter(|s| !s.is_hide_set())
		.map(|s| s.get_name())
		.collect();
	assert!(names.contains(&"server"));
	assert!(names.contains(&"inspect"));
	assert!(names.contains(&"install"));
}

#[test]
fn list_subcommands_navigation_finds_completions_subcommands() {
	let cmd = Cli::command();
	let completions = cmd.find_subcommand(":completions").unwrap();
	let names: Vec<&str> = completions
		.get_subcommands()
		.filter(|s| !s.is_hide_set())
		.map(|s| s.get_name())
		.collect();
	assert!(names.contains(&"install"));
	assert!(names.contains(&"uninstall"));
	assert!(names.contains(&"output"));
}

#[test]
fn list_subcommands_navigation_finds_generate_subcommands() {
	let cmd = Cli::command();
	let generate = cmd.find_subcommand(":generate").unwrap();
	let names: Vec<&str> = generate
		.get_subcommands()
		.filter(|s| !s.is_hide_set())
		.map(|s| s.get_name())
		.collect();
	assert!(names.contains(&"zed-tasks"));
	assert!(names.contains(&"jetbrains-run-configurations"));
}

#[test]
fn list_subcommands_navigation_finds_convert_subcommands() {
	let cmd = Cli::command();
	let convert = cmd.find_subcommand(":convert").unwrap();
	let names: Vec<&str> = convert
		.get_subcommands()
		.filter(|s| !s.is_hide_set())
		.map(|s| s.get_name())
		.collect();
	assert!(names.contains(&"makefile"));
	assert!(names.contains(&"package-json"));
}

#[test]
fn list_subcommands_navigation_config_shell_set() {
	// Verify 3-level navigation: :config -> shell -> set
	let cmd = Cli::command();
	let config = cmd.find_subcommand(":config").unwrap();
	let shell = config.find_subcommand("shell").unwrap();
	let set = shell.find_subcommand("set");
	assert!(set.is_some(), ":config.shell.set not found");
}
