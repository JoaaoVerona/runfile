use super::*;

// ── Security tests: sensitive fields NOT exposed ──────────────────

#[test]
fn tools_do_not_expose_env_files() {
	let mut spec = simple_spec(vec!["cargo build"], Some("Build"));
	spec.env_files = Some(vec![".env".into(), ".env.secret".into()]);
	let runfile = make_runfile(vec![("build", spec)]);
	let json = inspect_json(&runfile);
	assert!(!json.contains(".env"), "env_files must not appear in tool output");
	assert!(
		!json.contains("envFiles"),
		"envFiles key must not appear in tool output"
	);
}

#[test]
fn tools_do_not_expose_env_values() {
	let mut spec = simple_spec(vec!["cargo build"], Some("Build"));
	let mut env = HashMap::new();
	env.insert("SECRET_KEY".to_string(), EnvValue::String("s3cr3t".into()));
	spec.env = Some(env);
	let runfile = make_runfile(vec![("build", spec)]);
	let json = inspect_json(&runfile);
	assert!(!json.contains("s3cr3t"), "env values must not appear in tool output");
	assert!(!json.contains("SECRET_KEY"), "env keys must not appear in tool output");
}

#[test]
fn tools_do_not_expose_commands() {
	let runfile = make_runfile(vec![(
		"build",
		simple_spec(vec!["cargo build --secret-flag"], Some("Build")),
	)]);
	let json = inspect_json(&runfile);
	assert!(
		!json.contains("--secret-flag"),
		"command contents must not appear in tool output"
	);
}
