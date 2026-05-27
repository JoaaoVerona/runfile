use super::*;

// ── Env file parsing tests ────────────────────────────────────────

#[test]
fn parse_env_file_simple() {
	let content = "KEY=value\nANOTHER=hello world\n";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(
		pairs,
		vec![
			("KEY".to_string(), "value".to_string()),
			("ANOTHER".to_string(), "hello world".to_string()),
		]
	);
}

#[test]
fn parse_env_file_with_comments() {
	let content = "# This is a comment\nKEY=value\n// Another comment\nFOO=bar\n";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs.len(), 2);
	assert_eq!(pairs[0], ("KEY".to_string(), "value".to_string()));
	assert_eq!(pairs[1], ("FOO".to_string(), "bar".to_string()));
}

#[test]
fn parse_env_file_blank_lines() {
	let content = "\n\nKEY=value\n\n\nFOO=bar\n\n";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs.len(), 2);
}

#[test]
fn parse_env_file_spaces_around_equals() {
	let content = "KEY = value\nFOO =bar\nBAZ= baz\n";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0], ("KEY".to_string(), "value".to_string()));
	assert_eq!(pairs[1], ("FOO".to_string(), "bar".to_string()));
	assert_eq!(pairs[2], ("BAZ".to_string(), "baz".to_string()));
}

#[test]
fn parse_env_file_double_quoted() {
	let content = r#"KEY="hello world""#;
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0], ("KEY".to_string(), "hello world".to_string()));
}

#[test]
fn parse_env_file_single_quoted() {
	let content = "KEY='hello world'";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0], ("KEY".to_string(), "hello world".to_string()));
}

#[test]
fn parse_env_file_multiline_double_quoted() {
	let content = "KEY=\"line1\nline2\nline3\"";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0], ("KEY".to_string(), "line1\nline2\nline3".to_string()));
}

#[test]
fn parse_env_file_multiline_single_quoted() {
	let content = "KEY='line1\nline2'";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0], ("KEY".to_string(), "line1\nline2".to_string()));
}

#[test]
fn parse_env_file_empty_value() {
	let content = "KEY=";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0], ("KEY".to_string(), "".to_string()));
}

#[test]
fn parse_env_file_escape_sequences() {
	let content = r#"KEY="hello\nworld\ttab""#;
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0], ("KEY".to_string(), "hello\nworld\ttab".to_string()));
}

#[test]
fn parse_env_file_inline_comments() {
	let content = "KEY=value # this is a comment\nFOO=bar // another";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0], ("KEY".to_string(), "value".to_string()));
	assert_eq!(pairs[1], ("FOO".to_string(), "bar".to_string()));
}

#[test]
fn parse_env_file_export_prefix() {
	let content = "export KEY=value\nexport FOO=bar";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0], ("KEY".to_string(), "value".to_string()));
	assert_eq!(pairs[1], ("FOO".to_string(), "bar".to_string()));
}

#[test]
fn parse_env_file_error_no_equals() {
	let content = "INVALID_LINE";
	let err = parse_env_file(content);
	assert!(err.is_err());
}

#[test]
fn load_env_files_missing_file_ignored() {
	let dir = TempDir::new().unwrap();
	let args = RunArgs::default();
	let env = HashMap::new();
	let result = load_env_files(&[".env.nonexistent".to_string()], dir.path(), &args, &env);
	assert!(result.is_ok());
	assert!(result.unwrap().is_empty());
}

#[test]
fn load_env_files_reads_existing_file() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "MY_KEY=my_value\n").unwrap();
	let args = RunArgs::default();
	let env = HashMap::new();
	let result = load_env_files(&[".env".to_string()], dir.path(), &args, &env).unwrap();
	assert_eq!(result.get("MY_KEY").unwrap(), "my_value");
}

#[test]
fn load_env_files_later_overrides_earlier() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "KEY=first\n").unwrap();
	std::fs::write(dir.path().join(".env.local"), "KEY=second\n").unwrap();
	let args = RunArgs::default();
	let env = HashMap::new();
	let result = load_env_files(&[".env".to_string(), ".env.local".to_string()], dir.path(), &args, &env).unwrap();
	assert_eq!(result.get("KEY").unwrap(), "second");
}

