use crate::keyring_keys::*;

#[test]
fn parse_empty_string_yields_empty_blob() {
	let blob = parse_blob("").unwrap();
	assert!(blob.is_empty());
}

#[test]
fn parse_whitespace_yields_empty_blob() {
	let blob = parse_blob("   \n").unwrap();
	assert!(blob.is_empty());
}

#[test]
fn parse_object_yields_entries() {
	let raw = r#"{"aabb":"deadbeef","ccdd":"feedface"}"#;
	let blob = parse_blob(raw).unwrap();
	assert_eq!(blob.len(), 2);
	assert_eq!(blob.get("aabb").unwrap(), "deadbeef");
	assert_eq!(blob.get("ccdd").unwrap(), "feedface");
}

#[test]
fn parse_invalid_json_errors() {
	let err = parse_blob("not json").unwrap_err();
	assert!(matches!(err, KeyringKeysError::BlobParse(_)));
}

#[test]
fn serialize_is_deterministic_via_btreemap() {
	// BTreeMap iteration order is sorted, so serialization is stable.
	let mut blob = Blob::new();
	blob.insert("ccdd".to_string(), "feedface".to_string());
	blob.insert("aabb".to_string(), "deadbeef".to_string());
	let raw = serialize_blob(&blob).unwrap();
	// `aabb` must come before `ccdd` regardless of insertion order.
	let aa = raw.find("aabb").unwrap();
	let cc = raw.find("ccdd").unwrap();
	assert!(aa < cc, "expected sorted serialization, got {raw}");
}

#[test]
fn parse_env_pool_handles_blank_and_whitespace() {
	let raw = "  aa  \n\n bb\n\n  \ncc";
	assert_eq!(parse_env_pool(raw), vec!["aa", "bb", "cc"]);
}

#[test]
fn parse_env_pool_empty_input_yields_empty() {
	assert!(parse_env_pool("").is_empty());
	assert!(parse_env_pool("   \n\t\n").is_empty());
}

#[test]
fn merge_env_first_then_keyring() {
	let env = vec!["env_key".to_string()];
	let mut blob = Blob::new();
	blob.insert("fp1".to_string(), "ring_key".to_string());
	assert_eq!(merge_key_sources(env, vec![Ok(blob)]), vec!["env_key", "ring_key"]);
}

#[test]
fn merge_dedups_overlap() {
	// Same key registered in both pools — must appear only once so
	// `find_private_key_by_public_prefix` doesn't see spurious ambiguity.
	let env = vec!["shared".to_string(), "env_only".to_string()];
	let mut blob = Blob::new();
	blob.insert("fp1".to_string(), "shared".to_string());
	blob.insert("fp2".to_string(), "ring_only".to_string());
	let result = merge_key_sources(env, vec![Ok(blob)]);
	assert_eq!(result.iter().filter(|k| *k == "shared").count(), 1);
	assert!(result.contains(&"env_only".to_string()));
	assert!(result.contains(&"ring_only".to_string()));
}

#[test]
fn merge_keyring_error_with_env_pool_returns_env() {
	// Keyring unavailable + env pool present → return env keys, no panic.
	let env = vec!["env_key".to_string()];
	let result = merge_key_sources(env, vec![Err(KeyringKeysError::StoreUnavailable)]);
	assert_eq!(result, vec!["env_key"]);
}

#[test]
fn merge_keyring_error_no_env_returns_empty() {
	let result = merge_key_sources(Vec::new(), vec![Err(KeyringKeysError::StoreUnavailable)]);
	assert!(result.is_empty());
}

#[test]
fn roundtrip_preserves_entries() {
	let mut blob = Blob::new();
	blob.insert("fp1".to_string(), "key1".to_string());
	blob.insert("fp2".to_string(), "key2".to_string());
	let raw = serialize_blob(&blob).unwrap();
	let parsed = parse_blob(&raw).unwrap();
	assert_eq!(blob, parsed);
}

#[test]
fn merge_keeps_what_the_other_places_hold_when_one_fails() {
	// The key file is refused (say, readable by others) beside a Secret Service
	// that answered: the run still has the keys the Secret Service holds.
	let mut ring = Blob::new();
	ring.insert("fp1".to_string(), "ring_key".to_string());
	let mut file = Blob::new();
	file.insert("fp2".to_string(), "file_key".to_string());
	let result = merge_key_sources(
		Vec::new(),
		vec![
			Ok(ring),
			Err(KeyringKeysError::KeyFile("refused".to_string())),
			Ok(file),
		],
	);
	assert_eq!(result, vec!["ring_key", "file_key"]);
}

fn blob(pairs: &[(&str, &str)]) -> Blob {
	pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

#[test]
fn keys_left_in_keyutils_are_moved_into_the_store_and_keyutils_is_cleared() {
	let mut held = blob(&[("fp1", "kept")]);
	let mut written = None;
	let mut forgotten = false;
	absorb_legacy(
		&mut held,
		Some(r#"{"fp1":"stale","fp2":"moved"}"#.to_string()),
		|b| {
			written = Some(b.clone());
			Ok(())
		},
		|| forgotten = true,
	)
	.unwrap();
	// What the store already had keeps its value; the rest joins it.
	let want = blob(&[("fp1", "kept"), ("fp2", "moved")]);
	assert_eq!(held, want);
	assert_eq!(written, Some(want));
	assert!(forgotten);
}

#[test]
fn keyutils_is_not_cleared_when_the_store_could_not_be_written() {
	// The one ordering that matters: a key is in keyutils or in the store at
	// every moment, so a failed write must leave keyutils as it was.
	let mut held = Blob::new();
	let mut forgotten = false;
	let err = absorb_legacy(
		&mut held,
		Some(r#"{"fp1":"only_copy"}"#.to_string()),
		|_| Err(KeyringKeysError::KeyFile("read-only file system".to_string())),
		|| forgotten = true,
	)
	.unwrap_err();
	assert!(err.to_string().contains("read-only"), "{err}");
	assert!(!forgotten, "keyutils was cleared though the keys were not stored");
}

#[test]
fn keyutils_holding_nothing_new_is_cleared_without_a_write() {
	let mut held = blob(&[("fp1", "k")]);
	let mut wrote = false;
	let mut forgotten = false;
	absorb_legacy(
		&mut held,
		Some(r#"{"fp1":"k"}"#.to_string()),
		|_| {
			wrote = true;
			Ok(())
		},
		|| forgotten = true,
	)
	.unwrap();
	assert!(!wrote);
	assert!(forgotten);
}

#[test]
fn nothing_in_keyutils_or_an_entry_that_does_not_parse_is_left_alone() {
	for legacy in [None, Some("not json".to_string())] {
		let mut held = blob(&[("fp1", "k")]);
		let mut touched = false;
		absorb_legacy(
			&mut held,
			legacy,
			|_| Err(KeyringKeysError::KeyFile("must not be written".to_string())),
			|| touched = true,
		)
		.unwrap();
		assert_eq!(held, blob(&[("fp1", "k")]));
		assert!(!touched);
	}
}
