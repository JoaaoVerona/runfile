//! Ctrl+C behavior, driven end-to-end against the compiled `run` binary.
//!
//! Ctrl+C in a terminal delivers SIGINT to the whole foreground process group —
//! the running child AND `run` itself. `run` must survive it long enough to
//! skip the rest of the target and execute its `when: failure` / `when: always`
//! cleanup blocks, then exit 130. These tests reproduce that exactly: the child
//! is spawned into its own process group and signalled with `kill(-pgid)`.
//!
//! Unix-only. The Windows equivalent needs a console-attached process and a
//! `GenerateConsoleCtrlEvent` call, which a `cargo test` harness has no console
//! for; the handler shape is shared with the Unix one (see
//! `runfile-executor/src/interrupt.rs`).
#![cfg(unix)]

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Long enough that the signal always lands mid-command, short enough that a
/// regression fails the test in seconds instead of hanging the suite.
const SLEEP_SECS: u32 = 10;

/// Spawn `run <target>` in `dir` as its OWN process group leader (so `pgid ==
/// pid` and we can signal the group the way a terminal would), with a hermetic
/// environment.
fn spawn(dir: &Path, target: &str) -> Child {
	let home = dir.join("_home");
	std::fs::create_dir_all(&home).unwrap();
	Command::new(env!("CARGO_BIN_EXE_run"))
		.arg(target)
		.current_dir(dir)
		.env("HOME", &home)
		.env("XDG_CONFIG_HOME", home.join(".config"))
		.env("APPDATA", home.join("AppData"))
		.env("RUNFILE_CONFIG_DIR", home.join("runfile"))
		.env_remove("RUNFILE_TARGET")
		.env_remove("RUNFILE_ENV_FILE_TARGET")
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.process_group(0)
		.spawn()
		.expect("run binary spawns")
}

/// Block until `path` exists, or panic after `secs`. Targets touch marker files
/// instead of printing, so the test never has to race a pipe.
fn await_marker(path: &Path, secs: u64) {
	let deadline = Instant::now() + Duration::from_secs(secs);
	while Instant::now() < deadline {
		if path.exists() {
			return;
		}
		std::thread::sleep(Duration::from_millis(25));
	}
	panic!("marker {} never appeared", path.display());
}

/// Deliver SIGINT to the child's process group — what a terminal does on Ctrl+C.
fn interrupt_group(child: &Child) {
	let pgid = child.id() as libc::pid_t;
	assert_eq!(unsafe { libc::kill(-pgid, libc::SIGINT) }, 0, "kill(-pgid, SIGINT)");
}

/// Write `runfile` into a fresh temp dir, run `target`, Ctrl+C it once the
/// `started` marker appears, and return `(exit_code, dir)`.
fn interrupt_run(runfile: &str, target: &str) -> (Option<i32>, tempfile::TempDir) {
	let dir = tempfile::tempdir().unwrap();
	std::fs::write(dir.path().join("Runfile.json"), runfile).unwrap();

	let mut child = spawn(dir.path(), target);
	await_marker(&dir.path().join("started"), 30);
	interrupt_group(&child);

	let status = child.wait().expect("child is reaped");
	(status.code(), dir)
}

fn marker(dir: &tempfile::TempDir, name: &str) -> PathBuf {
	dir.path().join(name)
}

/// The bug this suite exists for: a Ctrl+C used to kill `run` outright, so
/// `when: always` (and `when: failure`) blocks never ran.
#[test]
fn sigint_runs_failure_and_always_blocks() {
	let runfile = format!(
		r#"{{
			"$schema": "x",
			"targets": {{
				"t": {{
					"commands": [
						"touch started; sleep {SLEEP_SECS}",
						"touch after-success",
						{{ "when": "failure", "commands": "touch on-failure" }},
						{{ "when": "always", "commands": "touch on-always" }}
					]
				}}
			}}
		}}"#
	);
	let (code, dir) = interrupt_run(&runfile, "t");

	assert!(marker(&dir, "on-always").exists(), "`when: always` block must run");
	assert!(marker(&dir, "on-failure").exists(), "`when: failure` block must run");
	assert!(
		!marker(&dir, "after-success").exists(),
		"steps after the interrupted one must be skipped"
	);
	assert_eq!(code, Some(130), "an interrupted run exits 128 + SIGINT");
}

