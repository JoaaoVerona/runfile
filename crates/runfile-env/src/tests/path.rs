use super::*;

#[test]
fn shell_path_beats_runfile_env_path_override() {
	// User tries to set PATH via Runfile env; shell's PATH must win.
	let dir = TempDir::new().unwrap();
	let mut cmd_env = HashMap::new();
	cmd_env.insert("PATH".to_string(), "/should/be/wiped/by/shell".to_string());

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
	let path = get_path_value(&env);
	let shell_path = std::env::var("PATH").unwrap_or_default();

	assert!(
		!path.contains("/should/be/wiped/by/shell"),
		"shell PATH must win over the Runfile-set PATH; got {path}"
	);
	assert_eq!(
		path, shell_path,
		"with no addToPath, final PATH should equal shell PATH"
	);
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
		parent_add_to_path_chain: None,
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
		parent_add_to_path_chain: None,
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
		parent_add_to_path_chain: None,
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
fn add_to_path_wins_even_when_runfile_env_tries_to_replace_path() {
	// User sets PATH via Runfile env AND has addToPath. Shell wipes the
	// Runfile-set PATH (step 3), then addToPath prepends to shell PATH (step 4).
	let dir = TempDir::new().unwrap();
	let mut cmd_env = HashMap::new();
	cmd_env.insert("PATH".to_string(), "/should/be/wiped".to_string());
	let paths = vec!["my_bin".to_string()];

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&cmd_env),
		add_to_path: Some(&paths),
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	let path = get_path_value(&env).replace('\\', "/");
	let resolved = dir.path().join("my_bin").to_string_lossy().replace('\\', "/");

	assert!(path.contains(&resolved), "addToPath entry should be in PATH");
	assert!(
		!path.contains("/should/be/wiped"),
		"the Runfile-set PATH should never reach the final env"
	);
}

#[test]
fn parent_add_to_path_chain_innermost_wins() {
	// Simulates A → @B → @C: the chain handed to C is [A_addToPath, B_addToPath]
	// (outermost first). C's own addToPath is the innermost. Final PATH order:
	// [C_paths, B_paths, A_paths, shell PATH].
	//
	// Use TempDir-derived absolute paths so the test is portable: on Windows
	// `/abs/...` isn't absolute (no drive letter) and would get resolved
	// against the working_dir, mangling the prefix string.
	let dir = TempDir::new().unwrap();
	let grand_path = dir.path().join("grand_bin").to_string_lossy().to_string();
	let parent_path = dir.path().join("parent_bin").to_string_lossy().to_string();
	let dep_path = dir.path().join("dep_bin").to_string_lossy().to_string();

	let parent_chain: Vec<Vec<String>> = vec![vec![grand_path.clone()], vec![parent_path.clone()]];
	let dep_paths = vec![dep_path.clone()];

	let params = EnvBuildParams {
		env_files: None,
		env: None,
		add_to_path: Some(&dep_paths),
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: Some(&parent_chain),
	};
	let env = build_env(&params, &no_substitute).unwrap();
	let path = get_path_value(&env);
	let separator = if cfg!(windows) { ";" } else { ":" };

	let dep_idx = path.find(&dep_path).expect("dep entry missing");
	let parent_idx = path.find(&parent_path).expect("parent entry missing");
	let grand_idx = path.find(&grand_path).expect("grand entry missing");
	assert!(
		dep_idx < parent_idx && parent_idx < grand_idx,
		"order must be dep < parent < grand (innermost first); got {path}"
	);

	let expected_prefix = format!("{dep_path}{separator}{parent_path}{separator}{grand_path}{separator}");
	assert!(
		path.starts_with(&expected_prefix),
		"PATH should start with full chain in innermost-first order; got {path}"
	);
}

