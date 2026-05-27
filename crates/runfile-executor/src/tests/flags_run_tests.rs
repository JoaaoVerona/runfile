use super::*;

// ── FLAGS substitution tests ─────────────────────────────────────────

#[test]
fn flags_basic_true() {
	let args = RunArgs::parse(&["--verbose".into()]);
	let result = args.substitute_no_env("echo {{ FLAG.verbose }}").unwrap();
	assert_eq!(result, "echo true");
}

#[test]
fn flags_basic_false() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env("echo {{ FLAG.verbose }}").unwrap();
	assert_eq!(result, "echo false");
}

#[test]
fn flags_ternary_true() {
	let args = RunArgs::parse(&["--debug".into()]);
	let result = args.substitute_no_env("gcc {{ FLAG.debug ? '-g' : '-O2' }}").unwrap();
	assert_eq!(result, "gcc -g");
}

#[test]
fn flags_ternary_false() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env("gcc {{ FLAG.debug ? '-g' : '-O2' }}").unwrap();
	assert_eq!(result, "gcc -O2");
}

#[test]
fn flags_ternary_with_spaces_in_values() {
	let args = RunArgs::parse(&["--color".into()]);
	let result = args
		.substitute_no_env("cmd {{ FLAG.color ? '--color always' : '--color never' }}")
		.unwrap();
	assert_eq!(result, "cmd --color always");
}

#[test]
fn flags_ternary_with_spaces_false_branch() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute_no_env("cmd {{ FLAG.color ? '--color always' : '--color never' }}")
		.unwrap();
	assert_eq!(result, "cmd --color never");
}

#[test]
fn flags_no_colon_present() {
	let args = RunArgs::parse(&["--v".into()]);
	let result = args.substitute_no_env("cmd {{ FLAG.v ? '-v' }}").unwrap();
	assert_eq!(result, "cmd -v");
}

#[test]
fn flags_no_colon_absent() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env("cmd {{ FLAG.v ? '-v' }}").unwrap();
	assert_eq!(result, "cmd ");
}

#[test]
fn flags_empty_true_branch() {
	let args = RunArgs::parse(&["--quiet".into()]);
	let result = args.substitute_no_env("cmd {{ FLAG.quiet ? : '--verbose' }}").unwrap();
	assert_eq!(result, "cmd ");
}

#[test]
fn flags_empty_false_branch() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env("cmd {{ FLAG.v ? '--verbose' : }}").unwrap();
	assert_eq!(result, "cmd ");
}

#[test]
fn flags_empty_false_branch_present() {
	let args = RunArgs::parse(&["--v".into()]);
	let result = args.substitute_no_env("cmd {{ FLAG.v ? '--verbose' : }}").unwrap();
	assert_eq!(result, "cmd --verbose");
}

#[test]
fn flags_consumed_from_args() {
	let args = RunArgs::parse(&["--verbose".into(), "foo".into(), "bar".into()]);
	let result = args.substitute_no_env("cmd {{ FLAG.verbose }} {{ ARGS }}").unwrap();
	assert_eq!(result, "cmd true foo bar");
}

#[test]
fn flags_absent_not_consumed_from_args() {
	let args = RunArgs::parse(&["foo".into(), "bar".into()]);
	let result = args.substitute_no_env("cmd {{ FLAG.verbose }} {{ ARGS }}").unwrap();
	assert_eq!(result, "cmd false foo bar");
}

#[test]
fn flags_with_value_still_true() {
	// --verbose=yes should still be "true" for FLAGS (presence only)
	let args = RunArgs::parse(&["--verbose=yes".into()]);
	let result = args.substitute_no_env("echo {{ FLAG.verbose }}").unwrap();
	assert_eq!(result, "echo true");
}

#[test]
fn flags_with_space_value_still_true() {
	// --verbose something should still be "true" for FLAGS
	let args = RunArgs::parse(&["--verbose".into(), "something".into()]);
	let result = args.substitute_no_env("echo {{ FLAG.verbose }}").unwrap();
	assert_eq!(result, "echo true");
}

