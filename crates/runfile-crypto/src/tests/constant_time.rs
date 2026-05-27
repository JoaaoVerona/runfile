use super::*;

// ── Constant-time comparison tests ────────────────────────────────

#[test]
fn find_matching_private_key_ct_eq_correct_match() {
	// Verify that the constant-time comparison still finds the right key
	let key1 = generate_key();
	let key2 = generate_key();
	let key3 = generate_key();
	let pub2 = derive_public_key(&key2).unwrap();
	let keys = vec![key1.clone(), key2.clone(), key3.clone()];
	assert_eq!(find_matching_private_key(&pub2, &keys), Some(key2));
}

#[test]
fn find_matching_private_key_ct_eq_no_match() {
	// Constant-time comparison must still correctly reject non-matching keys
	let key1 = generate_key();
	let key2 = generate_key();
	let unrelated = generate_key();
	let pub_unrelated = derive_public_key(&unrelated).unwrap();
	let keys = vec![key1, key2];
	assert_eq!(find_matching_private_key(&pub_unrelated, &keys), None);
}

#[test]
fn find_matching_private_key_ct_eq_rejects_prefix_match() {
	// A prefix of the public key should NOT match (constant-time eq requires same length)
	let key = generate_key();
	let public = derive_public_key(&key).unwrap();
	let prefix = &public[..32]; // half the public key
	let keys = vec![key.clone()];
	// ct_eq on different-length slices returns false
	assert_eq!(find_matching_private_key(prefix, &keys), None);
}

#[test]
fn find_matching_private_key_ct_eq_rejects_empty() {
	let key = generate_key();
	let keys = vec![key];
	assert_eq!(find_matching_private_key("", &keys), None);
}

#[test]
fn find_matching_private_key_ct_eq_single_key() {
	let key = generate_key();
	let public = derive_public_key(&key).unwrap();
	let keys = vec![key.clone()];
	assert_eq!(find_matching_private_key(&public, &keys), Some(key));
}
