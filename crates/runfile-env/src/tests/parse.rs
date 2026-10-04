use super::*;

#[test]
fn serialize_round_trips_awkward_values_and_leaves_simple_ones_bare() {
	// A value that would not read back bare is quoted; one that would is left
	// as it is, so an ordinary decrypted file stays unquoted (audit SA-030).
	let bare = [
		"value",
		"hello world",
		"http://example.com/a//b",
		"a#b",
		"",
		"/usr/bin:/bin",
	];
	for v in bare {
		assert_eq!(serialize_env_value(v), v, "{v:?} should stay bare");
	}
	let quoted = [
		"-----BEGIN KEY-----\nLINE\n-----END KEY-----", // multi-line PEM
		"first\nNODE_OPTIONS=--require ./evil.js",      // the injection the finding reproduces
		"has \" quote",
		"has \\ backslash",
		" leading-space",
		"trailing-space ",
		"'single-quoted'",
		"inline # comment",
		"inline // comment",
		"tab\there",
	];
	for v in quoted {
		let line = serialize_env_line("K", v);
		assert!(line.starts_with("K=\""), "{v:?} -> {line:?} should be quoted");
		let pairs = parse_env_file(&line).unwrap_or_else(|e| panic!("{v:?} -> {line:?} did not parse: {e:?}"));
		assert_eq!(
			pairs,
			vec![("K".to_string(), v.to_string())],
			"{v:?} did not round-trip via {line:?}"
		);
	}
	// The whole point: a decrypted multi-line value is one variable, not two.
	let two_vars = parse_env_file(&serialize_env_line("CERT", "first\nNODE_OPTIONS=x")).unwrap();
	assert_eq!(two_vars.len(), 1, "a newline must not inject a second variable");
}

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
	let content = "KEY=\"hello\\\\\"";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0].1, "hello\\");
}

#[test]
fn parse_env_file_single_quoted_no_escape_processing() {
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

// ── Regression: quoted value with trailing content must NOT enter multi-line
// mode (audit L1). Keying multi-line detection off "ends with a quote" wrongly
// swallowed following KEY=VALUE lines into the value. ─────────────────────────

#[test]
fn parse_env_file_double_quoted_value_with_inline_comment_stays_single_line() {
	let content = "API_URL=\"https://x\" # prod endpoint\nAUTH_REQUIRED=true\n";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(
		pairs,
		vec![
			("API_URL".to_string(), "https://x".to_string()),
			("AUTH_REQUIRED".to_string(), "true".to_string()),
		],
		"the trailing ` # comment` must be stripped and the following line must be its own var"
	);
}

#[test]
fn parse_env_file_single_quoted_value_with_trailing_content_stays_single_line() {
	let content = "NAME='alice' ignored trailing\nNEXT=2\n";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0], ("NAME".to_string(), "alice".to_string()));
	assert_eq!(pairs[1], ("NEXT".to_string(), "2".to_string()));
}

#[test]
fn parse_env_file_quoted_value_with_trailing_does_not_consume_following_lines() {
	// Three vars in a row; the first has a quoted value + trailing text. All
	// three must be parsed independently.
	let content = "A=\"one\" # c1\nB=\"two\" // c2\nC=three\n";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs.len(), 3);
	assert_eq!(pairs[0], ("A".to_string(), "one".to_string()));
	assert_eq!(pairs[1], ("B".to_string(), "two".to_string()));
	assert_eq!(pairs[2], ("C".to_string(), "three".to_string()));
}

#[test]
fn parse_env_file_genuine_multiline_quoted_value_still_works() {
	// A quote with NO close on the same line is still a real multi-line value.
	let content = "CERT=\"line1\nline2\"\nNEXT=after\n";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0], ("CERT".to_string(), "line1\nline2".to_string()));
	assert_eq!(pairs[1], ("NEXT".to_string(), "after".to_string()));
}

#[test]
fn parse_env_file_empty_quoted_value_with_trailing_comment() {
	let content = "EMPTY=\"\" # nothing here\nX=1\n";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0], ("EMPTY".to_string(), String::new()));
	assert_eq!(pairs[1], ("X".to_string(), "1".to_string()));
}

// ══════════════════════════════════════════════════════════════════════
// load_env_files tests
// ══════════════════════════════════════════════════════════════════════

#[test]
fn a_malformed_line_error_does_not_echo_the_line(/* audit SA-026 */) {
	// A line with no `=` -- a continuation of a multi-line secret, or a key body
	// left in plaintext -- must not have its content printed; only the line
	// number and what is wrong.
	let secret = "ghp_FAKE_0123456789abcdef";
	let (line, msg) = parse_env_file(&format!("API_TOKEN=\n{secret}\n")).unwrap_err();
	assert_eq!(line, 2);
	assert!(!msg.contains(secret), "the offending line leaked into the error: {msg}");
	assert!(
		msg.contains("KEY=VALUE") || msg.contains("'='"),
		"still says what is wrong: {msg}"
	);
}

#[test]
fn a_utf8_bom_is_not_part_of_the_first_key() {
	// Windows tools write one; it made the first key `\u{FEFF}KEY`, so a header on
	// line 1 went unseen at run time and by `:env set` (audit SA-034).
	let pairs = parse_env_file("\u{FEFF}RUNFILE_ENCRYPTION_PUBLIC_KEY=abc\nB=2\n").unwrap();
	assert_eq!(
		pairs[0],
		("RUNFILE_ENCRYPTION_PUBLIC_KEY".to_string(), "abc".to_string())
	);
	assert_eq!(pairs[1], ("B".to_string(), "2".to_string()));
}
