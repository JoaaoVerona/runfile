mod parse;

pub use parse::*;

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use thiserror::Error;

// Re-export crypto utilities for convenience
pub use runfile_crypto::has_encrypted_values;
pub use runfile_crypto::is_encrypted;

/// Provider of decryption private keys. `EnvBuildParams.available_private_keys`
/// takes a `&dyn PrivateKeyProvider` so callers can decide *when* the keys are
/// actually obtained. The trait's `keys()` method is only invoked from
/// [`resolve_decryption_key`], which itself only runs when the merged env
/// actually contains an `encrypted:` value — so a target that does no
/// decryption never triggers the underlying lookup.
///
/// The CLI uses [`LazyPrivateKeys`] to defer the credential-store fetch
/// (and any user-facing unlock prompt) until it's strictly needed.
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

/// Input parameters for building the complete environment variable map.
///
/// All env values should already be converted to strings (e.g. via `EnvValue::to_env_string()`).
/// The caller is responsible for converting non-string types before passing them here.
pub struct EnvBuildParams<'a> {
	/// Env file paths to load (in order; later files override earlier).
	pub env_files: Option<&'a [String]>,
	/// Env vars to set (applied after env files).
	pub env: Option<&'a HashMap<String, String>>,
	/// Directories to prepend to PATH. Entries should already be absolute —
	/// the runtime resolves relative `.add-path` entries against the anchor in
	/// `runfile_runtime::env::build`. The `working_dir` fallback in
	/// `apply_add_to_path` only kicks in for a stray relative entry that did
	/// not go through it.
	pub add_to_path: Option<&'a [String]>,
	/// Working directory the spawned command will run in (= the resolved
	/// `.workdir`). Used as a fallback for any relative `.add-path`
	/// entry that wasn't baked at parse time; not used for `.env-file`.
	pub working_dir: &'a Path,
	/// Base directory for resolving relative `.env-file` paths. Always the
	/// source Runfile's parent directory (`{{ RUN.parent }}`), regardless of
	/// `.workdir` — env files are configuration files co-located with
	/// the Runfile, so anchoring them to the Runfile dir is what users expect
	/// when they tweak `.workdir` for command execution.
	pub env_files_base_dir: &'a Path,
	/// Available private keys for decrypting `encrypted:` prefixed values.
	/// After merging, if encrypted values are detected, `RUNFILE_ENCRYPTION_PUBLIC_KEY`
	/// from the env is matched against this provider's keys to pick the right one.
	///
	/// The provider's `keys()` is invoked at most once per `build_env` call —
	/// and only when an encrypted value is actually present in the merged env.
	/// Callers that load keys from a slow or interactive source (e.g. an OS
	/// credential store) should wrap them in [`LazyPrivateKeys`] so the
	/// lookup is deferred until strictly needed.
	pub available_private_keys: Option<&'a dyn PrivateKeyProvider>,
	/// The environment this build is called with: the layer everything else is
	/// built on, and the one laid back over the `.env-file` values, so what was
	/// exported beats a file. `None` is the process's own environment, which is
	/// what a target run from the command line is called with. A target another
	/// target `run`s is called with its caller's -- less what `defaults` holds.
	pub base_env: Option<&'a HashMap<String, String>>,
	/// Values that only a calling target's `.env-file`s supplied. They stay
	/// defaults, beneath this build's own files: a file is a default however far
	/// its values travelled, so a called target's own `.env-file` still replaces
	/// them. `None` when no target called this one.
	pub defaults: Option<&'a HashMap<String, String>>,
}

