use super::*;

#[test]
fn generate_key_length() {
	let key = generate_key();
	assert_eq!(key.len(), 64, "key should be 64 hex chars (32 bytes)");
	// Verify it's valid hex
	assert!(hex::decode(&key).is_ok());
}

#[test]
fn generate_key_unique() {
	let key1 = generate_key();
	let key2 = generate_key();
	assert_ne!(key1, key2, "two generated keys should differ");
}

#[test]
fn encrypt_decrypt_roundtrip() {
	let key = generate_key();
	let plaintext = "hello world secret";
	let encrypted = encrypt(plaintext, &key).unwrap();

	assert!(encrypted.starts_with(ENCRYPTED_PREFIX));
	assert_ne!(encrypted, plaintext);

	let decrypted = decrypt(&encrypted, &key).unwrap();
	assert_eq!(decrypted, plaintext);
}

#[test]
fn encrypt_decrypt_empty_string() {
	let key = generate_key();
	let encrypted = encrypt("", &key).unwrap();
	let decrypted = decrypt(&encrypted, &key).unwrap();
	assert_eq!(decrypted, "");
}

#[test]
fn encrypt_decrypt_unicode() {
	let key = generate_key();
	let plaintext = "你好世界 🌍 café";
	let encrypted = encrypt(plaintext, &key).unwrap();
	let decrypted = decrypt(&encrypted, &key).unwrap();
	assert_eq!(decrypted, plaintext);
}

#[test]
fn decrypt_wrong_key_fails() {
	let key1 = generate_key();
	let key2 = generate_key();
	let encrypted = encrypt("secret", &key1).unwrap();

	let result = decrypt(&encrypted, &key2);
	assert!(result.is_err());
}

#[test]
fn decrypt_invalid_base64_fails() {
	let key = generate_key();
	let result = decrypt("encrypted:not-valid-base64!!!", &key);
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(err.contains("base64"), "error should mention base64: {err}");
}

#[test]
fn decrypt_truncated_payload_fails() {
	let key = generate_key();
	// Base64 of just 4 bytes (less than NONCE_SIZE)
	let short_b64 = base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3, 4]);
	let result = decrypt(&format!("encrypted:{short_b64}"), &key);
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(err.contains("too short"), "error should mention too short: {err}");
}

#[test]
fn decrypt_without_prefix() {
	let key = generate_key();
	let encrypted = encrypt("secret", &key).unwrap();
	// Strip the prefix manually
	let b64_part = encrypted.strip_prefix(ENCRYPTED_PREFIX).unwrap();
	let decrypted = decrypt(b64_part, &key).unwrap();
	assert_eq!(decrypted, "secret");
}

#[test]
fn invalid_key_too_short() {
	let result = encrypt("hello", "aabbcc");
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(err.contains("32 bytes"), "error should mention key size: {err}");
}

#[test]
fn invalid_key_not_hex() {
	let result = encrypt(
		"hello",
		"zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
	);
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(err.contains("hex"), "error should mention hex: {err}");
}

#[test]
fn is_encrypted_with_prefix() {
	assert!(is_encrypted("encrypted:abc123"));
	assert!(is_encrypted("encrypted:"));
}

#[test]
fn is_encrypted_without_prefix() {
	assert!(!is_encrypted("plaintext value"));
	assert!(!is_encrypted(""));
	assert!(!is_encrypted("encrypt:almost"));
}

#[test]
fn has_encrypted_values_mixed() {
	let mut env = HashMap::new();
	env.insert("PLAIN".into(), "value".into());
	assert!(!has_encrypted_values(&env));

	env.insert("SECRET".into(), "encrypted:abc".into());
	assert!(has_encrypted_values(&env));
}

#[test]
fn has_encrypted_values_empty() {
	let env: HashMap<String, String> = HashMap::new();
	assert!(!has_encrypted_values(&env));
}

#[test]
fn decrypt_env_values_mixed() {
	let key = generate_key();
	let encrypted_val = encrypt("my-secret", &key).unwrap();

	let mut env = HashMap::new();
	env.insert("PLAIN".into(), "hello".into());
	env.insert("SECRET".into(), encrypted_val);
	env.insert("ALSO_PLAIN".into(), "world".into());

	decrypt_env_values(&mut env, &key).unwrap();

	assert_eq!(env["PLAIN"], "hello");
	assert_eq!(env["SECRET"], "my-secret");
	assert_eq!(env["ALSO_PLAIN"], "world");
}

