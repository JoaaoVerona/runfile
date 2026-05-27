use super::*;

// ── Single-quoted strings interpolate nested {{ }} ──

#[test]
fn single_quoted_with_nested_substitution() {
	// `{{ define(cmd, 'docker -f {{ VAR.compose }} pull') }}` —
	// the inner `{{ VAR.compose }}` resolves before the value is
	// stored in VAR.cmd.
	let args = RunArgs::default();
	args.vars
		.lock()
		.unwrap()
		.insert("compose".to_string(), "services/web.yml".to_string());
	let _ = args
		.substitute("{{ define(cmd, 'docker -f {{ VAR.compose }} pull') }}", &HashMap::new())
		.unwrap();
	let v = args.substitute("{{ VAR.cmd }}", &HashMap::new()).unwrap();
	assert_eq!(v, "docker -f services/web.yml pull");
}

#[test]
fn single_quoted_with_multiple_nested_substitutions() {
	let args = RunArgs::parse(&["--first=alice".into(), "--last=brown".into()]);
	let result = args
		.substitute("echo {{ 'Hello {{ ARG.first }} {{ ARG.last }}!' }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "echo Hello alice brown!");
}

#[test]
fn single_quoted_empty_interpolates_to_empty() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute("[{{ ARG.x ? '' }}]", &HashMap::new()).unwrap();
	assert_eq!(result, "[]");
}

#[test]
fn single_quoted_with_arg_inside_function_arg() {
	// User example: `{{ define(images, 'raw image, image2') }}` — the
	// comma inside the single quotes doesn't split the function args.
	let args = RunArgs::parse(&[]);
	let _ = args
		.substitute("{{ define(images, 'raw image, image2') }}", &HashMap::new())
		.unwrap();
	let v = args.substitute("{{ VAR.images }}", &HashMap::new()).unwrap();
	assert_eq!(v, "raw image, image2");
}

// ── Double-quoted strings stay literal ──

#[test]
fn double_quoted_keeps_quote_chars() {
	// User example: `{{ define(images, "test") }}` stores the 6-char
	// string `"test"` (with the double-quote characters intact).
	let args = RunArgs::parse(&[]);
	let _ = args
		.substitute("{{ define(images, \"test\") }}", &HashMap::new())
		.unwrap();
	let v = args.substitute("{{ VAR.images }}", &HashMap::new()).unwrap();
	assert_eq!(v, "\"test\"");
}

#[test]
fn double_quoted_does_not_interpolate() {
	// Inside `"..."`, `{{ ... }}` stays literal — no recursion.
	let args = RunArgs::parse(&[]);
	args.vars.lock().unwrap().insert("x".to_string(), "value".to_string());
	let result = args
		.substitute("{{ ARG.y ? \"{{ VAR.x }}\" }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "\"{{ VAR.x }}\"");
}

// ── Bareword literals are rejected ──

#[test]
fn bareword_chain_default_errors() {
	let args = RunArgs::parse(&[]);
	let err = args
		.substitute("{{ ARG.env ? development }}", &HashMap::new())
		.unwrap_err();
	assert!(matches!(err, SubstitutionError::BarewordLiteralNotAllowed(_)));
}

#[test]
fn bareword_function_arg_errors() {
	let args = RunArgs::parse(&[]);
	let err = args.substitute("{{ to_upper(hello) }}", &HashMap::new()).unwrap_err();
	assert!(matches!(err, SubstitutionError::BarewordLiteralNotAllowed(_)));
}

#[test]
fn bareword_flags_branch_errors() {
	let args = RunArgs::parse(&["--debug".into()]);
	let err = args
		.substitute("{{ FLAG.debug ? on : off }}", &HashMap::new())
		.unwrap_err();
	assert!(matches!(err, SubstitutionError::BarewordLiteralNotAllowed(_)));
}

#[test]
fn quoted_chain_default_works() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ ARG.env ? 'development' }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "development");
}

#[test]
fn quoted_flags_ternary_works() {
	let args = RunArgs::parse(&["--ci".into()]);
	let result = args
		.substitute("{{ FLAG.ci ? '--ci' : '--stdin' }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "--ci");
}

#[test]
fn quoted_flags_ternary_false() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ FLAG.ci ? '--ci' : '--stdin' }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "--stdin");
}

#[test]
fn empty_trailing_question_still_works() {
	// `{{ ARG.x ? }}` stays valid as the empty-default form.
	let args = RunArgs::parse(&[]);
	let result = args.substitute("[{{ ARG.x ? }}]", &HashMap::new()).unwrap();
	assert_eq!(result, "[]");
}

// ── Nested `{{ }}` inside single-quoted literals ──

