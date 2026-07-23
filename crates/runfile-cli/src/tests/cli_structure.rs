use super::*;

// ── CLI structure tests ──────────────────────────────────────────

#[test]
fn cli_has_all_top_level_subcommands() {
	let cmd = Cli::command();
	let names: Vec<&str> = cmd
		.get_subcommands()
		.filter(|s| !s.is_hide_set())
		.map(|s| s.get_name())
		.collect();

	assert!(names.contains(&":config"), "missing :config");
	assert!(names.contains(&":list"), "missing :list");
	assert!(names.contains(&":init"), "missing :init");
	assert!(names.contains(&":mcp"), "missing :mcp");
	assert!(names.contains(&":completions"), "missing :completions");
	assert!(names.contains(&":generate"), "missing :generate");
	assert!(names.contains(&":convert"), "missing :convert");
	assert!(names.contains(&":env"), "missing :env");
	assert!(names.contains(&":update"), "missing :update");
}

#[test]
fn cli_does_not_have_utilities_subcommand() {
	let cmd = Cli::command();
	let names: Vec<&str> = cmd.get_subcommands().map(|s| s.get_name()).collect();
	assert!(!names.contains(&":utilities"), ":utilities should no longer exist");
}

#[test]
fn mcp_has_subcommands() {
	let cmd = Cli::command();
	let mcp = find_subcommand(&cmd, ":mcp");
	let names: Vec<&str> = mcp.get_subcommands().map(|s| s.get_name()).collect();

	assert!(names.contains(&"server"), "missing mcp server");
	assert!(names.contains(&"inspect"), "missing mcp inspect");
	assert!(names.contains(&"install"), "missing mcp install");
	assert_eq!(names.len(), 3, "unexpected mcp subcommands: {names:?}");
}

#[test]
fn mcp_subcommands_have_descriptions() {
	let cmd = Cli::command();
	let mcp = find_subcommand(&cmd, ":mcp");
	for sub in mcp.get_subcommands() {
		assert!(
			sub.get_about().is_some(),
			"mcp subcommand '{}' missing description",
			sub.get_name()
		);
	}
}

#[test]
fn completions_has_subcommands() {
	let cmd = Cli::command();
	let completions = find_subcommand(&cmd, ":completions");
	let names: Vec<&str> = completions.get_subcommands().map(|s| s.get_name()).collect();

	assert!(names.contains(&"install"), "missing completions install");
	assert!(names.contains(&"uninstall"), "missing completions uninstall");
	assert!(names.contains(&"output"), "missing completions output");
	assert_eq!(names.len(), 3, "unexpected completions subcommands: {names:?}");
}

#[test]
fn completions_subcommands_have_descriptions() {
	let cmd = Cli::command();
	let completions = find_subcommand(&cmd, ":completions");
	for sub in completions.get_subcommands() {
		assert!(
			sub.get_about().is_some(),
			"completions subcommand '{}' missing description",
			sub.get_name()
		);
	}
}

#[test]
fn completions_install_requires_shell_arg() {
	let cmd = Cli::command();
	let completions = find_subcommand(&cmd, ":completions");
	let install = find_subcommand(completions, "install");
	let args: Vec<&str> = install.get_arguments().map(|a| a.get_id().as_str()).collect();
	assert!(args.contains(&"shell"), "completions install missing shell arg");
}

#[test]
fn completions_uninstall_requires_shell_arg() {
	let cmd = Cli::command();
	let completions = find_subcommand(&cmd, ":completions");
	let uninstall = find_subcommand(completions, "uninstall");
	let args: Vec<&str> = uninstall.get_arguments().map(|a| a.get_id().as_str()).collect();
	assert!(args.contains(&"shell"), "completions uninstall missing shell arg");
}