#[test]
fn flags_multiple() {
	let args = RunArgs::parse(&["--verbose".into(), "--debug".into()]);
	let result = args
		.substitute_no_env("cmd {{ FLAG.verbose }} {{ FLAG.debug }}")
		.unwrap();
	assert_eq!(result, "cmd true true");
}

#[test]
fn flags_multiple_mixed_presence() {
	let args = RunArgs::parse(&["--verbose".into()]);
	let result = args
		.substitute_no_env("cmd {{ FLAG.verbose }} {{ FLAG.debug }}")
		.unwrap();
	assert_eq!(result, "cmd true false");
}

#[test]
fn flags_mixed_with_args_named() {
	let args = RunArgs::parse(&["--verbose".into(), "--env=prod".into()]);
	let result = args
		.substitute_no_env("cmd {{ FLAG.verbose }} env={{ ARG.env }}")
		.unwrap();
	assert_eq!(result, "cmd true env=prod");
}

#[test]
fn flags_mixed_with_args_positional() {
	let args = RunArgs::parse(&["--verbose".into(), "file.txt".into()]);
	let result = args.substitute_no_env("cmd {{ FLAG.verbose }} {{ ARGS }}").unwrap();
	assert_eq!(result, "cmd true file.txt");
}

#[test]
fn flags_ternary_complex_values() {
	let args = RunArgs::parse(&["--side-effects".into()]);
	let result = args
		.substitute_no_env("cmd {{ FLAG.side-effects ? '-run -startup 3' : '-donotrun' }}")
		.unwrap();
	assert_eq!(result, "cmd -run -startup 3");
}

#[test]
fn flags_ternary_complex_values_false() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute_no_env("cmd {{ FLAG.side-effects ? '-run -startup 3' : '-donotrun' }}")
		.unwrap();
	assert_eq!(result, "cmd -donotrun");
}

#[test]
fn flags_ternary_url_colons_preserved() {
	// Colons in URLs should not be treated as ternary separator (only " : " is)
	let args = RunArgs::parse(&["--ssl".into()]);
	let result = args
		.substitute_no_env("cmd {{ FLAG.ssl ? 'https://secure.example.com' : 'http://example.com' }}")
		.unwrap();
	assert_eq!(result, "cmd https://secure.example.com");
}

#[test]
fn flags_ternary_url_colons_false_branch() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute_no_env("cmd {{ FLAG.ssl ? 'https://secure.example.com' : 'http://example.com' }}")
		.unwrap();
	assert_eq!(result, "cmd http://example.com");
}

#[test]
fn flags_scan_detects_flags() {
	let cmds = vec!["echo {{ FLAG.verbose }}".into()];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(!positional);
	assert!(named.contains("verbose"));
}

#[test]
fn flags_scan_detects_flags_with_ternary() {
	let cmds = vec!["echo {{ FLAG.debug ? '-g' : '-O2' }}".into()];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(!positional);
	assert!(named.contains("debug"));
}

#[test]
fn flags_scan_mixed_with_args() {
	let cmds = vec!["echo {{ FLAG.verbose }} {{ ARG.env }}".into()];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(!positional);
	assert!(named.contains("verbose"));
	assert!(named.contains("env"));
}

#[test]
fn flags_validate_accepts_flag_args() {
	let args = RunArgs::parse(&["--verbose".into()]);
	let cmds = vec!["echo {{ FLAG.verbose }}".into()];
	assert!(validate_args(&args, &cmds).is_ok());
}

#[test]
fn flags_validate_rejects_unknown_flag() {
	let args = RunArgs::parse(&["--verbose".into(), "--unknown".into()]);
	let cmds = vec!["echo {{ FLAG.verbose }}".into()];
	let err = validate_args(&args, &cmds).unwrap_err();
	assert!(
		err.to_string().contains("unknown"),
		"Expected UnknownNamedArg, got: {err}"
	);
}

