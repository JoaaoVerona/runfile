use super::*;

/// Write a `.env` file and build against it, the one place an `encrypted:`
/// value is the author's own and so is decrypted.
fn build_with_env_file(contents: &str, keys: Option<&Vec<String>>) -> Result<HashMap<String, String>, crate::EnvError> {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), contents).unwrap();
	let files = [".env".to_string()];
	let params = EnvBuildParams {
		env_files: Some(&files),
		env: None,
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: keys.map(|k| k as &dyn crate::PrivateKeyProvider),
		base_env: None,
		defaults: None,
	};
	build_env(&params, &no_substitute)
}

#[test]
fn an_encrypted_env_file_value_is_decrypted_via_its_public_key() {
	let key_hex = runfile_crypto::generate_key();
	let public_key = runfile_crypto::derive_public_key(&key_hex).unwrap();
	let encrypted = runfile_crypto::encrypt("from_file_secret", &key_hex).unwrap();
	let contents = format!(
		"{}={public_key}\nFILE_SECRET={encrypted}\nPLAIN_VAR=plain_value\n",
		runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR
	);
	let env = build_with_env_file(&contents, Some(&vec![key_hex])).unwrap();
	assert_eq!(env.get("FILE_SECRET").unwrap(), "from_file_secret");
	assert_eq!(env.get("PLAIN_VAR").unwrap(), "plain_value");
}

#[test]
fn an_encrypted_env_file_value_with_no_key_available_errors() {
	let key_hex = runfile_crypto::generate_key();
	let public_key = runfile_crypto::derive_public_key(&key_hex).unwrap();
	let encrypted = runfile_crypto::encrypt("secret", &key_hex).unwrap();
	let contents = format!(
		"{}={public_key}\nSECRET={encrypted}\n",
		runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR
	);
	let err = build_with_env_file(&contents, None).unwrap_err().to_string();
	assert!(err.contains("Encryption error"), "got: {err}");
}

#[test]
fn an_encrypted_env_file_value_with_no_matching_key_errors() {
	let key_hex = runfile_crypto::generate_key();
	let wrong_key = runfile_crypto::generate_key();
	let public_key = runfile_crypto::derive_public_key(&key_hex).unwrap();
	let encrypted = runfile_crypto::encrypt("secret", &key_hex).unwrap();
	let contents = format!(
		"{}={public_key}\nSECRET={encrypted}\n",
		runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR
	);
	let err = build_with_env_file(&contents, Some(&vec![wrong_key]))
		.unwrap_err()
		.to_string();
	assert!(err.contains("no matching private key"), "got: {err}");
}

#[test]
fn an_encrypted_env_file_value_without_a_public_key_header_errors() {
	// An encrypted value with no RUNFILE_ENCRYPTION_PUBLIC_KEY header used to be
	// decryptable via the now-removed RUNFILE_ENCRYPTION_KEY env var. The only
	// supported path is a public-key fingerprint matched against the key pool, so
	// a file without the header is an error.
	let key = runfile_crypto::generate_key();
	let encrypted = runfile_crypto::encrypt("secret", &key).unwrap();
	let err = build_with_env_file(&format!("SECRET={encrypted}\n"), Some(&vec![key]))
		.unwrap_err()
		.to_string();
	assert!(
		err.contains("RUNFILE_ENCRYPTION_PUBLIC_KEY"),
		"error should point at missing public key header: {err}"
	);
}

// ══════════════════════════════════════════════════════════════════════
// SA-004: the decryption oracle is closed
//
// Only a `.env-file`'s own values are decrypted. An `encrypted:` ciphertext
// arriving any other way -- exported in the caller's shell, inherited, or
// written into the `.env` block from a value an outsider controls (a PR title a
// CI job exposes as `env:`, or `.env.X = ARG.x`) -- is handed through unchanged,
// never decrypted with the run's keys. Otherwise a ciphertext copied from a
// committed `.env` came back as plaintext wherever the job reflected it.
// ══════════════════════════════════════════════════════════════════════

#[test]
fn an_encrypted_value_in_the_env_block_is_not_decrypted() {
	let dir = TempDir::new().unwrap();
	let key_hex = runfile_crypto::generate_key();
	let public_key = runfile_crypto::derive_public_key(&key_hex).unwrap();
	let encrypted = runfile_crypto::encrypt("secret_password", &key_hex).unwrap();

	let mut cmd_env = HashMap::new();
	// As if `.env.LABEL = ARG.label` were handed a copied ciphertext.
	cmd_env.insert("LABEL".to_string(), encrypted.clone());
	cmd_env.insert(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR.to_string(), public_key);

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&cmd_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: Some(&[key_hex]),
		base_env: None,
		defaults: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	assert_eq!(
		env.get("LABEL").unwrap(),
		&encrypted,
		"the ciphertext is passed through, not decrypted"
	);
}

#[test]
fn an_encrypted_value_in_the_caller_environment_is_not_decrypted() {
	let dir = TempDir::new().unwrap();
	let key_hex = runfile_crypto::generate_key();
	let public_key = runfile_crypto::derive_public_key(&key_hex).unwrap();
	let encrypted = runfile_crypto::encrypt("secret_password", &key_hex).unwrap();

	// The exported shell / inherited environment the build is handed.
	let mut base = HashMap::new();
	base.insert("PR_TITLE".to_string(), encrypted.clone());
	base.insert(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR.to_string(), public_key);

	let params = EnvBuildParams {
		env_files: None,
		env: None,
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: Some(&[key_hex]),
		base_env: Some(&base),
		defaults: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	assert_eq!(
		env.get("PR_TITLE").unwrap(),
		&encrypted,
		"an exported ciphertext is never decrypted"
	);
}

// ══════════════════════════════════════════════════════════════════════
// Priority order tests
//
// Final ordering for `build_env` (low → high):
//   1. defaults (what only a calling target's env files supplied)
//   2. envFiles (later file wins per key; its encrypted values decrypted here)
//   3. the environment the build is called with — the shell, or a calling
//      target's — re-overlaid, so it beats every file
//   4. env (substituted; the target's own assignment beats all of the above)
//   5. addToPath — for PATH only, prepended, this target's entries first
//
// PATH (from std::env::vars()) is the only system var we can rely on being
// present cross-platform without mutating the test process env.
// ══════════════════════════════════════════════════════════════════════
