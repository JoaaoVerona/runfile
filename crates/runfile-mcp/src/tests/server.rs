use super::*;

// ── MCP Server construction tests ─────────────────────────────────

#[test]
fn server_can_be_constructed() {
	use crate::server::RunfileMcpServer;
	let runfile = make_runfile(vec![
		("build", simple_spec(vec!["cargo build"], Some("Build"))),
		("test", simple_spec(vec!["cargo test {{ ARGS }}"], Some("Test"))),
	]);
	// Just verify it doesn't panic
	let _server = RunfileMcpServer::new(
		&runfile,
		std::path::PathBuf::from("run"),
		std::path::PathBuf::from(RUNFILE_NAME),
	);
}

#[test]
fn server_empty_runfile() {
	use crate::server::RunfileMcpServer;
	// Need at least one target for a valid Runfile, but let's test the server
	// handles a single target fine
	let runfile = make_runfile(vec![("hello", simple_spec(vec!["echo hello"], None))]);
	let _server = RunfileMcpServer::new(
		&runfile,
		std::path::PathBuf::from("run"),
		std::path::PathBuf::from(RUNFILE_NAME),
	);
}

#[test]
fn build_tools_excludes_internal_targets() {
	let runfile = make_runfile(vec![
		("build", simple_spec(vec!["cargo build"], None)),
		("_setup", simple_spec(vec!["echo internal"], None)),
		("test", simple_spec(vec!["cargo test"], None)),
	]);
	let tools = build_tool_defs(&runfile);
	let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
	assert_eq!(names, vec!["build", "test"]);
	assert!(!names.contains(&"_setup"));
}
