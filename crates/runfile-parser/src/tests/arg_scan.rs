use crate::scan_arg_usage;

// ── The shared argument scanner ────────────────────────────────────────
//
// This is the single source of truth for "what does this target accept?".
// The executor validates the user's arguments against it and the MCP server
// turns it into a JSON schema, so a change here changes both — which is the
// point: the two used to scan separately and disagreed about whether
// `{{ one_of(ARGS, ...) }}` consumed positionals.

fn keys(mut v: Vec<&String>) -> Vec<String> {
	v.sort();
	v.into_iter().cloned().collect()
}

#[test]
fn scan_finds_nothing_in_a_plain_command() {
	let usage = scan_arg_usage(&["cargo build --release".into()]);
	assert_eq!(usage, Default::default());
}

#[test]
fn scan_detects_bare_args_wherever_it_appears() {
	// Every one of these consumes the caller's positionals: on its own, inside a
	// chain, and — the case the MCP server used to miss — as a function argument.
	for body in [
		"echo {{ ARGS }}",
		"{{ define(part, one_of(ARGS, 'major', 'minor', 'patch')) }}",
		"{{ concat('x', ARGS) }}",
		"{{ ARGS ? 'fallback' }}",
		"{{ upper(concat(ARGS)) }}",
		"echo {{ ARGS }} and again {{ ARGS }}",
	] {
		let usage = scan_arg_usage(&[body.into()]);
		assert!(usage.uses_positional, "{body} consumes positionals");
	}
}

#[test]
fn scan_does_not_mistake_lookalikes_for_bare_args() {
	// Identifier characters on either side mean it is a different token.
	for body in [
		"{{ concat(MYARGS) }}",
		"{{ concat(ARGS_FOO) }}",
		"{{ concat(ARGSY) }}",
		"{{ concat(X_ARGS) }}",
		"{{ ARG.key }}",
	] {
		let usage = scan_arg_usage(&[body.into()]);
		assert!(!usage.uses_positional, "{body} is not a bare ARGS reference");
	}
}

#[test]
fn scan_separates_named_args_from_flags() {
	let usage = scan_arg_usage(&["deploy --env={{ ARG.env }} {{ FLAG.force ? --force : }}".into()]);
	assert_eq!(keys(usage.arg_keys.iter().collect()), vec!["env"]);
	assert_eq!(keys(usage.flag_keys.iter().collect()), vec!["force"]);
	// Validation doesn't care which is which — it only asks if `--name` is used.
	assert_eq!(keys(usage.named_keys().iter().collect()), vec!["env", "force"]);
}

#[test]
fn scan_collects_every_key_in_a_chain() {
	let usage = scan_arg_usage(&["echo {{ ARG.a ? ARG.b ? 'x' }}".into()]);
	assert_eq!(keys(usage.arg_keys.iter().collect()), vec!["a", "b"]);
}

#[test]
fn scan_marks_a_key_required_only_without_a_fallback() {
	let usage = scan_arg_usage(&["echo {{ ARG.env }} {{ ARG.region ? 'us-east-1' }}".into()]);
	assert!(
		usage.required_keys.contains("env"),
		"no fallback, so it must be supplied"
	);
	assert!(
		!usage.required_keys.contains("region"),
		"the `?` default makes it optional"
	);
}

#[test]
fn scan_treats_a_key_as_required_if_any_use_lacks_a_fallback() {
	// One substitution defaults it, another doesn't — the caller still has to
	// supply it for the second to resolve.
	let usage = scan_arg_usage(&["echo {{ ARG.env ? 'dev' }}".into(), "echo {{ ARG.env }}".into()]);
	assert!(usage.required_keys.contains("env"));
}

#[test]
fn scan_ignores_escaped_substitutions() {
	// `\{{ ... \}}` is a literal, so nothing inside it is a reference.
	let usage = scan_arg_usage(&[r"echo \{{ ARG.x \}} \{{ ARGS \}}".into()]);
	assert_eq!(usage, Default::default());
}

#[test]
fn scan_ignores_an_unterminated_substitution() {
	let usage = scan_arg_usage(&["echo {{ ARG.x".into()]);
	assert_eq!(usage, Default::default());
}

#[test]
fn scan_reads_keys_out_of_nested_shell_syntax() {
	// The scan is brace-based, so a substitution inside `$( ... )` still counts.
	let usage = scan_arg_usage(&[r#"base=$(echo "$f" | sed 's/\.{{ ARG.env }}$//')"#.into()]);
	assert_eq!(keys(usage.arg_keys.iter().collect()), vec!["env"]);
	assert!(!usage.uses_positional);
}

#[test]
fn scan_handles_keys_with_dashes_and_underscores() {
	let usage = scan_arg_usage(&["x {{ ARG.side-effects }} {{ FLAG.dry_run }}".into()]);
	assert_eq!(keys(usage.arg_keys.iter().collect()), vec!["side-effects"]);
	assert_eq!(keys(usage.flag_keys.iter().collect()), vec!["dry_run"]);
}

#[test]
fn scan_walks_over_non_ascii_bodies() {
	// Substitutions carry prose; stepping by byte instead of character used to
	// panic here (`{{ error('… — …') }}`).
	let usage = scan_arg_usage(&[
		"echo {{ concat('café — 世界 🚀') }}".into(),
		"{{ error(concat('não encontrado — ', ARG.name)) }}".into(),
		"{{ concat('é', ARGS) }}".into(),
	]);
	assert_eq!(keys(usage.arg_keys.iter().collect()), vec!["name"]);
	assert!(usage.uses_positional, "a multi-byte char is not an identifier char");
}

#[test]
fn scan_accumulates_across_templates() {
	let usage = scan_arg_usage(&[
		"echo {{ ARG.a }}".into(),
		"echo {{ FLAG.b }}".into(),
		"echo {{ ARGS }}".into(),
	]);
	assert!(usage.uses_positional);
	assert_eq!(keys(usage.arg_keys.iter().collect()), vec!["a"]);
	assert_eq!(keys(usage.flag_keys.iter().collect()), vec!["b"]);
}
