//! A file that fails every time it runs is refused before any of it does, and
//! the facts the check reads about a run -- which keys `RUN` has, which values
//! `RUN.os` and `RUN.arch` take -- are held to the runner that supplies them.

use super::{host_run, project};

#[test]
fn a_file_that_fails_every_time_it_runs_runs_nothing() {
	// In a branch that is never taken, below a command that would have run.
	let d = project(&[(
		"runfiles/t.run",
		"$ touch ran\nif FLAG.never\n\tsleep(ARG.seconds)\nend\n",
	)]);
	let e = host_run(&d, "t").unwrap_err().to_string();
	assert!(
		e.contains("t.run: line 3: `sleep` takes a number, and `ARG.seconds` is a string [wrong-type]"),
		"{e}"
	);
	assert!(!d.path().join("ran").exists(), "nothing ran");
}

#[test]
fn the_language_s_findings_and_the_shell_s_are_reported_together_in_order() {
	let d = project(&[(
		"runfiles/t.run",
		"$ echo it's\nif RUN.os == \"darwin\"\n\tprint(\"x\")\nend\n",
	)]);
	let e = host_run(&d, "t").unwrap_err().to_string();
	let lines: Vec<&str> = e.lines().collect();
	assert_eq!(lines.len(), 2, "{e}");
	assert!(lines[0].contains("line 1:") && lines[0].contains("[unclosed]"), "{e}");
	assert!(
		lines[1].contains("line 2:") && lines[1].contains("[never-equal]"),
		"{e}"
	);
}

#[test]
fn a_shared_file_that_runs_a_command_in_a_let_refuses_every_target_below_it() {
	let d = project(&[
		("runfiles/_shared.run", "let branch = $ git branch --show-current\n"),
		("runfiles/t.run", "$ touch ran\n"),
	]);
	let e = host_run(&d, "t").unwrap_err().to_string();
	assert!(
		e.contains("_shared.run: line 1:") && e.contains("[capture-position]"),
		"{e}"
	);
	assert!(!d.path().join("ran").exists(), "nothing ran");
}

#[test]
fn a_shared_let_that_runs_a_command_is_refused_by_the_runner_too() {
	// What the check says, the runner does: the fold has no process to run it.
	let d = project(&[
		("runfiles/_shared.run", "let branch = $ echo main\n"),
		("runfiles/t.run", "print(branch)\n"),
	]);
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let t = &cat.targets["t"];
	let shared = runfile_lang::parse("let branch = $ echo main\n").unwrap();
	let mut sc = runfile_lang::Scope::new();
	crate::dispatch::populate_run_context(&mut sc, t, &cat);
	let e = crate::run::fold_shared(&shared.body, &crate::props::Props::default(), &mut sc).unwrap_err();
	assert!(e.to_string().contains("capture"), "{e}");
}

#[test]
fn run_takes_exactly_the_platform_names_the_check_knows() {
	let oses: std::collections::BTreeSet<&str> = ["linux", "macos", "windows", "freebsd", "android", "ios"]
		.into_iter()
		.map(crate::dispatch::os_named)
		.collect();
	let known: std::collections::BTreeSet<&str> = runfile_lang::eval::OS_NAMES.iter().copied().collect();
	assert_eq!(oses, known);
	let arches: std::collections::BTreeSet<&str> = ["x86_64", "aarch64", "riscv64", "x86", "arm", "wasm32"]
		.into_iter()
		.map(crate::dispatch::arch_named)
		.collect();
	let known: std::collections::BTreeSet<&str> = runfile_lang::eval::ARCH_NAMES.iter().copied().collect();
	assert_eq!(arches, known);
}

#[test]
fn run_has_exactly_the_keys_the_check_knows() {
	let d = project(&[("runfiles/t.run", "print(RUN.os)\n")]);
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let mut sc = runfile_lang::Scope::new();
	crate::dispatch::populate_run_context(&mut sc, &cat.targets["t"], &cat);
	for key in sc.run.keys() {
		assert!(runfile_lang::eval::RUN_KEYS.contains(&key.as_str()), "`RUN.{key}`");
	}
	// `user` is the one that depends on the environment naming someone.
	for key in runfile_lang::eval::RUN_KEYS.iter().filter(|k| **k != "user") {
		assert!(sc.run.contains_key(*key), "`RUN.{key}`");
	}
}

#[test]
fn a_code_of_where_the_runner_cannot_run_it_fails_when_run() {
	// The check calls these a mistake because this is what running them does.
	for src in [
		"if code_of($ true)\n\tprint(\"x\")\nend\n",
		"match code_of($ true)\ncase \"0\"\n\tprint(\"x\")\nend\n",
		"if contains(\"a\", $ echo a)\n\tprint(\"x\")\nend\n",
	] {
		let e = super::run_src(src, &super::Recorder::default()).unwrap_err();
		assert!(e.to_string().contains("capture"), "{src}\n{e}");
	}
}
