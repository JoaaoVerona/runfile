use super::*;

/// Write a `.env` file and read it, the one place an `encrypted:` value is the
/// author's own and so is decrypted.
fn build_with_env_file(contents: &str, keys: Option<&Vec<String>>) -> Result<HashMap<String, String>, crate::EnvError> {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), contents).unwrap();
	read_env_file(
		&dir.path().join(".env"),
		&HashMap::new(),
		keys.map(|k| k as &dyn PrivateKeyProvider),
	)
	.map(|v| v.expect("the file is there"))
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

#[test]
fn a_public_key_already_in_the_environment_decrypts_a_file_that_names_none() {
	// An encrypted file may leave its public key to a file loaded before it,
	// or to what a calling target handed over.
	let dir = TempDir::new().unwrap();
	let key_hex = runfile_crypto::generate_key();
	let public_key = runfile_crypto::derive_public_key(&key_hex).unwrap();
	let encrypted = runfile_crypto::encrypt("the-secret", &key_hex).unwrap();
	std::fs::write(dir.path().join(".env.secrets"), format!("SECRET={encrypted}\n")).unwrap();
	let env = HashMap::from([(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR.to_string(), public_key)]);
	let keys = vec![key_hex];
	let vars = read_env_file(&dir.path().join(".env.secrets"), &env, Some(&keys))
		.unwrap()
		.unwrap();
	assert_eq!(vars["SECRET"], "the-secret");
}

/// A key pool that fails the test if anything asks it.
struct NeverAsked;

impl PrivateKeyProvider for NeverAsked {
	fn keys(&self) -> &[String] {
		panic!("the key pool was asked for, with nothing encrypted to decrypt");
	}
}

#[test]
fn a_file_with_nothing_encrypted_in_it_asks_for_no_keys() {
	// Asking would be an unlock prompt for nothing.
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "PLAIN=1\n").unwrap();
	let vars = read_env_file(&dir.path().join(".env"), &HashMap::new(), Some(&NeverAsked))
		.unwrap()
		.unwrap();
	assert_eq!(vars["PLAIN"], "1");
}

// ══════════════════════════════════════════════════════════════════════
// SA-004: the decryption oracle is closed
//
// Only a file's own values are decrypted. An `encrypted:` ciphertext arriving
// any other way -- exported in the caller's shell, inherited, or set with
// `ENV.X = ARG.x` from a value an outsider controls -- is never decrypted with
// the run's keys: `read_env_file` hands back the file's values and nothing
// else, and never asks the pool for a file with nothing encrypted in it.
// ══════════════════════════════════════════════════════════════════════

#[test]
fn an_encrypted_value_already_in_the_environment_is_neither_decrypted_nor_handed_back() {
	let key_hex = runfile_crypto::generate_key();
	let public_key = runfile_crypto::derive_public_key(&key_hex).unwrap();
	let ciphertext = runfile_crypto::encrypt("committed-secret", &key_hex).unwrap();
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "PLAIN=1\n").unwrap();
	let env = HashMap::from([
		(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR.to_string(), public_key),
		("PR_TITLE".to_string(), ciphertext),
	]);
	let vars = read_env_file(&dir.path().join(".env"), &env, Some(&NeverAsked))
		.unwrap()
		.unwrap();
	assert_eq!(vars.len(), 1, "only the file's own values: {:?}", vars.keys());
}
