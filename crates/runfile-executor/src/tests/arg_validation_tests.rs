use super::*;

// ── Argument validation tests ──────────────────────────────────────────

#[test]
fn scan_args_detects_positional() {
	let cmds = vec!["echo {{ ARGS }}".into()];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(positional);
	assert!(named.is_empty());
}

#[test]
fn scan_args_detects_named() {
	let cmds = vec!["echo {{ ARG.env }}".into(), "echo {{ ARG.port ? '8080' }}".into()];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(!positional);
	assert!(named.contains("env"));
	assert!(named.contains("port"));
}

#[test]
fn scan_args_detects_both() {
	let cmds = vec!["echo {{ ARG.env }} {{ ARGS }}".into()];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(positional);
	assert!(named.contains("env"));
}

#[test]
fn scan_args_no_patterns() {
	let cmds = vec!["echo hello".into(), "npm run build".into()];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(!positional);
	assert!(named.is_empty());
}

#[test]
fn scan_args_detects_positional_inside_function_call() {
	// `one_of(ARGS, 'a', 'b')` consumes positional args even though the
	// substitution body isn't bare `ARGS`. validate_args would otherwise
	// reject the command for being unable to consume the user's input.
	let cmds = vec!["{{ one_of(ARGS, 'major', 'minor') }}".into()];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(positional);
	assert!(named.is_empty());
}

#[test]
fn scan_args_detects_positional_inside_define() {
	// `{{ define(x, ARGS) }}` is the natural form for stashing the
	// positional input — also has to count as positional usage.
	let cmds = vec!["{{ define(part, ARGS) }}".into()];
	let (positional, _named) = scan_args_usage(&cmds);
	assert!(positional);
}

#[test]
fn scan_args_distinguishes_bare_args_from_named_form() {
	// `ARG.env` is a named-key reference, NOT a bare-ARGS consumer.
	// Confirms the scanner doesn't double-count the same `ARGS` token.
	let cmds = vec!["{{ one_of(ARG.env, 'dev', 'prod') }}".into()];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(!positional);
	assert!(named.contains("env"));
}

#[test]
fn scan_args_does_not_misread_word_containing_args() {
	// Identifiers that merely contain "ARGS" (e.g. `MYARGS`, `ARGS_FOO`)
	// must NOT register as positional usage. They're invalid barewords
	// that surface elsewhere — the scanner just has to ignore them.
	let cmds = vec!["echo {{ ENV.MYARGS ? 'none' }}".into()];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(!positional);
	assert!(named.is_empty());
}

#[test]
fn validate_args_no_args_always_ok() {
	let args = RunArgs::default();
	let cmds = vec!["echo hello".into()];
	assert!(validate_args(&args, &cmds).is_ok());
}

#[test]
fn validate_args_unexpected_args_error() {
	let args = RunArgs::parse(&["foo".into()]);
	let cmds = vec!["echo hello".into()];
	let err = validate_args(&args, &cmds).unwrap_err();
	assert!(
		err.to_string().contains("No command in this target accepts arguments"),
		"Expected UnexpectedArgs, got: {err}"
	);
}

#[test]
fn validate_args_unexpected_named_args_error() {
	let args = RunArgs::parse(&["--env=prod".into()]);
	let cmds = vec!["echo hello".into()];
	let err = validate_args(&args, &cmds).unwrap_err();
	assert!(
		err.to_string().contains("No command in this target accepts arguments"),
		"Expected UnexpectedArgs, got: {err}"
	);
}

#[test]
fn validate_args_unknown_named_arg_error() {
	let args = RunArgs::parse(&["--env=prod".into(), "--port=8080".into()]);
	let cmds = vec!["echo {{ ARG.env }}".into()]; // only {{ ARG.env }}, not {{ ARG.port }}
	let err = validate_args(&args, &cmds).unwrap_err();
	assert!(
		err.to_string().contains("Unknown named argument \"--port\""),
		"Expected UnknownNamedArg, got: {err}"
	);
}

#[test]
fn validate_args_known_named_arg_ok() {
	let args = RunArgs::parse(&["--env=prod".into()]);
	let cmds = vec!["echo {{ ARG.env }}".into()];
	assert!(validate_args(&args, &cmds).is_ok());
}

#[test]
fn validate_args_positional_accepts_all() {
	// When {{ ARGS }} is used, all args are accepted (including unknown named ones)
	let args = RunArgs::parse(&["--env=prod".into(), "foo".into(), "bar".into()]);
	let cmds = vec!["echo {{ ARGS }}".into()];
	assert!(validate_args(&args, &cmds).is_ok());
}

