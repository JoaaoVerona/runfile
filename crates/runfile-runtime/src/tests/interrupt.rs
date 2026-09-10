//! Ctrl+C. The predicate is injected, so a test interrupts one run without
//! raising a real signal -- which would take the test runner with it -- and
//! without touching any state another test can see.

use std::sync::atomic::{AtomicBool, Ordering};

use super::project;

/// Run a target, tripping the interrupt flag after `after` statements' worth of
/// checks. `after = 0` interrupts before anything runs.
fn run_interrupting_after(
	files: &[(&str, &str)],
	target: &str,
	after: usize,
) -> (Result<(), crate::RunError>, tempfile::TempDir) {
	let d = project(files);
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let seen = AtomicBool::new(false);
	let count = std::sync::atomic::AtomicUsize::new(0);
	let pred = move || {
		// Trip once the walker has asked `after` times, so a test can let some
		// of a target run before the interrupt lands.
		if count.fetch_add(1, Ordering::SeqCst) >= after {
			seen.store(true, Ordering::SeqCst);
		}
		seen.load(Ordering::SeqCst)
	};
	let mut h = crate::dispatch::Host::new(&cat);
	h.assume_yes = true;
	h.interrupted = Some(&pred);
	let out = h.run(target, &[]);
	h.cleanup_temps();
	(out, d)
}

#[test]
fn an_interrupted_run_stops_before_the_next_statement() {
	let files = &[("runfiles/t.run", "$ echo one > one.txt\n")];
	let (out, d) = run_interrupting_after(files, "t", 0);
	assert!(matches!(out, Err(crate::RunError::Interrupted)), "{out:?}");
	assert!(!d.path().join("one.txt").exists(), "nothing ran at all");
}

#[test]
fn what_already_ran_is_left_alone() {
	// One statement runs, then the interrupt lands before the second.
	let files = &[(
		"runfiles/t.run",
		"$ echo one > one.txt\nlet x = \"1\"\n$ echo two > two.txt\n",
	)];
	let (out, d) = run_interrupting_after(files, "t", 1);
	assert!(matches!(out, Err(crate::RunError::Interrupted)), "{out:?}");
	assert!(d.path().join("one.txt").exists(), "the first statement ran");
	assert!(!d.path().join("two.txt").exists(), "the third did not");
}

#[test]
fn ignore_errors_does_not_shrug_off_an_interrupt() {
	// A failure it may swallow; a Ctrl+C it may not.
	let files = &[(
		"runfiles/t.run",
		".ignore-errors = true\n$ echo one > one.txt\nlet x = \"1\"\n$ echo late > late.txt\n",
	)];
	let (out, d) = run_interrupting_after(files, "t", 1);
	assert!(matches!(out, Err(crate::RunError::Interrupted)), "{out:?}");
	assert!(!d.path().join("late.txt").exists());
}

#[test]
fn a_run_with_no_interrupt_is_unaffected() {
	let d = project(&[("runfiles/t.run", "$ echo hi > out.txt\n")]);
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let mut h = crate::dispatch::Host::new(&cat);
	h.assume_yes = true;
	// No predicate at all is the default, and must not look interrupted.
	h.run("t", &[]).unwrap();
	assert!(d.path().join("out.txt").exists());
}

#[test]
fn temp_files_are_removed_when_a_run_is_interrupted() {
	// The reason an interrupt is caught rather than left to the OS: a decoded
	// credential must not survive Ctrl+C.
	let files = &[(
		"runfiles/t.run",
		"let f = temp_file(\"secret\")\n$ echo {{ f }} > path.txt\nlet x = \"1\"\n$ true\n",
	)];
	let (out, d) = run_interrupting_after(files, "t", 2);
	assert!(matches!(out, Err(crate::RunError::Interrupted)), "{out:?}");
	let path = std::fs::read_to_string(d.path().join("path.txt"))
		.unwrap()
		.trim()
		.to_string();
	assert!(!path.is_empty());
	assert!(!std::path::Path::new(&path).exists(), "cleaned up on the way out");
}

#[test]
fn the_exit_code_is_the_one_a_shell_reports() {
	assert_eq!(crate::interrupt::EXIT_CODE, 130);
}

#[test]
fn an_empty_loop_body_still_notices_an_interrupt() {
	// The walker checks *between* statements, and a `loop` with an empty body
	// has none -- so without a check of its own at the top of each pass this
	// spins past every chance to stop, and this test hangs rather than fails.
	let files = &[("runfiles/t.run", "loop\nend\n")];
	let (out, _d) = run_interrupting_after(files, "t", 3);
	assert!(matches!(out, Err(crate::RunError::Interrupted)), "{out:?}");
}
