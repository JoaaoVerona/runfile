use super::*;

#[test]
fn build_env_with_no_extras() {
	let dir = TempDir::new().unwrap();
	let params = EnvBuildParams {
		env_files: None,
		env: None,
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	// Should contain system env vars
	assert!(!env.is_empty());
	// PATH should be present
	assert!(env.iter().any(|(k, _)| k.eq_ignore_ascii_case("PATH")));
}

#[test]
fn build_env_with_env() {
	let dir = TempDir::new().unwrap();
	let mut global_env = HashMap::new();
	global_env.insert("MY_GLOBAL".to_string(), "global_value".to_string());

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&global_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	assert_eq!(env.get("MY_GLOBAL").unwrap(), "global_value");
}

#[test]
fn build_env_with_env_value() {
	let dir = TempDir::new().unwrap();
	let mut cmd_env = HashMap::new();
	cmd_env.insert("KEY".to_string(), "command".to_string());

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&cmd_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	assert_eq!(env.get("KEY").unwrap(), "command");
}

#[test]
fn build_env_add_to_path() {
	let dir = TempDir::new().unwrap();
	let paths = vec!["global_bin".to_string()];

	let params = EnvBuildParams {
		env_files: None,
		env: None,
		add_to_path: Some(&paths),
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	let path = get_path_value(&env).replace('\\', "/");
	let expected = dir.path().join("global_bin").to_string_lossy().replace('\\', "/");
	assert!(path.contains(&expected), "PATH should contain global_bin: {path}");
}

#[test]
fn build_env_add_to_path_multiple() {
	let dir = TempDir::new().unwrap();
	let paths = vec!["cmd_bin".to_string(), "global_bin".to_string()];

	let params = EnvBuildParams {
		env_files: None,
		env: None,
		add_to_path: Some(&paths),
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	let path = get_path_value(&env).replace('\\', "/");
	let cmd_expected = dir.path().join("cmd_bin").to_string_lossy().replace('\\', "/");
	let global_expected = dir.path().join("global_bin").to_string_lossy().replace('\\', "/");
	assert!(path.contains(&cmd_expected), "cmd_bin should be in PATH");
	assert!(path.contains(&global_expected), "global_bin should be in PATH");
}

#[test]
fn build_env_env_files_before_env() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "KEY=from_file\n").unwrap();

	let mut cmd_env = HashMap::new();
	cmd_env.insert("KEY".to_string(), "from_env".to_string());

	let params = EnvBuildParams {
		env_files: Some(&[".env".to_string()]),
		env: Some(&cmd_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	// env (inline) should override envFiles
	let env = build_env(&params, &no_substitute).unwrap();
	assert_eq!(env.get("KEY").unwrap(), "from_env");
}

#[test]
fn build_env_env_files_load() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "GLOBAL_KEY=global_value\n").unwrap();

	let params = EnvBuildParams {
		env_files: Some(&[".env".to_string()]),
		env: None,
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	assert_eq!(env.get("GLOBAL_KEY").unwrap(), "global_value");
}

#[test]
fn build_env_env_files() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env.target"), "KEY=target\n").unwrap();

	let params = EnvBuildParams {
		env_files: Some(&[".env.target".to_string()]),
		env: None,
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	assert_eq!(env.get("KEY").unwrap(), "target");
}

#[test]
fn build_env_substitution_in_env_values() {
	let dir = TempDir::new().unwrap();
	let mut cmd_env = HashMap::new();
	cmd_env.insert("GREETING".to_string(), "hello {{ NAME }}".to_string());

	// Substitute that replaces {{ NAME }} with "world"
	let substitute = |input: &str, _env: &HashMap<String, String>| -> Result<String, String> {
		Ok(input.replace("{{ NAME }}", "world"))
	};

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&cmd_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &substitute).unwrap();
	assert_eq!(env.get("GREETING").unwrap(), "hello world");
}

#[test]
fn build_env_substitution_error_propagated() {
	let dir = TempDir::new().unwrap();
	let mut cmd_env = HashMap::new();
	cmd_env.insert("KEY".to_string(), "{{ MISSING }}".to_string());

	let substitute = |input: &str, _env: &HashMap<String, String>| -> Result<String, String> {
		if input.contains("{{ MISSING }}") {
			Err("missing variable".to_string())
		} else {
			Ok(input.to_string())
		}
	};

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&cmd_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let result = build_env(&params, &substitute);
	assert!(result.is_err());
}

// ══════════════════════════════════════════════════════════════════════
// check_env_case_duplicates tests
// ══════════════════════════════════════════════════════════════════════

#[test]
fn check_env_case_duplicates_ok() {
	let mut env = HashMap::new();
	env.insert("NODE_ENV".to_string(), "production".to_string());
	env.insert("OTHER_VAR".to_string(), "value".to_string());
	assert!(check_env_case_duplicates(&env).is_ok());
}

#[test]
fn check_env_case_duplicates_detects_conflict() {
	let mut env = HashMap::new();
	env.insert("NODE_ENV".to_string(), "production".to_string());
	env.insert("node_env".to_string(), "development".to_string());
	let result = check_env_case_duplicates(&env);
	assert!(result.is_err());
}

#[test]
fn check_env_case_duplicates_same_case_ok() {
	let mut env = HashMap::new();
	env.insert("KEY".to_string(), "value".to_string());
	assert!(check_env_case_duplicates(&env).is_ok());
}

#[test]
fn check_env_case_duplicates_empty_env_ok() {
	let env: HashMap<String, String> = HashMap::new();
	assert!(check_env_case_duplicates(&env).is_ok());
}

// ══════════════════════════════════════════════════════════════════════
// collect_runfile_env tests
// ══════════════════════════════════════════════════════════════════════

#[test]
fn collect_runfile_env_empty() {
	let result = collect_runfile_env(None);
	assert!(result.is_empty());
}

#[test]
fn collect_runfile_env_with_values() {
	let mut env = HashMap::new();
	env.insert("A".to_string(), "1".to_string());
	let result = collect_runfile_env(Some(&env));
	assert_eq!(result, vec![("A".to_string(), "1".to_string())]);
}

#[test]
fn collect_runfile_env_sorted() {
	let mut env = HashMap::new();
	env.insert("Z".to_string(), "last".to_string());
	env.insert("A".to_string(), "first".to_string());
	let result = collect_runfile_env(Some(&env));
	assert_eq!(result[0].0, "A");
	assert_eq!(result[1].0, "Z");
}

// ══════════════════════════════════════════════════════════════════════
// Encrypted env tests
// ══════════════════════════════════════════════════════════════════════
