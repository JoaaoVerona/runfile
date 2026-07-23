//! Machine-local record of which preparation targets have run, and at what
//! command-content hash. Kept in `state.json` (a **separate** file from
//! `settings.json`) because it is ephemeral machine state, not user
//! configuration — the two shouldn't drift or share a schema.
//!
//! The file maps an absolute Runfile path to a map of prepare-invocation string
//! (`"setup"`, `"setup-tests --fast"`) → the hash recorded when that invocation
//! last completed successfully. The runner compares the current hash (from
//! [`runfile_parser::Runfile::prepare_command_hash`]) against the recorded one
//! to decide whether a preparation target still counts as satisfied.

use crate::paths;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PrepareStateError {
	#[error("Failed to read prepare-state file: {0}")]
	Io(#[from] std::io::Error),

	#[error("Failed to parse prepare-state: {0}")]
	Json5(#[from] json5::Error),

	#[error("Failed to serialize prepare-state: {0}")]
	Json(#[from] serde_json::Error),

	#[error("Cannot determine settings directory on this platform")]
	NoSettingsDir,
}

/// Persisted record of completed preparation runs.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct PrepareState {
	/// Absolute Runfile path → (prepare invocation → recorded command hash).
	#[serde(default, skip_serializing_if = "HashMap::is_empty")]
	pub prepared: HashMap<String, HashMap<String, String>>,
}

impl PrepareState {
	/// Load prepare-state from the default platform location.
	/// Returns empty state if the file doesn't exist.
	pub fn load() -> Result<Self, PrepareStateError> {
		let path = paths::state_file_path().ok_or(PrepareStateError::NoSettingsDir)?;
		Self::load_from(&path)
	}

	/// Load prepare-state from a specific path.
	/// Returns empty state if the file doesn't exist.
	pub fn load_from(path: &Path) -> Result<Self, PrepareStateError> {
		if !path.exists() {
			return Ok(Self::default());
		}
		let content = std::fs::read_to_string(path)?;
		let state: PrepareState = runfile_parser::from_json_str(&content)?;
		Ok(state)
	}

	/// Save prepare-state to the default platform location.
	pub fn save(&self) -> Result<(), PrepareStateError> {
		let path = paths::state_file_path().ok_or(PrepareStateError::NoSettingsDir)?;
		self.save_to(&path)
	}

	/// Save prepare-state to a specific path, creating parent dirs as needed.
	pub fn save_to(&self, path: &Path) -> Result<(), PrepareStateError> {
		if let Some(parent) = path.parent() {
			std::fs::create_dir_all(parent)?;
		}
		let json = serde_json::to_string_pretty(self)?;
		std::fs::write(path, json)?;
		Ok(())
	}

	/// The hash recorded for a given (Runfile, invocation), if any.
	pub fn recorded_hash(&self, runfile_path: &Path, invocation: &str) -> Option<&str> {
		self.prepared
			.get(&path_key(runfile_path))
			.and_then(|m| m.get(invocation))
			.map(String::as_str)
	}

	/// Record (or overwrite) the hash for a completed prepare invocation.
	pub fn record(&mut self, runfile_path: &Path, invocation: impl Into<String>, hash: impl Into<String>) {
		self.prepared
			.entry(path_key(runfile_path))
			.or_default()
			.insert(invocation.into(), hash.into());
	}
}

/// Normalise a Runfile path into the stable string key used in the state file.
/// Canonicalizes when possible so `./Runfile.json` and its absolute form map to
/// the same entry; falls back to the lossy string form when canonicalization
/// fails (e.g. the file was moved between record and lookup).
pub fn path_key(runfile_path: &Path) -> String {
	std::fs::canonicalize(runfile_path)
		.unwrap_or_else(|_| runfile_path.to_path_buf())
		.to_string_lossy()
		.into_owned()
}