#[test]
fn single_quoted_literal_with_nested_subst_using_quoted_arg() {
	// User-reported pattern: a `'...'` literal whose nested `{{ ... }}`
	// uses its own single-quoted arg (`' '` separator). The inner `'`
	// chars must NOT terminate the outer literal.
	let args = RunArgs::parse(&[]);
	args.vars
		.lock()
		.unwrap()
		.insert("part".to_string(), "android-30 google_apis_playstore".to_string());
	args.vars
		.lock()
		.unwrap()
		.insert("arch".to_string(), "x86_64".to_string());
	let result = args
		.substitute(
			"{{ define(image, 'system-images;{{ nth(VAR.part, ' ', '0') }};{{ nth(VAR.part, ' ', '1') }};{{ VAR.arch }}') }}",
			&HashMap::new(),
		)
		.unwrap();
	assert_eq!(result, "");
	let v = args.substitute("{{ VAR.image }}", &HashMap::new()).unwrap();
	assert_eq!(v, "system-images;android-30;google_apis_playstore;x86_64");
}

#[test]
fn single_quoted_literal_interpolates_nested_subst() {
	// Plain interpolation case: nested subst inside `'...'` resolves.
	let args = RunArgs::parse(&[]);
	args.vars.lock().unwrap().insert("v".to_string(), "1.2.3".to_string());
	let result = args
		.substitute("{{ ARG.tag ? 'v{{ VAR.v }}-stable' }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "v1.2.3-stable");
}

#[test]
fn function_args_with_nested_subst_using_quotes() {
	// `concat(...)` where each arg is itself a nested subst (function
	// call) that uses its own quoted args. The outer split must treat
	// nested `{{ ... }}` as opaque so the inner `,` and `'` don't
	// disturb arg boundaries.
	let args = RunArgs::parse(&[]);
	args.vars.lock().unwrap().insert("p".to_string(), "a:b:c".to_string());
	let result = args
		.substitute(
			"{{ concat('[', nth(VAR.p, ':', '0'), '|', nth(VAR.p, ':', '2'), ']') }}",
			&HashMap::new(),
		)
		.unwrap();
	assert_eq!(result, "[a|c]");
}

#[test]
fn dsl_condition_with_nested_subst_using_quotes() {
	// DSL detection (`==`) kicks in correctly when one side has a
	// nested subst whose body uses its own quoted args.
	let args = RunArgs::parse(&[]);
	args.vars.lock().unwrap().insert("p".to_string(), "a:b:c".to_string());
	let result = args
		.substitute("{{ nth(VAR.p, ':', '1') == 'b' }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "true");
}

// ── DSL inside `{{ }}` substitutions ──

#[test]
fn dsl_equality_returns_true_or_false() {
	let args = RunArgs::parse(&["--env=production".into()]);
	let result = args
		.substitute("{{ ARG.env == 'production' }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "true");

	let args = RunArgs::parse(&["--env=staging".into()]);
	let result = args
		.substitute("{{ ARG.env == 'production' }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "false");
}

#[test]
fn dsl_inequality_works() {
	let args = RunArgs::parse(&["--env=staging".into()]);
	let result = args
		.substitute("{{ ARG.env != 'production' }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "true");
}

#[test]
fn dsl_logical_and_works() {
	let args = RunArgs::parse(&["--env=prod".into(), "--ci=true".into()]);
	let result = args
		.substitute("{{ ARG.env == 'prod' && ARG.ci == 'true' }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "true");
}

#[test]
fn dsl_logical_or_works() {
	let args = RunArgs::parse(&["--env=prod".into()]);
	let result = args
		.substitute("{{ ARG.env == 'prod' || ARG.env == 'staging' }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "true");
}

#[test]
fn dsl_chained_negations_with_and() {
	// User's example pattern: `ARG.env != 'development' && ARG.env != 'production'`.
	let args = RunArgs::parse(&["--env=staging".into()]);
	let result = args
		.substitute(
			"{{ ARG.env != 'development' && ARG.env != 'production' }}",
			&HashMap::new(),
		)
		.unwrap();
	assert_eq!(result, "true");

	let args = RunArgs::parse(&["--env=development".into()]);
	let result = args
		.substitute(
			"{{ ARG.env != 'development' && ARG.env != 'production' }}",
			&HashMap::new(),
		)
		.unwrap();
	assert_eq!(result, "false");
}

#[test]
fn dsl_with_grouped_parens_works() {
	let args = RunArgs::parse(&["--env=prod".into(), "--deploy=yes".into()]);
	let result = args
		.substitute(
			"{{ (ARG.env == 'prod' || ARG.env == 'staging') && ARG.deploy != 'no' }}",
			&HashMap::new(),
		)
		.unwrap();
	assert_eq!(result, "true");
}

#[test]
fn dsl_with_function_call_value() {
	let args = RunArgs::parse(&["--env=PROD".into()]);
	let result = args
		.substitute("{{ to_lower(ARG.env) == 'prod' }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "true");
}

#[test]
fn dsl_inline_in_command_returns_true_string() {
	// User's "my-command --resolve {{ ARG.env == 'production' }}" example.
	let args = RunArgs::parse(&["--env=production".into()]);
	let result = args
		.substitute("my-command --resolve {{ ARG.env == 'production' }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "my-command --resolve true");

	let args = RunArgs::parse(&["--env=staging".into()]);
	let result = args
		.substitute("my-command --resolve {{ ARG.env == 'production' }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "my-command --resolve false");
}

#[test]
fn dsl_unary_negation_works() {
	let args = RunArgs::parse(&["--env=prod".into()]);
	let result = args
		.substitute("{{ !(ARG.env == 'staging') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "true");
}

// ── New if-block evaluation: substitute + check == "true" ──

fn parse_target_inline(json: &str, name: &str) -> CommandSpec {
	let rf = runfile_parser::parse_runfile(json).expect("test runfile must parse");
	rf.targets
		.into_iter()
		.find(|(k, _)| k == name)
		.expect("target not found")
		.1
}

#[test]
fn if_branch_taken_when_substitution_resolves_to_true() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target_inline(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"if":"{{ ARG.env == 'prod' }}","then":["echo prod-branch"],"else":["exit 1"]}
		]}}}"#,
		"t",
	);
	let args = RunArgs::parse(&["--env=prod".into()]);
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert!(result.final_status.success());
}

#[test]
fn if_branch_skipped_when_substitution_resolves_to_false() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target_inline(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"if":"{{ ARG.env == 'prod' }}","then":["exit 1"],"else":["echo other-branch"]}
		]}}}"#,
		"t",
	);
	let args = RunArgs::parse(&["--env=staging".into()]);
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert!(result.final_status.success());
}

