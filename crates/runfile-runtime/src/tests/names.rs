//! A file that names something nothing defines is refused before any of it
//! runs. The rules that decide what is in scope are the runner's, so the tests
//! at the end hold the runner to each one the check relies on: a rule the two
//! came to disagree about would refuse a file that runs, or pass one that fails.

use super::{Recorder, host_run, project, run_src};

#[test]
fn a_file_calling_a_function_that_does_not_exist_runs_nothing() {
	// The call sits in a branch that is never taken, below a command that would
	// have run: exactly the file that used to pass until the month it did not.
	let d = project(&[(
		"runfiles/t.run",
		"$ touch ran\nif false\n\tlet x = exists(\"a\")\nend\n",
	)]);
	let e = host_run(&d, "t").unwrap_err().to_string();
	assert!(e.contains("t.run: line 3: unknown function `exists`"), "{e}");
	assert!(e.contains("did you mean `directory_exists` or `file_exists`?"), "{e}");
	assert!(!d.path().join("ran").exists(), "nothing ran");
}

#[test]
fn every_name_that_does_not_resolve_is_reported_at_once() {
	let d = project(&[("runfiles/t.run", "print(a)\n$ true\nprint(b)\n")]);
	let e = host_run(&d, "t").unwrap_err().to_string();
	let lines: Vec<&str> = e.lines().collect();
	assert_eq!(lines.len(), 2, "one to a line: {e}");
	assert!(lines[0].ends_with("t.run: line 1: `a` is not defined"), "{e}");
	// Each line after the first carries the prefix the first is printed after.
	assert!(lines[1].contains("[runfile]") && lines[1].contains(" error: "), "{e}");
	assert!(lines[1].ends_with("t.run: line 3: `b` is not defined"), "{e}");
}

#[test]
fn a_shared_file_that_names_nothing_refuses_every_target_below_it() {
	let d = project(&[
		("runfiles/_shared.run", "let zone = nope(\"eu\")\n"),
		("runfiles/t.run", "$ touch ran\n"),
	]);
	let e = host_run(&d, "t").unwrap_err().to_string();
	assert!(e.contains("_shared.run: line 1: unknown function `nope`"), "{e}");
	assert!(!d.path().join("ran").exists(), "nothing ran");
}

#[test]
fn a_name_a_shared_file_binds_resolves_below_it() {
	let d = project(&[
		("runfiles/_shared.run", "let region = \"eu\"\n"),
		("runfiles/api/_shared.run", "let zone = concat(region, \"-1\")\n"),
		("runfiles/api/deploy.run", "$ echo {{ zone }} {{ region }} > out.txt\n"),
	]);
	host_run(&d, "api:deploy").unwrap();
	let out = std::fs::read_to_string(d.path().join("out.txt")).unwrap();
	assert_eq!(out.trim(), "eu-1 eu");
}

#[test]
fn every_target_in_this_repository_would_be_let_run() {
	// The repository's own files cannot rot into ones the runner refuses.
	let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
	let cat = runfile_discovery::discover(&root, None).unwrap();
	assert!(cat.targets.len() > 20, "found only {} targets", cat.targets.len());
	let host = crate::dispatch::Host::new(&cat);
	let refused: Vec<String> = cat
		.targets
		.values()
		.filter_map(|t| host.check(t).err().map(|e| format!("{}: {e}", t.name)))
		.collect();
	assert!(refused.is_empty(), "{}", refused.join("\n"));
}

// ---- the runner's rules, which the check follows

/// Whether the check has anything to say about `src`, on its own.
fn flagged(src: &str) -> bool {
	!runfile_lang::resolve::of_chain(&runfile_lang::parse(src).unwrap(), &[]).is_empty()
}

#[test]
fn a_loop_s_names_reach_the_first_line_of_its_body_when_run() {
	// A property at the top of a body used to be worked out once, before the
	// first item was bound, so `.workdir = dir` there was `dir is not defined`.
	// What it did is a line now, and a line runs on every pass.
	for src in [
		"for dir in [\"web\"]\n\tENV.D = dir\n\trun {{ ENV.D }}\nend\n",
		"parallel for dir in [\"web\"]\n\tENV.D = dir\n\trun {{ ENV.D }}\nend\n",
		"let dir = \"x\"\nfor dir in [\"web\"]\n\tENV.D = dir\n\trun {{ ENV.D }}\nend\n",
	] {
		let d = Recorder::default();
		run_src(src, &d).unwrap_or_else(|e| panic!("{src}: {e}"));
		assert_eq!(d.calls(), ["web"], "{src}");
		assert!(!flagged(src), "{src}");
	}
}

#[test]
fn a_let_outlives_its_block_and_a_pass_reads_what_the_last_one_bound_when_run_too() {
	let src = "if true\n\tlet v = \"x\"\nend\nrun {{ v }}\nfor i in [1, 2]\n\tif i == 2\n\t\trun {{ prev }}\n\tend\n\tlet prev = i\nend\n";
	let d = Recorder::default();
	run_src(src, &d).unwrap();
	assert_eq!(d.calls(), ["x", "1"]);
	assert!(!flagged(src));
}

#[test]
fn a_loop_s_names_and_a_branch_s_bindings_end_with_them_when_run_too() {
	for src in [
		"for n in [\"a\"]\n\trun {{ n }}\nend\nrun {{ n }}\n",
		"parallel do\n\tdo\n\t\tlet a = 1\n\tend\n\t$ true\nend\nrun {{ a }}\n",
		"parallel for i in [1]\n\tlet a = i\nend\nrun {{ a }}\n",
	] {
		let e = run_src(src, &Recorder::default()).unwrap_err().to_string();
		assert!(e.contains("is not defined"), "{src}: {e}");
		assert!(flagged(src), "{src}");
	}
}

#[test]
fn a_let_inside_a_block_of_a_shared_file_binds_for_the_target_as_it_runs() {
	// A shared file runs ahead of the target, so a binding on any path through
	// it is bound for the target, as one above in the file would be.
	let shared = "if true\n\tlet nested = 1\nend\nlet shown = 2\n";
	let target = runfile_lang::parse("$ echo {{ shown }} {{ nested }}\n").unwrap();
	let unresolved = runfile_lang::resolve::of_chain(&target, &[runfile_lang::parse(shared).unwrap()]);
	assert!(unresolved.is_empty(), "{unresolved:?}");
	let d = project(&[
		("runfiles/_shared.run", shared),
		("runfiles/t.run", "$ echo {{ shown }} {{ nested }}\n"),
	]);
	assert_eq!(host_run(&d, "t").unwrap(), ["echo 2 1"]);
}
