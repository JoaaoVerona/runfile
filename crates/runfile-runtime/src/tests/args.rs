//! How a target's command line is read, and what it refuses.

use std::sync::Mutex;

use super::project;

/// Run a target and collect what it printed and what it warned.
fn run_with(files: &[(&str, &str)], target: &str, args: &[&str]) -> (Vec<String>, Vec<String>) {
	let (trace, warned) = try_run(files, target, args).expect("ran");
	(trace, warned)
}

/// The same, for a run that is expected to be refused.
fn refused(files: &[(&str, &str)], target: &str, args: &[&str]) -> String {
	try_run(files, target, args).expect_err("refused").to_string()
}

fn try_run(
	files: &[(&str, &str)],
	target: &str,
	args: &[&str],
) -> Result<(Vec<String>, Vec<String>), crate::run::RunError> {
	let d = project(files);
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let warnings: Mutex<Vec<String>> = Mutex::new(Vec::new());
	let warn = |m: &str| warnings.lock().unwrap().push(m.to_string());
	let mut h = crate::dispatch::Host::new(&cat);
	h.assume_yes = true;
	h.dry_run = true;
	h.warn = Some(&warn);
	let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
	h.run(target, &args)?;
	let trace = h.trace.lock().unwrap().clone();
	let warned = warnings.into_inner().unwrap();
	Ok((trace, warned))
}

const WRAP: &str = "$ tool {{ ARGS }}\n";

#[test]
fn everything_after_a_double_dash_is_positional_as_typed() {
	let (trace, _) = run_with(&[("runfiles/w.run", WRAP)], "w", &["--", "s3api", "--bucket", "x"]);
	assert_eq!(trace, ["tool s3api --bucket x"], "flags survive, in order");
}

#[test]
fn the_double_dash_itself_is_consumed() {
	let (trace, _) = run_with(&[("runfiles/w.run", WRAP)], "w", &["--", "a"]);
	assert_eq!(trace, ["tool a"]);
}

#[test]
fn before_the_double_dash_parsing_is_normal() {
	let src = "$ tool {{ ARG.env }} {{ ARGS }}\n";
	let (trace, warned) = run_with(&[("runfiles/w.run", src)], "w", &["--env=prod", "--", "--verbose"]);
	assert_eq!(trace, ["tool prod --verbose"]);
	assert!(warned.is_empty(), "{warned:?}");
}

#[test]
fn a_key_value_after_the_double_dash_is_not_an_argument() {
	// `--x=1` past the separator belongs to the wrapped command.
	let src = "$ tool {{ ARG.x ? \"unset\" }} {{ ARGS }}\n";
	let (trace, _) = run_with(&[("runfiles/w.run", src)], "w", &["--", "--x=1"]);
	assert_eq!(trace, ["tool unset --x=1"]);
}

#[test]
fn a_flag_the_target_never_reads_is_refused() {
	// Forgetting `--` drops the flag silently. It warned while the check was
	// textual guesswork; walked from the tree it is exact, so it refuses.
	let e = refused(&[("runfiles/w.run", WRAP)], "w", &["s3api", "--bucket", "x"]);
	assert!(e.contains("`--bucket` was passed to `w`"), "{e}");
	assert!(e.contains("needs a `--` before it"), "{e}");
}

#[test]
fn a_flag_the_target_reads_is_fine() {
	let src = "if FLAG.verbose\n\t$ tool -v\nelse\n\t$ tool\nend\n";
	let (_, warned) = run_with(&[("runfiles/w.run", src)], "w", &["--verbose"]);
	assert!(warned.is_empty(), "{warned:?}");
}

#[test]
fn an_argument_the_target_never_reads_is_refused_too() {
	let e = refused(&[("runfiles/w.run", WRAP)], "w", &["--region=eu"]);
	assert!(e.contains("`--region`"), "{e}");
}

#[test]
fn a_name_read_only_in_a_comment_is_not_read() {
	// The tree has no comments in it. Scanning the text, this passed.
	let src = "# --region is no longer used; ARG.region was dropped in v2.\n$ tool\n";
	let e = refused(&[("runfiles/w.run", src)], "w", &["--region=eu"]);
	assert!(e.contains("`--region`"), "{e}");
}

#[test]
fn a_flag_read_by_the_shared_file_counts_as_read() {
	// `_shared.run` applies to every target, so a flag it reads is read.
	let files = &[
		("runfiles/_shared.run", "let level = FLAG.verbose\n"),
		("runfiles/w.run", WRAP),
	];
	let (_, warned) = run_with(files, "w", &["--verbose"]);
	assert!(warned.is_empty(), "{warned:?}");
}

#[test]
fn a_header_lookup_does_not_refuse_on_its_own() {
	// `header_props` is consulted before a run -- to read `.watch` -- and must
	// not be the thing that rejects the command line.
	let files = &[("runfiles/w.run", ".watch = \"src/**\"\n$ tool {{ ARGS }}\n")];
	let d = project(files);
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let mut h = crate::dispatch::Host::new(&cat);
	h.assume_yes = true;
	h.dry_run = true;
	let args = vec!["--oops".to_string()];
	let t = cat.resolve("w").unwrap();
	h.header_props(t, &args)
		.expect("a probe reads properties, nothing more");
	h.run("w", &args).expect_err("the run itself refuses");
}

#[test]
fn a_dispatched_target_is_checked_the_same_way() {
	// The case that found this: `run _aws s3api --bucket X` in a file.
	let files = &[
		("runfiles/outer.run", "run w s3api --bucket x\n"),
		("runfiles/w.run", WRAP),
	];
	let e = refused(files, "outer", &[]);
	assert!(e.contains("`--bucket` was passed to `w`"), "{e}");
}

#[test]
fn a_dispatched_target_can_be_given_a_double_dash() {
	let files = &[
		("runfiles/outer.run", "run w -- s3api --bucket x\n"),
		("runfiles/w.run", WRAP),
	];
	let (trace, warned) = run_with(files, "outer", &[]);
	assert_eq!(trace, ["tool s3api --bucket x"]);
	assert!(warned.is_empty(), "{warned:?}");
}

#[test]
fn a_dispatched_argument_with_spaces_stays_one_argument() {
	// `run` statement arguments are values handed to an in-process target, not
	// text handed to a shell: a value with a space must arrive as one
	// positional, not as a quoted word.
	let files = &[
		("runfiles/outer.run", "let name = \"a b\"\nrun w {{ name }}\n"),
		("runfiles/w.run", "$ tool {{ length(ARGS) }} {{ ARGS }}\n"),
	];
	let (trace, _) = run_with(files, "outer", &[]);
	assert_eq!(trace, ["tool 1 'a b'"], "one positional, quoted once for the shell");
}
