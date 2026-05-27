use super::*;

// ── excludeFromGenerateCommand metadata flag ──────────────────────────

#[test]
fn zed_excludes_flagged_targets() {
	let runfile = make_runfile_with_excluded(
		vec![
			("build", vec!["cargo build"]),
			("internal-helper", vec!["echo skip me"]),
			("test", vec!["cargo test"]),
		],
		&["internal-helper"],
	);
	let tasks = generate_zed_tasks(&runfile);
	let labels: Vec<&str> = tasks.iter().map(|t| t.label.as_str()).collect();
	assert_eq!(labels, vec!["run build", "run test"]);
}

#[test]
fn jetbrains_excludes_flagged_targets() {
	let runfile = make_runfile_with_excluded(
		vec![
			("build", vec!["cargo build"]),
			("private", vec!["echo skip me"]),
			("test", vec!["cargo test"]),
		],
		&["private"],
	);
	let configs = generate_jetbrains_configs(&runfile);
	let names: Vec<&str> = configs.iter().map(|c| c.config_name.as_str()).collect();
	assert_eq!(names, vec!["Build", "Test"]);
}

#[test]
fn vscode_excludes_flagged_targets() {
	let runfile = make_runfile_with_excluded(
		vec![
			("build", vec!["cargo build"]),
			("private", vec!["echo skip me"]),
			("test", vec!["cargo test"]),
		],
		&["private"],
	);
	let tasks = generate_vscode_tasks(&runfile);
	let labels: Vec<&str> = tasks.iter().map(|t| t.label.as_str()).collect();
	assert_eq!(labels, vec!["run build", "run test"]);
}

#[test]
fn excluded_flag_default_false_keeps_targets() {
	// Metadata present but excludeFromGenerateCommand omitted → not excluded.
	let mut rf = make_runfile(vec![("build", vec!["cargo build"])]);
	rf.targets.get_mut("build").unwrap().metadata = Some(Metadata {
		exclude_from_generate_command: None,
		extra: Default::default(),
	});
	assert_eq!(generate_zed_tasks(&rf).len(), 1);
	assert_eq!(generate_vscode_tasks(&rf).len(), 1);
	assert_eq!(generate_jetbrains_configs(&rf).len(), 1);
}

#[test]
fn excluded_flag_explicit_false_keeps_targets() {
	let mut rf = make_runfile(vec![("build", vec!["cargo build"])]);
	rf.targets.get_mut("build").unwrap().metadata = Some(Metadata {
		exclude_from_generate_command: Some(false),
		extra: Default::default(),
	});
	assert_eq!(generate_zed_tasks(&rf).len(), 1);
	assert_eq!(generate_vscode_tasks(&rf).len(), 1);
	assert_eq!(generate_jetbrains_configs(&rf).len(), 1);
}
