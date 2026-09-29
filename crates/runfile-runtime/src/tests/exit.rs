//! `exit()` ends the target it is written in. Its status is how that target
//! went: 0 hands back to whatever ran it, and anything else fails the line that
//! ran it -- which is what `$ run <target>` makes of the same `exit()`.

use super::{host_run, project};
use crate::RunError;

/// Preview `target`, handing back how it ended and what it would have run.
fn preview(d: &tempfile::TempDir, target: &str) -> (Result<(), RunError>, Vec<String>) {
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let mut h = crate::dispatch::Host::new(&cat);
	h.dry_run = true;
	let out = h.run(target, &[]);
	let trace = h.trace.lock().expect("trace").clone();
	(out, trace)
}

#[test]
fn an_exit_0_hands_back_to_the_target_that_ran_it() {
	// It used to end the whole run: the caller's remaining lines were skipped,
	// and the run reported success -- a build that never happened, called done.
	let d = project(&[
		("runfiles/_setup.run", "exit()\n$ touch setup-went-on\n"),
		("runfiles/build.run", "run _setup\n$ touch built\n"),
	]);
	host_run(&d, "build").expect("a target that ends with 0 went well");
	assert!(d.path().join("built").exists(), "the caller carried on");
	assert!(!d.path().join("setup-went-on").exists(), "the target itself stopped");
}

#[test]
fn any_other_status_fails_the_line_that_ran_the_target() {
	let d = project(&[
		("runfiles/_check.run", "exit(3)\n"),
		("runfiles/build.run", "run _check\n$ touch built\n"),
	]);
	let e = host_run(&d, "build").expect_err("3 is a target that did not go well");
	assert!(
		matches!(&e, RunError::Exited { target, code: 3 } if target == "_check"),
		"{e:?}"
	);
	assert!(!e.is_stop(), "to the caller it is a failure, not an instruction: {e}");
	assert!(!d.path().join("built").exists(), "nothing forgave it");
}

#[test]
fn a_caller_forgives_and_retries_that_failure_as_it_would_any_other() {
	// As at a `$ run` of the same target: the caller did not say `exit()`, so
	// it has no instruction of its own to carry out.
	let d = project(&[
		("runfiles/_flaky.run", "$ echo x >> attempts\nexit(3)\n"),
		(
			"runfiles/forgives.run",
			".ignore-errors\n\nrun _flaky\n$ touch went-on\n",
		),
		("runfiles/retries.run", "retry 3\n\trun _flaky\nend\n"),
	]);
	host_run(&d, "forgives").expect("forgiven");
	assert!(d.path().join("went-on").exists());

	let attempts = d.path().join("attempts");
	std::fs::remove_file(&attempts).unwrap();
	let e = host_run(&d, "retries").expect_err("it never went well");
	assert_eq!(e.exit_code(), Some(3), "the last attempt's status: {e}");
	let tried = std::fs::read_to_string(&attempts).unwrap().lines().count();
	assert_eq!(tried, 3, "every attempt was made");
}

#[test]
fn a_status_nothing_handles_keeps_its_number_on_the_way_up() {
	// Every `run` line it passes fails, none of them forgives it, and so the
	// run ends with it -- while a `code_of` above them all scores it.
	let d = project(&[
		("runfiles/c.run", "exit(3)\n"),
		("runfiles/b.run", "run c\n"),
		("runfiles/a.run", "run b\n"),
		("runfiles/score.run", "let s = code_of(run a)\n$ test {{ s }} = 3\n"),
	]);
	assert_eq!(host_run(&d, "a").unwrap_err().exit_code(), Some(3));
	host_run(&d, "score").expect("scored rather than stopped");
}

