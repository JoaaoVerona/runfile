//! Behavioral integration tests for the preparation-target gate, driving the
//! actual `run` binary as a subprocess.
//!
//! Each test runs in an isolated temp dir with `HOME` / `XDG_CONFIG_HOME` /
//! `APPDATA` / `RUNFILE_CONFIG_DIR` pointed at an empty dir so the prepare
//! `state.json` is scratch. Every CI-detection variable is cleared so the gate
//! stays *active* even when this suite itself runs in CI (otherwise `is_ci()`
//! would short-circuit the gate and the "blocks" assertions would be moot).

use std::path::Path;
use std::process::{Command, Output};

/// Every environment variable `ci_detect::is_ci()` consults. Cleared by default
/// so the gate is deterministically active.
const CI_VARS: &[&str] = &[
	"CI",
	"GITHUB_ACTIONS",
	"GITLAB_CI",
	"CIRCLECI",
	"TRAVIS",
	"BUILDKITE",
	"JENKINS_URL",
	"TF_BUILD",
	"TEAMCITY_VERSION",
	"BITBUCKET_BUILD_NUMBER",
];

/// Run the compiled `run` binary in `dir`, hermetic, with CI detection disabled
/// and `extra_env` applied on top.
fn run_env(dir: &Path, args: &[&str], extra_env: &[(&str, &str)]) -> Output {
	let home = dir.join("_home");
	std::fs::create_dir_all(&home).unwrap();
	let mut cmd = Command::new(env!("CARGO_BIN_EXE_run"));
	cmd.args(args)
		.current_dir(dir)
		.env("HOME", &home)
		.env("XDG_CONFIG_HOME", home.join(".config"))
		.env("APPDATA", home.join("AppData"))
		.env("RUNFILE_CONFIG_DIR", home.join("runfile"))
		.env_remove("RUNFILE_TARGET")
		.env_remove("RUNFILE_ENV_FILE_TARGET")
		.env_remove("RUNFILE_SKIP_PREPARE");
	for var in CI_VARS {
		cmd.env_remove(var);
	}
	for (k, v) in extra_env {
		cmd.env(k, v);
	}
	cmd.output().expect("run binary executes")
}

fn run(dir: &Path, args: &[&str]) -> Output {
	run_env(dir, args, &[])
}

fn write(path: &Path, contents: &str) {
	if let Some(parent) = path.parent() {
		std::fs::create_dir_all(parent).unwrap();
	}
	std::fs::write(path, contents).unwrap();
}

fn stdout_of(out: &Output) -> String {
	String::from_utf8(out.stdout.clone()).expect("stdout is UTF-8")
}
fn stderr_of(out: &Output) -> String {
	String::from_utf8(out.stderr.clone()).expect("stderr is UTF-8")
}

const GLOBAL_PREPARE: &str = r#"{
	"$schema": "x",
	"globals": { "prepare": "@setup" },
	"targets": {
		"setup": { "commands": "echo SETUP-RAN" },
		"build": { "commands": "echo BUILD-RAN" }
	}
}"#;

#[test]
fn gate_blocks_until_setup_runs_then_passes() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write(&root.join("Runfile.json"), GLOBAL_PREPARE);

	// 1) build is blocked — setup never ran.
	let blocked = run(root, &["build"]);
	assert!(!blocked.status.success(), "build should be blocked");
	let err = stderr_of(&blocked);
	assert!(err.contains("run setup"), "should point at setup: {err}");
	assert!(err.contains("never run"), "should say never run: {err}");
	assert!(!stdout_of(&blocked).contains("BUILD-RAN"), "build must not execute");

	// 2) running setup satisfies the requirement.
	let setup = run(root, &["setup"]);
	assert!(setup.status.success(), "setup should run: {}", stderr_of(&setup));
	assert!(stdout_of(&setup).contains("SETUP-RAN"));

	// 3) build now passes.
	let ok = run(root, &["build"]);
	assert!(ok.status.success(), "build should pass: {}", stderr_of(&ok));
	assert!(stdout_of(&ok).contains("BUILD-RAN"));
}

#[test]
fn editing_setup_commands_re_triggers_gate() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write(&root.join("Runfile.json"), GLOBAL_PREPARE);

	assert!(run(root, &["setup"]).status.success());
	assert!(run(root, &["build"]).status.success(), "build passes after setup");

	// Change setup's commands → the recorded hash no longer matches.
	write(
		&root.join("Runfile.json"),
		&GLOBAL_PREPARE.replace("echo SETUP-RAN", "echo SETUP-CHANGED"),
	);

	let blocked = run(root, &["build"]);
	assert!(!blocked.status.success(), "changed setup should re-block build");
	assert!(
		stderr_of(&blocked).contains("changed since you last ran it"),
		"should report a change: {}",
		stderr_of(&blocked)
	);
}

#[test]
fn target_prepare_is_additive_with_global() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write(
		&root.join("Runfile.json"),
		r#"{
			"$schema": "x",
			"globals": { "prepare": "@setup" },
			"targets": {
				"setup": { "commands": "echo SETUP" },
				"setup-tests": { "commands": "echo SETUP-TESTS" },
				"test": { "commands": "echo TEST-RAN", "prepare": "@setup-tests" }
			}
		}"#,
	);

	// setup done, but test still needs setup-tests.
	assert!(run(root, &["setup"]).status.success());
	let blocked = run(root, &["test"]);
	assert!(!blocked.status.success(), "test still needs setup-tests");
	let err = stderr_of(&blocked);
	assert!(err.contains("run setup-tests"), "{err}");
	assert!(!err.contains("run setup    "), "setup already satisfied: {err}");

	// After setup-tests, test passes.
	assert!(run(root, &["setup-tests"]).status.success());
	let ok = run(root, &["test"]);
	assert!(ok.status.success(), "test should pass: {}", stderr_of(&ok));
	assert!(stdout_of(&ok).contains("TEST-RAN"));
}

#[test]
fn ci_detection_skips_the_gate() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write(&root.join("Runfile.json"), GLOBAL_PREPARE);

	let out = run_env(root, &["build"], &[("CI", "1")]);
	assert!(out.status.success(), "CI should bypass the gate: {}", stderr_of(&out));
	assert!(stdout_of(&out).contains("BUILD-RAN"));
}

#[test]
fn skip_env_var_bypasses_the_gate() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write(&root.join("Runfile.json"), GLOBAL_PREPARE);

	let out = run_env(root, &["build"], &[("RUNFILE_SKIP_PREPARE", "1")]);
	assert!(out.status.success(), "skip var should bypass: {}", stderr_of(&out));
	assert!(stdout_of(&out).contains("BUILD-RAN"));
}

#[test]
fn dry_run_is_not_gated() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write(&root.join("Runfile.json"), GLOBAL_PREPARE);

	let out = run(root, &["--dry-run", "build"]);
	assert!(out.status.success(), "dry-run should not gate: {}", stderr_of(&out));
	assert!(stdout_of(&out).contains("echo BUILD-RAN"));
}

#[test]
fn missing_prepare_target_errors_clearly() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write(
		&root.join("Runfile.json"),
		r#"{ "$schema": "x", "targets": { "build": { "commands": "echo b", "prepare": "@nope" } } }"#,
	);

	let out = run(root, &["build"]);
	assert!(!out.status.success());
	assert!(
		stderr_of(&out).contains("does not exist"),
		"should report missing prepare target: {}",
		stderr_of(&out)
	);
}
