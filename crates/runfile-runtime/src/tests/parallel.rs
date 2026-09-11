//! `parallel do` and `parallel for`: what is a branch, what stays in order,
//! what a branch can see, and what happens when one fails.

use super::{Recorder, host_run, project, run_src};
use std::time::Instant;

/// A path a shell line can hold: forward slashes, since a backslash is an
/// escape there -- on Windows `touch C:\Users\…` makes one file with the
/// separators eaten out of its name. `{{ p }}` would quote it, but these build
/// the shell text by hand.
fn shell_path(p: &std::path::Path) -> String {
	p.display().to_string().replace('\\', "/")
}

// ---- what a branch is

#[test]
fn branches_actually_overlap_in_time() {
	// Three sleeps in turn would take ~1.2s. Wall-clock is the only way to
	// prove concurrency rather than assume it. The three `$` lines are
	// adjacent on purpose: under `.parallel` they folded into one process,
	// one branch, run one after the other.
	let d = Recorder::default();
	let start = Instant::now();
	run_src("parallel do\n\t$ sleep 0.4\n\t$ sleep 0.4\n\t$ sleep 0.4\nend\n", &d).expect("all three run");
	let elapsed = start.elapsed().as_secs_f64();
	assert!(elapsed < 1.0, "took {elapsed:.2}s; one after the other would be ~1.2s");
}

#[test]
fn a_call_is_a_branch_like_any_other() {
	// `.parallel` ran every call first, in order, before anything was parallel
	// -- so a `sleep` held up the whole block, and the prints came after it.
	let d = Recorder::default();
	let start = Instant::now();
	run_src("parallel do\n\tsleep(0.4)\n\tsleep(0.4)\n\tsleep(0.4)\nend\n", &d).unwrap();
	let elapsed = start.elapsed().as_secs_f64();
	assert!(elapsed < 1.0, "took {elapsed:.2}s: the sleeps ran in turn");
}

#[test]
fn a_block_inside_parallel_do_is_one_branch_and_runs_in_order() {
	// Each statement is a branch; what is inside one is ordinary code. A
	// `for` there is one branch, its iterations in turn -- `.parallel`
	// flattened it into the batch, so they ran at once.
	let f = std::env::temp_dir().join(format!("runfile-parallel-order-{}", std::process::id()));
	let _ = std::fs::remove_file(&f);
	let path = shell_path(&f);
	let d = Recorder::default();
	let src = format!(
		"parallel do\n\tfor n in [\"1\", \"2\", \"3\"]\n\t\t$ sleep 0.1; printf {{{{ n }}}} >> {path}\n\tend\n\t$ true\nend\n"
	);
	run_src(&src, &d).unwrap();
	assert_eq!(std::fs::read_to_string(&f).unwrap(), "123", "the loop ran in order");
	let _ = std::fs::remove_file(&f);
}

