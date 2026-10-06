//! `--stdin-args`: ask for a target's inputs **before** it runs.
//!
//! It used to ask lazily, when evaluating an `ARG.x` that resolved to nothing
//! -- so it could only ask about inputs a run happened to reach, it asked
//! after earlier statements had already done their work, and it never asked at
//! all about anything with a fallback, because a chain that resolves raises no
//! error to catch. A flag was never asked about either: an absent one is
//! `false`, not a failure.
//!
//! What a target reads is walked from its tree, so the whole list is known
//! before the first statement. This asks for all of it, up front, in one pass,
//! showing what each falls back to -- in the order the usage line gives it:
//! arguments, flags, positionals, then the environment.

use runfile_discovery::{Catalog, Target};
use runfile_lang::Arg;

/// Ask for everything the target reads that was not already supplied.
///
/// Answers for arguments, flags and positionals are added to `args`, which is
/// where the runner reads them from; an environment answer is set in this
/// process, which the target's environment is built on top of.
pub(crate) fn collect(cat: &Catalog, target: &Target, args: &mut Vec<String>) {
	let reads = crate::target_help::inputs(cat, target);
	let given = supplied(args, &reads);

	let mut named = Vec::new();
	for (name, u) in &reads.args {
		if given.args.contains(name) {
			continue;
		}
		if let Some(v) = crate::prompt::ask_input(&format!("--{name}"), u) {
			named.push(format!("--{name}={v}"));
		}
	}
	for name in &reads.flags {
		if given.flags.contains(name) {
			continue;
		}
		if crate::prompt::ask_flag(&format!("--{name}")) {
			named.push(format!("--{name}"));
		}
	}
	// Asked whenever the target reads `ARGS` and was handed none, and never
	// required: whether an empty list is a mistake is the target's to say --
	// `first(ARGS)` of one is `""`, not a failure -- and for a wrapper such as
	// `$ cargo build {{ ARGS }}` it is the usual case, so Enter passes nothing
	// and the wrapper runs as it would have. The line is split into words the
	// way an `exec` command line is, quotes and all: a person typing
	// positionals is typing a command line. A `one_of(first(ARGS), …)` is
	// offered as the choice it is.
	let positional = if reads.positional && !given.positional {
		let first = runfile_lang::inputs::Use {
			choices: reads.positional_choices.clone(),
			..Default::default()
		};
		crate::prompt::ask_input("ARGS", &first)
			.map(|line| runfile_runtime::exec::split_command(&line))
			.unwrap_or_default()
	} else {
		Vec::new()
	};
	for (name, u) in &reads.env {
		if std::env::var_os(name).is_some() {
			continue;
		}
		if let Some(v) = crate::prompt::ask_input(name, u) {
			// SAFETY: nothing has been dispatched yet, so this process is
			// still single-threaded. The target's environment is built on top
			// of this one, which is how the answer reaches `ENV.x` and every
			// command below it.
			unsafe { std::env::set_var(name, v) };
		}
	}

	// Ahead of a `--` the caller wrote. Past one every word is a positional, so
	// `--port=5000` appended after it reached `ARGS`, and `ARG.port` was still
	// missing when the target asked for it.
	let at = args.iter().position(|a| a == "--").unwrap_or(args.len());
	args.splice(at..at, named);
	// Behind one, so each word reaches `ARGS` exactly as typed: a `--release`
	// meant for the command a wrapper runs is not claimed by a flag the target
	// happens to read, which was asked about on its own line.
	if !positional.is_empty() {
		if !args.iter().any(|a| a == "--") {
			args.push("--".to_string());
		}
		args.extend(positional);
	}
}

/// What the caller already put on the command line, so it is not asked for
/// again.
struct Given {
	args: Vec<String>,
	flags: Vec<String>,
	/// Whether any word reaches `ARGS`. A `--key` no name reads counts: for a
	/// target that reads `ARGS` it is forwarded there, so the list is not
	/// empty.
	positional: bool,
}

/// Classified by the runner's own parser rather than by a second reading of
/// the same words: `--token given` supplies `ARG.token` exactly when the
/// runner will read it as one, so this cannot ask for something that was
/// given, or skip something that was not.
///
/// A command line the parser refuses -- a `--key` whose value is missing --
/// counts as supplying nothing. It is about to fail on that word either way,
/// and asking for one input too many is the harmless direction to be wrong in.
fn supplied(args: &[String], reads: &runfile_lang::Inputs) -> Given {
	let mut given = Given {
		args: Vec::new(),
		flags: Vec::new(),
		positional: false,
	};
	for x in runfile_lang::args::parse(args, reads).unwrap_or_default() {
		match x {
			Arg::Arg { key, .. } => given.args.push(key),
			Arg::Flag(key) => given.flags.push(key),
			Arg::Positional(_) | Arg::Unknown { .. } => given.positional = true,
		}
	}
	given
}
