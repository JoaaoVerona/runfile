//! Replacing a file in one step (audit SA-033, SA-038).

use crate::atomic::{Mode, write};

/// What is in `dir` besides `keep`: a temp file left behind would show up here.
fn others(dir: &std::path::Path, keep: &[&str]) -> Vec<String> {
	let mut left: Vec<String> = std::fs::read_dir(dir)
		.unwrap()
		.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
		.filter(|n| !keep.contains(&n.as_str()))
		.collect();
	left.sort();
	left
}

#[test]
fn contents_are_replaced_and_nothing_is_left_beside_them() {
	let d = tempfile::TempDir::new().unwrap();
	let p = d.path().join("tasks.json");
	std::fs::write(&p, "old contents").unwrap();
	write(&p, b"new", Mode::Keep).unwrap();
	assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
	// A new file is made the same way.
	let q = d.path().join("fresh.run");
	write(&q, b"$ true\n", Mode::Private).unwrap();
	assert_eq!(std::fs::read_to_string(&q).unwrap(), "$ true\n");
	assert_eq!(others(d.path(), &["tasks.json", "fresh.run"]), Vec::<String>::new());
}

#[cfg(unix)]
#[test]
fn the_new_contents_land_in_a_new_inode_that_no_earlier_descriptor_can_read() {
	// Truncating in place let a reader who had opened the file before the write
	// keep reading the new plaintext through that descriptor, and meant the one
	// file was half-written whenever a write failed. Replaced by a rename, the
	// old inode is never written at all: the earlier descriptor still reads what
	// was there, and the name now leads somewhere else.
	use std::io::Read;
	use std::os::unix::fs::MetadataExt;
	let d = tempfile::TempDir::new().unwrap();
	let p = d.path().join(".env");
	std::fs::write(&p, "KEY=old").unwrap();
	let mut earlier = std::fs::File::open(&p).unwrap();
	let before = std::fs::metadata(&p).unwrap().ino();
	write(&p, b"KEY=new-plaintext", Mode::Private).unwrap();
	let mut seen = String::new();
	earlier.read_to_string(&mut seen).unwrap();
	assert_eq!(seen, "KEY=old", "an earlier descriptor saw the new contents");
	assert_ne!(std::fs::metadata(&p).unwrap().ino(), before, "written in place");
	assert_eq!(std::fs::read_to_string(&p).unwrap(), "KEY=new-plaintext");
}

#[cfg(unix)]
#[test]
fn keep_preserves_the_mode_and_private_narrows_it() {
	use std::os::unix::fs::PermissionsExt;
	let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
	let d = tempfile::TempDir::new().unwrap();
	let profile = d.path().join(".zshrc");
	std::fs::write(&profile, "old").unwrap();
	std::fs::set_permissions(&profile, std::fs::Permissions::from_mode(0o640)).unwrap();
	write(&profile, b"new", Mode::Keep).unwrap();
	assert_eq!(mode(&profile), 0o640, "a rewrite keeps the mode the file had");

	let secret = d.path().join("out.env");
	std::fs::write(&secret, "old").unwrap();
	std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o644)).unwrap();
	write(&secret, b"KEY=plaintext", Mode::Private).unwrap();
	assert_eq!(mode(&secret), 0o600, "plaintext is owner-only whatever was there");

	// A new file under `Keep` gets exactly what `fs::write` would have given it.
	let ours = d.path().join("a.json");
	let reference = d.path().join("b.json");
	write(&ours, b"{}", Mode::Keep).unwrap();
	std::fs::write(&reference, "{}").unwrap();
	assert_eq!(mode(&ours), mode(&reference));
}

#[cfg(unix)]
#[test]
fn a_symlink_is_written_through_and_stays_a_symlink() {
	// A `~/.zshrc` linked into a dotfiles repository has to be updated there,
	// not replaced by a regular file that the repository never sees.
	let d = tempfile::TempDir::new().unwrap();
	let real = d.path().join("dotfiles-zshrc");
	let link = d.path().join(".zshrc");
	std::fs::write(&real, "old").unwrap();
	std::os::unix::fs::symlink(&real, &link).unwrap();
	write(&link, b"new", Mode::Keep).unwrap();
	assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
	assert_eq!(std::fs::read_to_string(&real).unwrap(), "new");

	// A link to a file not there yet creates that file, as `fs::write` did.
	let dangling = d.path().join("link-to-new");
	let target = d.path().join("new-target");
	std::os::unix::fs::symlink(&target, &dangling).unwrap();
	write(&dangling, b"made", Mode::Keep).unwrap();
	assert!(std::fs::symlink_metadata(&dangling).unwrap().file_type().is_symlink());
	assert_eq!(std::fs::read_to_string(&target).unwrap(), "made");
}

#[cfg(unix)]
#[test]
fn a_file_that_is_not_a_regular_one_is_written_in_place() {
	// There is nothing to replace beside `/dev/null`, and no temp file can be
	// made in `/dev`.
	write(std::path::Path::new("/dev/null"), b"discarded", Mode::Private).unwrap();
}

#[cfg(unix)]
#[test]
fn a_directory_that_refuses_a_new_file_is_written_in_place() {
	// A writable file in a read-only directory: `fs::write` handled it, and a
	// rename cannot, so it falls back rather than failing.
	use std::os::unix::fs::PermissionsExt;
	let d = tempfile::TempDir::new().unwrap();
	let p = d.path().join("f.run");
	std::fs::write(&p, "old").unwrap();
	std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
	let result = write(&p, b"new", Mode::Keep);
	std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
	result.unwrap();
	assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
}
