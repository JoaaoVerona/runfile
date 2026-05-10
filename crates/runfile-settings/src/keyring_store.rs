//! OS credential store integration via the `keyring-core` crate.
//!
//! Private keys are stored in the platform-native secret store
//! (Windows Credential Manager, macOS Keychain, Linux kernel keyutils) as a
//! single entry at `(service="runfile", user="__keystore__")` whose value
//! is a JSON object mapping each public-key fingerprint to its private key
//! hex. One entry, one source of truth.

use std::collections::HashMap;
use std::sync::Once;

pub(crate) const SERVICE_NAME: &str = "runfile";
pub(crate) const KEYSTORE_USER: &str = "__keystore__";

fn ensure_default_store() {
	static INIT: Once = Once::new();
	INIT.call_once(|| {
		let _ = init_default_store_inner();
	});
}

fn init_default_store_inner() -> keyring_core::Result<()> {
	let config: HashMap<&str, &str> = HashMap::new();

	#[cfg(target_os = "linux")]
	{
		use linux_keyutils_keyring_store::Store;
		keyring_core::set_default_store(Store::new_with_configuration(&config)?);
	}
	#[cfg(target_os = "macos")]
	{
		use apple_native_keyring_store::keychain::Store;
		keyring_core::set_default_store(Store::new_with_configuration(&config)?);
	}
	#[cfg(target_os = "windows")]
	{
		use windows_native_keyring_store::Store;
		keyring_core::set_default_store(Store::new_with_configuration(&config)?);
	}
	let _ = config;
	Ok(())
}

/// Read the keystore blob (raw JSON string). `Ok(None)` if the entry doesn't exist.
pub(crate) fn load_blob() -> Result<Option<String>, keyring_core::Error> {
	ensure_default_store();
	let entry = keyring_core::Entry::new(SERVICE_NAME, KEYSTORE_USER)?;
	match entry.get_password() {
		Ok(s) => Ok(Some(s)),
		Err(keyring_core::Error::NoEntry) => Ok(None),
		Err(e) => Err(e),
	}
}

/// Write the keystore blob. Replaces any existing value.
pub(crate) fn store_blob(blob: &str) -> Result<(), keyring_core::Error> {
	ensure_default_store();
	let entry = keyring_core::Entry::new(SERVICE_NAME, KEYSTORE_USER)?;
	entry.set_password(blob)
}

/// Delete the keystore entry entirely. `Ok(false)` if it didn't exist.
pub(crate) fn delete_blob() -> Result<bool, keyring_core::Error> {
	ensure_default_store();
	let entry = keyring_core::Entry::new(SERVICE_NAME, KEYSTORE_USER)?;
	match entry.delete_credential() {
		Ok(()) => Ok(true),
		Err(keyring_core::Error::NoEntry) => Ok(false),
		Err(e) => Err(e),
	}
}

/// Check whether the OS credential store is available and functional.
/// Attempts a no-op probe to detect headless environments where
/// no secret service is running.
pub fn is_available() -> bool {
	ensure_default_store();
	let entry = match keyring_core::Entry::new(SERVICE_NAME, "__runfile_probe__") {
		Ok(e) => e,
		Err(_) => return false,
	};
	match entry.get_password() {
		Ok(_) | Err(keyring_core::Error::NoEntry) => true,
		Err(_) => false,
	}
}
