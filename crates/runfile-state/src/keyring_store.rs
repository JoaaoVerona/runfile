//! OS credential store integration.
//!
//! Private keys are stored as a single JSON object mapping each public-key
//! fingerprint to its private key hex, kept at one [`Place`]. One entry, one
//! source of truth.
//!
//! Backends by platform:
//! - **Windows**: Credential Manager, via keyring-core.
//! - **macOS**: Keychain, via keyring-core.
//! - **Linux**: the D-Bus Secret Service (gnome-keyring / KWallet) when a
//!   session bus reaches a provider with a default collection; otherwise an
//!   owner-only file in the data directory (`file_store`), which is what a
//!   headless server gets. The choice is made once per process and **never
//!   errors**, and both survive a reboot.
//!
//! Linux reads two more things. Beside the Secret Service, the file is read
//! when it exists, so a key stored where no Secret Service could be reached --
//! over SSH, from cron -- still decrypts in the sessions where one can. And
//! kernel keyutils, the volatile store earlier versions fell back to, is read
//! so what is left in it can be moved to where new keys go (`legacy_load`)
//! before a reboot clears it.

use crate::keyring_keys::KeyringKeysError;
use std::fmt;
#[cfg(target_os = "linux")]
use std::path::PathBuf;
use std::sync::Once;

pub(crate) const SERVICE_NAME: &str = "runfile";
pub(crate) const KEYSTORE_USER: &str = "__keystore__";

/// Where keys are kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
	/// Credential Manager, Keychain, or the Secret Service.
	CredentialStore,
	/// An owner-only file, on a Linux machine with no Secret Service.
	#[cfg(target_os = "linux")]
	File(PathBuf),
}

impl fmt::Display for Place {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Place::CredentialStore => f.write_str("OS credential store"),
			#[cfg(target_os = "linux")]
			Place::File(path) => write!(f, "{}", path.display()),
		}
	}
}

// ---------------------------------------------------------------------------
// Linux backend selection
// ---------------------------------------------------------------------------

/// Which Linux backend this process stores keys in. Decided once, lazily, the
/// first time the store is touched.
#[cfg(target_os = "linux")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum LinuxBackend {
	/// D-Bus Secret Service (gnome-keyring / KWallet).
	SecretService,
	/// The owner-only file.
	File,
}

/// Pick the Linux backend exactly once. Prefers the Secret Service; falls back
/// to the file when no usable session bus / default collection is present.
/// Never panics or errors -- the file is always there to fall back to.
#[cfg(target_os = "linux")]
fn linux_backend() -> LinuxBackend {
	use std::sync::OnceLock;
	static BACKEND: OnceLock<LinuxBackend> = OnceLock::new();
	*BACKEND.get_or_init(|| {
		if super::secret_service_store::is_usable() {
			LinuxBackend::SecretService
		} else {
			LinuxBackend::File
		}
	})
}

/// `true` when the Linux Secret Service backend was selected for this process.
#[cfg(target_os = "linux")]
fn using_secret_service() -> bool {
	linux_backend() == LinuxBackend::SecretService
}

// ---------------------------------------------------------------------------
// keyring-core path (Windows, macOS, and the Linux keyutils leftovers)
// ---------------------------------------------------------------------------

fn ensure_default_store() {
	static INIT: Once = Once::new();
	INIT.call_once(|| {
		let _ = init_default_store_inner();
	});
}

fn init_default_store_inner() -> keyring_core::Result<()> {
	let config: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();

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

#[cfg(not(target_os = "linux"))]
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

#[cfg(not(target_os = "linux"))]
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
// Public API -- dispatches on the place
// ---------------------------------------------------------------------------

/// Where a new key is stored.
pub fn primary() -> Result<Place, KeyringKeysError> {
	#[cfg(target_os = "linux")]
	{
		if !using_secret_service() {
			return super::file_store::path().map(Place::File);
		}
	}
	Ok(Place::CredentialStore)
}

/// Every place keys are read from, the one new keys go to first.
pub(crate) fn places() -> Result<Vec<Place>, KeyringKeysError> {
	let primary = primary()?;
	#[cfg(target_os = "linux")]
	{
		if using_secret_service()
			&& let Ok(path) = super::file_store::path()
			&& super::file_store::present(&path)
		{
			return Ok(vec![primary, Place::File(path)]);
		}
	}
	Ok(vec![primary])
}

/// Read the blob kept at `place` (raw JSON string). `Ok(None)` if there is none.
pub(crate) fn load(place: &Place) -> Result<Option<String>, KeyringKeysError> {
	match place {
		Place::CredentialStore => {
			#[cfg(target_os = "linux")]
			let loaded = super::secret_service_store::load_blob();
			#[cfg(not(target_os = "linux"))]
			let loaded = keyring_core_load_blob();
			Ok(loaded?)
		}
		#[cfg(target_os = "linux")]
		Place::File(path) => super::file_store::load(path),
	}
}

/// Write the blob kept at `place`. Replaces any existing value.
pub(crate) fn store(place: &Place, blob: &str) -> Result<(), KeyringKeysError> {
	match place {
		Place::CredentialStore => {
			#[cfg(target_os = "linux")]
			let stored = super::secret_service_store::store_blob(blob);
			#[cfg(not(target_os = "linux"))]
			let stored = keyring_core_store_blob(blob);
			Ok(stored?)
		}
		#[cfg(target_os = "linux")]
		Place::File(path) => super::file_store::store(path, blob),
	}
}

/// Delete the blob kept at `place` entirely. `Ok(false)` if there was none.
pub(crate) fn delete(place: &Place) -> Result<bool, KeyringKeysError> {
	match place {
		Place::CredentialStore => {
			#[cfg(target_os = "linux")]
			let deleted = super::secret_service_store::delete_blob();
			#[cfg(not(target_os = "linux"))]
			let deleted = keyring_core_delete_blob();
			Ok(deleted?)
		}
		#[cfg(target_os = "linux")]
		Place::File(path) => super::file_store::delete(path),
	}
}

/// What an earlier runfile left in kernel keyutils, its fallback where there
/// was no Secret Service. `None` when there is nothing there, or when keyutils
/// cannot be reached at all (a container's seccomp profile refuses `keyctl`) --
/// either way there is nothing to move. Always `None` off Linux.
pub(crate) fn legacy_load() -> Option<String> {
	#[cfg(target_os = "linux")]
	{
		keyring_core_load_blob().ok().flatten()
	}
	#[cfg(not(target_os = "linux"))]
	{
		None
	}
}

/// Clear the keyutils entry, once what it held is stored somewhere that lasts.
pub(crate) fn legacy_forget() {
	#[cfg(target_os = "linux")]
	let _ = keyring_core_delete_blob();
}

/// Check whether the OS credential store is available and functional.
///
/// On Linux this is always `true`: the Secret Service only becomes the backend
/// after a successful probe, and the file is always there to fall back to --
/// a file that cannot be written says so when it is written. Windows/macOS
/// attempt a no-op probe to detect headless environments where no secret store
/// is running.
pub fn is_available() -> bool {
	#[cfg(target_os = "linux")]
	{
		true
	}
	#[cfg(not(target_os = "linux"))]
	{
		keyring_core_is_available()
	}
}
