use crate::keyring_store;
use crate::paths;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SettingsError {
	#[error("Failed to read settings file: {0}")]
	Io(#[from] std::io::Error),

	#[error("Failed to parse settings: {0}")]
	Json5(#[from] json5::Error),

	#[error("Failed to serialize settings: {0}")]
	Json(#[from] serde_json::Error),

	#[error("Cannot determine settings directory on this platform")]
	NoSettingsDir,

	#[error("OS credential store error: {0}")]
	Keyring(String),
}

/// Local user settings for Runfile.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Settings {
	/// Custom shell paths, keyed by shell name (e.g. "bash" -> "/custom/path/bash").
	#[serde(default, skip_serializing_if = "HashMap::is_empty")]
	pub shell_paths: HashMap<String, PathBuf>,

	/// Path aliases for the -f/--file flag (e.g. "root" -> "~/.config/dev/Runfile.json").
	#[serde(default, skip_serializing_if = "HashMap::is_empty")]
	pub path_aliases: HashMap<String, PathBuf>,

	/// Global Runfile.json files that are always merged with the local Runfile.
	#[serde(default, rename = "globalFiles", skip_serializing_if = "Vec::is_empty")]
	pub global_files: Vec<PathBuf>,

	/// Public key fingerprints whose private keys are stored in the OS
	/// credential store (Windows Credential Manager / macOS Keychain /
	/// Linux Secret Service). This field is non-sensitive.
	#[serde(default, rename = "secureKeyFingerprints", skip_serializing_if = "Vec::is_empty")]
	pub secure_key_fingerprints: Vec<String>,
}

impl Settings {
	/// Load settings from the default platform location.
	/// Returns default settings if the file doesn't exist.
	pub fn load() -> Result<Self, SettingsError> {
		let path = paths::settings_file_path().ok_or(SettingsError::NoSettingsDir)?;
		Self::load_from(&path)
	}

	/// Load settings from a specific path.
	/// Returns default settings if the file doesn't exist.
	pub fn load_from(path: &Path) -> Result<Self, SettingsError> {
		if !path.exists() {
			return Ok(Self::default());
		}
		let content = std::fs::read_to_string(path)?;
		let settings: Settings = runfile_parser::from_json_str(&content)?;
		Ok(settings)
	}

	/// Save settings to the default platform location.
	pub fn save(&self) -> Result<(), SettingsError> {
		let path = paths::settings_file_path().ok_or(SettingsError::NoSettingsDir)?;
		self.save_to(&path)
	}

	/// Save settings to a specific path, creating parent directories as needed.
	pub fn save_to(&self, path: &Path) -> Result<(), SettingsError> {
		if let Some(parent) = path.parent() {
			std::fs::create_dir_all(parent)?;
		}
		let json = serde_json::to_string_pretty(self)?;
		std::fs::write(path, json)?;
		Ok(())
	}

	/// Get a custom shell path, if one has been configured.
	pub fn get_shell_path(&self, shell_name: &str) -> Option<&PathBuf> {
		self.shell_paths.get(shell_name)
	}

	/// Set a custom shell path.
	pub fn set_shell_path(&mut self, shell_name: &str, path: PathBuf) {
		self.shell_paths.insert(shell_name.to_string(), path);
	}

	/// Get a path alias, if one has been configured.
	pub fn get_path_alias(&self, alias: &str) -> Option<&PathBuf> {
		self.path_aliases.get(alias)
	}

	/// Set a path alias.
	pub fn set_path_alias(&mut self, alias: &str, path: PathBuf) {
		self.path_aliases.insert(alias.to_string(), path);
	}

	/// Remove a path alias. Returns true if it existed.
	pub fn remove_path_alias(&mut self, alias: &str) -> bool {
		self.path_aliases.remove(alias).is_some()
	}

	/// Add a global file path. Returns false if already present (duplicate).
	pub fn add_global_file(&mut self, path: PathBuf) -> bool {
		if self.global_files.iter().any(|p| p == &path) {
			return false;
		}
		self.global_files.push(path);
		true
	}

	/// Remove a global file path. Returns true if it existed.
	pub fn remove_global_file(&mut self, path: &Path) -> bool {
		let before = self.global_files.len();
		self.global_files.retain(|p| p != path);
		self.global_files.len() < before
	}

	// ── Secure keyring-backed key management ────────────────────────

	/// Add a private key to the OS credential store.
	/// Stores the fingerprint in settings and the private key in the keyring.
	/// Returns an error if the OS credential store is unavailable.
	pub fn add_secret_key_secure(&mut self, private_key_hex: String) -> Result<bool, SettingsError> {
		let fingerprint =
			runfile_crypto::derive_public_key(&private_key_hex).map_err(|e| SettingsError::Keyring(e.to_string()))?;

		// Check for duplicates
		if self.secure_key_fingerprints.contains(&fingerprint) {
			return Ok(false);
		}

		if !keyring_store::is_available() {
			return Err(SettingsError::Keyring(
				"OS credential store is unavailable. Private keys require a working \
				 credential store (Windows Credential Manager / macOS Keychain / \
				 Linux Secret Service)."
					.to_string(),
			));
		}

		keyring_store::store_key(&fingerprint, &private_key_hex).map_err(|e| SettingsError::Keyring(e.to_string()))?;
		self.secure_key_fingerprints.push(fingerprint);

		Ok(true)
	}

	/// Remove a private key from the OS credential store by its fingerprint.
	pub fn remove_secret_key_secure(&mut self, fingerprint: &str) -> Result<bool, SettingsError> {
		let was_secure = self.secure_key_fingerprints.len();
		self.secure_key_fingerprints.retain(|f| f != fingerprint);
		let removed_secure = self.secure_key_fingerprints.len() < was_secure;

		if removed_secure {
			// Best-effort removal from keyring (may already be gone)
			let _ = keyring_store::delete_key(fingerprint);
			return Ok(true);
		}

		Ok(false)
	}

	/// Resolve all private keys from the OS credential store.
	/// Returns the actual private key hex strings ready for use in decryption.
	pub fn resolve_private_keys(&self) -> Vec<String> {
		let mut keys = Vec::new();

		for fingerprint in &self.secure_key_fingerprints {
			match keyring_store::load_key(fingerprint) {
				Ok(Some(private_key)) => keys.push(private_key),
				Ok(None) => {
					eprintln!("Warning: private key for fingerprint {fingerprint} not found in OS credential store.");
				}
				Err(e) => {
					eprintln!("Warning: failed to load key {fingerprint} from credential store: {e}");
				}
			}
		}

		keys
	}

	/// Delete the settings file from the default platform location.
	///
	/// Safety: only deletes the single file at the known settings path.
	/// Does NOT delete directories or use any recursive operations.
	/// Returns Ok(true) if the file was deleted, Ok(false) if it didn't exist.
	pub fn delete_settings_file() -> Result<bool, SettingsError> {
		let path = paths::settings_file_path().ok_or(SettingsError::NoSettingsDir)?;
		Self::delete_settings_file_at(&path)
	}

	/// Delete a specific settings file.
	///
	/// Safety: uses `symlink_metadata` (which does NOT follow symlinks) to
	/// verify the path is a regular file before deleting.  This avoids a
	/// TOCTOU race where an attacker could swap the file for a symlink
	/// between the check and the `remove_file` call.  `remove_file` on
	/// Unix removes the symlink itself (not its target), but we still
	/// refuse symlinks to make intent explicit and prevent surprises.
	pub fn delete_settings_file_at(path: &Path) -> Result<bool, SettingsError> {
		// symlink_metadata does NOT follow symlinks — it reports the
		// metadata of the link itself, so is_file() returns false for
		// symlinks (even those pointing to regular files).
		let metadata = match std::fs::symlink_metadata(path) {
			Ok(m) => m,
			Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
			Err(e) => return Err(SettingsError::Io(e)),
		};

		// Refuse anything that isn't a regular file (directories, symlinks,
		// pipes, etc.).  metadata.is_file() is false for symlinks because
		// we used symlink_metadata.
		if !metadata.is_file() {
			return Err(SettingsError::Io(std::io::Error::new(
				std::io::ErrorKind::InvalidInput,
				format!("Refusing to delete non-regular-file path: {}", path.display()),
			)));
		}

		std::fs::remove_file(path)?;
		Ok(true)
	}
}
