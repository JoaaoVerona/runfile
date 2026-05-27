use super::*;

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
