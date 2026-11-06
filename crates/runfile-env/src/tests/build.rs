use super::*;

// ══════════════════════════════════════════════════════════════════════
// check_env_case_duplicates tests
// ══════════════════════════════════════════════════════════════════════

#[test]
fn check_env_case_duplicates_ok() {
	let mut env = HashMap::new();
	env.insert("NODE_ENV".to_string(), "production".to_string());
	env.insert("OTHER_VAR".to_string(), "value".to_string());
	assert!(check_env_case_duplicates(&env).is_ok());
}

#[test]
fn check_env_case_duplicates_detects_conflict() {
	let mut env = HashMap::new();
	env.insert("NODE_ENV".to_string(), "production".to_string());
	env.insert("node_env".to_string(), "development".to_string());
	let result = check_env_case_duplicates(&env);
	assert!(result.is_err());
}

#[test]
fn check_env_case_duplicates_same_case_ok() {
	let mut env = HashMap::new();
	env.insert("KEY".to_string(), "value".to_string());
	assert!(check_env_case_duplicates(&env).is_ok());
}

#[test]
fn check_env_case_duplicates_empty_env_ok() {
	let env: HashMap<String, String> = HashMap::new();
	assert!(check_env_case_duplicates(&env).is_ok());
}

// ══════════════════════════════════════════════════════════════════════
// collect_runfile_env tests
// ══════════════════════════════════════════════════════════════════════

#[test]
fn collect_runfile_env_empty() {
	let result = collect_runfile_env(None);
	assert!(result.is_empty());
}

#[test]
fn collect_runfile_env_with_values() {
	let mut env = HashMap::new();
	env.insert("A".to_string(), "1".to_string());
	let result = collect_runfile_env(Some(&env));
	assert_eq!(result, vec![("A".to_string(), "1".to_string())]);
}

#[test]
fn collect_runfile_env_sorted() {
	let mut env = HashMap::new();
	env.insert("Z".to_string(), "last".to_string());
	env.insert("A".to_string(), "first".to_string());
	let result = collect_runfile_env(Some(&env));
	assert_eq!(result[0].0, "A");
	assert_eq!(result[1].0, "Z");
}
