use super::*;

// ── Tool definition tests ─────────────────────────────────────────

#[test]
fn build_tools_basic_target() {
	let runfile = make_runfile(vec![(
		"build",
		simple_spec(vec!["cargo build"], Some("Build the project")),
	)]);
	let tools = build_tool_defs(&runfile);
	assert_eq!(tools.len(), 1);
	assert_eq!(tools[0].name, "build");
	assert_eq!(tools[0].description, "Build the project");
}

#[test]
fn build_tools_no_description_gets_default() {
	let runfile = make_runfile(vec![("test", simple_spec(vec!["cargo test"], None))]);
	let tools = build_tool_defs(&runfile);
	assert_eq!(tools[0].description, "Run the \"test\" target");
}

#[test]
fn build_tools_sorted_alphabetically() {
	let runfile = make_runfile(vec![
		("test", simple_spec(vec!["cargo test"], None)),
		("build", simple_spec(vec!["cargo build"], None)),
		("lint", simple_spec(vec!["cargo clippy"], None)),
	]);
	let tools = build_tool_defs(&runfile);
	let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
	assert_eq!(names, vec!["build", "lint", "test"]);
}

#[test]
fn build_tools_target_with_positional_args_has_args_array() {
	let runfile = make_runfile(vec![("build", simple_spec(vec!["cargo build {{ ARGS }}"], None))]);
	let tools = build_tool_defs(&runfile);
	let schema = &tools[0].input_schema;
	let props = schema.get("properties").unwrap();
	let args_prop = props.get("args").unwrap();
	assert_eq!(args_prop.get("type").unwrap(), "array");
}

#[test]
fn build_tools_target_with_named_args_has_explicit_properties() {
	let runfile = make_runfile(vec![(
		"deploy",
		simple_spec(
			vec!["deploy --env={{ ARG.env }} --region={{ ARG.region ? us-east-1 }}"],
			None,
		),
	)]);
	let tools = build_tool_defs(&runfile);
	let schema = &tools[0].input_schema;
	let props = schema.get("properties").unwrap();
	// Named args should be explicit string properties
	assert_eq!(props.get("env").unwrap().get("type").unwrap(), "string");
	assert_eq!(props.get("region").unwrap().get("type").unwrap(), "string");
	// No generic "args" array since {{ ARGS }} is not used
	assert!(props.get("args").is_none());
	// "env" is required (no default), "region" is optional (has default)
	let required = schema.get("required").unwrap().as_array().unwrap();
	assert!(required.contains(&serde_json::json!("env")));
	assert!(!required.contains(&serde_json::json!("region")));
}

#[test]
fn build_tools_target_with_flags_has_boolean_properties() {
	let runfile = make_runfile(vec![(
		"build",
		simple_spec(vec!["cargo build {{ FLAG.release ? --release : }}"], None),
	)]);
	let tools = build_tool_defs(&runfile);
	let schema = &tools[0].input_schema;
	let props = schema.get("properties").unwrap();
	assert_eq!(props.get("release").unwrap().get("type").unwrap(), "boolean");
}

#[test]
fn build_tools_target_with_positional_and_named_args() {
	let runfile = make_runfile(vec![(
		"run",
		simple_spec(vec!["app --env={{ ARG.env }} {{ ARGS }}"], None),
	)]);
	let tools = build_tool_defs(&runfile);
	let schema = &tools[0].input_schema;
	let props = schema.get("properties").unwrap();
	// Both explicit named property and positional args array
	assert_eq!(props.get("env").unwrap().get("type").unwrap(), "string");
	assert_eq!(props.get("args").unwrap().get("type").unwrap(), "array");
}

#[test]
fn build_tools_target_without_args_has_empty_properties() {
	let runfile = make_runfile(vec![("build", simple_spec(vec!["cargo build"], None))]);
	let tools = build_tool_defs(&runfile);
	let schema = &tools[0].input_schema;
	let props = schema.get("properties").unwrap().as_object().unwrap();
	assert!(props.is_empty());
}

#[test]
fn build_tools_args_from_env_values_are_included() {
	let mut spec = simple_spec(vec!["echo $MY_VAR"], None);
	let mut env = HashMap::new();
	env.insert("MY_VAR".to_string(), EnvValue::String("{{ ARG.config }}".into()));
	spec.env = Some(env);
	let runfile = make_runfile(vec![("test", spec)]);
	let tools = build_tool_defs(&runfile);
	let schema = &tools[0].input_schema;
	let props = schema.get("properties").unwrap();
	assert_eq!(props.get("config").unwrap().get("type").unwrap(), "string");
}
