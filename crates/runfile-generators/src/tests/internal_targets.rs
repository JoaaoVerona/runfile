use super::*;

// ── Internal targets are excluded from generators ─────────────────────

#[test]
fn zed_excludes_internal_targets() {
	let runfile = make_runfile(vec![
		("build", vec!["cargo build"]),
		("_setup", vec!["echo internal"]),
		("test", vec!["cargo test"]),
	]);
	let tasks = generate_zed_tasks(&runfile);
	let labels: Vec<&str> = tasks.iter().map(|t| t.label.as_str()).collect();
	assert_eq!(labels, vec!["run build", "run test"]);
}

#[test]
fn jetbrains_excludes_internal_targets() {
	let runfile = make_runfile(vec![
		("build", vec!["cargo build"]),
		("_setup", vec!["echo internal"]),
		("test", vec!["cargo test"]),
	]);
	let configs = generate_jetbrains_configs(&runfile);
	let names: Vec<&str> = configs.iter().map(|c| c.config_name.as_str()).collect();
	assert_eq!(names, vec!["Build", "Test"]);
}

#[test]
fn vscode_excludes_internal_targets() {
	let runfile = make_runfile(vec![
		("build", vec!["cargo build"]),
		("_setup", vec!["echo internal"]),
		("test", vec!["cargo test"]),
	]);
	let tasks = generate_vscode_tasks(&runfile);
	let labels: Vec<&str> = tasks.iter().map(|t| t.label.as_str()).collect();
	assert_eq!(labels, vec!["run build", "run test"]);
}