#[test]
fn load_env_files_with_args_substitution() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env.production"), "DB=prod-db\n").unwrap();
	let args = RunArgs::parse(&["--env".into(), "production".into()]);
	let env = HashMap::new();
	let result = load_env_files(&[".env.{{ ARG.env }}".to_string()], dir.path(), &args, &env).unwrap();
	assert_eq!(result.get("DB").unwrap(), "prod-db");
}

#[test]
fn load_env_files_with_env_substitution() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env.staging"), "DB=staging-db\n").unwrap();
	let args = RunArgs::default();
	let mut env = HashMap::new();
	env.insert("environment".to_string(), "staging".to_string());
	let result = load_env_files(&[".env.{{ ENV.environment }}".to_string()], dir.path(), &args, &env).unwrap();
	assert_eq!(result.get("DB").unwrap(), "staging-db");
}

#[test]
fn load_env_files_with_default_substitution() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env.development"), "DB=dev-db\n").unwrap();
	let args = RunArgs::default();
	let env = HashMap::new();
	let result = load_env_files(
		&[".env.{{ ENV.environment ? 'development' }}".to_string()],
		dir.path(),
		&args,
		&env,
	)
	.unwrap();
	assert_eq!(result.get("DB").unwrap(), "dev-db");
}

#[test]
fn build_env_env_files_before_env() {
	let dir = TempDir::new().unwrap();
	// env file sets KEY=from_file
	std::fs::write(dir.path().join(".env"), "KEY=from_file\n").unwrap();

	let mut cmd_env = HashMap::new();
	cmd_env.insert("KEY".into(), EnvValue::String("from_env".into()));

	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.env_files = Some(vec![".env".into()]);
	spec.env = Some(cmd_env);

	// env (inline) should override envFiles
	let env = build_env(&spec, dir.path(), dir.path(), &RunArgs::default(), None).unwrap();
	assert_eq!(env.get("KEY").unwrap(), "from_env");
}

#[test]
fn build_env_global_env_files() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "GLOBAL_KEY=global_value\n").unwrap();

	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.env_files = Some(vec![".env".into()]);

	let env = build_env(&spec, dir.path(), dir.path(), &RunArgs::default(), None).unwrap();
	assert_eq!(env.get("GLOBAL_KEY").unwrap(), "global_value");
}

#[test]
fn build_env_target_env_files_override_global_env_files() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "KEY=global\n").unwrap();
	std::fs::write(dir.path().join(".env.target"), "KEY=target\n").unwrap();

	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.env_files = Some(vec![".env".into(), ".env.target".into()]);

	let env = build_env(&spec, dir.path(), dir.path(), &RunArgs::default(), None).unwrap();
	assert_eq!(env.get("KEY").unwrap(), "target");
}

#[test]
fn load_env_files_parse_error() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "INVALID_NO_EQUALS\n").unwrap();
	let args = RunArgs::default();
	let env = HashMap::new();
	let result = load_env_files(&[".env".to_string()], dir.path(), &args, &env);
	assert!(result.is_err());
}

// ══════════════════════════════════════════════════════════════════════
// Additional test coverage — env.rs
// ══════════════════════════════════════════════════════════════════════

#[test]
fn parse_env_file_empty_key_errors() {
	let content = "=value";
	let err = parse_env_file(content);
	assert!(err.is_err());
	let (line, msg) = err.unwrap_err();
	assert_eq!(line, 1);
	assert!(msg.contains("empty key"), "got: {msg}");
}

#[test]
fn parse_env_file_export_prefix_stripped() {
	// "export =value" — after stripping "export ", key is "" which is before the =
	// but the raw line is "export =value", key part is "export " which contains the export prefix.
	// Actually the line is parsed as key="export " value="value" before export stripping.
	// Let's test that export prefix is correctly handled with a valid key.
	let content = "export MY_KEY=my_value";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0], ("MY_KEY".to_string(), "my_value".to_string()));
}

