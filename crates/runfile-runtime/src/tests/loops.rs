//! `while`, `until`, `loop`, and the two ways out of one.
//!
//! Effects are `run` statements against a `Recorder`, so what a loop did is a
//! list of names rather than a pile of spawned processes.

use super::{Recorder, project, run_src};

#[test]
fn while_runs_again_while_its_condition_holds() {
	let d = Recorder::default();
	run_src("let n = 0\n\nwhile n < 3\n\tn = n + 1\n\trun step\nend\n", &d).unwrap();
	assert_eq!(d.calls(), ["step", "step", "step"]);
}

#[test]
fn a_while_whose_condition_never_held_runs_nothing() {
	// The condition is asked *before* each pass, so this is the difference
	// between `while` and a do-while nobody asked for.
	let d = Recorder::default();
	run_src("while false\n\trun step\nend\n", &d).unwrap();
	assert!(d.calls().is_empty(), "{:?}", d.calls());
}

#[test]
fn until_is_while_with_the_question_reversed() {
	let d = Recorder::default();
	run_src("let n = 0\n\nuntil n == 2\n\tn = n + 1\n\trun step\nend\n", &d).unwrap();
	assert_eq!(d.calls(), ["step", "step"]);
}

#[test]
fn loop_runs_until_a_break() {
	let d = Recorder::default();
	run_src(
		"let n = 0\n\nloop\n\tn = n + 1\n\trun step\n\n\tif n == 2\n\t\tbreak\n\tend\nend\n",
		&d,
	)
	.unwrap();
	assert_eq!(d.calls(), ["step", "step"]);
}

#[test]
fn continue_skips_the_rest_of_the_pass_and_break_ends_the_loop() {
	let d = Recorder::default();
	run_src(
		"for n in range(5)\n\
		 \tif n == 1\n\
		 \t\tcontinue\n\
		 \tend\n\n\
		 \tif n == 3\n\
		 \t\tbreak\n\
		 \tend\n\n\
		 \trun step-{{ n }}\n\
		 end\n",
		&d,
	)
	.unwrap();
	assert_eq!(d.calls(), ["step-0", "step-2"]);
}

#[test]
fn break_leaves_only_the_innermost_loop() {
	let d = Recorder::default();
	run_src(
		"for a in [\"x\", \"y\"]\n\tfor b in [\"1\", \"2\"]\n\t\trun {{ a }}{{ b }}\n\t\tbreak\n\tend\nend\n",
		&d,
	)
	.unwrap();
	assert_eq!(d.calls(), ["x1", "y1"], "the outer loop keeps going");
}

#[test]
fn a_break_travels_out_through_a_retry_rather_than_reading_as_a_failed_attempt() {
	// A `retry` runs its body again *while it fails*, and a `break` leaves as
	// an error -- so without `is_stop` covering it the retry would swallow the
	// break and run the body five times over.
	let d = Recorder::default();
	run_src(
		"for n in [\"a\", \"b\"]\n\tretry 5\n\t\trun {{ n }}\n\t\tbreak\n\tend\nend\n",
		&d,
	)
	.unwrap();
	assert_eq!(d.calls(), ["a"]);
}

#[test]
fn ignore_errors_does_not_forgive_a_break() {
	// A forgiven `break` is a statement that plainly did nothing, found out
	// much later by whoever wondered why the loop kept going.
	let d = Recorder::default();
	run_src(
		"for n in [\"a\", \"b\", \"c\"]\n\t.ignore-errors\n\n\trun {{ n }}\n\tbreak\nend\n",
		&d,
	)
	.unwrap();
	assert_eq!(d.calls(), ["a"]);
}

#[test]
fn a_previewed_loop_walks_its_body_once() {
	// `--dry-run` performs none of the effects the condition is asking about,
	// so the answer it would get never changes: this ran forever before the
	// body was capped at one pass.
	let p = project(&[("runfiles/watch.run", "loop\n\t$ echo forever\nend\n")]);
	let cat = runfile_discovery::discover(p.path(), None).unwrap();
	let mut h = crate::dispatch::Host::new(&cat);
	h.dry_run = true;
	h.run("watch", &[]).expect("previews");
	let trace = h.trace.lock().expect("trace").clone();
	assert_eq!(trace.len(), 1, "{trace:?}");
}

#[test]
fn a_conditional_loop_is_refused_inside_a_parallel_block() {
	// A fan-out collects every branch before any of them runs, and a `while`
	// has nothing to collect until its body has run at least once.
	for src in [
		".parallel\n\nwhile true\n\trun a\nend\n",
		".parallel\n\nloop\n\trun a\nend\n",
		".parallel\n\nfor n in [\"a\"]\n\tbreak\nend\n",
	] {
		let d = Recorder::default();
		let e = run_src(src, &d).expect_err("a fan-out cannot hold one");
		assert!(e.to_string().contains(".parallel"), "{src}: {e}");
	}
}

// ---- unpacking

#[test]
fn a_for_unpacks_each_item_across_its_names() {
	let d = Recorder::default();
	run_src(
		"for name, owner in [[\"a\", \"1\"], [\"b\", \"2\"]]\n\trun {{ name }}-{{ owner }}\nend\n",
		&d,
	)
	.unwrap();
	assert_eq!(d.calls(), ["a-1", "b-2"]);
}

#[test]
fn an_underscore_holds_a_position_without_binding_it() {
	let d = Recorder::default();
	run_src("let a, _, c = [\"x\", \"y\", \"z\"]\nrun {{ a }}{{ c }}\n", &d).unwrap();
	assert_eq!(d.calls(), ["xz"]);

	let d = Recorder::default();
	let e = run_src("let a, _ = [\"x\", \"y\"]\nrun {{ _ }}\n", &d).expect_err("`_` binds nothing");
	assert!(e.to_string().contains("not defined"), "{e}");
}

#[test]
fn unpacking_more_names_than_the_list_has_is_an_error_but_extra_elements_are_not() {
	let d = Recorder::default();
	let e = run_src("let a, b, c = [1, 2]\n", &d).expect_err("three names, two values");
	assert!(e.to_string().contains("3 names to unpack, but the list has 2"), "{e}");

	// Extra elements are simply not asked for.
	run_src("let a, b = [1, 2, 3]\nrun {{ b }}\n", &Recorder::default()).unwrap();
}

#[test]
fn one_name_binds_a_list_whole() {
	// Unpacking is what the commas ask for; nothing else changes because a
	// value happened to be a list.
	let d = Recorder::default();
	run_src("let xs = [\"a\", \"b\"]\nrun {{ xs }}\n", &d).unwrap();
	assert_eq!(d.calls(), ["a b"], "one argument per item, not the first item");
}

#[test]
fn a_reassignment_unpacks_the_same_way_and_reads_its_value_first() {
	let d = Recorder::default();
	run_src(
		"let a = \"1\"\nlet b = \"2\"\n\na, b = [b, a]\nrun {{ a }}{{ b }}\n",
		&d,
	)
	.unwrap();
	assert_eq!(d.calls(), ["21"]);
}
