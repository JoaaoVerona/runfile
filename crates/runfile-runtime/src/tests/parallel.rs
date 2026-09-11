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
	// Forward slashes, because this goes into a *shell* line and a backslash
	// is an escape there: on Windows `touch C:\Users\…` makes one file with
	// the separators eaten out of its name. The language does not have this
	// problem -- `{{ p }}` quotes what it interpolates -- but this builds the
	// shell text by hand, so it has to do the quoting by hand too.
	let path = f.display().to_string().replace('\\', "/");
	let src = format!(".parallel\n$ false\n\nlet a = 1\n$ touch {path}\n");
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

// ---- labelled output
//
// Several children write at once, so each line says which branch it came from.
// The label comes from the target name or the `exec` header rather than from
// shell text, which is what the old implementation had to guess from.

#[test]
fn a_branch_label_comes_from_the_exec_header() {
	assert_eq!(
		crate::run::exec_label_for_test(Some("python3 -c"), "print(1)"),
		"python3"
	);
	assert_eq!(crate::run::exec_label_for_test(Some("docker compose"), "up"), "docker");
}

#[test]
fn a_shell_header_is_never_the_label() {
	// Every `$` branch runs the default shell, so naming them all `bash` would
	// tell nobody anything. The command inside is the useful name.
	assert_eq!(
		crate::run::exec_label_for_test(Some("bash"), "cargo build\nmore"),
		"cargo"
	);
	assert_eq!(crate::run::exec_label_for_test(None, "pnpm install"), "pnpm");
}

#[test]
fn a_label_skips_blank_and_comment_lines() {
	assert_eq!(crate::run::exec_label_for_test(None, "\n# why\ncargo test"), "cargo");
}

#[test]
fn an_empty_body_still_has_a_label() {
	assert_eq!(crate::run::exec_label_for_test(None, ""), "exec");
}

#[test]
fn every_nested_block_form_inside_a_fan_out_applies_its_own_workdir() {
	// A leaf is rendered where it is collected, so a block-scoped property the
	// collecting walk does not layer is layered never: it parsed, it was
	// accepted, and the branch then ran in the anchor with no word about it.
	// `do`, `if`, `for` and `match` all reach the same arm.
	let bodies = [
		"do\n\t.workdir = \"sub\"\n\t$ test -f marker\nend\n",
		"if true\n\t.workdir = \"sub\"\n\t$ test -f marker\nend\n",
		"for x in [\"a\"]\n\t.workdir = \"sub\"\n\t$ test -f marker\nend\n",
		"match \"a\"\ncase \"a\"\n\t.workdir = \"sub\"\n\t$ test -f marker\nend\n",
	];
	for body in bodies {
		let src = format!(".parallel\n{body}");
		let d = project(&[("runfiles/w.run", src.as_str())]);
		std::fs::create_dir(d.path().join("sub")).unwrap();
		std::fs::write(d.path().join("sub").join("marker"), "").unwrap();
		host_run(&d, "w").unwrap_or_else(|e| panic!("{body} ran in the anchor, not in `sub`: {e}"));
	}
}

#[test]
fn a_nested_block_inside_a_fan_out_applies_its_own_env_and_shell() {
	// `.env` and `.shell` reach the branch by different routes -- one through
	// the leaf's environment, one through the command it is spawned with --
	// and both were read off the batch's properties rather than the block's.
	let d = Recorder::default();
	run_src(
		".parallel\ndo\n\t.env.GREET = \"hello\"\n\t$ test \"$GREET\" = hello\nend\n",
		&d,
	)
	.expect("the branch saw its own block's `.env`");
	// `.shell` is header-only, so the path this half is about is the header one
	// reaching a leaf collected inside a nested block.
	run_src(".parallel\n.shell = \"sh\"\ndo\n\t$ test \"${0##*/}\" = sh\nend\n", &d)
		.expect("the branch was spawned with the file's `.shell`");
}

#[test]
fn a_nested_block_inside_a_fan_out_reads_its_own_env_file() {
	// `.env-file` appends, which is how `with_block_env` tells it has to
	// rebuild -- so a collecting walk that never called it left every branch
	// with the target's environment.
	let d = project(&[(
		"runfiles/e.run",
		".parallel\ndo\n\t.env-file = \".env.extra\"\n\t$ test \"$FROMFILE\" = yes\nend\n",
	)]);
	std::fs::write(d.path().join(".env.extra"), "FROMFILE=yes\n").unwrap();
	host_run(&d, "e").expect("the branch saw its own block's `.env-file`");
}

