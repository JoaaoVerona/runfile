mod parse;

pub use parse::*;

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

// Re-export crypto utilities for convenience
pub use runfile_crypto::has_encrypted_values;
pub use runfile_crypto::is_encrypted;

#[derive(Debug, Error)]
pub enum EnvError {
	#[error("Failed to read env file \"{0}\": {1}")]
	ReadError(String, std::io::Error),

	#[error("Failed to parse env file \"{path}\" at line {line}: {message}")]
	ParseError { path: String, line: usize, message: String },

	#[error("{0}")]
	Substitution(String),

	#[error(
		"Duplicate environment variable with different casing: \"{0}\" and \"{1}\". Use a single consistent casing."
	)]
	DuplicateEnvCasing(String, String),

	#[error("Encryption error: {0}")]
	Encryption(String),
}

/// Input parameters for building the complete environment variable map.
///
/// All env values should already be converted to strings (e.g. via `EnvValue::to_env_string()`).
/// The caller is responsible for converting non-string types before passing them here.
pub struct EnvBuildParams<'a> {
	/// Env file paths to load (in order; later files override earlier).
	pub env_files: Option<&'a [String]>,
	/// Env vars to set (applied after env files).
	pub env: Option<&'a HashMap<String, String>>,
	/// Directories to prepend to PATH.
	pub add_to_path: Option<&'a [String]>,
	/// Working directory for resolving relative paths.
	pub working_dir: &'a Path,
	/// Available private keys for decrypting `encrypted:` prefixed values.
	/// After merging, if encrypted values are detected, the key is resolved by:
	/// 1. `RUNFILE_ENCRYPTION_KEY` env var (for CI/CD)
	/// 2. Auto-matching `RUNFILE_ENCRYPTION_PUBLIC_KEY` against these private keys
	pub available_private_keys: Option<&'a [String]>,
	/// Optional override for the env-var base. When `Some`, this map replaces
	/// the default `std::env::vars()` snapshot as the starting layer of the
	/// merged env. Used to pass a parent target's already-resolved env into a
	/// dependency invocation, so `@dep` sees the parent's env on top of which
	/// it layers its own envFiles/env/addToPath. When `None` (the default),
	/// the process's environment is used.
	pub base_env: Option<&'a HashMap<String, String>>,
}

/// Load environment variables from env files, applying substitution to file paths.
/// Missing files are silently skipped. Parse errors are returned.
///
/// The `substitute` function is called on each file path template with the current
/// environment, allowing `$(ARGS)` and `$(ENV)` expansion in paths.
#[allow(clippy::type_complexity)]
pub fn load_env_files(
	env_files: &[String],
	working_dir: &Path,
	substitute: &dyn Fn(&str, &HashMap<String, String>) -> Result<String, String>,
	current_env: &HashMap<String, String>,
) -> Result<HashMap<String, String>, EnvError> {
	let mut result = HashMap::new();

	for file_template in env_files {
		// Substitute $(ARGS) and $(ENV) in the file path
		let file_path_str = substitute(file_template, current_env).map_err(EnvError::Substitution)?;

		// Resolve relative to working directory
		let file_path = if Path::new(&file_path_str).is_absolute() {
			PathBuf::from(&file_path_str)
		} else {
			working_dir.join(&file_path_str)
		};

		// Skip if file doesn't exist
		if !file_path.exists() {
			continue;
		}

		// Read and parse
		let content =
			fs::read_to_string(&file_path).map_err(|e| EnvError::ReadError(file_path.display().to_string(), e))?;

		let pairs = parse_env_file(&content).map_err(|(_line, message)| EnvError::ParseError {
			path: file_path.display().to_string(),
			line: _line,
			message,
		})?;

		for (key, value) in pairs {
			result.insert(key, value);
		}
	}

	Ok(result)
}

/// Build the complete environment variable map for a command execution.
///
/// Merge order (highest priority last):
/// 1. System environment variables
/// 2. Env files (in order)
/// 3. Env vars (with substitution)
/// 4. PATH modifications (prepended to existing PATH)
///
/// The `substitute` function is called on env values and file paths, allowing
/// `$(ARGS)`, `$(FLAGS)`, and `$(ENV)` expansion.
#[allow(clippy::type_complexity)]
pub fn build_env(
	params: &EnvBuildParams<'_>,
	substitute: &dyn Fn(&str, &HashMap<String, String>) -> Result<String, String>,
) -> Result<HashMap<String, String>, EnvError> {
	let mut env_map: HashMap<String, String> = match params.base_env {
		Some(base) => base.clone(),
		None => env::vars().collect(),
	};

	// Apply env files (loaded with system env available for $(ENV) substitution)
	if let Some(env_files) = params.env_files {
		let file_vars = load_env_files(env_files, params.working_dir, substitute, &env_map)?;
		env_map.extend(file_vars);
	}

	// Apply env vars (with substitution, override env files)
	if let Some(env_vars) = params.env {
		for (key, raw) in env_vars {
			let resolved = substitute(raw, &env_map).map_err(EnvError::Substitution)?;
			env_map.insert(key.clone(), resolved);
		}
	}

	// Prepend addToPath entries to existing PATH
	if let Some(paths) = params.add_to_path {
		// Find the actual key casing used by the system (e.g. "Path" on Windows, "PATH" on Unix)
		let path_key = env_map
			.keys()
			.find(|k| k.eq_ignore_ascii_case("PATH"))
			.cloned()
			.unwrap_or_else(|| "PATH".to_string());

		let current_path = env_map.get(&path_key).cloned().unwrap_or_default();
		let separator = if cfg!(windows) { ";" } else { ":" };

		let resolve = |p: &String| -> String {
			let path = PathBuf::from(p);
			if path.is_absolute() {
				path.to_string_lossy().to_string()
			} else {
				params.working_dir.join(p).to_string_lossy().to_string()
			}
		};

		let mut new_paths: Vec<String> = paths.iter().map(&resolve).collect();
		if !current_path.is_empty() {
			new_paths.push(current_path);
		}

		env_map.insert(path_key, new_paths.join(separator));
	}

	// Decrypt any encrypted env values if present
	if runfile_crypto::has_encrypted_values(&env_map) {
		let key_hex = resolve_decryption_key(&env_map, params.available_private_keys)?;
		runfile_crypto::decrypt_env_values(&mut env_map, &key_hex).map_err(|e| EnvError::Encryption(e.to_string()))?;
	}

	Ok(env_map)
}