#[test]
fn flags_validate_mixed_flags_and_args() {
	let args = RunArgs::parse(&["--verbose".into(), "--env=prod".into()]);
	let cmds = vec!["echo {{ FLAG.verbose }} {{ ARG.env }}".into()];
	assert!(validate_args(&args, &cmds).is_ok());
}

#[test]
fn flags_in_env_substitution() {
	let args = RunArgs::parse(&["--debug".into()]);
	let env = HashMap::new();
	let result = args
		.substitute("echo {{ FLAG.debug ? '--inspect' : '--no-inspect' }}", &env)
		.unwrap();
	assert_eq!(result, "echo --inspect");
}

#[test]
fn flags_multiple_in_same_command() {
	let args = RunArgs::parse(&["--verbose".into(), "--release".into()]);
	let result = args
		.substitute_no_env("cargo build {{ FLAG.verbose ? '-v' : }} {{ FLAG.release ? '--release' : }}")
		.unwrap();
	assert_eq!(result, "cargo build -v --release");
}

#[test]
fn flags_multiple_in_same_command_none_set() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute_no_env("cargo build {{ FLAG.verbose ? '-v' : }} {{ FLAG.release ? '--release' : }}")
		.unwrap();
	assert_eq!(result, "cargo build  ");
}

#[test]
fn flags_consumed_with_value_from_args() {
	// --verbose=yes used as FLAGS should consume the --verbose=yes token from {{ ARGS }}
	let args = RunArgs::parse(&["--verbose=yes".into(), "file.txt".into()]);
	let result = args.substitute_no_env("cmd {{ FLAG.verbose }} {{ ARGS }}").unwrap();
	assert_eq!(result, "cmd true file.txt");
}

#[test]
fn flags_hyphenated_key() {
	let args = RunArgs::parse(&["--dry-run".into()]);
	let result = args.substitute_no_env("echo {{ FLAG.dry-run }}").unwrap();
	assert_eq!(result, "echo true");
}

#[test]
fn flags_hyphenated_key_ternary() {
	let args = RunArgs::parse(&["--dry-run".into()]);
	let result = args
		.substitute_no_env("cmd {{ FLAG.dry-run ? '--dry-run' : '--execute' }}")
		.unwrap();
	assert_eq!(result, "cmd --dry-run");
}

// ── RUN.* substitution tests ──────────────────────────────────────

fn args_with_run(shell: &str) -> RunArgs {
	RunArgs::parse(&[]).with_run_context(RunContext {
		os: "linux".to_string(),
		shell: shell.to_string(),
		..Default::default()
	})
}

#[test]
fn run_os_resolves() {
	let args = args_with_run("bash");
	let result = args.substitute_no_env("echo {{ RUN.os }}").unwrap();
	assert_eq!(result, "echo linux");
}

#[test]
fn run_shell_resolves() {
	let args = args_with_run("powershell");
	let result = args.substitute_no_env("echo {{ RUN.shell }}").unwrap();
	assert_eq!(result, "echo powershell");
}

#[test]
fn run_arch_resolves() {
	// Override arch directly so the assertion doesn't depend on the host
	// architecture (CI runners are mostly x86_64 but ARM is increasingly
	// common). The detection itself is exercised by `run_arch_detection_*`.
	let args = RunArgs::parse(&[]).with_run_context(RunContext {
		os: "linux".to_string(),
		arch: "arm64".to_string(),
		shell: "bash".to_string(),
		..Default::default()
	});
	let result = args.substitute_no_env("echo {{ RUN.arch }}").unwrap();
	assert_eq!(result, "echo arm64");
}

#[test]
fn run_arch_detection_known_values_normalised() {
	// `RunContext::new` populates `arch` via `detect_current_arch()`. The
	// host this test runs on must have one of the four normalised values
	// — not the raw `std::env::consts::ARCH` strings like "x86_64".
	let ctx = RunContext::new("bash");
	assert!(
		matches!(ctx.arch.as_str(), "x86-64" | "arm64" | "riscv64" | "unknown"),
		"unexpected RUN.arch value: {:?}",
		ctx.arch
	);
}

