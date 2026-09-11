use super::*;

#[test]
fn a_target_path_assignment_replaces_the_search_path() {
	// `.env.PATH` is an assignment like any other, so it wins over the shell's
	// PATH. The shell used to win *here* while the runtime laid the property
	// back on top for commands -- so a command ran with the property's PATH
	// anyway, and `{{ ENV.PATH }}` said something else. To add a directory
	// rather than replace the whole path, `.add-path` is the property.
	let dir = TempDir::new().unwrap();
	let mut cmd_env = HashMap::new();
	cmd_env.insert("PATH".to_string(), "/opt/only/this".to_string());

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&cmd_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	assert_eq!(get_path_value(&env), "/opt/only/this", "the assignment is the path");
}

#[test]
fn shell_path_beats_runfile_envfile_path_override() {
	// Same idea but PATH coming from an env file.
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "PATH=/from/file/should/be/wiped\n").unwrap();

	let params = EnvBuildParams {
		env_files: Some(&[".env".to_string()]),
		env: None,
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	let path = get_path_value(&env);

	assert!(
		!path.contains("/from/file/should/be/wiped"),
		"shell PATH must win over envFile-set PATH; got {path}"
	);
}

#[test]
fn runfile_env_kept_for_keys_not_in_shell() {
	// Keys that don't conflict with shell vars survive untouched.
	let dir = TempDir::new().unwrap();
	let mut cmd_env = HashMap::new();
	cmd_env.insert("RUNFILE_TEST_UNIQUE_KEY_42".to_string(), "runfile_kept".to_string());

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&cmd_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	assert_eq!(env.get("RUNFILE_TEST_UNIQUE_KEY_42").unwrap(), "runfile_kept");
}

#[test]
fn add_to_path_prepends_to_shell_path_after_overlay() {
	// addToPath is applied AFTER the shell-env overlay, so it always lands at
	// the front of PATH — never gets wiped by the overlay.
	let dir = TempDir::new().unwrap();
	let paths = vec!["custom_bin".to_string()];

	let params = EnvBuildParams {
		env_files: None,
		env: None,
		add_to_path: Some(&paths),
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	let path = get_path_value(&env).replace('\\', "/");
	let resolved = dir.path().join("custom_bin").to_string_lossy().replace('\\', "/");
	let shell_path = std::env::var("PATH").unwrap_or_default().replace('\\', "/");

	let separator = if cfg!(windows) { ";" } else { ":" };
	let expected_prefix = format!("{resolved}{separator}");
	assert!(
		path.starts_with(&expected_prefix),
		"addToPath entry should be at the front of PATH; got {path}"
	);
	assert!(
		path.ends_with(&shell_path),
		"shell PATH should be preserved at the tail; got {path}"
	);
}

#[test]
fn add_path_prepends_onto_a_target_path_assignment() {
	// Both at once: the assignment sets the path, and `.add-path` goes in
	// front of whatever the path turned out to be. The runtime's old overlay
	// replaced the whole value for commands, so an `.add-path` beside a
	// `.env.PATH` was silently dropped.
	let dir = TempDir::new().unwrap();
	let mut cmd_env = HashMap::new();
	cmd_env.insert("PATH".to_string(), "/opt/only/this".to_string());
	let paths = vec!["my_bin".to_string()];

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&cmd_env),
		add_to_path: Some(&paths),
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	let path = get_path_value(&env).replace('\\', "/");
	let resolved = dir.path().join("my_bin").to_string_lossy().replace('\\', "/");

	assert!(path.starts_with(&resolved), "the `.add-path` entry comes first: {path}");
	assert!(path.ends_with("/opt/only/this"), "onto the assignment: {path}");
}

#[test]
fn dep_runfile_env_beats_parent_runfile_env_when_shell_does_not_have_key() {
	// For keys not in shell, the dep's env layer wins over parent's because it's
	// applied later. Use a unique key shell can't possibly have.
	let dir = TempDir::new().unwrap();
	let mut parent_resolved = HashMap::new();
	parent_resolved.insert(
		"RUNFILE_TEST_DEP_BEATS_PARENT_99".to_string(),
		"from_parent".to_string(),
	);

	let mut dep_env = HashMap::new();
	dep_env.insert("RUNFILE_TEST_DEP_BEATS_PARENT_99".to_string(), "from_dep".to_string());

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&dep_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: Some(&parent_resolved),
	};
	let env = build_env(&params, &no_substitute).unwrap();
	assert_eq!(
		env.get("RUNFILE_TEST_DEP_BEATS_PARENT_99").unwrap(),
		"from_dep",
		"dep's later layer should win over parent's value when shell doesn't define the key"
	);
}

#[test]
fn a_dependency_path_assignment_wins_over_the_parent_and_the_shell() {
	// The same order under a `base_env`: the parent's layer, then the shell,
	// then this invocation's own assignment.
	let dir = TempDir::new().unwrap();
	let mut parent_resolved = HashMap::new();
	parent_resolved.insert("PATH".to_string(), "/parent/baked/path".to_string());

	let mut dep_env = HashMap::new();
	dep_env.insert("PATH".to_string(), "/dep/tries/to/win".to_string());

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&dep_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: Some(&parent_resolved),
	};
	let env = build_env(&params, &no_substitute).unwrap();
	assert_eq!(get_path_value(&env), "/dep/tries/to/win");
}

#[test]
fn no_add_to_path_leaves_path_untouched() {
	// Sanity: with nothing to prepend, PATH equals the shell's exactly -- no
	// stray separator, no empty leading segment.
	let dir = TempDir::new().unwrap();

	let params = EnvBuildParams {
		env_files: None,
		env: None,
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	let path = get_path_value(&env);
	let shell_path = std::env::var("PATH").unwrap_or_default();
	assert_eq!(path, shell_path);
}
