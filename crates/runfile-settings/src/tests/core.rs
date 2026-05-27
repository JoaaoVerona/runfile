use super::*;

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
