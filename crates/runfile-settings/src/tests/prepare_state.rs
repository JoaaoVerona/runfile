use super::*;

#[test]
fn default_state_is_empty() {
	assert!(PrepareState::default().prepared.is_empty());
}

#[test]
fn load_nonexistent_returns_default() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("nonexistent.json");
	assert_eq!(PrepareState::load_from(&path).unwrap(), PrepareState::default());
}

#[test]
fn record_and_read_back() {
	let dir = TempDir::new().unwrap();
	let runfile = dir.path().join("Runfile.json");
	std::fs::write(&runfile, "{}").unwrap();

	let mut state = PrepareState::default();
	state.record(&runfile, "setup", "hash-abc");
	state.record(&runfile, "setup-tests --fast", "hash-def");

	assert_eq!(state.recorded_hash(&runfile, "setup"), Some("hash-abc"));
	assert_eq!(state.recorded_hash(&runfile, "setup-tests --fast"), Some("hash-def"));
	assert_eq!(state.recorded_hash(&runfile, "unknown"), None);
}

#[test]
fn save_and_load_roundtrip() {
	let dir = TempDir::new().unwrap();
	let runfile = dir.path().join("Runfile.json");
	std::fs::write(&runfile, "{}").unwrap();
	let state_path = dir.path().join("state.json");

	let mut state = PrepareState::default();
	state.record(&runfile, "setup", "deadbeef");
	state.save_to(&state_path).unwrap();

	let loaded = PrepareState::load_from(&state_path).unwrap();
	assert_eq!(state, loaded);
	assert_eq!(loaded.recorded_hash(&runfile, "setup"), Some("deadbeef"));
}

#[test]
fn record_overwrites_existing_hash() {
	let dir = TempDir::new().unwrap();
	let runfile = dir.path().join("Runfile.json");
	std::fs::write(&runfile, "{}").unwrap();

	let mut state = PrepareState::default();
	state.record(&runfile, "setup", "old");
	state.record(&runfile, "setup", "new");
	assert_eq!(state.recorded_hash(&runfile, "setup"), Some("new"));
}

#[test]
fn path_key_canonicalizes_relative_and_absolute() {
	let dir = TempDir::new().unwrap();
	let runfile = dir.path().join("Runfile.json");
	std::fs::write(&runfile, "{}").unwrap();

	// A record keyed by the absolute path is found again via a
	// non-canonical spelling of the same file (trailing "./").
	let mut state = PrepareState::default();
	state.record(&runfile, "setup", "h");
	let dotted = dir.path().join(".").join("Runfile.json");
	assert_eq!(state.recorded_hash(&dotted, "setup"), Some("h"));
}