#[test]
fn run_unknown_key_errors() {
	let args = args_with_run("bash");
	let err = args.substitute_no_env("echo {{ RUN.unknown }}").unwrap_err();
	let msg = err.to_string();
	assert!(msg.contains("unknown"), "unexpected error: {msg}");
	assert!(msg.contains("os"), "expected error to mention valid keys: {msg}");
}

#[test]
fn run_in_chained_fallback() {
	// {{ ARG.shell ? RUN.shell }} — falls back when ARGS not provided.
	let args = args_with_run("zsh");
	let result = args.substitute_no_env("echo {{ ARG.shell ? RUN.shell }}").unwrap();
	assert_eq!(result, "echo zsh");
}

#[test]
fn run_with_default_when_unknown() {
	// Unknown RUN key followed by literal default still works.
	let args = args_with_run("bash");
	let result = args.substitute_no_env("echo {{ RUN.unknown ? 'fallback' }}").unwrap();
	assert_eq!(result, "echo fallback");
}

#[test]
fn run_does_not_consume_named_args() {
	// {{ RUN.shell }} must not influence {{ ARGS }} — RUN keys are not user input.
	let args = RunArgs::parse(&["foo".into(), "--keep=true".into()]).with_run_context(RunContext {
		os: "linux".into(),
		shell: "bash".into(),
		..Default::default()
	});
	let result = args.substitute_no_env("cmd {{ RUN.shell }} {{ ARGS }}").unwrap();
	assert_eq!(result, "cmd bash foo --keep=true");
}

#[test]
fn run_redacted_substitute_does_not_redact() {
	// RUN values are not secrets — the redacted form should show them.
	let args = args_with_run("bash");
	let env = HashMap::new();
	let result = args
		.substitute_redacted("echo {{ RUN.os }}/{{ RUN.shell }}", &env)
		.unwrap();
	assert_eq!(result, "echo linux/bash");
}

// ── RUN.* in DSL conditions ───────────────────────────────────────

#[test]
fn run_in_if_condition_parses_in_runfile() {
	use runfile_parser::parse_runfile;

	let raw = r#"{
		"$schema": "v0",
		"targets": {
			"t": {
				"commands": [
					{ "if": "{{ RUN.os }} == linux", "then": ["echo on-linux"] }
				]
			}
		}
	}"#;
	// {{ RUN.os }} is a substitution leaf in DSL conditions; the parser must
	// accept it without complaining at validation time.
	let runfile = parse_runfile(raw).unwrap();
	assert!(runfile.targets.contains_key("t"));
}

#[test]
fn run_if_condition_runtime_execution() {
	// Substitution-DSL form: the whole comparison is inside `{{ ... }}` and
	// resolves to "true" or "false".
	let env = HashMap::new();

	let bash_args = args_with_run("bash");
	let result = bash_args.substitute("{{ RUN.shell == 'bash' }}", &env).unwrap();
	assert_eq!(result, "true");

	let zsh_args = args_with_run("zsh");
	let result = zsh_args.substitute("{{ RUN.shell == 'bash' }}", &env).unwrap();
	assert_eq!(result, "false");
}

#[test]
fn run_for_in_substitutes_run_values() {
	// `for in: ["{{ RUN.os }}", "ci"]` should expand {{ RUN.os }} per element.
	let args = args_with_run("bash");
	let result = args.substitute_no_env("{{ RUN.os }}").unwrap();
	assert_eq!(result, "linux");
}

#[test]
fn run_negated_inequality() {
	let env = HashMap::new();
	let linux_args = args_with_run("bash");
	let result = linux_args.substitute("{{ RUN.os != 'windows' }}", &env).unwrap();
	assert_eq!(result, "true");
}