#[test]
fn validate_args_named_only_rejects_positional() {
	// Commands only use {{ ARG.env }}, but user passes positional args
	let args = RunArgs::parse(&["--env=prod".into(), "extra_arg".into()]);
	let cmds = vec!["echo {{ ARG.env }}".into()];
	let err = validate_args(&args, &cmds).unwrap_err();
	assert!(
		err.to_string().contains("No command in this target accepts arguments")
			|| err.to_string().contains("extra_arg"),
		"Expected error about unexpected positional args, got: {err}"
	);
}

// ── Integration: run_target rejects unexpected args ────────────────────

#[test]
fn run_target_rejects_unexpected_args() {
	use crate::runner::run_target;
	use runfile_parser::Runfile;

	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["echo hello"] }
        }
    }"#;

	let runfile: Runfile = serde_json::from_str(json).unwrap();
	let shell = ResolvedShell {
		kind: ShellKind::Bash,
		path: PathBuf::from("/bin/bash"),
	};
	let args = RunArgs::parse(&["--env=prod".into()]);
	let dir = TempDir::new().unwrap();

	let result = run_target("build", &runfile, &shell, &args, dir.path());
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(
		err.contains("No command in this target accepts arguments"),
		"Expected unexpected args error, got: {err}"
	);
}

#[test]
fn run_target_rejects_unknown_named_arg() {
	use crate::runner::run_target;
	use runfile_parser::Runfile;

	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "deploy": { "commands": ["echo deploying to {{ ARG.env }}"] }
        }
    }"#;

	let runfile: Runfile = serde_json::from_str(json).unwrap();
	let shell = ResolvedShell {
		kind: ShellKind::Bash,
		path: PathBuf::from("/bin/bash"),
	};
	let args = RunArgs::parse(&["--env=prod".into(), "--unknown=val".into()]);
	let dir = TempDir::new().unwrap();

	let result = run_target("deploy", &runfile, &shell, &args, dir.path());
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(
		err.contains("Unknown named argument \"--unknown\""),
		"Expected unknown named arg error, got: {err}"
	);
}

#[test]
fn run_target_accepts_valid_args() {
	use crate::runner::run_target;
	use runfile_parser::Runfile;

	let shell = detect_default_shell().unwrap();
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "greet": { "commands": ["echo hello {{ ARGS }}"] }
        }
    }"#;

	let runfile: Runfile = serde_json::from_str(json).unwrap();
	let args = RunArgs::parse(&["world".into()]);
	let dir = TempDir::new().unwrap();

	let result = run_target("greet", &runfile, &shell, &args, dir.path());
	assert!(result.is_ok());
}

// ── Integration: extract rejects unexpected args ───────────────────────

#[test]
fn extract_rejects_unexpected_args() {
	use crate::extract::extract_target;
	use runfile_parser::Runfile;

	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["echo hello"] }
        }
    }"#;

	let runfile: Runfile = serde_json::from_str(json).unwrap();
	let args = RunArgs::parse(&["extra".into()]);
	let dir = TempDir::new().unwrap();

	let result = extract_target("build", &runfile, &args, dir.path());
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(
		err.contains("No command in this target accepts arguments"),
		"Expected unexpected args error, got: {err}"
	);
}

#[test]
fn validate_args_considers_dependency_commands() {
	// If the dependency uses {{ ARGS }}, args should be accepted
	let args = RunArgs::parse(&["world".into()]);
	let cmds = vec!["echo clean".into(), "echo {{ ARGS }}".into()];
	assert!(validate_args(&args, &cmds).is_ok());
}

#[test]
fn run_target_dependency_args_accepted() {
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = detect_default_shell().unwrap();
	// `@setup {{ ARGS }}` forwards the parent's args explicitly.
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["@setup {{ ARGS }}", "echo building"] },
            "setup": { "commands": ["echo setup {{ ARGS }}"] }
        }
    }"#;

	let runfile = parse_runfile(json).unwrap();
	let args = RunArgs::parse(&["myarg".into()]);
	let dir = TempDir::new().unwrap();

	let result = run_target("build", &runfile, &shell, &args, dir.path());
	assert!(result.is_ok());
}

// ── Arg validation also scans non-`commands` template fields ──────────
//
// Regression: {{ ARG.x }}/{{ FLAG.x }} references in env values, envFiles,
// forceShell, addToPath, workingDirectory, confirm, and extendStdio paths
// must be recognised by `validate_args` so users can pass --x without
// also referencing the arg from a command string.

