//! OS credential store integration via the `keyring-core` crate.
//!
//! Private keys are stored in the platform-native secret store
//! (Windows Credential Manager, macOS Keychain, Linux kernel keyutils)
//! keyed by their public key fingerprint.

use std::collections::HashMap;
use std::sync::Once;

const SERVICE_NAME: &str = "runfile";

/// Initialize keyring-core's default credential store the first time we need one.
/// `keyring-core` v1 separates the API from the backend — we have to register a
/// platform-native store before any `Entry::new(...)` call. Idempotent and safe
/// to call from any thread.
fn ensure_default_store() {
	static INIT: Once = Once::new();
	INIT.call_once(|| {
		// Init failures are intentionally swallowed: subsequent `Entry::new`
		// calls return `NoDefaultStore`, which `is_available()` already maps
		// to "store unavailable" and the public callers surface as a normal
		// keyring error.
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

/// Store a private key in the OS credential store, keyed by its public fingerprint.
pub fn store_key(fingerprint: &str, private_key_hex: &str) -> Result<(), keyring_core::Error> {
	ensure_default_store();
	let entry = keyring_core::Entry::new(SERVICE_NAME, fingerprint)?;
	entry.set_password(private_key_hex)
}

/// Load a private key from the OS credential store by its public fingerprint.
/// Returns `None` if the entry doesn't exist (rather than propagating the error).
pub fn load_key(fingerprint: &str) -> Result<Option<String>, keyring_core::Error> {
	ensure_default_store();
	let entry = keyring_core::Entry::new(SERVICE_NAME, fingerprint)?;
	match entry.get_password() {
		Ok(password) => Ok(Some(password)),
		Err(keyring_core::Error::NoEntry) => Ok(None),
		Err(e) => Err(e),
	}
}

/// Delete a private key from the OS credential store by its public fingerprint.
/// Returns `Ok(true)` if deleted, `Ok(false)` if it didn't exist.
pub fn delete_key(fingerprint: &str) -> Result<bool, keyring_core::Error> {
	ensure_default_store();
	let entry = keyring_core::Entry::new(SERVICE_NAME, fingerprint)?;
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
	// Try to create an entry — this is the lightest operation that
	// exercises the backend without modifying any real credentials.
	let entry = match keyring_core::Entry::new(SERVICE_NAME, "__runfile_probe__") {
		Ok(e) => e,
		Err(_) => return false,
	};
	// Attempt a read. NoEntry is fine (store is reachable), any other
	// error means the backend is not usable.
	match entry.get_password() {
		Ok(_) | Err(keyring_core::Error::NoEntry) => true,
		Err(_) => false,
	}
}