#[test]
fn decrypt_env_values_no_encrypted() {
	let key = generate_key();
	let mut env = HashMap::new();
	env.insert("A".into(), "1".into());
	env.insert("B".into(), "2".into());

	decrypt_env_values(&mut env, &key).unwrap();

	assert_eq!(env["A"], "1");
	assert_eq!(env["B"], "2");
}

#[test]
fn decrypt_env_values_wrong_key_reports_var_name() {
	let key1 = generate_key();
	let key2 = generate_key();
	let encrypted_val = encrypt("secret", &key1).unwrap();

	let mut env = HashMap::new();
	env.insert("MY_VAR".into(), encrypted_val);

	let result = decrypt_env_values(&mut env, &key2);
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(err.contains("MY_VAR"), "error should mention the var name: {err}");
}

#[test]
fn encrypt_produces_different_ciphertexts() {
	// Same plaintext + same key should produce different ciphertexts due to random nonce
	let key = generate_key();
	let enc1 = encrypt("same", &key).unwrap();
	let enc2 = encrypt("same", &key).unwrap();
	assert_ne!(enc1, enc2, "encryptions should differ due to random nonce");

	// But both should decrypt to the same value
	assert_eq!(decrypt(&enc1, &key).unwrap(), "same");
	assert_eq!(decrypt(&enc2, &key).unwrap(), "same");
}

#[test]
fn derive_public_key_deterministic() {
	let key = generate_key();
	let pub1 = derive_public_key(&key).unwrap();
	let pub2 = derive_public_key(&key).unwrap();
	assert_eq!(pub1, pub2, "same private key should produce same public key");
	assert_eq!(pub1.len(), 64, "public key should be 64 hex chars");
}

#[test]
fn derive_public_key_differs_for_different_keys() {
	let key1 = generate_key();
	let key2 = generate_key();
	let pub1 = derive_public_key(&key1).unwrap();
	let pub2 = derive_public_key(&key2).unwrap();
	assert_ne!(
		pub1, pub2,
		"different private keys should produce different public keys"
	);
}

#[test]
fn derive_public_key_differs_from_private() {
	let key = generate_key();
	let public = derive_public_key(&key).unwrap();
	assert_ne!(key, public, "public key should differ from private key");
}

#[test]
fn find_matching_private_key_found() {
	let key1 = generate_key();
	let key2 = generate_key();
	let pub1 = derive_public_key(&key1).unwrap();
	let keys = vec![key1.clone(), key2];
	assert_eq!(find_matching_private_key(&pub1, &keys), Some(key1));
}

#[test]
fn find_matching_private_key_not_found() {
	let key1 = generate_key();
	let key2 = generate_key();
	let key3 = generate_key();
	let pub3 = derive_public_key(&key3).unwrap();
	let keys = vec![key1, key2];
	assert_eq!(find_matching_private_key(&pub3, &keys), None);
}

#[test]
fn find_matching_private_key_empty_list() {
	let key = generate_key();
	let public = derive_public_key(&key).unwrap();
	assert_eq!(find_matching_private_key(&public, &[]), None);
}

#[test]
fn find_private_key_by_public_prefix_full_match() {
	let key1 = generate_key();
	let key2 = generate_key();
	let pub1 = derive_public_key(&key1).unwrap();
	let keys = vec![key1.clone(), key2];
	// Full public key should match exactly one
	let found = find_private_key_by_public_prefix(&pub1, &keys).unwrap();
	assert_eq!(found, key1);
}

#[test]
fn find_private_key_by_public_prefix_partial_match() {
	let key1 = generate_key();
	let key2 = generate_key();
	let pub1 = derive_public_key(&key1).unwrap();
	let keys = vec![key1.clone(), key2];
	// First 16 chars of public key should be enough to match
	let found = find_private_key_by_public_prefix(&pub1[..16], &keys).unwrap();
	assert_eq!(found, key1);
}

#[test]
fn find_private_key_by_public_prefix_no_match() {
	let key = generate_key();
	let keys = vec![key];
	let result = find_private_key_by_public_prefix("zzzzzz", &keys);
	assert!(result.is_err());
}

#[test]
fn find_private_key_by_public_prefix_ambiguous() {
	// We need two keys whose public keys share a prefix.
	// Brute-force is impractical, so we just test the error path with a prefix
	// that matches both (empty prefix matches everything).
	let key1 = generate_key();
	let key2 = generate_key();
	let keys = vec![key1, key2];
	let result = find_private_key_by_public_prefix("", &keys);
	assert!(result.is_err());
	assert!(result.unwrap_err().to_string().contains("2 keys"));
}