#[test]
fn parse_env_file_unterminated_double_quote() {
	let content = "KEY=\"this is never closed\n";
	let err = parse_env_file(content);
	assert!(err.is_err());
	let (_, msg) = err.unwrap_err();
	assert!(msg.contains("unterminated"), "got: {msg}");
}

#[test]
fn parse_env_file_unterminated_single_quote() {
	let content = "KEY='this is never closed\n";
	let err = parse_env_file(content);
	assert!(err.is_err());
	let (_, msg) = err.unwrap_err();
	assert!(msg.contains("unterminated"), "got: {msg}");
}

#[test]
fn parse_env_file_escaped_double_quote_in_value() {
	let content = r#"KEY="say \"hello\"""#;
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0].1, r#"say "hello""#);
}

#[test]
fn parse_env_file_escaped_backslash() {
	let content = r#"KEY="path\\to\\file""#;
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0].1, r"path\to\file");
}

#[test]
fn parse_env_file_carriage_return_escape() {
	let content = r#"KEY="line\r""#;
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0].1, "line\r");
}

#[test]
fn parse_env_file_unknown_escape_preserved() {
	let content = r#"KEY="hello\x""#;
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0].1, "hello\\x");
}

#[test]
fn parse_env_file_trailing_backslash_in_double_quotes() {
	// A trailing backslash before closing quote: \"hello\\\" is:
	// opening ", hello, \\(escaped backslash), \"(escaped quote) — no closing quote
	// So this is actually an unterminated string.
	// Test that the parser correctly detects an escaped closing quote vs real closing.
	let content = "KEY=\"hello\\\\\"";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0].1, "hello\\");
}

#[test]
fn parse_env_file_single_quoted_no_escape_processing() {
	// Single-quoted values should NOT process escape sequences
	let content = r#"KEY='hello\nworld'"#;
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0].1, r"hello\nworld");
}

#[test]
fn parse_env_file_multiple_entries() {
	let content = "A=1\nB=2\nC=3\n";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs.len(), 3);
	assert_eq!(pairs[2], ("C".to_string(), "3".to_string()));
}

#[test]
fn parse_env_file_value_with_equals() {
	let content = "KEY=abc=def";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0], ("KEY".to_string(), "abc=def".to_string()));
}

#[test]
fn parse_env_file_only_comments_and_blanks() {
	let content = "# comment\n\n// another\n\n";
	let pairs = parse_env_file(content).unwrap();
	assert!(pairs.is_empty());
}

#[test]
fn parse_env_file_empty_content() {
	let pairs = parse_env_file("").unwrap();
	assert!(pairs.is_empty());
}

#[test]
fn load_env_files_absolute_path() {
	let dir = TempDir::new().unwrap();
	let env_path = dir.path().join("abs.env");
	std::fs::write(&env_path, "ABS_KEY=abs_value\n").unwrap();
	let args = RunArgs::default();
	let env = HashMap::new();
	let result = load_env_files(&[env_path.to_str().unwrap().to_string()], dir.path(), &args, &env).unwrap();
	assert_eq!(result.get("ABS_KEY").unwrap(), "abs_value");
}

#[test]
fn load_env_files_multiple_missing_files_all_skipped() {
	let dir = TempDir::new().unwrap();
	let args = RunArgs::default();
	let env = HashMap::new();
	let result = load_env_files(
		&[
			".env.missing1".to_string(),
			".env.missing2".to_string(),
			".env.missing3".to_string(),
		],
		dir.path(),
		&args,
		&env,
	);
	assert!(result.is_ok());
	assert!(result.unwrap().is_empty());
}

// ══════════════════════════════════════════════════════════════════════
// Additional test coverage — args.rs
// ══════════════════════════════════════════════════════════════════════

#[test]
fn parse_bare_double_dash() {
	let args = RunArgs::parse(&["--".into()]);
	assert_eq!(args.original, vec!["--"]);
	// Bare "--" should not add to named (empty stripped)
	assert!(args.named.is_empty());
}

