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

/// The environment these properties describe, for a target anchored at
/// `anchor`: what `ENV.X` reads and what a command is handed.
///
/// The one builder. The walker rebuilds through it at block boundaries, and a
/// declaration region rebuilds through it part-way when a value is about to
/// read the environment -- so the two cannot come to describe it differently.
/// The same deferred pool the `decrypt` function uses, so an encrypted
/// `.env-file` value resolves, and an unencrypted one never touches the
/// credential store.
pub fn for_props(props: &Props, anchor: &Path, keys: &runfile_lang::Keys) -> Result<HashMap<String, String>, EnvError> {
	let workdir = match &props.workdir {
		Some(w) => anchor.join(w),
		None => anchor.to_path_buf(),
	};
	build(props, anchor, &workdir, Some(&Provider(keys.clone())))
}

/// Merge process env, the declared env files and `.env` into one map, with
/// `.add-path` entries prepended to PATH.
pub fn build(
	props: &Props,
	anchor: &Path,
	workdir: &Path,
	keys: Option<&dyn PrivateKeyProvider>,
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
	let params = EnvBuildParams {
		env_files: Some(&props.env_files),
		env: Some(&env),
		add_to_path: Some(&paths),
		working_dir: workdir,
		env_files_base_dir: anchor,
		available_private_keys: keys,
		base_env: None,
	};
	build_env(&params, &|s, _| Ok(s.to_string())).map_err(|e| EnvError::Build(e.to_string()))
}
