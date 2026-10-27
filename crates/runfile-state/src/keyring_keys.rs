//! Single source of truth for runfile's secret keys: the OS credential store,
//! or on a Linux machine without one, an owner-only file.
//!
//! All private keys live in one entry as a JSON object
//! `{ fingerprint: private_key_hex, ... }`, at the [`Place`] new keys go to;
//! see `keyring_store` for which that is, and for the one other place Linux
//! reads. This module is the only path through which the CLI talks to that
//! storage. settings.json no longer tracks fingerprints — drift between the
//! two stores is impossible by construction.

use crate::keyring_store::{self, Place};
use std::collections::{BTreeMap, HashSet};
use thiserror::Error;

/// Newline-separated list of 64-char hex private keys, merged into
/// [`all_private_keys`] ahead of the keyring blob. Designed for ephemeral
/// environments (CI runners, containers) where bootstrapping an OS credential
/// store just to round-trip secrets that already live in process env is wasted
/// work. Whitespace-only lines are skipped. A non-empty pool also silences the
/// "credential store unavailable" warning, since the caller has explicitly
/// opted out of relying on the keyring.
pub const ENV_PRIVATE_KEYS_VAR: &str = "RUNFILE_PRIVATE_KEYS";

#[derive(Debug, Error)]
pub enum KeyringKeysError {
	#[error("OS credential store error: {0}")]
	Keyring(String),

	#[error(
		"OS credential store is unavailable. Private keys require a working credential store \
		 (Windows Credential Manager / macOS Keychain). On Linux this never triggers — keys fall \
		 back to an owner-only file when no D-Bus Secret Service is available."
	)]
	StoreUnavailable,

	/// The Linux key file: unreadable, unwritable, or refused as not private.
	/// The message names the file and what is wrong with it.
	#[error("{0}")]
	KeyFile(String),

	#[error("the keys kernel keyutils holds could not be moved to {0}: {1}")]
	Migrate(String, Box<KeyringKeysError>),

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
pub(crate) type Blob = BTreeMap<String, String>;

pub(crate) fn parse_blob(raw: &str) -> Result<Blob, KeyringKeysError> {
	if raw.trim().is_empty() {
		return Ok(Blob::new());
	}
	serde_json::from_str(raw).map_err(|e| KeyringKeysError::BlobParse(e.to_string()))
}

pub(crate) fn serialize_blob(blob: &Blob) -> Result<String, KeyringKeysError> {
	serde_json::to_string(blob).map_err(|e| KeyringKeysError::BlobSerialize(e.to_string()))
}

fn read(place: &Place) -> Result<Blob, KeyringKeysError> {
	match keyring_store::load(place)? {
		Some(raw) => parse_blob(&raw),
		None => Ok(Blob::new()),
	}
}

fn write(place: &Place, blob: &Blob) -> Result<(), KeyringKeysError> {
	if blob.is_empty() {
		// No keys left — remove the entry rather than store an empty object,
		// so the credential manager doesn't show an empty placeholder.
		let _ = keyring_store::delete(place)?;
		return Ok(());
	}
	let raw = serialize_blob(blob)?;
	keyring_store::store(place, &raw)
}

/// What the place new keys go to holds, with whatever an earlier runfile left
/// in kernel keyutils moved into it.
fn read_primary(place: &Place) -> Result<Blob, KeyringKeysError> {
	let mut blob = read(place)?;
	absorb_legacy(
		&mut blob,
		keyring_store::legacy_load(),
		|b| write(place, b).map_err(|e| KeyringKeysError::Migrate(place.to_string(), Box::new(e))),
		keyring_store::legacy_forget,
	)?;
	Ok(blob)
}

/// Move into `blob` the keys an earlier runfile left in kernel keyutils, its
/// fallback where there was no Secret Service -- which a reboot, or a few days
/// logged out, would have cleared along with everything encrypted under them.
///
/// `write` stores the result and `forget` clears keyutils, and `forget` is only
/// reached once `write` has succeeded, so a key is in one of the two at every
/// moment. A key `blob` already has keeps its value, and a keyutils entry that
/// does not parse is left where it is: there is nothing in it to move.
pub(crate) fn absorb_legacy(
	blob: &mut Blob,
	legacy: Option<String>,
	write: impl FnOnce(&Blob) -> Result<(), KeyringKeysError>,
	forget: impl FnOnce(),
) -> Result<(), KeyringKeysError> {
	let Some(Ok(legacy)) = legacy.as_deref().map(parse_blob) else {
		return Ok(());
	};
	let before = blob.len();
	for (fingerprint, key) in legacy {
		blob.entry(fingerprint).or_insert(key);
	}
	if blob.len() != before {
		write(blob)?;
	}
	forget();
	Ok(())
}

/// Each place keys are kept, with what it holds or why it could not be read.
type Each = Vec<(Place, Result<Blob, KeyringKeysError>)>;

/// Each place keys are kept, with what it holds -- the place new keys go to
/// first. A place that cannot be read is reported beside the others rather
/// than in place of them, so a run still has the keys that could be read.
fn read_each() -> Result<Each, KeyringKeysError> {
	let places = keyring_store::places()?;
	Ok(places
		.into_iter()
		.enumerate()
		.map(|(i, place)| {
			let blob = if i == 0 { read_primary(&place) } else { read(&place) };
			(place, blob)
		})
		.collect())
}

