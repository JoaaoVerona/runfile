use std::path::{Path, PathBuf};
use thiserror::Error;

/// The default (primary) filename, used in error messages and documentation.
pub const RUNFILE_NAME: &str = "Runfile.json";

#[derive(Debug, Error)]
pub enum DiscoverError {
	#[error("No Runfile.json found in {0} or any parent directory")]
	NotFound(PathBuf),
}

/// Search for a `Runfile.json` starting from `start_dir`
/// and walking up through parent directories until one is found or the
/// filesystem root is reached.
pub fn discover_runfile(start_dir: &Path) -> Result<PathBuf, DiscoverError> {
	let mut current = start_dir.to_path_buf();
	loop {
		let candidate = current.join(RUNFILE_NAME);

		if candidate.is_file() {
			return Ok(candidate);
		}

		if !current.pop() {
			return Err(DiscoverError::NotFound(start_dir.to_path_buf()));
		}
	}
}

/// Search for a `Runfile.json` from the current working directory.
pub fn discover_runfile_cwd() -> Result<PathBuf, DiscoverError> {
	let cwd = std::env::current_dir().map_err(|_| DiscoverError::NotFound(PathBuf::from(".")))?;
	discover_runfile(&cwd)
}
