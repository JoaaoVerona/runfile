//! OS credential store integration via the `keyring` crate.
//!
//! Private keys are stored in the platform-native secret store
//! (Windows Credential Manager, macOS Keychain, Linux Secret Service)
//! keyed by their public key fingerprint.

const SERVICE_NAME: &str = "runfile";

/// Store a private key in the OS credential store, keyed by its public fingerprint.
pub fn store_key(fingerprint: &str, private_key_hex: &str) -> Result<(), keyring::Error> {
	let entry = keyring::Entry::new(SERVICE_NAME, fingerprint)?;
	entry.set_password(private_key_hex)
}

/// Load a private key from the OS credential store by its public fingerprint.
/// Returns `None` if the entry doesn't exist (rather than propagating the error).
pub fn load_key(fingerprint: &str) -> Result<Option<String>, keyring::Error> {
	let entry = keyring::Entry::new(SERVICE_NAME, fingerprint)?;
	match entry.get_password() {
		Ok(password) => Ok(Some(password)),
		Err(keyring::Error::NoEntry) => Ok(None),
		Err(e) => Err(e),
	}
}

/// Delete a private key from the OS credential store by its public fingerprint.
/// Returns `Ok(true)` if deleted, `Ok(false)` if it didn't exist.
pub fn delete_key(fingerprint: &str) -> Result<bool, keyring::Error> {
	let entry = keyring::Entry::new(SERVICE_NAME, fingerprint)?;
	match entry.delete_credential() {
		Ok(()) => Ok(true),
		Err(keyring::Error::NoEntry) => Ok(false),
		Err(e) => Err(e),
	}
}

/// Check whether the OS credential store is available and functional.
/// Attempts a no-op probe to detect headless environments where
/// no secret service is running.
pub fn is_available() -> bool {
	// Try to create an entry — this is the lightest operation that
	// exercises the backend without modifying any real credentials.
	let entry = match keyring::Entry::new(SERVICE_NAME, "__runfile_probe__") {
		Ok(e) => e,
		Err(_) => return false,
	};
	// Attempt a read. NoEntry is fine (store is reachable), any other
	// error means the backend is not usable.
	match entry.get_password() {
		Ok(_) | Err(keyring::Error::NoEntry) => true,
		Err(_) => false,
	}
}
