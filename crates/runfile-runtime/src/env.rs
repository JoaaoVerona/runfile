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
		parent_add_to_path_chain: None,
	};
	build_env(&params, &|s, _| Ok(s.to_string())).map_err(|e| EnvError::Build(e.to_string()))
}
