use super::*;

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
