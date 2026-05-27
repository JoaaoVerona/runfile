use super::*;

// ── New Package JSON tests: lifecycle scripts, hooks, npm run, tools ──

#[test]
fn convert_npm_skips_all_lifecycle_scripts() {
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"preinstall": "echo pre",
			"install": "echo install",
			"postinstall": "node scripts/setup.js",
			"prepublishOnly": "npm run build",
			"prepare": "husky install",
			"build": "tsc"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	assert_eq!(result.targets.len(), 1);
	assert!(result.targets.contains_key("build"));
	assert!(!result.targets.contains_key("install"));
	assert!(!result.targets.contains_key("preinstall"));
	assert!(!result.targets.contains_key("postinstall"));
	assert!(!result.targets.contains_key("prepublishOnly"));
	assert!(!result.targets.contains_key("prepare"));
}

#[test]
fn convert_npm_pre_hook_prepends_to_commands() {
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"pretest": "eslint .",
			"test": "jest"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	assert_eq!(result.targets.len(), 1, "pretest should not be a standalone target");
	assert!(!result.targets.contains_key("pretest"));
	let test_spec = &result.targets["test"];
	// First command should be the pre hook.
	assert_eq!(test_spec.commands[0], "eslint .");
}

#[test]
fn convert_npm_post_hook_appends_to_commands() {
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"build": "tsc",
			"postbuild": "cp -r dist/ output/"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	assert_eq!(result.targets.len(), 1, "postbuild should not be a standalone target");
	assert!(!result.targets.contains_key("postbuild"));
	let build_spec = &result.targets["build"];
	// Last command should be the post hook.
	assert_eq!(
		build_spec.commands.last().unwrap(),
		&runfile_parser::CommandStep::Shell("cp -r dist/ output/".into())
	);
}

#[test]
fn convert_npm_pre_and_post_hooks_together() {
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"prebuild": "rimraf dist/",
			"build": "tsc",
			"postbuild": "node scripts/copy-assets.js"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	assert_eq!(result.targets.len(), 1);
	let spec = &result.targets["build"];
	// Pre/post hooks now flank the main command in `commands`.
	assert_eq!(
		spec.commands.first().unwrap(),
		&runfile_parser::CommandStep::Shell("rimraf dist/".into())
	);
	assert_eq!(
		spec.commands.last().unwrap(),
		&runfile_parser::CommandStep::Shell("node scripts/copy-assets.js".into())
	);
}

#[test]
fn convert_npm_pre_hook_without_base_stays_as_target() {
	// "preflight" is not pre + "flight" because "flight" doesn't exist
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"preflight": "eslint . && tsc --noEmit"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	assert_eq!(result.targets.len(), 1);
	assert!(result.targets.contains_key("preflight"));
}

#[test]
fn convert_npm_run_references_replaced() {
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"lint": "eslint .",
			"test": "jest",
			"ci": "npm run lint && npm run test"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let ci = &result.targets["ci"];
	assert_eq!(ci.commands, vec!["run lint", "run test"]);
}

#[test]
fn convert_npm_run_unknown_script_kept() {
	// "npm run unknown" where "unknown" is not in the scripts map stays unchanged
	let json: serde_json::Value = serde_json::from_str(r#"{"ci": "npm run unknown-thing"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let ci = &result.targets["ci"];
	assert_eq!(ci.commands, vec!["npm run unknown-thing"]);
}

#[test]
fn convert_npm_yarn_run_replaced() {
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"build": "tsc",
			"ci": "yarn run build"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	assert_eq!(result.targets["ci"].commands, vec!["run build"]);
}

#[test]
fn convert_npm_yarn_shorthand_replaced() {
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"build": "tsc",
			"ci": "yarn build"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	assert_eq!(result.targets["ci"].commands, vec!["run build"]);
}

#[test]
fn convert_npm_npx_prefix_stripped() {
	let json: serde_json::Value = serde_json::from_str(r#"{"test": "npx jest --coverage"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	assert_eq!(result.targets["test"].commands, vec!["jest --coverage"]);
}

#[test]
fn convert_npm_node_modules_bin_stripped() {
	let json: serde_json::Value = serde_json::from_str(r#"{"test": "./node_modules/.bin/jest --verbose"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	assert_eq!(result.targets["test"].commands, vec!["jest --verbose"]);
}

#[test]
fn convert_npm_run_s_sequential() {
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"lint": "eslint .",
			"test": "jest",
			"build": "tsc",
			"ci": "run-s lint test build"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let ci = &result.targets["ci"];
	assert_eq!(ci.commands, vec!["run lint", "run test", "run build"]);
	assert!(ci.parallel.is_none());
}

#[test]
fn convert_npm_run_p_parallel() {
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"watch:css": "postcss --watch",
			"watch:js": "tsc --watch",
			"dev": "run-p watch:css watch:js"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let dev = &result.targets["dev"];
	assert_eq!(dev.commands, vec!["run watch:css", "run watch:js"]);
	assert_eq!(dev.parallel, Some(true));
}

#[test]
fn convert_npm_run_all_parallel_flag() {
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"lint": "eslint .",
			"test": "jest",
			"check": "npm-run-all --parallel lint test"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let check = &result.targets["check"];
	assert_eq!(check.commands, vec!["run lint", "run test"]);
	assert_eq!(check.parallel, Some(true));
}

#[test]
fn convert_npm_concurrently() {
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"watch:css": "postcss --watch",
			"watch:js": "tsc --watch",
			"dev": "concurrently \"npm run watch:css\" \"npm run watch:js\""
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let dev = &result.targets["dev"];
	assert_eq!(dev.commands, vec!["run watch:css", "run watch:js"]);
	assert_eq!(dev.parallel, Some(true));
}

#[test]
fn convert_npm_concurrently_with_flags() {
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"serve": "node server.js",
			"watch": "tsc --watch",
			"dev": "concurrently --kill-others \"npm run serve\" \"npm run watch\""
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let dev = &result.targets["dev"];
	assert_eq!(dev.commands, vec!["run serve", "run watch"]);
	assert_eq!(dev.parallel, Some(true));
}

#[test]
fn convert_npm_pre_hook_cleans_npm_run() {
	// Pre-hook commands should also get npm run → run replacement
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"lint": "eslint .",
			"prebuild": "npm run lint",
			"build": "tsc"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let build = &result.targets["build"];
	// Pre-hook now lives at the start of `commands`, not in a separate before list.
	assert_eq!(
		build.commands.first().unwrap(),
		&runfile_parser::CommandStep::Shell("run lint".into())
	);
}