#[test]
fn parent_chain_re_prepended_after_shell_overlay_wipes_parent_resolved_path() {
	// Realistic @dep flow: parent's resolved env (passed as base_env) carries
	// a stale PATH that already had parent's addToPath baked in. The shell
	// overlay wipes that, then we re-prepend the chain.
	let dir = TempDir::new().unwrap();
	let mut parent_resolved = HashMap::new();
	parent_resolved.insert("PATH".to_string(), "/abs/parent/bin:/old/system/snapshot".to_string());
	parent_resolved.insert("PARENT_KEPT".to_string(), "from_parent".to_string());

	let parent_chain: Vec<Vec<String>> = vec![vec!["/abs/parent/bin".to_string()]];
	let dep_paths = vec!["/abs/dep/bin".to_string()];

	let params = EnvBuildParams {
		env_files: None,
		env: None,
		add_to_path: Some(&dep_paths),
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: Some(&parent_resolved),
		parent_add_to_path_chain: Some(&parent_chain),
	};
	let env = build_env(&params, &no_substitute).unwrap();
	let path = get_path_value(&env).replace('\\', "/");
	let shell_path = std::env::var("PATH").unwrap_or_default().replace('\\', "/");

	assert!(
		!path.contains("/old/system/snapshot"),
		"stale parent-resolved PATH must be wiped by the shell overlay; got {path}"
	);
	assert!(path.contains("/abs/parent/bin"), "parent chain entry should be in PATH");
	assert!(path.contains("/abs/dep/bin"), "dep addToPath should be in PATH");
	assert!(
		path.ends_with(&shell_path),
		"shell PATH should be at the tail (after the chain); got {path}"
	);
	let dep_idx = path.find("/abs/dep/bin").unwrap();
	let parent_idx = path.find("/abs/parent/bin").unwrap();
	assert!(dep_idx < parent_idx, "dep should precede parent in PATH");

	// Non-PATH parent contribution survives because shell doesn't define PARENT_KEPT.
	assert_eq!(env.get("PARENT_KEPT").unwrap(), "from_parent");
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
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	assert_eq!(
		env.get("RUNFILE_TEST_DEP_BEATS_PARENT_99").unwrap(),
		"from_dep",
		"dep's later layer should win over parent's value when shell doesn't define the key"
	);
}

#[test]
fn shell_beats_dep_runfile_env_too_for_keys_in_shell() {
	// Shell-wins applies to dep contributions, not just top-level. Setting PATH
	// in dep's env doesn't survive — shell overlay wipes it.
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
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	let path = get_path_value(&env);
	let shell_path = std::env::var("PATH").unwrap_or_default();

	assert!(!path.contains("/dep/tries/to/win"));
	assert!(!path.contains("/parent/baked/path"));
	assert_eq!(path, shell_path);
}

#[test]
fn empty_chain_with_no_local_add_to_path_leaves_path_untouched() {
	// Sanity: when neither chain nor local target contributes anything, PATH
	// equals shell PATH exactly (no separator/empty-entry edge cases).
	let dir = TempDir::new().unwrap();
	let parent_chain: Vec<Vec<String>> = vec![];

	let params = EnvBuildParams {
		env_files: None,
		env: None,
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: Some(&parent_chain),
	};
	let env = build_env(&params, &no_substitute).unwrap();
	let path = get_path_value(&env);
	let shell_path = std::env::var("PATH").unwrap_or_default();
	assert_eq!(path, shell_path);
}

#[test]
fn empty_inner_chain_layer_does_not_emit_stray_separator() {
	// Defensive: a chain entry that's an empty Vec (e.g. ancestor had
	// `addToPath: []`) shouldn't add a stray separator that turns into an
	// empty PATH segment.
	let dir = TempDir::new().unwrap();
	let parent_chain: Vec<Vec<String>> = vec![vec![]];

	let params = EnvBuildParams {
		env_files: None,
		env: None,
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: Some(&parent_chain),
	};
	let env = build_env(&params, &no_substitute).unwrap();
	let path = get_path_value(&env);
	let shell_path = std::env::var("PATH").unwrap_or_default();
	assert_eq!(path, shell_path, "empty chain layer should not perturb PATH");
}
