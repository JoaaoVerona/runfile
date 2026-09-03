use super::*;

#[test]
fn a_fresh_state_is_empty() {
	assert!(PrepareState::default().prepared.is_empty());
}

#[test]
fn a_missing_file_loads_as_empty_rather_than_failing() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("state.json");
	assert_eq!(PrepareState::load_from(&path).unwrap(), PrepareState::default());
}

#[test]
fn a_recorded_gate_round_trips_through_the_file() {
	let dir = TempDir::new().unwrap();
	let gate = dir.path().join("runfiles/setup.run");
	std::fs::create_dir_all(gate.parent().unwrap()).unwrap();
	std::fs::write(&gate, "$ true\n").unwrap();
	let path = dir.path().join("state.json");

	let mut state = PrepareState::default();
	state.record(&gate, "fingerprint-abc");
	state.save_to(&path).unwrap();

	let loaded = PrepareState::load_from(&path).unwrap();
	assert_eq!(loaded.recorded_hash(&gate), Some("fingerprint-abc"));
}

#[test]
fn recording_again_replaces_the_previous_fingerprint() {
	// One gate per directory, so a second record is an update, not an addition.
	let dir = TempDir::new().unwrap();
	let gate = dir.path().join("setup.run");
	std::fs::write(&gate, "$ true\n").unwrap();
	let mut state = PrepareState::default();
	state.record(&gate, "one");
	state.record(&gate, "two");
	assert_eq!(state.prepared.len(), 1);
	assert_eq!(state.recorded_hash(&gate), Some("two"));
}

#[test]
fn an_unrecorded_gate_has_no_fingerprint() {
	let dir = TempDir::new().unwrap();
	assert_eq!(
		PrepareState::default().recorded_hash(&dir.path().join("setup.run")),
		None
	);
}
