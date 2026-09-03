//! `.parallel`: what fans out, what stays ordered, and what happens on failure.

use super::{Recorder, host_run, project, run_src};
use std::time::Instant;

#[test]
fn branches_actually_overlap_in_time() {
	// Three half-second sleeps sequentially would take ~1.5s. Asserting on
	// wall-clock is the only way to prove concurrency rather than assume it.
	let d = Recorder::default();
	let src = ".parallel\n$ sleep 0.4\n\nlet a = 1\n$ sleep 0.4\n\nlet b = 2\n$ sleep 0.4\n";
	let start = Instant::now();
	run_src(src, &d).expect("all three branches run");
	let elapsed = start.elapsed().as_secs_f64();
	assert!(elapsed < 1.0, "took {elapsed:.2}s; sequential would be ~1.2s");
}

#[test]
fn bindings_evaluate_in_order_before_the_fan_out() {
	// The rule: lets resolve in source order, then executable leaves fan out.
	// If `n` were resolved on the thread, the later binding could win.
	let d = Recorder::default();
	run_src(
		".parallel\nlet n = \"1\"\n$ test {{ n }} = 1\nlet n = \"2\"\n$ test {{ n }} = 2\n",
		&d,
	)
	.expect("each leaf captured the binding visible where it was written");
}

#[test]
fn control_flow_inside_a_parallel_block_contributes_its_leaves() {
	let d = Recorder::default();
	run_src(".parallel\nfor x in [\"a\", \"b\"]\n\trun {{ x }}:build\nend\n", &d).unwrap();
	let mut calls = d.calls();
	calls.sort();
	assert_eq!(calls, vec!["a:build", "b:build"], "the loop expands into the batch");
}

#[test]
fn every_branch_completes_even_when_one_fails() {
	// Stopping the others would leave a half-started fan-out behind.
	let f = std::env::temp_dir().join("runfile-parallel-completion");
	let _ = std::fs::remove_file(&f);
	let d = Recorder::default();
	let src = format!(".parallel\n$ false\n\nlet a = 1\n$ touch {}\n", f.display());
	let e = run_src(&src, &d).unwrap_err();
	assert!(e.to_string().contains("status 1"), "the failure still surfaces: {e}");
	assert!(f.exists(), "the sibling branch ran to completion anyway");
	let _ = std::fs::remove_file(&f);
}

#[test]
fn ignore_errors_swallows_a_failed_branch() {
	let d = Recorder::default();
	run_src(".parallel\n.ignore-errors\n$ false\n\nlet a = 1\n$ true\n", &d).expect("failure is contained");
}

#[test]
fn a_parallel_fan_out_over_namespaces_dispatches_each_subproject() {
	// The shape every aggregator in the corpus uses.
	let d = project(&[
		(
			"runfiles/dev.run",
			".parallel\nfor n in RUN.namespaces\n\trun {{ n }}:dev\nend\n",
		),
		("api/runfiles/dev.run", "$ true\n"),
		("web/runfiles/dev.run", "$ true\n"),
	]);
	let trace = host_run(&d, "dev").expect("fans out");
	assert_eq!(trace.len(), 2);
}

#[test]
fn a_cycle_is_still_caught_across_a_parallel_branch() {
	// Each branch carries its own chain, so one branch cannot look like a
	// cycle to another -- but a real self-call must still be caught.
	let d = project(&[
		("runfiles/a.run", ".parallel\nrun b\nrun c\n"),
		("runfiles/b.run", "$ true\n"),
		("runfiles/c.run", "run a\n"),
	]);
	let e = host_run(&d, "a").unwrap_err();
	assert!(e.to_string().contains("a -> c -> a"), "{e}");
}

#[test]
fn sibling_branches_do_not_see_each_others_chains() {
	// Both branches dispatch the same target. With a shared stack the second
	// would be reported as a cycle; with per-path chains neither is.
	let d = project(&[
		("runfiles/a.run", ".parallel\nrun x\nrun y\n"),
		("runfiles/x.run", "run shared\n"),
		("runfiles/y.run", "run shared\n"),
		("runfiles/shared.run", "$ true\n"),
	]);
	host_run(&d, "a").expect("the same target on two paths is not a cycle");
}