#[test]
fn ignore_errors_is_per_branch_in_a_fan_out() {
	// Block-scoped, so the block that named it is the one forgiven and a
	// sibling that did not is still a failure. Both halves matter: read off
	// the batch's own properties, one of them is wrong whichever way it is set.
	let d = Recorder::default();
	run_src(".parallel\ndo\n\t.ignore-errors\n\t$ false\nend\n", &d).expect("the block that named it is forgiven");
	let e = run_src(
		".parallel\ndo\n\t.ignore-errors\n\t$ false\nend\n\ndo\n\t$ false\nend\n",
		&d,
	)
	.unwrap_err();
	assert!(
		e.to_string().contains("status 1"),
		"the sibling that did not name it still fails the run: {e}"
	);
}

#[test]
fn a_capture_as_a_condition_inside_a_fan_out_runs_it() {
	// A fan-out's preamble is what decides *what* fans out, so it is answered
	// before the batch exists -- and through the process host, the way the
	// sequential walk answers it. Reaching the pure evaluator instead made
	// `if $ cmd` an error inside a `.parallel` block and a condition outside
	// one, for the same line.
	let d = Recorder::default();
	run_src(".parallel\nif $ true\n\trun taken\nelse\n\trun other\nend\n", &d).unwrap();
	assert_eq!(d.calls(), vec!["taken"], "a zero exit is the true branch");

	let d = Recorder::default();
	run_src(".parallel\nif $ false\n\trun taken\nelse\n\trun other\nend\n", &d).unwrap();
	assert_eq!(d.calls(), vec!["other"], "and a non-zero one is the false branch");
}

#[test]
fn a_match_on_a_capture_inside_a_fan_out_dispatches_on_the_exit_code() {
	// `subject_of` already ran the command, so this arm was the one that was
	// right. It is pinned here so the three that were not cannot drift back
	// away from it.
	let d = Recorder::default();
	run_src(
		".parallel\nmatch $ sh -c 'exit 3'\ncase \"0\"\n\trun zero\ncase \"3\"\n\trun three\ndefault\n\trun other\nend\n",
		&d,
	)
	.unwrap();
	assert_eq!(d.calls(), vec!["three"], "the case is the command's exit code");
}

#[test]
fn a_fan_out_can_be_built_from_a_captures_output() {
	// The list is what says how many branches there are, so it has to be
	// answered before the batch is built. `for f in lines($ git ls-files)` is
	// the shape this makes possible.
	let d = Recorder::default();
	run_src(
		".parallel\nfor f in lines($ printf \"a\\nb\\n\")\n\trun {{ f }}:build\nend\n",
		&d,
	)
	.unwrap();
	let mut calls = d.calls();
	calls.sort();
	assert_eq!(
		calls,
		vec!["a:build", "b:build"],
		"one branch per line the command wrote"
	);
}

#[test]
fn a_bare_capture_call_inside_a_fan_out_runs_it() {
	// `code_of($ …)` as a statement of its own: run it, ignore the status. It
	// sits in the preamble, so it runs where it is written rather than joining
	// the batch.
	let f = std::env::temp_dir().join("runfile-parallel-bare-capture");
	let _ = std::fs::remove_file(&f);
	// Forward slashes, and for the reason the completion test above gives:
	// this builds shell text by hand, so it does the quoting by hand too.
	let path = f.display().to_string().replace('\\', "/");
	let d = Recorder::default();
	run_src(&format!(".parallel\ncode_of($ touch {path})\nrun after\n"), &d).unwrap();
	assert!(f.exists(), "the bare capture ran during collection");
	assert_eq!(d.calls(), vec!["after"], "and the batch still fanned out");
	let _ = std::fs::remove_file(&f);
}

#[test]
fn a_capture_in_a_fan_outs_preamble_runs_under_its_own_blocks_properties() {
	// Both halves meet here: the preamble runs through the process host, and
	// it runs under the properties of the block it is written in rather than
	// the batch's -- `value_of` and `cond_of` take the `props` the collecting
	// walk is holding, which is the nested block's.
	let d = project(&[(
		"runfiles/w.run",
		".parallel\ndo\n\t.workdir = \"sub\"\n\tif $ test -f marker\n\t\t$ true\n\telse\n\t\t$ false\n\tend\nend\n",
	)]);
	std::fs::create_dir(d.path().join("sub")).unwrap();
	std::fs::write(d.path().join("sub").join("marker"), "").unwrap();
	host_run(&d, "w").expect("the condition's own command ran in `sub`");
}