/// `ignoreErrors` forgives a command that failed; it has no business forgiving
/// "the user asked to stop". Without the exemption the walker would march
/// through every remaining command after the Ctrl+C.
#[test]
fn sigint_is_not_swallowed_by_ignore_errors() {
	let runfile = format!(
		r#"{{
			"$schema": "x",
			"targets": {{
				"t": {{
					"ignoreErrors": true,
					"commands": [
						"touch started; sleep {SLEEP_SECS}",
						"touch after-success",
						{{ "when": "always", "commands": "touch on-always" }}
					]
				}}
			}}
		}}"#
	);
	let (code, dir) = interrupt_run(&runfile, "t");

	assert!(marker(&dir, "on-always").exists(), "`when: always` block must run");
	assert!(
		!marker(&dir, "after-success").exists(),
		"`ignoreErrors` must not resume the run after an interrupt"
	);
	assert_eq!(code, Some(130), "an interrupted run never reports success");
}

/// A `for` loop must stop iterating on Ctrl+C rather than draining the
/// remaining values — with `ignoreErrors` it would otherwise keep launching the
/// very commands the user asked to stop.
#[test]
fn sigint_stops_for_loop_iterations() {
	let runfile = format!(
		r#"{{
			"$schema": "x",
			"targets": {{
				"t": {{
					"commands": [
						{{
							"for": "i",
							"in": ["1", "2", "3"],
							"ignoreErrors": true,
							"do": "touch iter-{{{{ VAR.i }}}}; touch started; sleep {SLEEP_SECS}"
						}},
						{{ "when": "always", "commands": "touch on-always" }}
					]
				}}
			}}
		}}"#
	);
	let (code, dir) = interrupt_run(&runfile, "t");

	assert!(marker(&dir, "iter-1").exists(), "the first iteration ran");
	assert!(!marker(&dir, "iter-2").exists(), "later iterations must not start");
	assert!(marker(&dir, "on-always").exists(), "`when: always` block must run");
	assert_eq!(code, Some(130));
}

/// The common teardown shape: cleanup delegated to another target. The
/// dispatched target must run its own body in full — the abort has to be
/// suppressed for everything reachable from a cleanup block, not just for the
/// block's own direct steps.
#[test]
fn sigint_runs_target_call_inside_always_block() {
	let runfile = format!(
		r#"{{
			"$schema": "x",
			"targets": {{
				"t": {{
					"commands": [
						"touch started; sleep {SLEEP_SECS}",
						{{ "when": "always", "commands": "@_teardown" }}
					]
				}},
				"_teardown": {{
					"commands": [
						"touch teardown-1",
						"touch teardown-2",
						{{ "for": "n", "in": ["a", "b"], "do": "touch teardown-{{{{ VAR.n }}}}" }}
					]
				}}
			}}
		}}"#
	);
	let (code, dir) = interrupt_run(&runfile, "t");

	for name in ["teardown-1", "teardown-2", "teardown-a", "teardown-b"] {
		assert!(marker(&dir, name).exists(), "cleanup target step `{name}` must run");
	}
	assert_eq!(code, Some(130));
}

/// A `parallel: true` batch partitions its leaves by `when`; the always
/// partition has to survive the interrupt too.
#[test]
fn sigint_runs_always_block_in_parallel_target() {
	let runfile = format!(
		r#"{{
			"$schema": "x",
			"targets": {{
				"t": {{
					"parallel": true,
					"commands": [
						"touch started; sleep {SLEEP_SECS}",
						{{ "when": "always", "commands": "touch on-always" }}
					]
				}}
			}}
		}}"#
	);
	let (code, dir) = interrupt_run(&runfile, "t");

	assert!(marker(&dir, "on-always").exists(), "`when: always` block must run");
	assert_eq!(code, Some(130));
}

