//! The key file a Linux machine with no Secret Service keeps its keys in.

use super::*;
use crate::file_store::{delete, load, load_as, present, store};
use std::os::unix::fs::{MetadataExt, PermissionsExt};

fn mode(p: &std::path::Path) -> u32 {
	std::fs::metadata(p).unwrap().mode() & 0o777
}

#[test]
fn a_missing_file_is_no_keys_rather_than_an_error() {
	let d = TempDir::new().unwrap();
	let p = d.path().join("runfile").join("keys.json");
	assert!(!present(&p));
	assert_eq!(load(&p).unwrap(), None);
	assert!(!delete(&p).unwrap());
}

#[test]
fn the_file_is_made_owner_only_in_an_owner_only_directory() {
	let d = TempDir::new().unwrap();
	// Neither directory exists yet: `~/.local/share` is made the ordinary way,
	// and only the directory that is ours is narrowed.
	let share = d.path().join("share");
	let p = share.join("runfile").join("keys.json");
	store(&p, r#"{"fp":"key"}"#).unwrap();
	assert!(present(&p));
	assert_eq!(mode(&p), 0o600);
	assert_eq!(mode(p.parent().unwrap()), 0o700);
	assert_ne!(mode(&share), 0o700, "a directory that is not ours was narrowed");
	assert_eq!(load(&p).unwrap().as_deref().map(str::trim), Some(r#"{"fp":"key"}"#));
	assert!(delete(&p).unwrap());
	assert!(!present(&p));
}

#[test]
fn rewriting_a_file_someone_widened_narrows_it_again() {
	let d = TempDir::new().unwrap();
	let p = d.path().join("keys.json");
	std::fs::write(&p, "{}").unwrap();
	std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
	store(&p, r#"{"fp":"key"}"#).unwrap();
	assert_eq!(mode(&p), 0o600);
}

#[test]
fn a_file_others_can_read_or_write_is_refused_with_the_fix() {
	let d = TempDir::new().unwrap();
	let p = d.path().join("keys.json");
	std::fs::write(&p, r#"{"fp":"key"}"#).unwrap();
	for wide in [0o644, 0o640, 0o604, 0o620, 0o602] {
		std::fs::set_permissions(&p, std::fs::Permissions::from_mode(wide)).unwrap();
		let err = load(&p).unwrap_err().to_string();
		assert!(err.contains(&format!("mode {wide:04o}")), "{wide:o}: {err}");
		assert!(err.contains("chmod 600"), "{err}");
	}
	std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
	assert!(load(&p).unwrap().is_some());
	// Narrower than 0600 is the owner's business.
	std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o400)).unwrap();
	assert!(load(&p).unwrap().is_some());
}

#[test]
fn a_file_owned_by_someone_else_is_refused() {
	// A test cannot make a file another account owns, so the account reading
	// is the one that changes.
	let d = TempDir::new().unwrap();
	let p = d.path().join("keys.json");
	store(&p, r#"{"fp":"key"}"#).unwrap();
	let owner = std::fs::metadata(&p).unwrap().uid();
	assert!(load_as(&p, owner).unwrap().is_some());
	let err = load_as(&p, owner + 1).unwrap_err().to_string();
	assert!(err.contains(&format!("owned by uid {owner}")), "{err}");
}

#[test]
fn something_that_is_not_a_file_is_refused() {
	let d = TempDir::new().unwrap();
	let p = d.path().join("keys.json");
	std::fs::create_dir(&p).unwrap();
	std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap();
	let err = load(&p).unwrap_err().to_string();
	assert!(err.contains("not a regular file"), "{err}");
}

#[test]
fn a_symlink_is_judged_by_the_file_it_names() {
	// Someone keeping the file on an encrypted volume links it there; the link
	// is followed, and the file at the end of it has to be private.
	let d = TempDir::new().unwrap();
	let real = d.path().join("vault-keys.json");
	store(&real, r#"{"fp":"key"}"#).unwrap();
	let p = d.path().join("keys.json");
	std::os::unix::fs::symlink(&real, &p).unwrap();
	assert!(load(&p).unwrap().is_some());
	store(&p, r#"{"fp":"other"}"#).unwrap();
	assert!(
		std::fs::symlink_metadata(&p).unwrap().file_type().is_symlink(),
		"the link was replaced"
	);
	assert!(std::fs::read_to_string(&real).unwrap().contains("other"));
	std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o644)).unwrap();
	assert!(load(&p).is_err());
}
