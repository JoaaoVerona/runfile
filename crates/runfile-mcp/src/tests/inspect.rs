use super::*;

// ── Inspect JSON format tests ─────────────────────────────────────

#[test]
fn inspect_json_is_valid_json_array() {
	let runfile = make_runfile(vec![
		("build", simple_spec(vec!["cargo build"], Some("Build"))),
		("test", simple_spec(vec!["cargo test"], Some("Test"))),
	]);
	let json = inspect_json(&runfile);
	let parsed: serde_json::Value = serde_json::from_str(&json).expect("should be valid JSON");
	assert!(parsed.is_array());
	assert_eq!(parsed.as_array().unwrap().len(), 2);
}

#[test]
fn inspect_json_has_required_fields() {
	let runfile = make_runfile(vec![("build", simple_spec(vec!["cargo build"], Some("Build")))]);
	let json = inspect_json(&runfile);
	let parsed: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
	let tool = &parsed[0];
	assert!(tool.get("name").is_some());
	assert!(tool.get("description").is_some());
	assert!(tool.get("inputSchema").is_some());
}
