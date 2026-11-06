use super::*;

/// Read the file `name` in `dir`, with nothing in the environment so far.
fn read(dir: &TempDir, name: &str) -> Result<Option<HashMap<String, String>>, crate::EnvError> {
	read_env_file(&dir.path().join(name), &HashMap::new(), None)
}

#[test]
fn a_file_that_is_not_there_is_none_rather_than_an_error() {
	// A project whose `.env` is git-ignored still has to run.
	let dir = TempDir::new().unwrap();
	assert!(read(&dir, ".env.nonexistent").unwrap().is_none());
}

#[test]
fn a_file_that_is_there_is_its_values() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "MY_KEY=my_value\nOTHER=\"quoted value\"\n").unwrap();
	let vars = read(&dir, ".env").unwrap().unwrap();
	assert_eq!(vars["MY_KEY"], "my_value");
	assert_eq!(vars["OTHER"], "quoted value");
}

#[test]
fn a_name_written_twice_in_one_file_takes_the_later_value() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "KEY=first\nKEY=second\n").unwrap();
	assert_eq!(read(&dir, ".env").unwrap().unwrap()["KEY"], "second");
}

#[test]
fn a_parse_error_names_the_file_and_the_line_and_not_its_text() {
	// The line may be a secret (audit SA-026).
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "OK=1\nhunter2_NO_EQUALS\n").unwrap();
	let err = read(&dir, ".env").unwrap_err().to_string();
	assert!(err.contains(".env") && err.contains("line 2"), "{err}");
	assert!(
		!err.contains("hunter2"),
		"the line's text leaked into the message: {err}"
	);
}