#[test]
fn a_status_is_a_byte_as_a_processes_is() {
	// What `$ run` would report of the same `exit()`: the shell's 255 for -1,
	// and for 256 a 0 -- a target that went well.
	let d = project(&[
		("runfiles/minus.run", "exit(-1)\n"),
		("runfiles/wraps.run", "exit(256)\n"),
		(
			"runfiles/score.run",
			"let m = code_of(run minus)\n$ test {{ m }} = 255\nrun wraps\n$ touch went-on\n",
		),
	]);
	host_run(&d, "score").expect("both are statuses");
	assert!(d.path().join("went-on").exists(), "256 is 0");
}

#[test]
fn an_exit_in_a_shared_let_ends_only_the_target_it_was_evaluated_for() {
	// The `_shared.run` chain is folded into the target, so its `let`s are
	// part of the target, and so is an `exit()` in one.
	let d = project(&[
		("runfiles/sub/_shared.run", "let stop = exit(0)\n"),
		("runfiles/sub/quits.run", "$ touch unreached\n"),
		("runfiles/top.run", "run sub:quits\n$ touch went-on\n"),
	]);
	host_run(&d, "top").expect("0 is a target that went well");
	assert!(d.path().join("went-on").exists());
	assert!(!d.path().join("unreached").exists());
}

#[test]
fn an_exit_in_a_property_value_is_an_exit_like_any_other() {
	// Evaluating the value raised it, and it used to be wrapped as the
	// property's own error: not a stop, so `.ignore-errors` forgave it in a
	// block, and not a status, so a header's was reported as an error.
	let d = project(&[
		(
			"runfiles/nested.run",
			".ignore-errors\n\nif true\n\t.env.X = exit(3)\n\n\t$ touch inside\nend\n\n$ touch after\n",
		),
		("runfiles/header.run", ".env.X = exit(0)\n\n$ touch unreached\n"),
		("runfiles/top.run", "run header\n$ touch went-on\n"),
	]);
	let e = host_run(&d, "nested").expect_err("nothing inside the target forgives it");
	assert_eq!(e.exit_code(), Some(3), "{e}");
	assert!(!d.path().join("after").exists());

	host_run(&d, "top").expect("0 went well");
	assert!(d.path().join("went-on").exists());
	assert!(!d.path().join("unreached").exists());
}

#[test]
fn a_branch_that_runs_a_target_ending_with_0_went_well() {
	let d = project(&[
		("runfiles/quits.run", "exit()\n"),
		(
			"runfiles/fan.run",
			"parallel do\n\trun quits\n\t$ true\nend\n$ touch went-on\n",
		),
	]);
	host_run(&d, "fan").expect("a branch is ordinary code");
	assert!(d.path().join("went-on").exists());
}

#[test]
fn a_preview_keeps_what_a_target_ran_before_it_stopped() {
	// A target that stopped part-way still ran everything above the stop, and
	// that belongs where the call appeared. It used to be dropped, so a preview
	// showed a caller carrying on after a call that seemed to do nothing.
	let d = project(&[
		("runfiles/done.run", "$ echo in-done\nexit()\n$ echo unreached\n"),
		("runfiles/quits.run", "$ echo in-quits\nexit(3)\n"),
		(
			"runfiles/top.run",
			"$ echo first\nlet s = code_of(run quits)\nrun done\n$ echo last\n",
		),
	]);
	let (out, trace) = preview(&d, "top");
	out.expect("previews");
	assert_eq!(trace, ["echo first", "echo in-quits", "echo in-done", "echo last"]);

	// The target the run started with, too, whichever way it stopped.
	let (out, trace) = preview(&d, "done");
	out.expect("0 went well");
	assert_eq!(trace, ["echo in-done"]);
	let (out, trace) = preview(&d, "quits");
	assert_eq!(out.unwrap_err().exit_code(), Some(3));
	assert_eq!(trace, ["echo in-quits"]);
}

#[test]
fn a_preview_that_fails_keeps_what_came_before_the_failure() {
	let d = project(&[("runfiles/boom.run", "$ echo before\nerror(\"boom\")\n$ echo after\n")]);
	let (out, trace) = preview(&d, "boom");
	assert!(out.unwrap_err().to_string().contains("boom"));
	assert_eq!(trace, ["echo before"]);
}
