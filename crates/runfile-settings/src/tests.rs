use crate::*;
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn default_settings() {
	let settings = Settings::default();
	assert!(settings.shell_paths.is_empty());
	assert!(settings.path_aliases.is_empty());
}

#[test]
fn load_nonexistent_returns_default() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("nonexistent.json");
	let settings = Settings::load_from(&path).unwrap();
	assert_eq!(settings, Settings::default());
}

#[test]
fn save_and_load_roundtrip() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("settings.json");

	let mut settings = Settings::default();
	settings.set_shell_path("bash", PathBuf::from("/custom/bash"));
	settings.set_shell_path("zsh", PathBuf::from("/opt/zsh"));

	settings.save_to(&path).unwrap();
	let loaded = Settings::load_from(&path).unwrap();
	assert_eq!(settings, loaded);
	assert_eq!(loaded.get_shell_path("bash").unwrap(), &PathBuf::from("/custom/bash"));
	assert_eq!(loaded.get_shell_path("zsh").unwrap(), &PathBuf::from("/opt/zsh"));
}

#[test]
fn save_creates_parent_directories() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("deep").join("nested").join("settings.json");

	let settings = Settings::default();
	settings.save_to(&path).unwrap();
	assert!(path.exists());
}

#[test]
fn get_shell_path_returns_none_for_unknown() {
	let settings = Settings::default();
	assert!(settings.get_shell_path("bash").is_none());
}

#[test]
fn set_shell_path_overwrites() {
	let mut settings = Settings::default();
	settings.set_shell_path("bash", PathBuf::from("/first"));
	settings.set_shell_path("bash", PathBuf::from("/second"));
	assert_eq!(settings.get_shell_path("bash").unwrap(), &PathBuf::from("/second"));
}

#[test]
fn settings_dir_is_some() {
	// Should succeed on all major platforms
	assert!(settings_dir().is_some());
}

#[test]
fn settings_file_path_is_some() {
	assert!(settings_file_path().is_some());
	let path = settings_file_path().unwrap();
	assert!(path.ends_with("settings.json"));
}

#[test]
fn load_malformed_json_returns_error() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("bad.json");
	std::fs::write(&path, "not json at all").unwrap();
	assert!(Settings::load_from(&path).is_err());
}

#[test]
fn load_valid_json_with_extra_fields_is_flexible() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("settings.json");
	// serde default behavior: unknown fields are ignored (no deny_unknown_fields on Settings)
	std::fs::write(&path, r#"{"shell_paths": {}, "unknown_field": true}"#).unwrap();
	// This should work since Settings doesn't deny_unknown_fields
	let result = Settings::load_from(&path);
	assert!(result.is_ok());
}

#[test]
fn serialized_settings_is_valid_json() {
	let mut settings = Settings::default();
	settings.set_shell_path("bash", PathBuf::from("/usr/bin/bash"));
	let json = serde_json::to_string_pretty(&settings).unwrap();
	let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
	assert!(parsed.is_object());
	assert!(parsed["shell_paths"]["bash"].is_string());
}

// ── Path alias tests ───────────────────────────────────────────────

#[test]
fn set_and_get_path_alias() {
	let mut settings = Settings::default();
	settings.set_path_alias("root", PathBuf::from("/home/user/Runfile.json"));
	assert_eq!(
		settings.get_path_alias("root").unwrap(),
		&PathBuf::from("/home/user/Runfile.json")
	);
}

#[test]
fn get_path_alias_returns_none_for_unknown() {
	let settings = Settings::default();
	assert!(settings.get_path_alias("nope").is_none());
}

#[test]
fn set_path_alias_overwrites() {
	let mut settings = Settings::default();
	settings.set_path_alias("dev", PathBuf::from("/first"));
	settings.set_path_alias("dev", PathBuf::from("/second"));
	assert_eq!(settings.get_path_alias("dev").unwrap(), &PathBuf::from("/second"));
}

#[test]
fn remove_path_alias_returns_true_when_exists() {
	let mut settings = Settings::default();
	settings.set_path_alias("temp", PathBuf::from("/tmp/Runfile.json"));
	assert!(settings.remove_path_alias("temp"));
	assert!(settings.get_path_alias("temp").is_none());
}

