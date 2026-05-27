use super::*;

// ── dotenvx tests ───────────────────────────────────────────────────

#[test]
fn convert_npm_dotenvx_basic() {
	let json: serde_json::Value = serde_json::from_str(r#"{"dev": "dotenvx run -- node server.js"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let spec = &result.targets["dev"];
	assert_eq!(spec.commands, vec!["node server.js"]);
	assert!(spec.env_files.is_none(), "no -f flag means no explicit envFiles");
}

#[test]
fn convert_npm_dotenvx_with_env_file() {
	let json: serde_json::Value =
		serde_json::from_str(r#"{"dev": "dotenvx run -f .env.local -- node server.js"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let spec = &result.targets["dev"];
	assert_eq!(spec.commands, vec!["node server.js"]);
	let env_files = spec.env_files.as_ref().unwrap();
	assert_eq!(env_files, &[".env.local"]);
}

#[test]
fn convert_npm_dotenvx_multiple_env_files() {
	let json: serde_json::Value =
		serde_json::from_str(r#"{"dev": "dotenvx run -f .env -f .env.local -- node server.js"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let spec = &result.targets["dev"];
	assert_eq!(spec.commands, vec!["node server.js"]);
	let env_files = spec.env_files.as_ref().unwrap();
	assert_eq!(env_files, &[".env", ".env.local"]);
}

#[test]
fn convert_npm_dotenvx_no_separator() {
	// dotenvx run without -- separator
	let json: serde_json::Value = serde_json::from_str(r#"{"dev": "dotenvx run node server.js"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let spec = &result.targets["dev"];
	assert_eq!(spec.commands, vec!["node server.js"]);
}

#[test]
fn convert_npm_dotenvx_with_npm_run() {
	// dotenvx wrapping an npm run command that references a known script
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"serve": "node server.js",
			"dev": "dotenvx run -f .env.dev -- npm run serve"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let dev = &result.targets["dev"];
	assert_eq!(dev.commands, vec!["run serve"]);
	assert_eq!(dev.env_files.as_ref().unwrap(), &[".env.dev"]);
}

#[test]
fn convert_npm_dotenvx_in_chain() {
	// dotenvx in a && chain — env files are collected
	let json: serde_json::Value =
		serde_json::from_str(r#"{"ci": "dotenvx run -f .env.test -- jest && dotenvx run -f .env.test -- eslint ."}"#)
			.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let spec = &result.targets["ci"];
	assert_eq!(spec.commands, vec!["jest", "eslint ."]);
	// Both parts referenced the same file — should be deduplicated
	assert_eq!(spec.env_files.as_ref().unwrap(), &[".env.test"]);
}

#[test]
fn convert_npm_dotenvx_env_file_long_form() {
	let json: serde_json::Value =
		serde_json::from_str(r#"{"dev": "dotenvx run --env-file=.env.prod -- node app.js"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let spec = &result.targets["dev"];
	assert_eq!(spec.commands, vec!["node app.js"]);
	assert_eq!(spec.env_files.as_ref().unwrap(), &[".env.prod"]);
}
