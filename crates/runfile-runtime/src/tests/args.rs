//! How a target's command line is read, and what it refuses.

use super::project;

/// Run a target and collect the commands it reached.
///
/// A command line that is accepted has to be checked by what it *did*, not by
/// what it did not say: these used to assert that nothing was warned, which
/// stopped meaning anything the day an unread input became a refusal.
fn run_with(files: &[(&str, &str)], target: &str, args: &[&str]) -> Vec<String> {
	try_run(files, target, args).expect("ran")
}

/// The same, for a run that is expected to be refused.
fn refused(files: &[(&str, &str)], target: &str, args: &[&str]) -> String {
	try_run(files, target, args).expect_err("refused").to_string()
}

fn try_run(files: &[(&str, &str)], target: &str, args: &[&str]) -> Result<Vec<String>, crate::run::RunError> {
	let d = project(files);
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let mut h = crate::dispatch::Host::new(&cat);
	h.assume_yes = true;
	h.dry_run = true;
	let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
	h.run(target, &args)?;
	let trace = h.trace.lock().unwrap().clone();
	Ok(trace)
}

const WRAP: &str = "$ tool {{ ARGS }}\n";

#[test]
fn everything_after_a_double_dash_is_positional_as_typed() {
	let trace = run_with(&[("runfiles/w.run", WRAP)], "w", &["--", "s3api", "--bucket", "x"]);
	assert_eq!(trace, ["tool s3api --bucket x"], "flags survive, in order");
}

#[test]
fn the_double_dash_itself_is_consumed() {
	let trace = run_with(&[("runfiles/w.run", WRAP)], "w", &["--", "a"]);
	assert_eq!(trace, ["tool a"]);
}

#[test]
fn before_the_double_dash_parsing_is_normal() {
	let src = "$ tool {{ ARG.env }} {{ ARGS }}\n";
	let trace = run_with(&[("runfiles/w.run", src)], "w", &["--env=prod", "--", "--verbose"]);
	assert_eq!(trace, ["tool prod --verbose"]);
}

#[test]
fn a_key_value_after_the_double_dash_is_not_an_argument() {
	// `--x=1` past the separator belongs to the wrapped command.
	let src = "$ tool {{ ARG.x ? \"unset\" }} {{ ARGS }}\n";
	let trace = run_with(&[("runfiles/w.run", src)], "w", &["--", "--x=1"]);
	assert_eq!(trace, ["tool unset --x=1"]);
}

#[test]
fn a_flag_no_name_reads_goes_to_args_when_the_target_reads_args() {
	// Forgetting `--` used to drop the flag silently, and then, once the check
	// was exact, to refuse the run. Neither is necessary: a target that reads
	// `ARGS` can read the word, so it gets the word, in the place it was
	// written.
	let trace = run_with(&[("runfiles/w.run", WRAP)], "w", &["s3api", "--bucket", "x"]);
	assert_eq!(trace, ["tool s3api --bucket x"]);
}

#[test]
fn a_flag_no_name_reads_is_refused_when_there_are_no_positionals() {
	// The other half of the rule. With no `ARGS` the word has nowhere to go
	// and nothing to forward it to, so a mistyped flag is still caught here
	// rather than quietly failing to take effect.
	let e = refused(&[("runfiles/w.run", "$ tool\n")], "w", &["--bucket"]);
	assert!(e.contains("`--bucket` was passed to `w`"), "{e}");
	assert!(e.contains("reads no arguments or flags at all"), "{e}");
	// The advice to write a `--` is gone with the case that needed it.
	assert!(!e.contains("`--` before it"), "{e}");
}

#[test]
fn the_refusal_quotes_the_word_as_it_was_typed() {
	// It used to reconstruct `--region` from the key, so a message about
	// `--region=eu` named something the person had not written.
	let e = refused(&[("runfiles/w.run", "$ tool\n")], "w", &["--region=eu"]);
	assert!(e.contains("`--region=eu` was passed"), "{e}");
	assert!(e.contains("never reads `FLAG.region` or `ARG.region`"), "{e}");
}

#[test]
fn a_flag_the_target_reads_is_fine() {
	let src = "if FLAG.verbose\n\t$ tool -v\nelse\n\t$ tool\nend\n";
	let trace = run_with(&[("runfiles/w.run", src)], "w", &["--verbose"]);
	assert_eq!(trace, ["tool -v"], "the flag was read, so the branch was taken");
}

#[test]
fn a_key_value_no_name_reads_follows_the_same_split() {
	// Forwarded whole, so the wrapped command sees what was typed.
	let trace = run_with(&[("runfiles/w.run", WRAP)], "w", &["--region=eu"]);
	assert_eq!(trace, ["tool --region=eu"]);
	let e = refused(&[("runfiles/w.run", "$ tool\n")], "w", &["--region=eu"]);
	assert!(e.contains("`--region=eu`"), "{e}");
}

