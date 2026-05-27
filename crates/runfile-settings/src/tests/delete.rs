use super::*;

// ── Reset / delete settings tests ──────────────────────────────────

#[test]
fn delete_settings_file_removes_file() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("settings.json");

	let mut settings = Settings::default();
	settings.set_shell_path("bash", PathBuf::from("/bin/bash"));
	settings.save_to(&path).unwrap();
	assert!(path.exists());

	let deleted = Settings::delete_settings_file_at(&path).unwrap();
	assert!(deleted);
	assert!(!path.exists());
}

#[test]
fn delete_settings_file_returns_false_when_missing() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("nonexistent.json");

	let deleted = Settings::delete_settings_file_at(&path).unwrap();
	assert!(!deleted);
}

#[test]
fn delete_settings_file_refuses_directory() {
	let dir = TempDir::new().unwrap();
	// Try to "delete" a directory — must be refused
	let result = Settings::delete_settings_file_at(dir.path());
	assert!(result.is_err());
}

#[test]
fn delete_settings_file_only_removes_target_file() {
	let dir = TempDir::new().unwrap();
	let settings_path = dir.path().join("settings.json");
	let other_file = dir.path().join("other.txt");

	// Create both files
	std::fs::write(&settings_path, "{}").unwrap();
	std::fs::write(&other_file, "keep me").unwrap();

	Settings::delete_settings_file_at(&settings_path).unwrap();

	// settings.json should be gone, other.txt untouched
	assert!(!settings_path.exists());
	assert!(other_file.exists());
	assert_eq!(std::fs::read_to_string(&other_file).unwrap(), "keep me");
}

// ── Fix #15: TOCTOU-safe delete with symlink_metadata ─────────────

#[test]
fn delete_settings_file_returns_false_for_nonexistent() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("does_not_exist.json");
	let result = Settings::delete_settings_file_at(&path).unwrap();
	assert!(!result);
}

#[test]
fn delete_settings_file_deletes_regular_file() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("settings.json");
	std::fs::write(&path, "{}").unwrap();
	assert!(path.exists());

	let deleted = Settings::delete_settings_file_at(&path).unwrap();
	assert!(deleted);
	assert!(!path.exists());
}

#[test]
fn delete_settings_file_refuses_directory_with_symlink_metadata() {
	let dir = TempDir::new().unwrap();
	let subdir = dir.path().join("subdir");
	std::fs::create_dir(&subdir).unwrap();

	let result = Settings::delete_settings_file_at(&subdir);
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(
		err.contains("non-regular-file"),
		"should mention non-regular-file: {err}"
	);
	// Directory should still exist
	assert!(subdir.exists());
}

#[cfg(unix)]
#[test]
fn delete_settings_file_refuses_symlink_to_file() {
	let dir = TempDir::new().unwrap();
	let target = dir.path().join("target.json");
	let link = dir.path().join("link.json");

	std::fs::write(&target, "{}").unwrap();
	std::os::unix::fs::symlink(&target, &link).unwrap();

	// symlink_metadata sees the link itself (not a regular file)
	let result = Settings::delete_settings_file_at(&link);
	assert!(result.is_err(), "should refuse to delete symlink");

	// Both should still exist
	assert!(target.exists(), "target should not be deleted");
	assert!(link.exists(), "symlink should not be deleted");
}

#[cfg(unix)]
#[test]
fn delete_settings_file_refuses_symlink_to_directory() {
	let dir = TempDir::new().unwrap();
	let target_dir = dir.path().join("target_dir");
	let link = dir.path().join("link_dir");

	std::fs::create_dir(&target_dir).unwrap();
	std::os::unix::fs::symlink(&target_dir, &link).unwrap();

	let result = Settings::delete_settings_file_at(&link);
	assert!(result.is_err(), "should refuse to delete symlink to directory");
	assert!(target_dir.exists(), "target directory should not be deleted");
}

#[cfg(unix)]
#[test]
fn delete_settings_file_refuses_dangling_symlink() {
	let dir = TempDir::new().unwrap();
	let link = dir.path().join("dangling.json");

	// Create a symlink to a non-existent target
	std::os::unix::fs::symlink("/nonexistent/path", &link).unwrap();

	// symlink_metadata should still see the symlink even though target doesn't exist
	let result = Settings::delete_settings_file_at(&link);
	// The link itself exists, symlink_metadata succeeds, but is_file() is false for symlinks
	assert!(result.is_err(), "should refuse to delete dangling symlink");
}

#[test]
fn delete_settings_file_does_not_affect_other_files() {
	let dir = TempDir::new().unwrap();
	let settings = dir.path().join("settings.json");
	let other1 = dir.path().join("other1.txt");
	let other2 = dir.path().join("other2.txt");

	std::fs::write(&settings, "{}").unwrap();
	std::fs::write(&other1, "keep1").unwrap();
	std::fs::write(&other2, "keep2").unwrap();

	Settings::delete_settings_file_at(&settings).unwrap();

	assert!(!settings.exists());
	assert_eq!(std::fs::read_to_string(&other1).unwrap(), "keep1");
	assert_eq!(std::fs::read_to_string(&other2).unwrap(), "keep2");
}

#[test]
fn delete_settings_file_error_message_includes_path() {
	let dir = TempDir::new().unwrap();
	let subdir = dir.path().join("mydir");
	std::fs::create_dir(&subdir).unwrap();

	let result = Settings::delete_settings_file_at(&subdir);
	let err = result.unwrap_err().to_string();
	assert!(err.contains("mydir"), "error should include the path: {err}");
}