#[test]
fn run_target_accepts_flag_referenced_only_in_env() {
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = detect_default_shell().unwrap();
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "test": {
                "commands": "echo running",
                "env": { "RUN_TESTS_WITH_SIDE_EFFECTS": "{{ FLAG.side-effects }}" }
            }
        }
    }"#;

	let runfile = parse_runfile(json).unwrap();
	let args = RunArgs::parse(&["--side-effects".into()]);
	let dir = TempDir::new().unwrap();

	let result = run_target("test", &runfile, &shell, &args, dir.path());
	assert!(result.is_ok(), "expected run to succeed, got: {:?}", result.err());
}

#[test]
fn run_target_accepts_arg_referenced_only_in_env() {
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = detect_default_shell().unwrap();
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "deploy": {
                "commands": "echo deploying",
                "env": { "TARGET_ENV": "{{ ARG.env }}" }
            }
        }
    }"#;

	let runfile = parse_runfile(json).unwrap();
	let args = RunArgs::parse(&["--env=prod".into()]);
	let dir = TempDir::new().unwrap();

	let result = run_target("deploy", &runfile, &shell, &args, dir.path());
	assert!(result.is_ok(), "expected run to succeed, got: {:?}", result.err());
}

#[test]
fn run_target_accepts_arg_referenced_only_in_env_files() {
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = detect_default_shell().unwrap();
	// envFiles paths support substitution; missing files are silently skipped,
	// so this still runs successfully even though `.env.prod` doesn't exist.
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "deploy": {
                "commands": "echo deploying",
                "envFiles": [".env.{{ ARG.env }}"]
            }
        }
    }"#;

	let runfile = parse_runfile(json).unwrap();
	let args = RunArgs::parse(&["--env=prod".into()]);
	let dir = TempDir::new().unwrap();

	let result = run_target("deploy", &runfile, &shell, &args, dir.path());
	assert!(result.is_ok(), "expected run to succeed, got: {:?}", result.err());
}

#[test]
fn run_target_accepts_arg_referenced_only_in_force_shell() {
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	// Pass --shellname=bash but reference it only via forceShell: {{ ARG.shellname }}.
	// We don't care which shell ends up resolved — only that validate_args
	// doesn't reject the unknown-arg.
	let shell = detect_default_shell().unwrap();
	let shell_name = shell.kind.name().to_string();
	let json = format!(
		r#"{{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "x": {{
                "commands": "echo go",
                "forceShell": "{{{{ ARG.shellname ? {shell_name} }}}}"
            }}
        }}
    }}"#
	);

	let runfile = parse_runfile(&json).unwrap();
	let args = RunArgs::parse(&[format!("--shellname={shell_name}")]);
	let dir = TempDir::new().unwrap();

	let result = run_target("x", &runfile, &shell, &args, dir.path());
	assert!(result.is_ok(), "expected run to succeed, got: {:?}", result.err());
}

#[test]
fn validate_args_rejects_truly_unknown_named_arg_with_aux_fields() {
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = detect_default_shell().unwrap();
	// env references --side-effects only. --bogus is genuinely unknown.
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "test": {
                "commands": "echo running",
                "env": { "X": "{{ FLAG.side-effects }}" }
            }
        }
    }"#;

	let runfile = parse_runfile(json).unwrap();
	let args = RunArgs::parse(&["--bogus".into()]);
	let dir = TempDir::new().unwrap();

	let result = run_target("test", &runfile, &shell, &args, dir.path());
	let err = result.unwrap_err().to_string();
	assert!(
		err.contains("Unknown named argument \"--bogus\""),
		"expected unknown-arg error, got: {err}"
	);
}

// ── Non-ASCII inside a substitution ────────────────────────────────────
//
// The scanner walks the substitution body looking for `ARG.` / `FLAG.` /
// `ARGS`, and it must step over characters, not bytes: slicing a `&str` at an
// offset that lands inside a multi-byte character panics. Real Runfiles carry
// prose in their substitutions — `{{ error('… — …') }}` — so every one of these
// must scan cleanly.

#[test]
fn scan_args_usage_survives_non_ascii_in_substitution() {
	for body in [
		"echo {{ concat('café') }}",               // accented Latin (2-byte)
		"echo {{ concat('a — b') }}",              // em dash (3-byte)
		"echo {{ concat('世界') }}",               // CJK (3-byte)
		"echo {{ concat('🚀 ship it') }}",         // emoji (4-byte)
		"echo {{ concat('é', '—', '世', '🚀') }}", // all of them at once
	] {
		let (positional, named) = scan_args_usage(&[body.into()]);
		assert!(!positional, "no ARGS reference in {body:?}");
		assert!(named.is_empty(), "no named keys in {body:?}, got {named:?}");
	}
}

