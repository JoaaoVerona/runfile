//! A command line, classified against what the target reads.
//!
//! Built from real source rather than a hand-made `Inputs`, so these cannot
//! agree with themselves about what a target reads while disagreeing with the
//! walker that answers the same question at run time.

use crate::args::{Arg, parse};
use crate::inputs;

fn reads(src: &str) -> crate::Inputs {
	inputs::of(&crate::parse(src).unwrap_or_else(|e| panic!("{src:?}: {e}")))
}

/// Classify `argv` against what `src` reads, as a list of terse renderings:
/// `key=value` for an argument, `+key` for a flag, `?token` for a word read
/// under no name, and the word itself for a positional.
fn classify(src: &str, argv: &[&str]) -> Vec<String> {
	let words: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
	parse(&words, &reads(src))
		.unwrap_or_else(|e| panic!("{argv:?}: {e}"))
		.into_iter()
		.map(|a| match a {
			Arg::Arg { key, value } => format!("{key}={value}"),
			Arg::Flag(key) => format!("+{key}"),
			Arg::Positional(v) => v,
			Arg::Unknown { token, .. } => format!("?{token}"),
		})
		.collect()
}

fn refuse(src: &str, argv: &[&str]) -> crate::args::MissingValue {
	let words: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
	parse(&words, &reads(src)).expect_err(&format!("{argv:?} should be refused"))
}

const ARG: &str = "print(ARG.target)\n";
const FLAG: &str = "if FLAG.target\n\tprint(\"on\")\nend\n";
const BOTH: &str = "if FLAG.target\n\tprint(ARG.target)\nend\n";
const WRAP: &str = "$ cargo build {{ ARGS }}\n";

// -------------------------------------------------------------- the rule

#[test]
fn the_equals_form_is_an_argument_whatever_else_is_true() {
	// It was never ambiguous and its meaning does not move.
	assert_eq!(classify(ARG, &["--target=arm"]), ["target=arm"]);
	assert_eq!(classify(BOTH, &["--target=arm"]), ["target=arm"]);
}

#[test]
fn a_bare_key_read_as_an_argument_takes_the_next_word() {
	// The motivating case: `--target aarch64` used to be a flag and a
	// positional, so the triple reached `cargo` with no `--target` in front.
	assert_eq!(classify(ARG, &["--target", "arm"]), ["target=arm"]);
}

#[test]
fn a_bare_key_read_as_a_flag_stays_a_flag() {
	assert_eq!(classify(FLAG, &["--target", "arm"]), ["+target", "arm"]);
}

#[test]
fn a_name_read_as_both_is_a_flag_in_its_bare_form() {
	// This is the only shape where a command line that works *today* could
	// have changed meaning: `--target arm` is a flag and a positional now, and
	// preferring the flag is what keeps it that way.
	assert_eq!(classify(BOTH, &["--target", "arm"]), ["+target", "arm"]);
}

#[test]
fn a_key_no_name_reads_is_neither_and_keeps_the_word_as_typed() {
	// Both spellings, because a caller that forwards them forwards what was
	// written rather than a reconstruction.
	assert_eq!(classify(ARG, &["--nope", "--other=1"]), ["?--nope", "?--other=1"]);
}

#[test]
fn a_word_that_is_not_a_key_is_a_positional() {
	// A single dash is not the marker, so `-x` is a word like any other.
	assert_eq!(classify(WRAP, &["build", "-x", "--"]), ["build", "-x"]);
}

#[test]
fn a_key_with_an_empty_name_is_a_word_rather_than_a_name_read_by_nobody() {
	// There is no name in `--=v` to report as unread, so refusing it would
	// have to name nothing.
	assert_eq!(classify(ARG, &["--=v"]), ["--=v"]);
}

// --------------------------------------------------- taking a value, or not

#[test]
fn a_value_that_looks_like_a_flag_is_refused_rather_than_swallowed() {
	// Swallowing it sets the triple to `--release` and fails later, inside
	// `cargo`, where the mistake is no longer visible.
	let e = refuse(ARG, &["--target", "--release"]);
	assert_eq!(e.key, "target");
	assert_eq!(e.next.as_deref(), Some("--release"));
}