#[test]
fn a_key_the_target_reads_takes_the_next_word_as_its_value() {
	let src = "$ tool {{ ARG.env }} {{ ARGS }}\n";
	let trace = run_with(&[("runfiles/w.run", src)], "w", &["--env", "prod", "extra"]);
	assert_eq!(trace, ["tool prod extra"], "the value is consumed, not left in ARGS");
}

#[test]
fn a_key_the_target_reads_and_a_key_it_does_not_can_sit_side_by_side() {
	// The target claims its own and forwards the rest -- which is the shape
	// nearly every wrapper in a real project has.
	let src = "$ tool {{ ARG.env }} -- {{ ARGS }}\n";
	let trace = run_with(&[("runfiles/w.run", src)], "w", &["--env", "prod", "--bucket", "x"]);
	assert_eq!(trace, ["tool prod -- --bucket x"]);
}

#[test]
fn a_key_with_no_value_is_refused_by_name() {
	let src = "$ tool {{ ARG.env }}\n";
	let e = refused(&[("runfiles/w.run", src)], "w", &["--env"]);
	assert!(e.contains("`--env` was passed to `w` with no value"), "{e}");
	let e = refused(&[("runfiles/w.run", src)], "w", &["--env", "--other"]);
	assert!(e.contains("`--other` looks like another flag"), "{e}");
}

#[test]
fn an_argument_only_the_shared_file_reads_takes_a_value_too() {
	// The chain has to be walked for what it reads *before* the command line
	// is classified. Folding it in where it used to be folded in was too late:
	// `--region eu` would already have been read as a flag and a positional.
	let files = &[
		("runfiles/_shared.run", "let region = ARG.region ? \"us\"\n"),
		("runfiles/w.run", "$ tool {{ region }} {{ length(ARGS) }}\n"),
	];
	let trace = run_with(files, "w", &["--region", "eu"]);
	assert_eq!(trace, ["tool eu 0"]);
}

#[test]
fn a_name_read_only_in_a_comment_is_not_read() {
	// The tree has no comments in it. Scanning the text, this passed.
	let src = "# --region is no longer used; ARG.region was dropped in v2.\n$ tool\n";
	let e = refused(&[("runfiles/w.run", src)], "w", &["--region=eu"]);
	assert!(e.contains("never reads `FLAG.region` or `ARG.region`"), "{e}");
	// And it is not read as a name that takes a value either: a comment must
	// not make the word after it disappear into an argument.
	let e = refused(&[("runfiles/w.run", src)], "w", &["--region", "eu"]);
	assert!(e.contains("`--region` was passed"), "{e}");
}

#[test]
fn a_flag_read_by_the_shared_file_counts_as_read() {
	// `_shared.run` applies to every target, so a flag it reads is read.
	let files = &[
		("runfiles/_shared.run", "let level = FLAG.verbose\n"),
		("runfiles/w.run", "$ tool {{ length(ARGS) }}\n"),
	];
	// Claimed as a flag rather than forwarded, which is the sharper way to say
	// it was read: an unread one would have landed in `ARGS` instead.
	let trace = run_with(files, "w", &["--verbose"]);
	assert_eq!(trace, ["tool 0"]);
}

#[test]
fn a_header_lookup_does_not_refuse_on_its_own() {
	// `header_props` is consulted before a run -- to read `.watch` -- and must
	// not be the thing that rejects the command line.
	let files = &[("runfiles/w.run", ".watch = \"src/**\"\n$ tool\n")];
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
fn a_dispatched_target_is_classified_the_same_way() {
	// The case that found this: `run _aws s3api --bucket X` in a file. It is
	// forwarded now, because `w` reads `ARGS`.
	let files = &[
		("runfiles/outer.run", "run w s3api --bucket x\n"),
		("runfiles/w.run", WRAP),
	];
	let trace = run_with(files, "outer", &[]);
	assert_eq!(trace, ["tool s3api --bucket x"]);

	// And a name the dispatched target reads takes its value across the call.
	let files = &[
		("runfiles/outer.run", "run w --env prod\n"),
		("runfiles/w.run", "$ tool {{ ARG.env }}\n"),
	];
	let trace = run_with(files, "outer", &[]);
	assert_eq!(trace, ["tool prod"]);
}

#[test]
fn a_dispatched_target_that_reads_nothing_still_refuses() {
	let files = &[
		("runfiles/outer.run", "run w --bucket x\n"),
		("runfiles/w.run", "$ tool\n"),
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
	let trace = run_with(files, "outer", &[]);
	assert_eq!(trace, ["tool s3api --bucket x"]);
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
	let trace = run_with(files, "outer", &[]);
	assert_eq!(trace, ["tool 1 'a b'"], "one positional, quoted once for the shell");
}