/// Resolve the private key for decrypting encrypted env values.
///
/// Resolution order:
/// 1. `RUNFILE_ENCRYPTION_KEY` env var in the env map (for CI/CD)
/// 2. `RUNFILE_ENCRYPTION_PUBLIC_KEY` in the env map → match against available private keys
/// 3. Error if no key can be resolved
fn resolve_decryption_key(
	env_map: &HashMap<String, String>,
	available_private_keys: Option<&[String]>,
) -> Result<String, EnvError> {
	// 1. Check RUNFILE_ENCRYPTION_KEY in the merged env (includes system env)
	if let Some(key) = env_map.get("RUNFILE_ENCRYPTION_KEY") {
		if !key.is_empty() {
			// Validate format: must be 64 hex chars
			if key.len() != 64 || hex::decode(key).is_err() {
				return Err(EnvError::Encryption(
					"RUNFILE_ENCRYPTION_KEY must be a 64-character hex string (256-bit AES key).".to_string(),
				));
			}
			// If RUNFILE_ENCRYPTION_PUBLIC_KEY is also present, verify the key matches
			if let Some(public_key) = env_map.get(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR) {
				if let Ok(derived) = runfile_crypto::derive_public_key(key) {
					if derived != *public_key {
						return Err(EnvError::Encryption(format!(
							"RUNFILE_ENCRYPTION_KEY does not match {}. \
							 The provided key's fingerprint ({}) differs from the expected fingerprint ({}).",
							runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR,
							&derived[..16],
							&public_key[..public_key.len().min(16)],
						)));
					}
				}
			}
			return Ok(key.clone());
		}
	}

	// 2. Check RUNFILE_ENCRYPTION_PUBLIC_KEY and match against available private keys
	if let Some(public_key) = env_map.get(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR) {
		if let Some(private_keys) = available_private_keys {
			if let Some(matched) = runfile_crypto::find_matching_private_key(public_key, private_keys) {
				return Ok(matched);
			}
			return Err(EnvError::Encryption(format!(
				"Found {} in env but no matching private key is configured. \
				 Run `run :env secret-keys add` to add a key.",
				runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR
			)));
		}
		return Err(EnvError::Encryption(format!(
			"Found {} in env but no private keys are available. \
			 Set RUNFILE_ENCRYPTION_KEY env var or configure keys via `run :env secret-keys add`.",
			runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR
		)));
	}

	Err(EnvError::Encryption(
		"Encrypted env values found but no encryption key available. \
		 Set RUNFILE_ENCRYPTION_KEY env var or ensure env files contain RUNFILE_ENCRYPTION_PUBLIC_KEY."
			.to_string(),
	))
}

/// Check for duplicate env var keys with different casing.
/// Returns an error if e.g. both "NODE_ENV" and "node_env" are defined.
pub fn check_env_case_duplicates(env: &HashMap<String, String>) -> Result<(), EnvError> {
	let mut seen: HashMap<String, String> = HashMap::new(); // lowercase -> original
	for key in env.keys() {
		let lower = key.to_lowercase();
		if let Some(existing) = seen.get(&lower) {
			if existing != key {
				return Err(EnvError::DuplicateEnvCasing(existing.clone(), key.clone()));
			}
		} else {
			seen.insert(lower, key.clone());
		}
	}
	Ok(())
}

/// Collect only the env vars explicitly set by the Runfile.
/// Returns them in a deterministic order (sorted by key).
///
/// This does NOT include system env vars — only the vars defined in the Runfile.
pub fn collect_runfile_env(env: Option<&HashMap<String, String>>) -> Vec<(String, String)> {
	let mut pairs: Vec<(String, String)> = match env {
		Some(e) => e.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
		None => Vec::new(),
	};
	pairs.sort_by(|a, b| a.0.cmp(&b.0));
	pairs
}

#[cfg(test)]
mod tests;