#[test]
fn a_double_dash_is_not_taken_as_a_value_either() {
	let e = refuse(ARG, &["--target", "--", "arm"]);
	assert_eq!(e.next.as_deref(), Some("--"));
}

#[test]
fn a_key_at_the_end_of_the_line_has_nothing_to_take() {
	let e = refuse(ARG, &["--target"]);
	assert_eq!(e.key, "target");
	assert_eq!(e.next, None, "nothing was there, so the message must not claim one was");
}

#[test]
fn the_equals_form_is_how_a_value_starting_with_a_dash_is_written() {
	assert_eq!(classify(ARG, &["--target=--release"]), ["target=--release"]);
}

#[test]
fn a_consumed_value_is_not_also_read_as_something_else() {
	// `--force` is the value here, not the flag it would otherwise be.
	let src = "print(ARG.target)\nif FLAG.force\n\tprint(\"on\")\nend\n";
	assert_eq!(
		classify(src, &["--target=--force", "--force"]),
		["target=--force", "+force"]
	);
	// And a consumed positional does not stay one.
	assert_eq!(classify(ARG, &["--target", "arm"]).len(), 1);
}

#[test]
fn only_a_key_read_as_an_argument_consumes_anything() {
	// The unread `--nope` must not eat `arm`, or a wrapper's word count would
	// depend on which of its flags the target happened to read.
	assert_eq!(classify(WRAP, &["--nope", "arm"]), ["?--nope", "arm"]);
}

// ------------------------------------------------------------- the escape

#[test]
fn a_double_dash_ends_parsing_and_everything_after_it_is_a_word() {
	// Including a name the target reads: `--` is how a wrapper forwards a word
	// this would otherwise claim for itself.
	assert_eq!(
		classify(ARG, &["--target", "arm", "--", "--target", "x86", "--"]),
		["target=arm", "--target", "x86", "--"]
	);
}

#[test]
fn a_double_dash_with_nothing_after_it_contributes_nothing() {
	assert_eq!(classify(ARG, &["--"]), Vec::<String>::new());
}

// ---------------------------------------------------------------- shapes

#[test]
fn order_is_preserved_across_every_kind_of_word() {
	let src = "print(ARG.target)\nif FLAG.release\n\tprint(ARGS)\nend\n";
	assert_eq!(
		classify(src, &["a", "--target", "arm", "b", "--release", "--nope", "c"]),
		["a", "target=arm", "b", "+release", "?--nope", "c"]
	);
}

#[test]
fn an_empty_command_line_classifies_to_nothing() {
	assert_eq!(classify(ARG, &[]), Vec::<String>::new());
}

#[test]
fn a_repeated_key_is_classified_every_time_it_appears() {
	// What the last one wins is the caller's business; the classification is
	// per word.
	assert_eq!(
		classify(ARG, &["--target", "arm", "--target=x86"]),
		["target=arm", "target=x86"]
	);
}

#[test]
fn a_value_may_be_anything_that_is_not_a_flag() {
	for v in ["", "with space", "a=b", "-", "{{ x }}", "--"] {
		let argv = ["--target".to_string(), v.to_string()];
		let got = parse(&argv, &reads(ARG));
		// A leading `-` is the one refusal; everything else is a value.
		if v.starts_with('-') && !v.is_empty() {
			assert!(got.is_err(), "{v:?} should not be taken as a value");
		} else {
			assert_eq!(
				got.unwrap(),
				[Arg::Arg {
					key: "target".into(),
					value: v.into()
				}],
				"{v:?}"
			);
		}
	}
}

#[test]
fn a_name_a_shared_file_reads_takes_a_value_too() {
	// The chain is folded in before a command line is classified, which is the
	// whole reason `prepare` walks it in two passes.
	let mut all = reads("$ echo {{ ARGS }}\n");
	all.extend(reads("let e = ARG.env\n"));
	let argv = ["--env".to_string(), "prod".to_string()];
	assert_eq!(
		parse(&argv, &all).unwrap(),
		[Arg::Arg {
			key: "env".into(),
			value: "prod".into()
		}]
	);
}