#[test]
fn if_string_false_and_empty_take_else_branch() {
	// Only `"true"` / `"false"` / `""` are valid — `"false"` and `""`
	// take the else branch without erroring.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	for false_value in ["false", ""] {
		let spec_json = format!(
			r#"{{"$schema":"x","targets":{{"t":{{"commands":[
				{{"if":"{{{{ ARG.x ? '{false_value}' }}}}","then":["exit 1"],"else":["echo went-else"]}}
			]}}}}}}"#
		);
		let spec = parse_target_inline(&spec_json, "t");
		let args = RunArgs::parse(&[]);
		let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
		assert!(
			result.final_status.success(),
			"expected else-branch for if-value {false_value:?}"
		);
	}
}

#[test]
fn if_non_boolean_value_errors() {
	// Anything that's NOT "true", "false", or "" surfaces as
	// IfConditionNotBoolean. Catches typos like "True" / "1" / "yes" /
	// missing comparison operators.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	for bad_value in ["True", "1", "yes", "hello", "FALSE", "0", " true"] {
		let spec_json = format!(
			r#"{{"$schema":"x","targets":{{"t":{{"commands":[
				{{"if":"{{{{ ARG.x ? '{bad_value}' }}}}","then":["echo went-then"]}}
			]}}}}}}"#
		);
		let spec = parse_target_inline(&spec_json, "t");
		let args = RunArgs::parse(&[]);
		let err = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap_err();
		let msg = err.to_string();
		assert!(
			msg.contains("not a boolean"),
			"value {bad_value:?} should error with 'not a boolean'; got: {msg}"
		);
	}
}

#[test]
fn if_with_literal_true_is_taken() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target_inline(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"if":"true","then":["echo went-then"],"else":["exit 1"]}
		]}}}"#,
		"t",
	);
	let args = RunArgs::parse(&[]);
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert!(result.final_status.success());
}

// ── User's `pull` example from the issue ──

#[test]
fn user_example_with_nested_substitution_in_define() {
	// From the user's example: `{{ define(cmd, 'docker compose -f {{ VAR.compose }} pull') }}`.
	let args = RunArgs::default();
	args.vars
		.lock()
		.unwrap()
		.insert("compose".to_string(), "infra/web/docker-compose.yml".to_string());
	let _ = args
		.substitute(
			"{{ define(cmd, 'docker compose -f {{ VAR.compose }} pull') }}",
			&HashMap::new(),
		)
		.unwrap();
	let v = args.substitute("{{ VAR.cmd }}", &HashMap::new()).unwrap();
	assert_eq!(v, "docker compose -f infra/web/docker-compose.yml pull");
}

// ── parse_static_name (define name) is bareword-only ──

#[test]
fn define_name_must_be_bareword_no_quotes() {
	let args = RunArgs::parse(&[]);
	// Single-quoted name → invalid.
	let err = args
		.substitute("{{ define('name', 'value') }}", &HashMap::new())
		.unwrap_err();
	assert!(matches!(err, SubstitutionError::InvalidVarName(_)));
	// Double-quoted name → invalid.
	let err = args
		.substitute("{{ define(\"name\", 'value') }}", &HashMap::new())
		.unwrap_err();
	assert!(matches!(err, SubstitutionError::InvalidVarName(_)));
	// Bareword name → ok.
	let _ = args.substitute("{{ define(name, 'value') }}", &HashMap::new()).unwrap();
	let v = args.substitute("{{ VAR.name }}", &HashMap::new()).unwrap();
	assert_eq!(v, "value");
}
