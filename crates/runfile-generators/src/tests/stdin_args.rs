use super::*;

// ── --stdin-args insertion across generators ──────────────────────────

#[test]
fn vscode_args_include_stdin_args_flag() {
	let runfile = make_runfile(vec![("build", vec!["cargo build"])]);
	let tasks = generate_vscode_tasks(&runfile);
	assert_eq!(tasks[0].command, "run");
	assert_eq!(tasks[0].args, vec!["--stdin-args", "build"]);
}

#[test]
fn vscode_args_target_using_args_keeps_input_args_after_stdin_args() {
	// `${input:args}` still works for callers who know what to pass; missing
	// values are then prompted in the integrated terminal.
	let runfile = make_runfile(vec![("test", vec!["cargo test {{ ARGS }}"])]);
	let tasks = generate_vscode_tasks(&runfile);
	assert_eq!(tasks[0].args, vec!["--stdin-args", "test", "${input:args}"]);
}

#[test]
fn jetbrains_xml_includes_stdin_args_flag() {
	let runfile = make_runfile(vec![("build", vec!["cargo build"])]);
	let xml = &generate_jetbrains_configs(&runfile)[0].xml;
	assert!(xml.contains("value=\"run --stdin-args build\""));
}
