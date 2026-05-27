use super::*;

#[test]
fn load_env_files_missing_file_ignored() {
	let dir = TempDir::new().unwrap();
	let env = HashMap::new();
	let result = load_env_files(&[".env.nonexistent".to_string()], dir.path(), &no_substitute, &env);
	assert!(result.is_ok());
	assert!(result.unwrap().is_empty());
}

#[test]
fn load_env_files_reads_existing_file() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "MY_KEY=my_value\n").unwrap();
	let env = HashMap::new();
	let result = load_env_files(&[".env".to_string()], dir.path(), &no_substitute, &env).unwrap();
	assert_eq!(result.get("MY_KEY").unwrap(), "my_value");
}

#[test]
fn load_env_files_later_overrides_earlier() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "KEY=first\n").unwrap();
	std::fs::write(dir.path().join(".env.local"), "KEY=second\n").unwrap();
	let env = HashMap::new();
	let result = load_env_files(
		&[".env".to_string(), ".env.local".to_string()],
		dir.path(),
		&no_substitute,
		&env,
	)
	.unwrap();
	assert_eq!(result.get("KEY").unwrap(), "second");
}

#[test]
fn load_env_files_parse_error() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "INVALID_NO_EQUALS\n").unwrap();
	let env = HashMap::new();
	let result = load_env_files(&[".env".to_string()], dir.path(), &no_substitute, &env);
	assert!(result.is_err());
}

#[test]
fn load_env_files_absolute_path() {
	let dir = TempDir::new().unwrap();
	let env_path = dir.path().join("abs.env");
	std::fs::write(&env_path, "ABS_KEY=abs_value\n").unwrap();
	let env = HashMap::new();
	let result = load_env_files(
		&[env_path.to_str().unwrap().to_string()],
		dir.path(),
		&no_substitute,
		&env,
	)
	.unwrap();
	assert_eq!(result.get("ABS_KEY").unwrap(), "abs_value");
}

#[test]
fn load_env_files_multiple_missing_files_all_skipped() {
	let dir = TempDir::new().unwrap();
	let env = HashMap::new();
	let result = load_env_files(
		&[
			".env.missing1".to_string(),
			".env.missing2".to_string(),
			".env.missing3".to_string(),
		],
		dir.path(),
		&no_substitute,
		&env,
	);
	assert!(result.is_ok());
	assert!(result.unwrap().is_empty());
}

#[test]
fn load_env_files_with_substitution() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env.production"), "DB=prod-db\n").unwrap();
	let env = HashMap::new();
	// Simulate substitution that replaces {{ MYVAR }} with "production"
	let substitute = |input: &str, _env: &HashMap<String, String>| -> Result<String, String> {
		Ok(input.replace("{{ MYVAR }}", "production"))
	};
	let result = load_env_files(&[".env.{{ MYVAR }}".to_string()], dir.path(), &substitute, &env).unwrap();
	assert_eq!(result.get("DB").unwrap(), "prod-db");
}

// ══════════════════════════════════════════════════════════════════════
// build_env tests
// ══════════════════════════════════════════════════════════════════════
