use super::*;

/// `substitute` against an empty env map.
fn sub(args: &RunArgs, template: &str) -> Result<String, SubstitutionError> {
	args.substitute(template, &HashMap::new())
}

/// Build a `RunArgs` whose run-wide VAR map is pre-seeded.
fn args_with_vars(pairs: &[(&str, &str)]) -> RunArgs {
	let args = RunArgs::default();
	{
		let mut m = args.vars.lock().unwrap();
		for (k, v) in pairs {
			m.insert((*k).to_string(), (*v).to_string());
		}
	}
	args
}

// ── Source prefixes resolve ─────────────────────────────────────────

#[test]
fn arg_dot_resolves_named() {
	let args = RunArgs::parse(&["--env=prod".into()]);
	assert_eq!(sub(&args, "{{ ARG.env }}").unwrap(), "prod");
}

#[test]
fn arg_dot_with_default_and_missing_errors() {
	let args = RunArgs::parse(&[]);
	assert_eq!(sub(&args, "{{ ARG.env ? 'dev' }}").unwrap(), "dev");
	// No default → hard error (MissingArg).
	assert!(matches!(
		sub(&args, "{{ ARG.env }}").unwrap_err(),
		SubstitutionError::MissingArg(_)
	));
}

#[test]
fn flag_dot_boolean_and_ternary() {
	let present = RunArgs::parse(&["--prod".into()]);
	assert_eq!(sub(&present, "{{ FLAG.prod }}").unwrap(), "true");
	assert_eq!(sub(&present, "{{ FLAG.prod ? 'yes' : 'no' }}").unwrap(), "yes");
	let absent = RunArgs::parse(&[]);
	assert_eq!(sub(&absent, "{{ FLAG.prod }}").unwrap(), "false");
	assert_eq!(sub(&absent, "{{ FLAG.prod ? 'yes' : 'no' }}").unwrap(), "no");
}

#[test]
fn flag_dot_with_hyphen() {
	let args = RunArgs::parse(&["--dry-run".into()]);
	assert_eq!(sub(&args, "{{ FLAG.dry-run }}").unwrap(), "true");
}

#[test]
fn var_dot_resolves() {
	let args = args_with_vars(&[("greeting", "hello")]);
	assert_eq!(sub(&args, "{{ VAR.greeting }}").unwrap(), "hello");
}

#[test]
fn var_dot_in_function_and_chain() {
	let args = args_with_vars(&[("name", "world")]);
	assert_eq!(sub(&args, "{{ to_upper(VAR.name) }}").unwrap(), "WORLD");
	assert_eq!(sub(&args, "{{ ARG.x ? VAR.name }}").unwrap(), "world");
}

#[test]
fn bare_args_still_passes_all_positional() {
	let args = RunArgs::parse(&["a".into(), "b".into(), "c".into()]);
	assert_eq!(sub(&args, "run {{ ARGS }}").unwrap(), "run a b c");
}

#[test]
fn bare_args_as_function_arg_still_works() {
	// `one_of(ARGS, ...)` consumes the positional value — bare ARGS, not ARG.
	let args = RunArgs::parse(&["minor".into()]);
	assert_eq!(
		sub(&args, "{{ one_of(ARGS, 'major', 'minor', 'patch') }}").unwrap(),
		"minor"
	);
}

// ── New forms inside DSL conditions ─────────────────────────────────

#[test]
fn new_forms_work_in_dsl() {
	let args = RunArgs::parse(&["--env=prod".into(), "--wsl".into()]);
	assert_eq!(sub(&args, "{{ ARG.env == 'prod' }}").unwrap(), "true");
	// Bare FLAG is truthy inside DSL; comparison form needs a quoted literal.
	assert_eq!(sub(&args, "{{ FLAG.wsl == 'true' }}").unwrap(), "true");
	assert_eq!(sub(&args, "{{ RUN.os == 'nope' || FLAG.wsl }}").unwrap(), "true");
	let v = args_with_vars(&[("mode", "fast")]);
	assert_eq!(sub(&v, "{{ VAR.mode == 'fast' }}").unwrap(), "true");
}