/// Load environment variables from env files, applying substitution to file paths.
/// Missing files are silently skipped. Parse errors are returned.
///
/// The `substitute` function is called on each file path template with the current
/// environment, allowing `{{ ARG.* }}` and `{{ ENV.* }}` expansion in paths.
#[allow(clippy::type_complexity)]
pub fn load_env_files(
	env_files: &[String],
	working_dir: &Path,
	substitute: &dyn Fn(&str, &HashMap<String, String>) -> Result<String, String>,
	current_env: &HashMap<String, String>,
) -> Result<HashMap<String, String>, EnvError> {
	let mut result = HashMap::new();

	for file_template in env_files {
		// Substitute {{ ARG.* }} and {{ ENV.* }} in the file path
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
/// Merge order (lowest → highest priority for non-PATH vars):
/// 1. `defaults` — what only a calling target's `.env-file`s supplied. Still a
///    file's values, so they give way to everything below, this build's own
///    files included.
/// 2. `.env-file` — loaded left-to-right, later files override earlier
/// 3. **The environment the build is called with** — `base_env`, or the
///    process's own, re-overlaid so an exported value beats a file's. A file
///    is a default, which is the dotenv convention: a checked-in `.env` must
///    not clobber what someone exported. For a target another target `run`s,
///    that is everything its caller's commands had but the file values in
///    `defaults`: its caller's `.env` and `.add-path` beat its own files the
///    way a shell's variables would.
/// 4. `env` — the target's own `.env.NAME = value`, which beats all of it. It
///    is an assignment written in the file, and the one place a target can
///    *force* a value; a default the caller may override is spelled out
///    instead, as `.env.PORT = ENV.PORT ? "3000"`. It used to sit below the
///    shell here while the runtime overlaid it back on top for commands, so one
///    target saw two answers: `$PORT` was the property's and `{{ ENV.PORT }}`
///    the caller's.
/// 5. `.add-path` — for PATH only, prepended onto whatever PATH turned out to
///    be, this target's entries first. A called target's therefore go in front
///    of its caller's, which arrived as part of PATH in step 3.
/// 6. Decryption — `encrypted:` values rewritten in place
///
/// Step 1 and step 3 start the map together, so `{{ ENV.X }}` substitution
/// while the files load sees both; the re-overlay in step 3 is what enforces
/// exported-over-file on the final env.
///
/// The `substitute` function is called on env values and file paths, allowing
/// `{{ ARG.* }}`, `{{ FLAG.* }}`, and `{{ ENV.* }}` expansion.
#[allow(clippy::type_complexity)]
pub fn build_env(
	params: &EnvBuildParams<'_>,
	substitute: &dyn Fn(&str, &HashMap<String, String>) -> Result<String, String>,
) -> Result<HashMap<String, String>, EnvError> {
	let process: HashMap<String, String>;
	let base = match params.base_env {
		Some(base) => base,
		None => {
			process = env::vars().collect();
			&process
		}
	};
	// The defaults go in first, so the base is what stands wherever both name a
	// key. A caller hands over the two halves of one environment, so they never
	// do; this only says which would win.
	let mut env_map: HashMap<String, String> = params.defaults.cloned().unwrap_or_default();
	env_map.extend(base.iter().map(|(k, v)| (k.clone(), v.clone())));

	// Layer `.env-file`s (substitution sees the env_map built so far). Relative
	// `.env-file` paths resolve against `env_files_base_dir` — the
	// anchor: the parent of `runfiles/` — NOT the resolved `.workdir`. Env files are
	// configuration co-located with the Runfile.
	if let Some(env_files) = params.env_files {
		let file_vars = load_env_files(env_files, params.env_files_base_dir, substitute, &env_map)?;
		env_map.extend(file_vars);
	}

	// Decrypt encrypted file-loaded values BEFORE the env block runs so that
	// `{{ ENV.SECRET }}` references inside an `env` block see the decrypted
	// plaintext (e.g. so `base64_decode(ENV.X)` works on a value that's both
	// Runfile-encrypted in the file AND base64-encoded). Without this, the
	// env block would see the literal `encrypted:abc...` form and any
	// post-processing would error.
	//
	// `RUNFILE_ENCRYPTION_PUBLIC_KEY` is read from `env_map`, which already
	// contains the environment the build was called with and any defaults, so
	// a key set in the shell, or in a calling target's env file, works the same
	// as one set in this env file. Any decrypted value can still be overwritten
	// by `overlay_base` below — what was exported wins for keys it defines.
	if runfile_crypto::has_encrypted_values(&env_map) {
		let key_hex = resolve_decryption_key(&env_map, params.available_private_keys)?;
		runfile_crypto::decrypt_env_values(&mut env_map, &key_hex).map_err(|e| EnvError::Encryption(e.to_string()))?;
	}

	// Re-overlay the environment the build was called with. Any key it defines
	// now beats whatever a `.env-file` set, restoring the inherited value. PATH
	// is case-aware (Windows uses "Path", Unix "PATH") so we don't end up with
	// two case-different PATH keys.
	overlay_base(&mut env_map, base);

	// Layer the target's own `env` last, so it beats the shell as well as the
	// files: see step 3 above. Substitution sees the env_map built so far, and
	// any encrypted file values have been decrypted by now, so substitutions
	// like `{{ base64_decode(ENV.X) }}` work without the user having to think
	// about decryption ordering.
	if let Some(env_vars) = params.env {
		for (key, raw) in env_vars {
			let resolved = substitute(raw, &env_map).map_err(EnvError::Substitution)?;
			env_map.insert(key.clone(), resolved);
		}
	}

	// Prepend this target's `.add-path` to PATH. After the shell-env overlay
	// PATH is the shell's, so this re-prepends on top of it.
	apply_add_to_path(&mut env_map, params.add_to_path, params.working_dir);

	// Final decrypt pass: if the env block (or shell overlay) somehow
	// introduced an `encrypted:...` value — uncommon but possible — make
	// sure it doesn't leak through to the child process.
	if runfile_crypto::has_encrypted_values(&env_map) {
		let key_hex = resolve_decryption_key(&env_map, params.available_private_keys)?;
		runfile_crypto::decrypt_env_values(&mut env_map, &key_hex).map_err(|e| EnvError::Encryption(e.to_string()))?;
	}

	Ok(env_map)
}

/// Re-overlay the environment the build was called with, so it wins per key.
/// Handles PATH's case-insensitive identity on Windows: if env_map already
/// contains a case-insensitive PATH match, the base's PATH value is written
/// to that existing key rather than introducing a duplicate "Path"/"PATH"
/// pair that would later confuse `Command::envs`.
fn overlay_base(env_map: &mut HashMap<String, String>, base: &HashMap<String, String>) {
	let existing_path_key = env_map.keys().find(|k| k.eq_ignore_ascii_case("PATH")).cloned();
	for (k, v) in base {
		if k.eq_ignore_ascii_case("PATH") {
			let target = existing_path_key.clone().unwrap_or_else(|| k.clone());
			env_map.insert(target, v.clone());
		} else {
			env_map.insert(k.clone(), v.clone());
		}
	}
}

/// Prepend this target's `.add-path` entries to PATH, in the order written, so
/// they are found ahead of whatever PATH already held. Relative paths resolve
/// against `working_dir`. No-op when there are none.
///
/// An entry PATH already holds is moved to the front rather than added a
/// second time. A target called by another is called with its caller's PATH,
/// so a `_shared.run` both of them read would otherwise put the same directory
/// on it again at every level. The move finds exactly what the copy would
/// have: PATH is searched in order, so a later copy of a directory is never
/// the one a lookup reaches.
fn apply_add_to_path(env_map: &mut HashMap<String, String>, this_target: Option<&[String]>, working_dir: &Path) {
	let this_layer: &[String] = this_target.unwrap_or(&[]);
	if this_layer.is_empty() {
		return;
	}

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
			working_dir.join(p).to_string_lossy().to_string()
		}
	};

	// This target's entries first, then whatever PATH already held, less the
	// entries it has just put in front. What is left is split and rejoined on
	// the separator it was split on, so a PATH none of them was on comes back
	// exactly as it was.
	let added: Vec<String> = this_layer.iter().map(&resolve).collect();
	let mut new_paths = added.clone();
	if !current_path.is_empty() {
		new_paths.extend(
			current_path
				.split(separator)
				.filter(|held| !added.iter().any(|a| a == held))
				.map(str::to_string),
		);
	}

	env_map.insert(path_key, new_paths.join(separator));
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