#[test]
fn parse_named_arg_with_empty_equals() {
	let args = RunArgs::parse(&["--key=".into()]);
	assert_eq!(args.named["key"], "");
}

#[test]
fn parse_flag_at_end_is_empty_string() {
	let args = RunArgs::parse(&["--verbose".into()]);
	assert_eq!(args.named.get("verbose").unwrap(), "");
}

#[test]
fn parse_flag_followed_by_another_flag() {
	let args = RunArgs::parse(&["--verbose".into(), "--debug".into()]);
	assert_eq!(args.named["verbose"], "");
	assert_eq!(args.named["debug"], "");
}

#[test]
fn substitute_shell_dollar_paren_expression_preserved() {
	// Shell `$(...)` command substitutions pass through verbatim — they're
	// not Runfile syntax and the substituter doesn't touch them.
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env("echo $(date +%s)").unwrap();
	assert_eq!(result, "echo $(date +%s)");
}

#[test]
fn substitute_resolves_braces_inside_shell_command_substitution() {
	// A shell `$(...)` command substitution is opaque to Runfile — it stays
	// literal in the output. But any `{{ ... }}` reference *inside* the shell
	// substitution body is still resolved so users can do
	// `$(echo "{{ ARG.env }}")` and have the env get substituted.
	let args = RunArgs::parse(&["--env=development".into()]);
	let result = args
		.substitute_no_env(r#"base=$(echo "$f" | sed 's/\.{{ ARG.env }}$//')"#)
		.unwrap();
	assert_eq!(result, r#"base=$(echo "$f" | sed 's/\.development$//')"#);
}

#[test]
fn substitute_resolves_braces_in_deeply_nested_shell_substitution() {
	let args = RunArgs::parse(&["--name=world".into()]);
	let mut env = HashMap::new();
	env.insert("GREETING".to_string(), "hello".to_string());
	let result = args
		.substitute(r#"x=$(printf '%s' $(echo "{{ ENV.GREETING }} {{ ARG.name }}"))"#, &env)
		.unwrap();
	assert_eq!(result, r#"x=$(printf '%s' $(echo "hello world"))"#);
}

#[test]
fn substitute_propagates_missing_arg_inside_shell_substitution() {
	// A missing `{{ ARG.x }}` inside a shell `$(...)` should still error
	// rather than silently leaking through unsubstituted.
	let args = RunArgs::parse(&[]);
	let err = args.substitute_no_env(r#"x=$(echo "{{ ARG.missing }}")"#).unwrap_err();
	matches!(err, SubstitutionError::MissingArg(_));
}

#[test]
fn substitute_redacts_env_inside_shell_substitution() {
	let args = RunArgs::parse(&[]);
	let mut env = HashMap::new();
	env.insert("TOKEN".to_string(), "secret123".to_string());
	let result = args
		.substitute_redacted(r#"x=$(echo "{{ ENV.TOKEN }}")"#, &env)
		.unwrap();
	assert_eq!(result, r#"x=$(echo "***")"#);
}

#[test]
fn scan_args_usage_finds_args_inside_shell_substitution() {
	// validate_args needs to see `--env` referenced even when its only use
	// is nested inside a shell `$(echo {{ ARG.env }})`-style command sub.
	let cmds = vec![r#"base=$(echo "$f" | sed 's/\.{{ ARG.env }}$//')"#.into()];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(!positional);
	assert!(named.contains("env"));
}

#[test]
fn substitute_multiple_args_placeholders() {
	let args = RunArgs::parse(&["hello".into()]);
	let result = args.substitute_no_env("echo {{ ARGS }} and {{ ARGS }}").unwrap();
	assert_eq!(result, "echo hello and hello");
}

#[test]
fn substitute_adjacent_dollar_signs() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env("echo $$HOME").unwrap();
	assert_eq!(result, "echo $$HOME");
}

#[test]
fn substitute_dollar_without_paren() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env("echo $HOME").unwrap();
	assert_eq!(result, "echo $HOME");
}