#[test]
fn remove_path_alias_returns_false_when_missing() {
	let mut settings = Settings::default();
	assert!(!settings.remove_path_alias("nonexistent"));
}

#[test]
fn path_aliases_roundtrip() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("settings.json");

	let mut settings = Settings::default();
	settings.set_path_alias("root", PathBuf::from("/project/Runfile.json"));
	settings.set_path_alias("ci", PathBuf::from("/ci/Runfile-ci.json"));

	settings.save_to(&path).unwrap();
	let loaded = Settings::load_from(&path).unwrap();
	assert_eq!(
		loaded.get_path_alias("root").unwrap(),
		&PathBuf::from("/project/Runfile.json")
	);
	assert_eq!(
		loaded.get_path_alias("ci").unwrap(),
		&PathBuf::from("/ci/Runfile-ci.json")
	);
}

#[test]
fn path_aliases_not_serialized_when_empty() {
	let settings = Settings::default();
	let json = serde_json::to_string(&settings).unwrap();
	assert!(!json.contains("path_aliases"));
}

// ── Global files tests ─────────────────────────────────────────────

#[test]
fn add_global_file() {
	let mut settings = Settings::default();
	assert!(settings.add_global_file(PathBuf::from("/path/to/Runfile.json")));
	assert_eq!(settings.global_files.len(), 1);
}

#[test]
fn add_global_file_duplicate_returns_false() {
	let mut settings = Settings::default();
	settings.add_global_file(PathBuf::from("/path/to/Runfile.json"));
	assert!(!settings.add_global_file(PathBuf::from("/path/to/Runfile.json")));
	assert_eq!(settings.global_files.len(), 1);
}

#[test]
fn remove_global_file_returns_true_when_exists() {
	let mut settings = Settings::default();
	settings.add_global_file(PathBuf::from("/path/to/Runfile.json"));
	assert!(settings.remove_global_file(std::path::Path::new("/path/to/Runfile.json")));
	assert!(settings.global_files.is_empty());
}

#[test]
fn remove_global_file_returns_false_when_missing() {
	let mut settings = Settings::default();
	assert!(!settings.remove_global_file(std::path::Path::new("/nope")));
}

#[test]
fn global_files_roundtrip() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("settings.json");

	let mut settings = Settings::default();
	settings.add_global_file(PathBuf::from("/a/Runfile.json"));
	settings.add_global_file(PathBuf::from("/b/Runfile.json"));

	settings.save_to(&path).unwrap();
	let loaded = Settings::load_from(&path).unwrap();
	assert_eq!(loaded.global_files.len(), 2);
}

#[test]
fn global_files_not_serialized_when_empty() {
	let settings = Settings::default();
	let json = serde_json::to_string(&settings).unwrap();
	assert!(!json.contains("globalFiles"));
}

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

// ── Secret-key isolation tests ────────────────────────────────────

#[test]
fn settings_never_serializes_secret_key_state() {
	// Settings.json carries no secret-key state in any form. The keyring
	// is the sole source of truth.
	let settings = Settings::default();
	let json = serde_json::to_string(&settings).unwrap();
	assert!(!json.contains("secretKeys"), "unexpected secretKeys field: {json}");
	assert!(
		!json.contains("secureKeyFingerprints"),
		"unexpected secureKeyFingerprints field: {json}"
	);
}

#[test]
fn settings_silently_drops_legacy_secret_key_fields() {
	// Older binaries wrote `secureKeyFingerprints`. Settings doesn't use
	// deny_unknown_fields, so the field is silently ignored on load and
	// stripped from disk on the next save.
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("settings.json");
	let legacy_json = r#"{
		"secureKeyFingerprints": [
			"abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
		]
	}"#;
	std::fs::write(&path, legacy_json).unwrap();

	let settings = Settings::load_from(&path).unwrap();
	settings.save_to(&path).unwrap();

	let reread = std::fs::read_to_string(&path).unwrap();
	assert!(
		!reread.contains("secureKeyFingerprints"),
		"saved settings.json must not carry legacy secret-key fields: {reread}"
	);
}
