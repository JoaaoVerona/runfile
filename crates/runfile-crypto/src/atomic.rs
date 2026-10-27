//! Replacing a file's contents without ever leaving it half-written.
//!
//! `std::fs::write` truncates a file and only then writes it: a full disk, an I/O
//! error or a kill between the two leaves the original empty or partial, with no
//! copy anywhere -- a `.run` file `:lint` was formatting, a task file `:generate`
//! was merging into, a shell profile `:completions` was editing, an encrypted
//! `.env` `:env rotate` was re-keying (audit SA-038). For plaintext secrets it was
//! worse: the file was created at the umask's mode and narrowed only after the
//! write, so another local user could open it in between and keep reading
//! through that descriptor (audit SA-033).
//!
//! [`write`] puts the new contents in a temporary file beside the target --
//! created owner-only, so it is never wider than the result -- syncs it, gives it
//! the mode it should end with, and renames it over the target. A rename replaces
//! the name in one step, so a reader sees the old file or the new one and never
//! part of either; and the new contents live in a new inode, so a descriptor
//! opened on the old file never sees them.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// The permissions the written file ends with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
	/// What it had before, or the umask's default for a new file -- exactly what
	/// `fs::write` would have left. For source and configuration files.
	Keep,
	/// Owner read/write only (`0600` on Unix), whatever it had before. For a file
	/// that may hold plaintext secrets.
	Private,
}

/// Replace the contents of `path` with `bytes` in one step; see the module docs.
///
/// A symlink is followed to the file it names, which is replaced while the link
/// is kept -- what `fs::write` did, so a `~/.zshrc` linked into a dotfiles
/// repository stays linked. A path that is not a regular file (`/dev/stdout`, a
/// FIFO) is written in place, there being no file to replace; so is one whose
/// directory refuses a new file (a writable file in a read-only directory), or
/// whose rename is refused (on Windows, another process holding the file without
/// sharing deletion). Each of those is what `fs::write` did, and none is worse.
pub fn write(path: &Path, bytes: &[u8], mode: Mode) -> io::Result<()> {
	let target = resolve(path);
	let existing = match fs::metadata(&target) {
		Ok(m) if !m.is_file() => return in_place(&target, bytes, mode),
		Ok(m) => Some(m),
		Err(e) if e.kind() == io::ErrorKind::NotFound => None,
		// A loop of links, a directory that cannot be searched, …
		Err(e) => return Err(e),
	};
	let Some(name) = target.file_name() else {
		return in_place(&target, bytes, mode);
	};
	// Created owner-only unless it is a brand-new file with nothing to keep
	// private, which gets the umask's mode exactly as `fs::write` would give it.
	let narrow = !(mode == Mode::Keep && existing.is_none());
	let (tmp, mut file) = match temp_beside(&parent(&target), name, narrow) {
		Ok(t) => t,
		Err(e) if e.kind() == io::ErrorKind::PermissionDenied => return in_place(&target, bytes, mode),
		Err(e) => return Err(e),
	};
	let written = file
		.write_all(bytes)
		.and_then(|()| finish_mode(&file, existing.as_ref(), mode))
		.and_then(|()| file.sync_all());
	// Closed before the rename, which Windows refuses while the file is open.
	drop(file);
	match written {
		Ok(()) => match fs::rename(&tmp, &target) {
			Ok(()) => Ok(()),
			Err(_) => {
				let _ = fs::remove_file(&tmp);
				in_place(&target, bytes, mode)
			}
		},
		Err(e) => {
			let _ = fs::remove_file(&tmp);
			Err(e)
		}
	}
}

/// Follow a chain of symlinks to the path it ends at, which need not exist yet.
/// Bounded the way the kernel bounds it; a loop is then reported by the `stat`.
fn resolve(path: &Path) -> PathBuf {
	let mut p = path.to_path_buf();
	for _ in 0..40 {
		let is_link = fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink());
		if !is_link {
			break;
		}
		match fs::read_link(&p) {
			Ok(next) if next.is_absolute() => p = next,
			Ok(next) => p = parent(&p).join(next),
			Err(_) => break,
		}
	}
	p
}

fn parent(p: &Path) -> PathBuf {
	match p.parent() {
		Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
		_ => PathBuf::from("."),
	}
}

/// A fresh file in `dir` named after `name`, hidden and unique to this process.
/// `create_new`, so a stale one from a killed run is never written into.
fn temp_beside(dir: &Path, name: &std::ffi::OsStr, narrow: bool) -> io::Result<(PathBuf, File)> {
	static NEXT: AtomicU32 = AtomicU32::new(0);
	#[cfg(not(unix))]
	let _ = narrow;
	let pid = std::process::id();
	let mut collided = None;
	for _ in 0..64 {
		let mut tmp_name = std::ffi::OsString::from(".");
		tmp_name.push(name);
		tmp_name.push(format!(".{pid}.{}.tmp", NEXT.fetch_add(1, Ordering::Relaxed)));
		let tmp = dir.join(tmp_name);
		let mut opts = OpenOptions::new();
		opts.write(true).create_new(true);
		#[cfg(unix)]
		if narrow {
			use std::os::unix::fs::OpenOptionsExt;
			opts.mode(0o600);
		}
		match opts.open(&tmp) {
			Ok(f) => return Ok((tmp, f)),
			Err(e) if e.kind() == io::ErrorKind::AlreadyExists => collided = Some(e),
			Err(e) => return Err(e),
		}
	}
	Err(collided.unwrap_or_else(|| io::Error::other("no free name for a temporary file")))
}

/// Give the written temp file the mode -- and, where it can, the owner -- the
/// result should have, before it takes the target's name.
#[cfg(unix)]
fn finish_mode(file: &File, existing: Option<&fs::Metadata>, mode: Mode) -> io::Result<()> {
	use std::os::unix::fs::{MetadataExt, PermissionsExt};
	match (mode, existing) {
		(Mode::Private, _) => file.set_permissions(fs::Permissions::from_mode(0o600)),
		(Mode::Keep, Some(old)) => {
			// Best effort: only a privileged process can give a file away, and a
			// user rewriting their own file has nothing to change. Before the mode,
			// since a change of owner may clear set-id bits.
			let now = file.metadata()?;
			if (now.uid(), now.gid()) != (old.uid(), old.gid()) {
				let _ = std::os::unix::fs::fchown(file, Some(old.uid()), Some(old.gid()));
			}
			file.set_permissions(old.permissions())
		}
		(Mode::Keep, None) => Ok(()),
	}
}

#[cfg(not(unix))]
fn finish_mode(_: &File, _: Option<&fs::Metadata>, _: Mode) -> io::Result<()> {
	Ok(())
}

/// The `fs::write` this replaces, for where a rename cannot be used. A secret is
/// still narrowed to its owner before a byte of it is written.
fn in_place(target: &Path, bytes: &[u8], mode: Mode) -> io::Result<()> {
	let mut opts = OpenOptions::new();
	opts.write(true).create(true).truncate(true);
	#[cfg(unix)]
	if mode == Mode::Private {
		use std::os::unix::fs::OpenOptionsExt;
		opts.mode(0o600);
	}
	let mut f = opts.open(target)?;
	#[cfg(unix)]
	if mode == Mode::Private && f.metadata()?.is_file() {
		use std::os::unix::fs::PermissionsExt;
		f.set_permissions(fs::Permissions::from_mode(0o600))?;
	}
	#[cfg(not(unix))]
	let _ = mode;
	f.write_all(bytes)
}
