use super::*;

#[test]
fn build_env_decrypts_via_public_key_matching() {
	let dir = TempDir::new().unwrap();
	let key_hex = runfile_crypto::generate_key();
	let public_key = runfile_crypto::derive_public_key(&key_hex).unwrap();
	let encrypted = runfile_crypto::encrypt("secret_password", &key_hex).unwrap();
	let private_keys = vec![key_hex];

	let mut cmd_env = HashMap::new();
	cmd_env.insert("DB_PASS".to_string(), encrypted);
	cmd_env.insert("PLAIN_VAR".to_string(), "plain_value".to_string());
	cmd_env.insert(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR.to_string(), public_key);

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&cmd_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: Some(&private_keys),
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	assert_eq!(env.get("DB_PASS").unwrap(), "secret_password");
	assert_eq!(env.get("PLAIN_VAR").unwrap(), "plain_value");
}

#[test]
fn build_env_encrypted_no_public_key_no_keys_errors() {
	let dir = TempDir::new().unwrap();
	let key_hex = runfile_crypto::generate_key();
	let encrypted = runfile_crypto::encrypt("secret", &key_hex).unwrap();

	let mut cmd_env = HashMap::new();
	cmd_env.insert("SECRET".to_string(), encrypted);
	// No RUNFILE_ENCRYPTION_PUBLIC_KEY, no key pool

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
	let result = build_env(&params, &no_substitute);
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(err.contains("Encryption error"), "got: {err}");
}

#[test]
fn build_env_encrypted_no_matching_private_key_errors() {
	let dir = TempDir::new().unwrap();
	let key_hex = runfile_crypto::generate_key();
	let wrong_key = runfile_crypto::generate_key();
	let public_key = runfile_crypto::derive_public_key(&key_hex).unwrap();
	let encrypted = runfile_crypto::encrypt("secret", &key_hex).unwrap();
	let wrong_keys = vec![wrong_key];

	let mut cmd_env = HashMap::new();
	cmd_env.insert("SECRET".to_string(), encrypted);
	cmd_env.insert(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR.to_string(), public_key);

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&cmd_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: Some(&wrong_keys),
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let result = build_env(&params, &no_substitute);
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(err.contains("no matching private key"), "got: {err}");
}

#[test]
fn build_env_decrypts_env_file_with_public_key() {
	let dir = TempDir::new().unwrap();
	let key_hex = runfile_crypto::generate_key();
	let public_key = runfile_crypto::derive_public_key(&key_hex).unwrap();
	let encrypted = runfile_crypto::encrypt("from_file_secret", &key_hex).unwrap();
	let private_keys = vec![key_hex];

	std::fs::write(
		dir.path().join(".env"),
		format!(
			"{}={public_key}\nFILE_SECRET={encrypted}\n",
			runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR
		),
	)
	.unwrap();

	let params = EnvBuildParams {
		env_files: Some(&[".env".to_string()]),
		env: None,
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: Some(&private_keys),
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	assert_eq!(env.get("FILE_SECRET").unwrap(), "from_file_secret");
}

// ══════════════════════════════════════════════════════════════════════
// Encrypted-without-public-key error
// ══════════════════════════════════════════════════════════════════════

#[test]
fn encrypted_value_without_public_key_header_errors() {
	// An encrypted value with no RUNFILE_ENCRYPTION_PUBLIC_KEY header used to
	// be decryptable via the now-removed RUNFILE_ENCRYPTION_KEY env var.
	// After collapsing to a single env-var path, the only supported way to
	// decrypt is via a public-key fingerprint matched against the key pool.
	// Files without the header are produced by no Runfile tooling, so this
	// path is an error.
	let dir = TempDir::new().unwrap();
	let key = runfile_crypto::generate_key();
	let encrypted = runfile_crypto::encrypt("secret", &key).unwrap();

	let mut cmd_env = HashMap::new();
	cmd_env.insert("SECRET".to_string(), encrypted);
	let private_keys = vec![key];

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&cmd_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: Some(&private_keys),
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let result = build_env(&params, &no_substitute);

	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(
		err.contains("RUNFILE_ENCRYPTION_PUBLIC_KEY"),
		"error should point at missing public key header: {err}"
	);
}

// ══════════════════════════════════════════════════════════════════════
// Priority order tests
//
// Final ordering for `build_env` (low → high):
//   1. envFiles (later file wins per key)
//   2. env (substituted; wins over envFiles within the Runfile layer)
//   3. **current shell env always wins** — re-overlays Runfile-defined keys
//   4. addToPath chain — for PATH only, prepended innermost-first
//      (`[this target's addToPath..., parent's..., ..., shell PATH]`)
//   5. decryption
//
// PATH (from std::env::vars()) is the only system var we can rely on being
// present cross-platform without mutating the test process env.
// ══════════════════════════════════════════════════════════════════════
