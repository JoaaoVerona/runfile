use super::*;

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
