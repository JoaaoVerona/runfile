//! Pure-Rust D-Bus Secret Service backend (Linux only).
//!
//! Persists the keystore blob in the platform Secret Service (gnome-keyring,
//! KWallet, …) when a D-Bus session bus + secret-service provider are
//! available. This is the **persistent** Linux backend — unlike the kernel
//! keyutils store, items written here survive logout and reboot.
//!
//! The blob lives in the user's **default** collection as a single item
//! identified by the same `(service, user)` pair the keyring-core path uses,
//! so there is exactly one source of truth on every platform.
//!
//! Everything here goes through the crate's *blocking* API, which drives
//! zbus's `async-io` reactor under the hood — no async runtime or `block_on`
//! is needed at the call sites, and no C libraries are linked (so the static
//! musl release binaries keep building).
//!
//! [`is_usable`] is the probe `keyring_store` uses to decide whether to take
//! this path at all; when it returns `false` (no session bus, no provider, or
//! no default collection) the caller silently falls back to keyutils.

use keyring_core::Error as KeyringError;
use secret_service::blocking::SecretService;
use secret_service::{EncryptionType, Error as SsError};
use std::collections::HashMap;

use super::keyring_store::{KEYSTORE_USER, SERVICE_NAME};

/// Human-readable label shown for our item in keyring UIs (e.g. Seahorse).
const ITEM_LABEL: &str = "Runfile secret keys";
/// MIME type stored alongside the secret. Our blob is JSON text.
const CONTENT_TYPE: &str = "application/json";

/// Lookup attributes that uniquely identify our single keystore item within
/// the default collection. Mirrors the keyring-core `(service, user)` pair.
fn item_attributes() -> HashMap<&'static str, &'static str> {
	HashMap::from([("service", SERVICE_NAME), ("user", KEYSTORE_USER)])
}

/// Map a Secret Service error into the `keyring_core::Error` the rest of the
/// store API speaks. A locked store is surfaced as `NoStorageAccess`
/// (matching keyring-core's own semantics); everything else is a generic
/// platform failure.
fn to_keyring_err(err: SsError) -> KeyringError {
	match &err {
		SsError::Locked => KeyringError::NoStorageAccess(Box::new(err)),
		_ => KeyringError::PlatformFailure(Box::new(err)),
	}
}

/// Probe whether the Secret Service is usable for *persistent* storage.
///
/// Returns `true` only when we can reach the session bus **and** resolve a
/// default collection to store into. Deliberately does not unlock anything,
/// so the probe never triggers an interactive unlock prompt. A `false` result
/// is the signal for `keyring_store` to fall back to kernel keyutils.
pub(crate) fn is_usable() -> bool {
	match SecretService::connect(EncryptionType::Dh) {
		Ok(ss) => ss.get_default_collection().is_ok(),
		Err(_) => false,
	}
}

/// Read the keystore blob. `Ok(None)` if no item exists yet.
///
/// Each call opens its own connection (encrypting secrets in transit via DH)
/// and unlocks the default collection if necessary — key-management calls are
/// infrequent, and per-call scoping sidesteps the self-referential lifetime
/// between `SecretService` and its borrowed `Collection`/`Item` handles.
pub(crate) fn load_blob() -> Result<Option<String>, KeyringError> {
	let ss = SecretService::connect(EncryptionType::Dh).map_err(to_keyring_err)?;
	let collection = ss.get_default_collection().map_err(to_keyring_err)?;
	if collection.is_locked().map_err(to_keyring_err)? {
		collection.unlock().map_err(to_keyring_err)?;
	}

	let items = collection.search_items(item_attributes()).map_err(to_keyring_err)?;
	let Some(item) = items.first() else {
		return Ok(None);
	};
	if item.is_locked().map_err(to_keyring_err)? {
		item.unlock().map_err(to_keyring_err)?;
	}

	let secret = item.get_secret().map_err(to_keyring_err)?;
	let text = String::from_utf8(secret).map_err(|e| KeyringError::BadEncoding(e.into_bytes()))?;
	Ok(Some(text))
}

/// Write the keystore blob, replacing any existing item in place.
pub(crate) fn store_blob(blob: &str) -> Result<(), KeyringError> {
	let ss = SecretService::connect(EncryptionType::Dh).map_err(to_keyring_err)?;
	let collection = ss.get_default_collection().map_err(to_keyring_err)?;
	if collection.is_locked().map_err(to_keyring_err)? {
		collection.unlock().map_err(to_keyring_err)?;
	}

	// `replace = true` upserts the item matched by our attributes, so we never
	// accumulate duplicates across saves.
	collection
		.create_item(ITEM_LABEL, item_attributes(), blob.as_bytes(), true, CONTENT_TYPE)
		.map_err(to_keyring_err)?;
	Ok(())
}

/// Delete the keystore item. `Ok(false)` if there was nothing to delete.
pub(crate) fn delete_blob() -> Result<bool, KeyringError> {
	let ss = SecretService::connect(EncryptionType::Dh).map_err(to_keyring_err)?;
	let collection = ss.get_default_collection().map_err(to_keyring_err)?;
	if collection.is_locked().map_err(to_keyring_err)? {
		collection.unlock().map_err(to_keyring_err)?;
	}

	let items = collection.search_items(item_attributes()).map_err(to_keyring_err)?;
	if items.is_empty() {
		return Ok(false);
	}
	for item in &items {
		if item.is_locked().map_err(to_keyring_err)? {
			item.unlock().map_err(to_keyring_err)?;
		}
		item.delete().map_err(to_keyring_err)?;
	}
	Ok(true)
}