/// Every target on the stack gets to clean up: the interrupted dependency runs
/// its own `when: always` block, then the caller runs its own.
#[test]
fn sigint_runs_always_blocks_up_the_dependency_chain() {
	let runfile = format!(
		r#"{{
			"$schema": "x",
			"targets": {{
				"t": {{
					"commands": ["@_inner", {{ "when": "always", "commands": "touch outer-always" }}]
				}},
				"_inner": {{
					"commands": [
						"touch started; sleep {SLEEP_SECS}",
						{{ "when": "always", "commands": "touch inner-always" }}
					]
				}}
			}}
		}}"#
	);
	let (code, dir) = interrupt_run(&runfile, "t");

	assert!(
		marker(&dir, "inner-always").exists(),
		"the dependency's cleanup must run"
	);
	assert!(marker(&dir, "outer-always").exists(), "the caller's cleanup must run");
	assert_eq!(code, Some(130));
}

/// A second Ctrl+C is the escape hatch: an interrupted run must never become
/// unkillable because a cleanup block hangs.
///
/// The cleanup block here IGNORES SIGINT (`trap '' INT`, inherited by its
/// `sleep`), so the signal alone cannot end it — only `run`'s own second-Ctrl+C
/// exit can. `forceShell: "sh"` pins the trap syntax to a POSIX shell.
#[test]
fn second_sigint_exits_immediately() {
	let runfile = format!(
		r#"{{
			"$schema": "x",
			"targets": {{
				"t": {{
					"forceShell": "sh",
					"commands": [
						"touch started; sleep {SLEEP_SECS}",
						{{
							"when": "always",
							"commands": "trap '' INT; touch cleanup-started; sleep {SLEEP_SECS}"
						}}
					]
				}}
			}}
		}}"#
	);
	let dir = tempfile::tempdir().unwrap();
	std::fs::write(dir.path().join("Runfile.json"), &runfile).unwrap();

	let mut child = spawn(dir.path(), "t");
	let pgid = child.id() as libc::pid_t;
	await_marker(&dir.path().join("started"), 30);
	interrupt_group(&child);

	// Wait for the signal-proof cleanup block to actually be running, then
	// interrupt again and time how long `run` takes to give up.
	await_marker(&dir.path().join("cleanup-started"), 30);
	let second = Instant::now();
	interrupt_group(&child);
	let status = child.wait().expect("child is reaped");
	let waited = second.elapsed();

	// Leave no orphaned `sh`/`sleep` behind now that `run` is gone.
	unsafe { libc::kill(-pgid, libc::SIGKILL) };

	assert!(
		waited < Duration::from_secs(SLEEP_SECS as u64 / 2),
		"the second Ctrl+C must exit without waiting out the cleanup block (waited {waited:?})"
	);
	assert_eq!(status.code(), Some(130));
}

/// Guard against over-correcting: an uninterrupted run must be untouched by any
/// of this — normal steps run, and the exit code is still the target's own.
#[test]
fn uninterrupted_run_is_unaffected() {
	let runfile = r#"{
		"$schema": "x",
		"targets": {
			"t": {
				"commands": [
					"touch started",
					"touch after-success",
					{ "when": "failure", "commands": "touch on-failure" },
					{ "when": "always", "commands": "touch on-always" }
				]
			}
		}
	}"#;
	let dir = tempfile::tempdir().unwrap();
	std::fs::write(dir.path().join("Runfile.json"), runfile).unwrap();

	let status = spawn(dir.path(), "t").wait().expect("child is reaped");

	assert_eq!(status.code(), Some(0));
	assert!(dir.path().join("after-success").exists());
	assert!(dir.path().join("on-always").exists());
	assert!(!dir.path().join("on-failure").exists());
}
