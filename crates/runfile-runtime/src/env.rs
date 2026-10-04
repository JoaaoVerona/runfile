//! Building the environment a block's process sees.
//!
//! `.env-file` is resolved *before* the body is evaluated, which is what lets
//! `{{ ENV.x }}` and `--stdin-args` see those values. Decryption rides on the
//! existing `runfile-env` path, so an `encrypted:` value is handled without the
//! runtime knowing anything about keys.

use crate::props::Props;
use runfile_env::{EnvBuildParams, PrivateKeyProvider, build_env};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum EnvError {
	#[error("{0}")]
	Build(String),
}

/// Adapts the language crate's deferred key pool to the env crate's provider
/// trait. Neither crate depends on the other, so the bridge lives here.
pub struct Provider(pub runfile_lang::Keys);

impl PrivateKeyProvider for Provider {
	fn keys(&self) -> &[String] {
		self.0.get()
	}
}

/// The environment a target is `run` with, as the target that ran it hands it
/// over: what the caller's commands have at that line, in two halves.
///
/// A value that only a `.env-file` supplied is a **default**, and goes beneath
/// the called target's own files, so its own `.env-file` still replaces it.
/// Everything else is **exported** -- the caller's shell, its `.env`, its
/// `.add-path`, and whatever it was run with itself -- and stands where a
/// shell's variables stand: above the called target's files, below its `.env`.
///
/// Handing the whole of it over as exported is what `$ run x` does, since a new
/// process is given nothing but variables. It is what would let a caller's
/// `.env` beat the called target's `.env.test`: `run test` from `ci` would test
/// against the development database.
#[derive(Clone, Default, PartialEq)]
pub struct Inherited {
	pub exported: HashMap<String, String>,
	pub defaults: HashMap<String, String>,
}

impl Inherited {
	/// What a target run with this has before it sets anything itself, which
	/// is what `ENV.X` reads until one of its properties changes it.
	pub fn environment(&self) -> HashMap<String, String> {
		let mut env = self.defaults.clone();
		env.extend(self.exported.iter().map(|(k, v)| (k.clone(), v.clone())));
		env
	}
}

impl std::fmt::Debug for Inherited {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		// Names only: a value may well be a decrypted secret.
		fn names(half: &HashMap<String, String>) -> Vec<&String> {
			let mut keys: Vec<&String> = half.keys().collect();
			keys.sort();
			keys
		}
		f.debug_struct("Inherited")
			.field("exported", &names(&self.exported))
			.field("defaults", &names(&self.defaults))
			.finish()
	}
}

/// What a `run` at this line hands the target it runs: `env`, the environment
/// a command here would be given, split into its two halves.
///
/// A value is a file's when neither layer that beats a file put it there --
/// the environment these properties were themselves run with (the process's,
/// for a target run from the command line), and their `.env`. A value from
/// either of those is exported, as is PATH always: most of what a target does
/// to PATH is its `.add-path`, and a called target's own entries belong in
/// front of that, not beneath its files.
pub fn handed_over(props: &Props, env: &[(String, String)]) -> Inherited {
	let process: HashMap<String, String>;
	let run_with = match &props.inherited {
		Some(inherited) => &inherited.exported,
		None => {
			process = std::env::vars().collect();
			&process
		}
	};
	let mut out = Inherited::default();
	for (k, v) in env {
		let from_a_file = !k.eq_ignore_ascii_case("PATH") && !run_with.contains_key(k) && !props.env.contains_key(k);
		let half = if from_a_file {
			&mut out.defaults
		} else {
			&mut out.exported
		};
		half.insert(k.clone(), v.clone());
	}
	out
}

/// The environment these properties describe, for a target anchored at
/// `anchor`: what `ENV.X` reads and what a command is handed.
///
/// The one builder. The walker rebuilds through it at block boundaries, and a
/// declaration region rebuilds through it part-way when a value is about to
/// read the environment -- so the two cannot come to describe it differently.
/// The same deferred pool the `decrypt` function uses, so an encrypted
/// `.env-file` value resolves, and an unencrypted one never touches the
/// credential store.
pub fn for_props(
	props: &Props,
	anchor: &Path,
	keys: &runfile_lang::Keys,
	preview: bool,
) -> Result<HashMap<String, String>, EnvError> {
	let workdir = match &props.workdir {
		Some(w) => anchor.join(w),
		None => anchor.to_path_buf(),
	};
	build(props, anchor, &workdir, Some(&Provider(keys.clone())), preview)
}

/// Merge the environment the target was run with -- the process's, unless
/// another target ran it -- the declared env files and `.env` into one map,
/// with `.add-path` entries prepended to PATH.
///
/// A preview (`dry_run`) reads no `.env-file` and asks for no key: both would be
/// a side effect of a command documented as changing nothing, and reading an
/// untrusted repository's `.env-file`s unlocks the credential store to decrypt
/// its values with the user's keys -- audit SA-008. `ENV.X` from a file is then
/// absent in the preview, and an `encrypted:` value is left as its ciphertext.
pub fn build(
	props: &Props,
	anchor: &Path,
	workdir: &Path,
	keys: Option<&dyn PrivateKeyProvider>,
	preview: bool,
) -> Result<HashMap<String, String>, EnvError> {
	let env: HashMap<String, String> = props.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
	// Relative `.add-path` entries anchor to the target's directory, the same
	// rule cwd and `.env-file` use, so one anchor explains all three.
	let paths: Vec<String> = props
		.add_paths
		.iter()
		.map(|p| {
			let path = Path::new(p);
			if path.is_absolute() {
				p.clone()
			} else {
				anchor.join(path).to_string_lossy().into_owned()
			}
		})
		.collect();

	// `.env-file` values arrive already evaluated -- properties are evaluated
	// when applied -- so the substitution hook is the identity.
	let inherited = props.inherited.as_deref();
	let params = EnvBuildParams {
		// A preview reads no files and holds no keys: see the note on `build`.
		env_files: if preview { None } else { Some(&props.env_files) },
		env: Some(&env),
		add_to_path: Some(&paths),
		working_dir: workdir,
		env_files_base_dir: anchor,
		available_private_keys: if preview { None } else { keys },
		base_env: inherited.map(|i| &i.exported),
		defaults: inherited.map(|i| &i.defaults),
	};
	build_env(&params, &|s, _| Ok(s.to_string())).map_err(|e| EnvError::Build(e.to_string()))
}
