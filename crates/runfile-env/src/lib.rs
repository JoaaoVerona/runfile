mod parse;

pub use parse::*;

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;
use thiserror::Error;

// Re-export crypto utilities for convenience
pub use runfile_crypto::has_encrypted_values;
pub use runfile_crypto::is_encrypted;

/// Provider of decryption private keys. [`read_env_file`] takes a
/// `&dyn PrivateKeyProvider` so callers can decide *when* the keys are actually
/// obtained: `keys()` is only invoked when a file actually holds an
/// `encrypted:` value -- so a target that decrypts nothing never triggers the
/// underlying lookup.
///
/// The language's deferred key pool is one, so the OS credential store (and any
/// user-facing unlock prompt) is only reached when strictly needed.
///
/// `Sync` is a supertrait because a provider can be shared by reference
/// across parallel-execution worker threads.
pub trait PrivateKeyProvider: Sync {
	/// Return the key pool to try when matching `RUNFILE_ENCRYPTION_PUBLIC_KEY`.
	fn keys(&self) -> &[String];
}

// Blanket impl so any `Sync` type that hands out a `&[String]` is a provider.
// Covers `Vec<String>` and `[String; N]` directly — call sites that pass
// `Some(&vec_of_keys)` coerce automatically to `Option<&dyn PrivateKeyProvider>`.
impl<T: AsRef<[String]> + Sync + ?Sized> PrivateKeyProvider for T {
	fn keys(&self) -> &[String] {
		self.as_ref()
	}
}

/// Lazy [`PrivateKeyProvider`]: invokes `loader` on first `keys()` call and
/// memoizes the result for the lifetime of the value. The CLI wraps the
/// `keyring_keys::all_private_keys()` lookup in one of these so the OS
/// credential store is only touched when an encrypted env value is actually
/// encountered during env build.
pub struct LazyPrivateKeys {
	cell: OnceLock<Vec<String>>,
	loader: Box<dyn Fn() -> Vec<String> + Send + Sync>,
}

impl LazyPrivateKeys {
	pub fn new(loader: impl Fn() -> Vec<String> + Send + Sync + 'static) -> Self {
		Self {
			cell: OnceLock::new(),
			loader: Box::new(loader),
		}
	}
}

impl PrivateKeyProvider for LazyPrivateKeys {
	fn keys(&self) -> &[String] {
		self.cell.get_or_init(|| (self.loader)())
	}
}

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

/// One `.env` file's values, its `encrypted:` ones decrypted -- or `None` when
/// there is no file at `path`, which is passed over rather than refused: a
/// project whose `.env` is git-ignored still has to run.
///
/// **Only the file's own values are decrypted.** A ciphertext is the author's
/// exactly when a committed file holds it; one arriving any other way --
/// exported in the caller's shell, or set with `ENV.X = ARG.x` from a value an
/// outsider controls (a PR title a CI job exposes) -- used to be decrypted too,
/// with the job's key, and handed back as plaintext: an oracle for every secret
/// in the repository's committed `.env` files (audit SA-004). The public key a
/// ciphertext needs is looked up in `env`, what the run has so far, with the
/// file's own values over it, since a file usually carries its own
/// `RUNFILE_ENCRYPTION_PUBLIC_KEY` line. `keys` is asked only when the file
/// holds an encrypted value, which is what keeps a plain `.env` from touching
/// the credential store.
///
/// A parse error names the file and the line, and never the line's text: it may
/// be a secret (audit SA-026).
pub fn read_env_file(
	path: &Path,
	env: &HashMap<String, String>,
	keys: Option<&dyn PrivateKeyProvider>,
) -> Result<Option<HashMap<String, String>>, EnvError> {
	if !path.exists() {
		return Ok(None);
	}
	let content = fs::read_to_string(path).map_err(|e| EnvError::ReadError(path.display().to_string(), e))?;
	let pairs = parse_env_file(&content).map_err(|(line, message)| EnvError::ParseError {
		path: path.display().to_string(),
		line,
		message,
	})?;
	let mut vars: HashMap<String, String> = pairs.into_iter().collect();
	if runfile_crypto::has_encrypted_values(&vars) {
		let mut view = env.clone();
		view.extend(vars.iter().map(|(k, v)| (k.clone(), v.clone())));
		let key_hex = resolve_decryption_key(&view, keys)?;
		runfile_crypto::decrypt_env_values(&mut vars, &key_hex).map_err(|e| EnvError::Encryption(e.to_string()))?;
	}
	Ok(Some(vars))
}

/// The process environment as a UTF-8 map. `vars_os` rather than `vars`: a
/// value that is not valid UTF-8 -- bash exports a Latin-1 `OLDPWD`/`PWD` after
/// `cd` into such a directory -- makes `std::env::vars` panic, which aborted
/// every `run <target>` before anything ran (audit SA-023). A non-UTF-8 entry
/// is skipped here and still reaches a child untouched, since the runner never
/// clears the environment, only overrides keys.
pub fn process_env() -> HashMap<String, String> {
	env::vars_os()
		.filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
		.collect()
}

/// Resolve the private key for decrypting encrypted env values.
///
/// Looks up the value of `RUNFILE_ENCRYPTION_PUBLIC_KEY` in the merged env
/// and matches it against the pool returned by `available_private_keys`
/// (which itself merges `RUNFILE_PRIVATE_KEYS` with the OS credential store
/// — see `runfile_state::keyring_keys::all_private_keys`). Errors if the
/// public key is missing, the pool is empty, or no key in the pool matches.
fn resolve_decryption_key(
	env_map: &HashMap<String, String>,
	available_private_keys: Option<&dyn PrivateKeyProvider>,
) -> Result<String, EnvError> {
	let public_key = env_map.get(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR).ok_or_else(|| {
		EnvError::Encryption(format!(
			"Encrypted env values found but {} is not set in the env. \
			 Encrypted files must declare their public key — re-create the file via `run :env init` \
			 or `run :env encrypt`, or set {} on a line above the encrypted values.",
			runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR,
			runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR,
		))
	})?;

	let provider = available_private_keys.ok_or_else(|| {
		EnvError::Encryption(format!(
			"Found {} in env but no private keys are available. \
			 Set RUNFILE_PRIVATE_KEYS (newline-separated 64-char hex keys) or configure keys via \
			 `run :env secret-keys add`.",
			runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR
		))
	})?;

	let private_keys = provider.keys();
	runfile_crypto::find_matching_private_key(public_key, private_keys).ok_or_else(|| {
		EnvError::Encryption(format!(
			"Found {} in env but no matching private key is configured. \
			 Set RUNFILE_PRIVATE_KEYS or add a key via `run :env secret-keys add`.",
			runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR
		))
	})
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
