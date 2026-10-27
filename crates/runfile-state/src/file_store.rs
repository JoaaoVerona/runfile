//! The Linux key store for a machine with no Secret Service: one owner-only file.
//!
//! A headless server -- Ubuntu Server, a VM, WSL, a container with a home that
//! outlives it -- has no gnome-keyring or KWallet, and the kernel keyutils
//! store that used to stand in for one forgets everything on reboot and a few
//! days after logout (keyrings(7)), taking every value encrypted under its keys
//! with it (audit SA-017). This file is what ssh, age and gh keep on such a
//! machine: `$XDG_DATA_HOME/runfile/keys.json`, else
//! `~/.local/share/runfile/keys.json`, mode `0600` in a `0700` directory.
//!
//! It is no weaker than what it replaces against anyone but root: any process
//! running as the owner can read keyutils, or an unlocked Secret Service, just
//! as it can read this file, and nothing else can read any of the three. What it
//! gives up is encryption at rest -- a disk image or a backup holds the keys.
//! It is the data directory rather than `~/.config` because the configuration
//! directory is the one people commit to a dotfiles repository.
//!
//! The file holds the same JSON blob the credential stores hold, so it is one
//! more place for the blob and nothing more. It is refused, the way ssh refuses
//! an unprotected private key, when it is not a regular file, is not owned by
//! the account reading it, or can be read or written by anyone else: a file
//! someone else owns or can write is a file someone else chose the keys in.

use crate::keyring_keys::KeyringKeysError;
use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

/// Where the keys are kept: `<data dir>/runfile/keys.json`.
pub(crate) fn path() -> Result<PathBuf, KeyringKeysError> {
	dirs::data_dir()
		.map(|d| d.join("runfile").join("keys.json"))
		.ok_or_else(|| {
			KeyringKeysError::KeyFile(
				"there is no home directory to keep private keys in -- set HOME, or pass the keys in \
				 RUNFILE_PRIVATE_KEYS"
					.to_string(),
			)
		})
}

/// Whether there is anything at `path` to read. An error counts as something,
/// so that reading it reports the error rather than the keys going unseen.
pub(crate) fn present(path: &Path) -> bool {
	!matches!(path.try_exists(), Ok(false))
}

/// Read the blob. `Ok(None)` when there is no file yet.
pub(crate) fn load(path: &Path) -> Result<Option<String>, KeyringKeysError> {
	// SAFETY: `geteuid` takes nothing and cannot fail.
	load_as(path, unsafe { libc::geteuid() })
}

/// [`load`], for the account `uid` -- a parameter so a test can be someone else.
pub(crate) fn load_as(path: &Path, uid: u32) -> Result<Option<String>, KeyringKeysError> {
	let meta = match fs::metadata(path) {
		Ok(m) => m,
		Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
		Err(e) => return Err(failed("read", path, &e)),
	};
	check_private(path, &meta, uid)?;
	fs::read_to_string(path).map(Some).map_err(|e| failed("read", path, &e))
}

/// Refuse a file anyone but `uid` could have written or can read.
fn check_private(path: &Path, meta: &fs::Metadata, uid: u32) -> Result<(), KeyringKeysError> {
	let shown = path.display();
	if !meta.is_file() {
		return Err(KeyringKeysError::KeyFile(format!("{shown} is not a regular file")));
	}
	if meta.uid() != uid {
		return Err(KeyringKeysError::KeyFile(format!(
			"{shown} is owned by uid {}, not by this account (uid {uid}), so its keys are not yours to trust",
			meta.uid()
		)));
	}
	let mode = meta.mode() & 0o777;
	if mode & 0o077 != 0 {
		return Err(KeyringKeysError::KeyFile(format!(
			"{shown} is open to other users (mode {mode:04o}); private keys must be readable by their owner \
			 alone: chmod 600 {shown}"
		)));
	}
	Ok(())
}

/// Write the blob, owner-only and in one step, making the directory if needed.
pub(crate) fn store(path: &Path, blob: &str) -> Result<(), KeyringKeysError> {
	if let Some(dir) = path.parent() {
		make_private_dir(dir).map_err(|e| failed("create the directory for", path, &e))?;
	}
	let mut bytes = blob.as_bytes().to_vec();
	bytes.push(b'\n');
	runfile_crypto::atomic::write(path, &bytes, runfile_crypto::atomic::Mode::Private)
		.map_err(|e| failed("write", path, &e))
}

/// `dir`, made `0700` if it is not there; the directories above it are made the
/// ordinary way, since `~/.local/share` is not ours to narrow.
fn make_private_dir(dir: &Path) -> io::Result<()> {
	if dir.is_dir() {
		return Ok(());
	}
	if let Some(above) = dir.parent() {
		fs::create_dir_all(above)?;
	}
	match fs::DirBuilder::new().mode(0o700).create(dir) {
		Err(e) if e.kind() == io::ErrorKind::AlreadyExists && dir.is_dir() => Ok(()),
		other => other,
	}
}

/// Remove the file. `Ok(false)` if there was none.
pub(crate) fn delete(path: &Path) -> Result<bool, KeyringKeysError> {
	match fs::remove_file(path) {
		Ok(()) => Ok(true),
		Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
		Err(e) => Err(failed("remove", path, &e)),
	}
}

fn failed(doing: &str, path: &Path, e: &io::Error) -> KeyringKeysError {
	KeyringKeysError::KeyFile(format!("could not {doing} {}: {e}", path.display()))
}
