//! Single source of truth for runfile's secret keys: the OS credential store.
//!
//! All private keys live in one keyring entry as a JSON object
//! `{ fingerprint: private_key_hex, ... }`. This module is the only path
//! through which the CLI talks to that storage. settings.json no longer
//! tracks fingerprints — drift between the two stores is impossible by
//! construction.

use crate::keyring_store;
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum KeyringKeysError {
	#[error("OS credential store error: {0}")]
	Keyring(String),

	#[error(
		"OS credential store is unavailable. Private keys require a working credential store \
		 (Windows Credential Manager / macOS Keychain / Linux Secret Service)."
	)]
	StoreUnavailable,

	#[error("invalid private key: {0}")]
	InvalidKey(String),

	#[error("failed to parse keystore blob: {0}")]
	BlobParse(String),

	#[error("failed to serialize keystore blob: {0}")]
	BlobSerialize(String),
}

impl From<keyring_core::Error> for KeyringKeysError {
	fn from(e: keyring_core::Error) -> Self {
		KeyringKeysError::Keyring(e.to_string())
	}
}

/// In-memory map: fingerprint → private key hex. `BTreeMap` so list/serialize
/// orderings are deterministic.
type Blob = BTreeMap<String, String>;

fn parse_blob(raw: &str) -> Result<Blob, KeyringKeysError> {
	if raw.trim().is_empty() {
		return Ok(Blob::new());
	}
	serde_json::from_str(raw).map_err(|e| KeyringKeysError::BlobParse(e.to_string()))
}

fn serialize_blob(blob: &Blob) -> Result<String, KeyringKeysError> {
	serde_json::to_string(blob).map_err(|e| KeyringKeysError::BlobSerialize(e.to_string()))
}

fn read_blob() -> Result<Blob, KeyringKeysError> {
	match keyring_store::load_blob()? {
		Some(raw) => parse_blob(&raw),
		None => Ok(Blob::new()),
	}
}

fn write_blob(blob: &Blob) -> Result<(), KeyringKeysError> {
	if blob.is_empty() {
		// No keys left — remove the entry rather than store an empty object,
		// so the credential manager doesn't show an empty placeholder.
		let _ = keyring_store::delete_blob()?;
		return Ok(());
	}
	let raw = serialize_blob(blob)?;
	keyring_store::store_blob(&raw)?;
	Ok(())
}

/// Add a private key. Returns `Ok(false)` if a key with the same fingerprint
/// is already stored (no-op), `Ok(true)` on insert.
pub fn add(private_key_hex: &str) -> Result<bool, KeyringKeysError> {
	let fingerprint =
		runfile_crypto::derive_public_key(private_key_hex).map_err(|e| KeyringKeysError::InvalidKey(e.to_string()))?;

	if !keyring_store::is_available() {
		return Err(KeyringKeysError::StoreUnavailable);
	}

	let mut blob = read_blob()?;
	if blob.contains_key(&fingerprint) {
		return Ok(false);
	}
	blob.insert(fingerprint, private_key_hex.to_string());
	write_blob(&blob)?;
	Ok(true)
}

/// Remove a key by full fingerprint. `Ok(false)` if no such fingerprint was
/// stored. Match is exact — for prefix matching, callers should resolve
/// `prefix → full fingerprint` first via [`list_fingerprints`].
pub fn remove(fingerprint: &str) -> Result<bool, KeyringKeysError> {
	let mut blob = read_blob()?;
	if blob.remove(fingerprint).is_none() {
		return Ok(false);
	}
	write_blob(&blob)?;
	Ok(true)
}

/// Resolve a fingerprint prefix to a full fingerprint. Errors if zero or
/// more than one stored fingerprint matches.
pub fn resolve_prefix(prefix: &str) -> Result<String, KeyringKeysError> {
	let fps = list_fingerprints()?;
	let matches: Vec<&String> = fps.iter().filter(|fp| fp.starts_with(prefix)).collect();
	match matches.len() {
		0 => Err(KeyringKeysError::InvalidKey(format!(
			"no key has a public key starting with \"{prefix}\""
		))),
		1 => Ok(matches[0].clone()),
		n => Err(KeyringKeysError::InvalidKey(format!(
			"{n} keys have public keys starting with \"{prefix}\" — provide more characters to disambiguate"
		))),
	}
}

/// List every stored fingerprint, sorted lexicographically.
pub fn list_fingerprints() -> Result<Vec<String>, KeyringKeysError> {
	let blob = read_blob()?;
	Ok(blob.into_keys().collect())
}

/// Return every stored private key (raw hex). Used as the available-keys
/// pool for decryption / prefix matching.
///
/// On keyring read failure prints a warning and returns whatever was
/// recoverable (empty if nothing). The executor's env-build path treats
/// "no keys" as "no decryption possible" rather than aborting the run.
pub fn all_private_keys() -> Vec<String> {
	match read_blob() {
		Ok(blob) => blob.into_values().collect(),
		Err(e) => {
			eprintln!("Warning: failed to load private keys from credential store: {e}");
			Vec::new()
		}
	}
}

#[cfg(test)]
mod blob_tests {
	use super::*;

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
	fn roundtrip_preserves_entries() {
		let mut blob = Blob::new();
		blob.insert("fp1".to_string(), "key1".to_string());
		blob.insert("fp2".to_string(), "key2".to_string());
		let raw = serialize_blob(&blob).unwrap();
		let parsed = parse_blob(&raw).unwrap();
		assert_eq!(blob, parsed);
	}
}
