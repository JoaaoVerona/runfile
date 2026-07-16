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
