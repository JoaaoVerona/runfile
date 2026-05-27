use super::*;

// ── Zeroize tests ─────────────────────────────────────────────────

#[test]
fn make_cipher_zeroizes_on_bad_length() {
	// Key that is valid hex but wrong length (16 bytes instead of 32)
	let short_key = "aa".repeat(16);
	let result = encrypt("test", &short_key);
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(err.contains("32 bytes"), "should report key size: {err}");
}

#[test]
fn derive_public_key_zeroizes_on_bad_length() {
	let short_key = "bb".repeat(16);
	let result = derive_public_key(&short_key);
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(err.contains("32 bytes"), "should report key size: {err}");
}

#[test]
fn encrypt_decrypt_roundtrip_after_zeroize_changes() {
	// Ensure the zeroize changes didn't break normal encrypt/decrypt
	let key = generate_key();
	let large = "x".repeat(10_000);
	let values = vec![
		"simple",
		"with spaces and symbols: !@#$%^&*()",
		"unicode: 日本語 🎉",
		"",
		"a",
		large.as_str(),
	];
	for plaintext in values {
		let encrypted = encrypt(plaintext, &key).unwrap();
		let decrypted = decrypt(&encrypted, &key).unwrap();
		assert_eq!(decrypted, plaintext);
	}
}

#[test]
fn make_cipher_error_on_invalid_init() {
	// Verify the unwrap() -> map_err change works: non-hex key
	let result = encrypt(
		"hello",
		"not-hex-at-all-not-hex-at-all-not-hex-at-all-not-hex-at-all-xxxx",
	);
	assert!(result.is_err());
}

// ── Decrypt with zeroize tests ────────────────────────────────────

#[test]
fn decrypt_still_works_after_zeroize_changes() {
	// Verify the Zeroizing wrapper doesn't break decryption
	let key = generate_key();
	let large = "x".repeat(100_000);
	let values = vec![
		"",
		"a",
		"simple value",
		"unicode: こんにちは 🌍",
		"special chars: !@#$%^&*(){}[]|\\/<>?",
		&large,
	];
	for plaintext in values {
		let encrypted = encrypt(plaintext, &key).unwrap();
		let decrypted = decrypt(&encrypted, &key).unwrap();
		assert_eq!(decrypted, plaintext);
	}
}

#[test]
fn decrypt_env_values_still_works_with_zeroize() {
	let key = generate_key();
	let enc1 = encrypt("secret1", &key).unwrap();
	let enc2 = encrypt("secret2", &key).unwrap();

	let mut env = HashMap::new();
	env.insert("DB_PASS".into(), enc1);
	env.insert("API_KEY".into(), enc2);
	env.insert("PLAIN".into(), "visible".into());

	decrypt_env_values(&mut env, &key).unwrap();
	assert_eq!(env["DB_PASS"], "secret1");
	assert_eq!(env["API_KEY"], "secret2");
	assert_eq!(env["PLAIN"], "visible");
}