#[test]
fn scan_args_usage_finds_keys_alongside_non_ascii() {
	// The key must still be found when multi-byte text sits on either side of
	// it — scanning must not stop at, or trip over, the first non-ASCII char.
	let cmds = vec![
		"echo {{ ARG.env }} — déjà vu".into(),
		"echo 🚀 {{ ARG.port ? '8080' }}".into(),
	];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(!positional);
	assert!(named.contains("env"), "got {named:?}");
	assert!(named.contains("port"), "got {named:?}");
}

#[test]
fn scan_args_usage_finds_keys_after_non_ascii_inside_one_substitution() {
	// Same substitution body: prose first, reference second. This is the shape
	// that panicked in the wild — a message string followed by an arg lookup.
	let cmds = vec!["{{ error(concat('não encontrado — ', ARG.name)) }}".into()];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(!positional);
	assert!(named.contains("name"), "got {named:?}");
}

#[test]
fn scan_args_usage_detects_positional_alongside_non_ascii() {
	let cmds = vec![
		"{{ define(p, one_of(ARGS, 'major', 'minor')) }}".into(),
		"echo 'não — ok'".into(),
	];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(positional);
	assert!(named.is_empty(), "got {named:?}");

	// And when the multi-byte text is inside the *same* body, before ARGS.
	let (positional, _) = scan_args_usage(&["{{ concat('café — ', ARGS) }}".into()]);
	assert!(positional);
}

#[test]
fn scan_args_usage_non_ascii_adjacent_to_args_is_not_a_word_char() {
	// A multi-byte character butting up against `ARGS` must not be mistaken for
	// an identifier character, which would suppress the positional reference.
	let (positional, _) = scan_args_usage(&["{{ concat('é', ARGS) }}".into()]);
	assert!(positional, "ARGS preceded by a multi-byte char still counts");

	// `ARGS` as part of a longer identifier is still not a reference.
	let (positional, named) = scan_args_usage(&["echo {{ concat('x', MYARGS_é) }}".into()]);
	assert!(!positional, "MYARGS_é is not a bare ARGS reference");
	assert!(named.is_empty());
}

#[test]
fn scan_args_usage_non_ascii_outside_substitutions_is_ignored() {
	// Prose outside the braces never reached the body scanner, but pin it so the
	// two loops stay in agreement.
	let cmds = vec!["echo 'olá — mundo' && echo {{ ARG.who }} 🚀".into()];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(!positional);
	assert!(named.contains("who"), "got {named:?}");
}

// ── Agreement with the shared scanner ──────────────────────────────────
//
// Validation here and the MCP server's tool schema are two projections of the
// same scan (runfile_parser::scan_arg_usage). These pin this side's projection
// over the shapes that matter, so a target the schema advertises as taking
// input is one this validator will actually accept input for.

#[test]
fn scan_args_accepts_positional_referenced_from_a_function_call() {
	// The shape that used to be invisible to the MCP server: the positional is
	// consumed by `one_of`, not by a bare `{{ ARGS }}` body.
	for cmd in [
		"{{ define(part, one_of(ARGS, 'major', 'minor', 'patch')) }}",
		"{{ concat('x', ARGS) }}",
		"{{ upper(concat(ARGS)) }}",
	] {
		let (positional, named) = scan_args_usage(&[cmd.into()]);
		assert!(positional, "{cmd} consumes positionals");
		assert!(named.is_empty(), "{cmd} names no keys, got {named:?}");
	}
}

#[test]
fn scan_args_lookalike_identifiers_are_not_positional() {
	for cmd in ["{{ concat(MYARGS) }}", "{{ concat(ARGS_FOO) }}", "{{ concat(X_ARGS) }}"] {
		let (positional, _) = scan_args_usage(&[cmd.into()]);
		assert!(!positional, "{cmd} is not a bare ARGS reference");
	}
}

#[test]
fn scan_args_named_keys_merge_args_and_flags() {
	// Validation asks only "is `--name` referenced?", so both kinds land in one
	// set — unlike the schema side, which needs them apart (string vs boolean).
	let (positional, named) = scan_args_usage(&["deploy {{ ARG.env }} {{ FLAG.force ? --force : }}".into()]);
	assert!(!positional);
	assert!(named.contains("env"));
	assert!(named.contains("force"));
	assert_eq!(named.len(), 2);
}