/// [`read_each`], failing at the first place that cannot be read: a command
/// that changes the keys must not act on part of them.
fn read_all() -> Result<Vec<(Place, Blob)>, KeyringKeysError> {
	read_each()?
		.into_iter()
		.map(|(place, blob)| blob.map(|b| (place, b)))
		.collect()
}

/// Add a private key, to the place new keys go to. Returns `Ok(false)` if a
/// key with the same fingerprint is already stored anywhere keys are read
/// (no-op), `Ok(true)` on insert.
pub fn add(private_key_hex: &str) -> Result<bool, KeyringKeysError> {
	let fingerprint =
		runfile_crypto::derive_public_key(private_key_hex).map_err(|e| KeyringKeysError::InvalidKey(e.to_string()))?;

	if !keyring_store::is_available() {
		return Err(KeyringKeysError::StoreUnavailable);
	}

	let mut held = read_all()?;
	if held.iter().any(|(_, blob)| blob.contains_key(&fingerprint)) {
		return Ok(false);
	}
	let (place, blob) = &mut held[0];
	blob.insert(fingerprint, private_key_hex.to_string());
	write(place, blob)?;
	Ok(true)
}

/// Remove a key by full fingerprint, from every place it is kept. `Ok(false)`
/// if no such fingerprint was stored. Match is exact — for prefix matching,
/// callers should resolve `prefix → full fingerprint` first via
/// [`list_fingerprints`].
pub fn remove(fingerprint: &str) -> Result<bool, KeyringKeysError> {
	let mut removed = false;
	for (place, mut blob) in read_all()? {
		if blob.remove(fingerprint).is_some() {
			write(&place, &blob)?;
			removed = true;
		}
	}
	Ok(removed)
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

/// Every stored fingerprint with where it is kept, sorted by fingerprint. A
/// key kept in two places is listed once for each, the place new keys go to
/// first.
pub fn list() -> Result<Vec<(String, Place)>, KeyringKeysError> {
	let mut stored: Vec<(String, Place)> = read_all()?
		.into_iter()
		.flat_map(|(place, blob)| blob.into_keys().map(move |fp| (fp, place.clone())))
		.collect();
	// Stable, so a fingerprint in two places keeps the order of the places.
	stored.sort_by(|a, b| a.0.cmp(&b.0));
	Ok(stored)
}

/// List every stored fingerprint, sorted lexicographically, each once.
pub fn list_fingerprints() -> Result<Vec<String>, KeyringKeysError> {
	let mut fps: Vec<String> = list()?.into_iter().map(|(fp, _)| fp).collect();
	fps.dedup();
	Ok(fps)
}

/// Parse the [`ENV_PRIVATE_KEYS_VAR`] env-var value into a key list:
/// newline-separated, whitespace trimmed, blank lines skipped. No validation
/// of hex / length is performed here — invalid keys simply fail to match
/// downstream (`find_matching_private_key` runs `derive_public_key` and
/// silently ignores anything that can't be parsed as hex).
pub(crate) fn parse_env_pool(raw: &str) -> Vec<String> {
	raw.lines()
		.map(str::trim)
		.filter(|line| !line.is_empty())
		.map(str::to_string)
		.collect()
}

/// Merge the env-supplied key pool with what each place keys are kept holds.
///
/// Env keys come first so they take precedence in any first-match scan
/// (e.g. [`runfile_crypto::find_matching_private_key`]'s linear walk).
/// Duplicates are stripped to keep `find_private_key_by_public_prefix` —
/// which counts matches and errors on >1 — from spuriously reporting
/// ambiguity when a key is registered in both places.
///
/// A place that could not be read is warned about, unless the env pool
/// already supplied keys (the caller has clearly opted into env-only key
/// supply, no need to nag about a missing credential store).
pub(crate) fn merge_key_sources(env_pool: Vec<String>, stores: Vec<Result<Blob, KeyringKeysError>>) -> Vec<String> {
	let mut keys = env_pool;
	let from_env = !keys.is_empty();
	for store in stores {
		match store {
			Ok(blob) => keys.extend(blob.into_values()),
			Err(e) => {
				if !from_env {
					// Plain, not coloured: this crate sits below the one that owns
					// the tag, and nothing else here writes to a terminal.
					eprintln!("[runfile] warning: could not load private keys: {e}");
				}
			}
		}
	}
	let mut seen: HashSet<String> = HashSet::new();
	keys.retain(|k| seen.insert(k.clone()));
	keys
}

/// Return every available private key (raw hex). Sourced from
/// [`ENV_PRIVATE_KEYS_VAR`] plus every place keys are kept.
///
/// Used as the available-keys pool for decryption / prefix matching. The
/// executor's env-build path treats "no keys" as "no decryption possible"
/// rather than aborting the run.
pub fn all_private_keys() -> Vec<String> {
	let env_pool = std::env::var(ENV_PRIVATE_KEYS_VAR)
		.map(|raw| parse_env_pool(&raw))
		.unwrap_or_default();
	let stores = match read_each() {
		Ok(each) => each.into_iter().map(|(_, blob)| blob).collect(),
		Err(e) => vec![Err(e)],
	};
	merge_key_sources(env_pool, stores)
}