#[test]
fn completions_output_requires_shell_arg() {
	let cmd = Cli::command();
	let completions = find_subcommand(&cmd, ":completions");
	let output = find_subcommand(completions, "output");
	let args: Vec<&str> = output.get_arguments().map(|a| a.get_id().as_str()).collect();
	assert!(args.contains(&"shell"), "completions output missing shell arg");
}

#[test]
fn generate_has_subcommands() {
	let cmd = Cli::command();
	let generate = find_subcommand(&cmd, ":generate");
	let names: Vec<&str> = generate.get_subcommands().map(|s| s.get_name()).collect();

	assert!(names.contains(&"zed-tasks"), "missing generate zed-tasks");
	assert!(
		names.contains(&"jetbrains-run-configurations"),
		"missing generate jetbrains-run-configurations"
	);
	assert!(names.contains(&"vscode-tasks"), "missing generate vscode-tasks");
	assert!(names.contains(&"task-descriptors"), "missing generate task-descriptors");
	assert_eq!(names.len(), 4, "unexpected generate subcommands: {names:?}");
}

#[test]
fn generate_subcommands_have_descriptions() {
	let cmd = Cli::command();
	let generate = find_subcommand(&cmd, ":generate");
	for sub in generate.get_subcommands() {
		assert!(
			sub.get_about().is_some(),
			"generate subcommand '{}' missing description",
			sub.get_name()
		);
	}
}

#[test]
fn convert_has_subcommands() {
	let cmd = Cli::command();
	let convert = find_subcommand(&cmd, ":convert");
	let names: Vec<&str> = convert.get_subcommands().map(|s| s.get_name()).collect();

	assert!(names.contains(&"makefile"), "missing convert makefile");
	assert!(names.contains(&"package-json"), "missing convert package-json");
	assert_eq!(names.len(), 2, "unexpected convert subcommands: {names:?}");
}

#[test]
fn convert_subcommands_have_descriptions() {
	let cmd = Cli::command();
	let convert = find_subcommand(&cmd, ":convert");
	for sub in convert.get_subcommands() {
		assert!(
			sub.get_about().is_some(),
			"convert subcommand '{}' missing description",
			sub.get_name()
		);
	}
}

#[test]
fn config_has_subcommands() {
	let cmd = Cli::command();
	let config = find_subcommand(&cmd, ":config");
	let names: Vec<&str> = config.get_subcommands().map(|s| s.get_name()).collect();

	assert!(names.contains(&"path-alias"), "missing config path-alias");
	assert!(names.contains(&"reset"), "missing config reset");
	assert!(names.contains(&"shell"), "missing config shell");
	assert!(names.contains(&"global-files"), "missing config global-files");
}

#[test]
fn config_shell_has_subcommands() {
	let cmd = Cli::command();
	let config = find_subcommand(&cmd, ":config");
	let shell = find_subcommand(config, "shell");
	let names: Vec<&str> = shell.get_subcommands().map(|s| s.get_name()).collect();

	assert!(names.contains(&"list"), "missing config shell list");
	assert!(names.contains(&"set"), "missing config shell set");
}

#[test]
fn config_path_alias_has_subcommands() {
	let cmd = Cli::command();
	let config = find_subcommand(&cmd, ":config");
	let pa = find_subcommand(config, "path-alias");
	let names: Vec<&str> = pa.get_subcommands().map(|s| s.get_name()).collect();

	assert!(names.contains(&"add"), "missing config path-alias add");
	assert!(names.contains(&"list"), "missing config path-alias list");
	assert!(names.contains(&"remove"), "missing config path-alias remove");
}

#[test]
fn config_global_files_has_subcommands() {
	let cmd = Cli::command();
	let config = find_subcommand(&cmd, ":config");
	let gf = find_subcommand(config, "global-files");
	let names: Vec<&str> = gf.get_subcommands().map(|s| s.get_name()).collect();

	assert!(names.contains(&"add"), "missing config global-files add");
	assert!(names.contains(&"list"), "missing config global-files list");
	assert!(names.contains(&"remove"), "missing config global-files remove");
}
