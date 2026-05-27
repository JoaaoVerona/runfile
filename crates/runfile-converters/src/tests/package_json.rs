use super::*;

// ── Package JSON tests ─────────────────────────────────────────────

#[test]
fn convert_simple_npm_scripts() {
	let json: serde_json::Value =
		serde_json::from_str(r#"{"build": "tsc", "test": "jest", "lint": "eslint ."}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	assert_eq!(result.targets.len(), 3);
	assert!(result.targets.contains_key("build"));
	assert!(result.targets.contains_key("test"));
	assert!(result.targets.contains_key("lint"));
	assert_eq!(result.targets["build"].commands, vec!["tsc"]);
}

#[test]
fn convert_npm_with_env_extraction() {
	let json: serde_json::Value = serde_json::from_str(r#"{"dev": "NODE_ENV=development node server.js"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let spec = &result.targets["dev"];
	assert_eq!(spec.commands, vec!["node server.js"]);
	let env = spec.env.as_ref().unwrap();
	assert!(env.contains_key("NODE_ENV"));
}

#[test]
fn convert_npm_skips_prepare() {
	let json: serde_json::Value = serde_json::from_str(r#"{"prepare": "husky install", "build": "tsc"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	assert_eq!(result.targets.len(), 1);
	assert!(!result.targets.contains_key("prepare"));
}

#[test]
fn convert_npm_skips_on_collision() {
	let mut existing = HashSet::new();
	existing.insert("build".to_string());

	let json: serde_json::Value = serde_json::from_str(r#"{"build": "tsc", "test": "jest"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &existing);
	assert!(!result.targets.contains_key("build"));
	assert!(result.targets.contains_key("test"));
	assert_eq!(result.skipped, vec!["build"]);
}

#[test]
fn convert_npm_chained_commands() {
	let json: serde_json::Value =
		serde_json::from_str(r#"{"ci": "npm run lint && npm run test && npm run build"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let spec = &result.targets["ci"];
	assert_eq!(spec.commands.len(), 3);
}
