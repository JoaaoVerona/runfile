//! OS credential store integration.
//!
//! Private keys are stored in the platform-native secret store as a single
//! entry at `(service="runfile", user="__keystore__")` whose value is a JSON
//! object mapping each public-key fingerprint to its private key hex. One
//! entry, one source of truth.
//!
//! Backends by platform:
//! - **Windows**: Credential Manager (persistent), via keyring-core.
//! - **macOS**: Keychain (persistent), via keyring-core.
//! - **Linux**: the D-Bus Secret Service (gnome-keyring / KWallet, persistent)
//!   when a session bus + provider are available; otherwise the kernel
//!   keyutils store (in-memory, cleared on reboot) as a non-persistent
//!   fallback. The choice is made once per process and **never errors** — a
//!   missing Secret Service simply degrades to keyutils, so adding keys keeps
//!   working exactly as before in headless / CI environments.

use std::collections::HashMap;
use std::sync::Once;

pub(crate) const SERVICE_NAME: &str = "runfile";
pub(crate) const KEYSTORE_USER: &str = "__keystore__";

// ---------------------------------------------------------------------------
// Linux backend selection
// ---------------------------------------------------------------------------

/// Which Linux credential backend this process is using. Decided once, lazily,
/// the first time the store is touched.
#[cfg(target_os = "linux")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum LinuxBackend {
	/// Persistent D-Bus Secret Service (gnome-keyring / KWallet).
	SecretService,
	/// Non-persistent kernel keyutils (the legacy fallback).
	Keyutils,
}

/// Pick the Linux backend exactly once. Prefers the persistent Secret Service;
/// falls back to keyutils when no usable session bus / default collection is
/// present. Never panics or errors — the fallback path is always available.
#[cfg(target_os = "linux")]
fn linux_backend() -> LinuxBackend {
	use std::sync::OnceLock;
	static BACKEND: OnceLock<LinuxBackend> = OnceLock::new();
	*BACKEND.get_or_init(|| {
		if super::secret_service_store::is_usable() {
			LinuxBackend::SecretService
		} else {
			LinuxBackend::Keyutils
		}
	})
}

/// `true` when the Linux Secret Service backend was selected for this process.
#[cfg(target_os = "linux")]
fn using_secret_service() -> bool {
	linux_backend() == LinuxBackend::SecretService
}

// ---------------------------------------------------------------------------
// keyring-core path (Windows, macOS, and the Linux keyutils fallback)
// ---------------------------------------------------------------------------

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

fn keyring_core_load_blob() -> Result<Option<String>, keyring_core::Error> {
	ensure_default_store();
	let entry = keyring_core::Entry::new(SERVICE_NAME, KEYSTORE_USER)?;
	match entry.get_password() {
		Ok(s) => Ok(Some(s)),
		Err(keyring_core::Error::NoEntry) => Ok(None),
		Err(e) => Err(e),
	}
}

fn keyring_core_store_blob(blob: &str) -> Result<(), keyring_core::Error> {
	ensure_default_store();
	let entry = keyring_core::Entry::new(SERVICE_NAME, KEYSTORE_USER)?;
	entry.set_password(blob)
}

fn keyring_core_delete_blob() -> Result<bool, keyring_core::Error> {
	ensure_default_store();
	let entry = keyring_core::Entry::new(SERVICE_NAME, KEYSTORE_USER)?;
	match entry.delete_credential() {
		Ok(()) => Ok(true),
		Err(keyring_core::Error::NoEntry) => Ok(false),
		Err(e) => Err(e),
	}
}

fn keyring_core_is_available() -> bool {
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

// ---------------------------------------------------------------------------
// Public API — dispatches to the selected backend
// ---------------------------------------------------------------------------

/// Read the keystore blob (raw JSON string). `Ok(None)` if the entry doesn't exist.
pub(crate) fn load_blob() -> Result<Option<String>, keyring_core::Error> {
	#[cfg(target_os = "linux")]
	{
		if using_secret_service() {
			return super::secret_service_store::load_blob();
		}
	}
	keyring_core_load_blob()
}

/// Write the keystore blob. Replaces any existing value.
pub(crate) fn store_blob(blob: &str) -> Result<(), keyring_core::Error> {
	#[cfg(target_os = "linux")]
	{
		if using_secret_service() {
			return super::secret_service_store::store_blob(blob);
		}
	}
	keyring_core_store_blob(blob)
}

/// Delete the keystore entry entirely. `Ok(false)` if it didn't exist.
pub(crate) fn delete_blob() -> Result<bool, keyring_core::Error> {
	#[cfg(target_os = "linux")]
	{
		if using_secret_service() {
			return super::secret_service_store::delete_blob();
		}
	}
	keyring_core_delete_blob()
}

/// Check whether the OS credential store is available and functional.
///
/// On Linux this is always `true`: the Secret Service path only becomes the
/// active backend after a successful probe, and the keyutils fallback is
/// always present. Windows/macOS attempt a no-op probe to detect headless
/// environments where no secret store is running.
pub fn is_available() -> bool {
	#[cfg(target_os = "linux")]
	{
		if using_secret_service() {
			return true;
		}
	}
	keyring_core_is_available()
}