#[test]
fn parallel_for_runs_every_iteration_at_once_and_each_body_in_order() {
	// `.parallel` put every command of every iteration in one flat batch, so a
	// service's `push` could finish before its own `build`. Here `mark` is
	// only ever written after `step`, per iteration, while the iterations
	// themselves overlap.
	let dir = std::env::temp_dir().join(format!("runfile-parallel-for-{}", std::process::id()));
	let _ = std::fs::remove_dir_all(&dir);
	std::fs::create_dir_all(&dir).unwrap();
	let path = shell_path(&dir);
	let d = Recorder::default();
	let src = format!(
		"parallel for s in [\"a\", \"b\", \"c\"]\n\t$ sleep 0.4; touch {path}/{{{{ s }}}}.step\n\t$ test -e {path}/{{{{ s }}}}.step && touch {path}/{{{{ s }}}}.mark\nend\n"
	);
	let start = Instant::now();
	run_src(&src, &d).expect("each body ran in order");
	let elapsed = start.elapsed().as_secs_f64();
	assert!(elapsed < 1.0, "took {elapsed:.2}s: the iterations ran in turn");
	for s in ["a", "b", "c"] {
		assert!(
			dir.join(format!("{s}.mark")).exists(),
			"{s}: its second step saw its first"
		);
	}
	let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn nested_parallel_blocks_fan_out_again() {
	// Only what is scoped with `parallel` is concurrent: a `parallel do`
	// inside a `parallel for` makes each iteration's statements branches too.
	let d = Recorder::default();
	let start = Instant::now();
	run_src(
		"parallel for s in [\"a\", \"b\"]\n\tparallel do\n\t\t$ sleep 0.4\n\t\t$ sleep 0.4\n\tend\nend\n",
		&d,
	)
	.unwrap();
	let elapsed = start.elapsed().as_secs_f64();
	assert!(elapsed < 0.75, "took {elapsed:.2}s: four sleeps, two at a time at best");
}

// ---- what a branch can see

#[test]
fn a_branch_sees_what_was_bound_before_the_block() {
	let d = Recorder::default();
	run_src(
		"let who = \"x\"\nparallel do\n\t$ test {{ who }} = x\n\t$ test {{ who }} = x\nend\n",
		&d,
	)
	.expect("both branches read it");
}

#[test]
fn what_a_branch_binds_stays_in_it() {
	// Each branch works on a copy of the scope, so nothing it binds reaches
	// its siblings or the code after the block. The parser refuses assigning
	// a name from outside, which would be lost; a `let` of its own is fine,
	// and is gone once the block ends.
	let d = Recorder::default();
	let e = run_src("parallel for n in [1, 2]\n\tlet inside = n\nend\nprint(inside)\n", &d).unwrap_err();
	assert!(e.to_string().contains("not defined"), "{e}");
}

#[test]
fn each_iteration_has_its_own_loop_variable() {
	// Bound into each branch's own copy, so no iteration sees another's value.
	let d = Recorder::default();
	run_src(
		"parallel for n in [\"1\", \"2\", \"3\"]\n\t$ sleep 0.1\n\t$ test {{ n }} = {{ n }}\nend\n",
		&d,
	)
	.unwrap();
	let d = Recorder::default();
	run_src("parallel for n in [\"a\", \"b\"]\n\trun {{ n }}:build\nend\n", &d).unwrap();
	let mut calls = d.calls();
	calls.sort();
	assert_eq!(calls, ["a:build", "b:build"]);
}

#[test]
fn continue_ends_one_iteration_of_a_parallel_for() {
	let d = Recorder::default();
	run_src(
		"parallel for n in [\"a\", \"b\", \"c\"]\n\tif n == \"b\"\n\t\tcontinue\n\tend\n\trun {{ n }}\nend\n",
		&d,
	)
	.unwrap();
	let mut calls = d.calls();
	calls.sort();
	assert_eq!(calls, ["a", "c"], "`b` ended early; the others did not");
}

#[test]
fn a_branch_can_hold_what_the_old_fan_out_refused() {
	// `.parallel` had to know every command before any ran, so it refused
	// `retry` and every conditional loop. A branch is ordinary code.
	let d = Recorder::default();
	run_src(
		"parallel do\n\tretry 2\n\t\trun r\n\tend\n\tfor n in [1]\n\t\twhile false\n\t\t\trun never\n\t\tend\n\tend\nend\n",
		&d,
	)
	.expect("a retry and a while inside branches");
	assert_eq!(d.calls(), ["r"]);
}

// ---- failure

#[test]
fn every_branch_completes_even_when_one_fails() {
	// Stopping the others would leave a half-started set of services behind.
	let f = std::env::temp_dir().join(format!("runfile-parallel-completion-{}", std::process::id()));
	let _ = std::fs::remove_file(&f);
	let d = Recorder::default();
	let src = format!("parallel do\n\t$ false\n\t$ sleep 0.3; touch {}\nend\n", shell_path(&f));
	let e = run_src(&src, &d).unwrap_err();
	assert!(e.to_string().contains("status 1"), "the failure still surfaces: {e}");
	assert!(f.exists(), "the sibling ran to completion first");
	let _ = std::fs::remove_file(&f);
}

#[test]
fn a_stop_is_reported_ahead_of_a_failure() {
	// Ctrl+C reaches every branch, and the first to fail may be a command the
	// signal killed; the run was interrupted, and should say so. `exit()` is
	// the stop a test can raise.
	let d = Recorder::default();
	let e = run_src("parallel do\n\t$ false\n\texit(3)\nend\n", &d).unwrap_err();
	assert_eq!(e.exit_code(), Some(3), "{e}");
}

#[test]
fn ignore_errors_on_the_block_forgives_each_branch() {
	let d = Recorder::default();
	run_src("parallel do\n\t.ignore-errors\n\n\t$ false\n\t$ true\nend\n", &d).expect("contained");
}

#[test]
fn ignore_errors_inside_one_branch_forgives_that_branch_only() {
	let d = Recorder::default();
	run_src("parallel do\n\tdo\n\t\t.ignore-errors\n\n\t\t$ false\n\tend\nend\n", &d)
		.expect("the block that named it is forgiven");
	let e = run_src(
		"parallel do\n\tdo\n\t\t.ignore-errors\n\n\t\t$ true\n\tend\n\t$ false\nend\n",
		&d,
	)
	.unwrap_err();
	assert!(e.to_string().contains("status 1"), "a sibling is not: {e}");
}

// ---- properties

#[test]
fn a_property_on_the_block_is_every_branchs() {
	let d = project(&[(
		"runfiles/w.run",
		"parallel do\n\t.workdir = \"sub\"\n\n\t$ test -f marker\n\t$ test -f marker\nend\n",
	)]);
	std::fs::create_dir(d.path().join("sub")).unwrap();
	std::fs::write(d.path().join("sub/marker"), "").unwrap();
	host_run(&d, "w").expect("both branches ran in `sub`");
}

#[test]
fn a_property_between_branches_is_the_ones_below_it() {
	// Properties are applied where they are written, and branches are set up
	// in order before any starts.
	let d = project(&[(
		"runfiles/w.run",
		"parallel do\n\t$ test ! -f marker\n\t.workdir = \"sub\"\n\t$ test -f marker\nend\n",
	)]);
	std::fs::create_dir(d.path().join("sub")).unwrap();
	std::fs::write(d.path().join("sub/marker"), "").unwrap();
	host_run(&d, "w").expect("the first ran in the anchor, the second in `sub`");
}

#[test]
fn a_block_inside_a_branch_applies_its_own_properties() {
	for body in ["do", "if true"] {
		let src = format!("parallel do\n\t{body}\n\t\t.workdir = \"sub\"\n\n\t\t$ test -f marker\n\tend\nend\n");
		let d = project(&[("runfiles/w.run", src.as_str())]);
		std::fs::create_dir(d.path().join("sub")).unwrap();
		std::fs::write(d.path().join("sub/marker"), "").unwrap();
		host_run(&d, "w").unwrap_or_else(|e| panic!("{body} ran in the anchor, not in `sub`: {e}"));
	}
	let d = Recorder::default();
	run_src(
		"parallel do\n\tdo\n\t\t.env.GREET = \"hello\"\n\n\t\t$ test \"$GREET\" = hello\n\tend\nend\n",
		&d,
	)
	.expect("the branch saw its own block's `.env`");
}

#[test]
fn a_block_inside_a_branch_reads_its_own_env_file() {
	let d = project(&[(
		"runfiles/e.run",
		"parallel do\n\tdo\n\t\t.env-file = \".env.extra\"\n\n\t\t$ test \"$FROMFILE\" = yes\n\tend\nend\n",
	)]);
	std::fs::write(d.path().join(".env.extra"), "FROMFILE=yes\n").unwrap();
	host_run(&d, "e").expect("the branch saw its own block's `.env-file`");
}

#[test]
fn a_parallel_for_list_is_worked_out_under_the_properties_around_it() {
	// The list is worked out once, before any iteration starts, and a capture
	// in it runs where the enclosing block says -- here, in `sub`.
	let d = project(&[(
		"runfiles/w.run",
		"do\n\t.workdir = \"sub\"\n\n\tparallel for f in lines($ ls)\n\t\t$ test -f {{ f }}\n\tend\nend\n",
	)]);
	std::fs::create_dir(d.path().join("sub")).unwrap();
	std::fs::write(d.path().join("sub/one"), "").unwrap();
	std::fs::write(d.path().join("sub/two"), "").unwrap();
	host_run(&d, "w").expect("the capture listed `sub`, and each branch found its file there");
}

// ---- dispatch

#[test]
fn a_parallel_for_over_namespaces_dispatches_each_subproject() {
	// The shape every aggregator in the corpus uses.
	let d = project(&[
		(
			"runfiles/dev.run",
			"parallel for n in RUN.namespaces\n\trun {{ n }}:dev\nend\n",
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
		("runfiles/a.run", "parallel do\n\trun b\n\trun c\nend\n"),
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
		("runfiles/a.run", "parallel do\n\trun x\n\trun y\nend\n"),
		("runfiles/x.run", "run shared\n"),
		("runfiles/y.run", "run shared\n"),
		("runfiles/shared.run", "$ true\n"),
	]);
	host_run(&d, "a").expect("the same target on two paths is not a cycle");
}

// ---- labels
//
// Several branches write at once, so each line says which one it came from.

fn labels(src: &str) -> Vec<String> {
	crate::run::labels_for_test(src)
}

#[test]
fn a_branch_is_labelled_by_what_it_runs() {
	// A dispatch is its target, a command its first word -- the `exec` header
	// when there is one -- a call its function, and a block its keyword and
	// line, since nothing shorter tells two `if`s apart.
	assert_eq!(
		labels(
			"parallel do\n\trun api:dev\n\t$ pnpm install\n\texec python3 -c\n\t\tprint(1)\n\tend\n\tsleep(1)\n\tif true\n\t\t$ true\n\tend\nend\n"
		),
		["api:dev", "pnpm", "python3", "sleep", "if:8"]
	);
}

#[test]
fn siblings_are_told_apart_by_as_many_words_as_it_takes() {
	// `cargo` twice would say nothing about which line came from which.
	assert_eq!(
		labels("parallel do\n\t$ cargo test\n\t$ cargo clippy -- -D warnings\n\t$ npm run dev\nend\n"),
		["cargo test", "cargo clippy", "npm"]
	);
	assert_eq!(
		labels("parallel do\n\trun build --target=a\n\trun build --target=b\nend\n"),
		["build --target=a", "build --target=b"]
	);
}

#[test]
fn siblings_no_words_tell_apart_say_where_they_are() {
	// The same call twice, or the same command: only the line differs.
	assert_eq!(
		labels("parallel do\n\tprint(\"a\")\n\t$ echo b\n\tprint(\"c\")\n\t$ make\n\t$ make\nend\n"),
		["print:2", "echo", "print:4", "make:5", "make:6"]
	);
}

#[test]
fn a_label_stops_at_the_first_interpolation() {
	// Rendering would run what is interpolated a second time, so the words stop
	// there -- and a word the interpolation runs into is only half of one.
	assert_eq!(
		labels(
			"parallel do\n\t$ docker compose -f {{ a }} up\n\t$ docker compose -f {{ b }} down\n\t$ echo x{{ a }}\nend\n"
		),
		["docker:2", "docker:3", "echo"]
	);
	assert_eq!(labels("parallel do\n\trun {{ ns }}:check\nend\n"), ["run"]);
}

#[test]
fn a_shell_header_is_never_the_label() {
	// Every `$` branch runs the default shell, so naming them all `bash` would
	// tell nobody anything. The command inside is the useful name, found past
	// blank and comment lines.
	assert_eq!(
		labels("parallel do\n\texec bash\n\n\t\t# why\n\t\tcargo build\n\t\tmore\n\tend\nend\n"),
		["cargo"]
	);
	assert_eq!(labels("parallel do\n\texec sh\n\tend\nend\n"), ["exec"]);
}

#[test]
fn a_label_reads_past_a_continuation() {
	assert_eq!(
		labels("parallel do\n\t$ cargo test \\\n\t\t--all\n\t$ cargo test \\\n\t\t--doc\nend\n"),
		["cargo test --all", "cargo test --doc"]
	);
}

#[test]
fn a_nested_parallel_block_has_no_label_of_its_own() {
	// Its branches are named instead: `web`, not `parallel:2/web`.
	assert_eq!(
		labels("parallel do\n\tparallel for n in [\"a\"]\n\t\t$ true\n\tend\n\t$ true\nend\n"),
		["", "true"]
	);
}
