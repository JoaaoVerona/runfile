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
//! showing what each falls back to.

use runfile_discovery::{Catalog, Target};
use runfile_lang::Arg;

/// Ask for everything the target reads that was not already supplied.
///
/// Answers for arguments and flags are appended to `args`, which is where the
/// runner reads them from; an environment answer is set in this process, which
/// the target's environment is built on top of.
pub(crate) fn collect(cat: &Catalog, target: &Target, args: &mut Vec<String>) {
	let reads = crate::target_help::inputs(cat, target);
	let (given_args, given_flags) = supplied(args, &reads);

	for (name, u) in &reads.args {
		if given_args.contains(name) {
			continue;
		}
		if let Some(v) = crate::prompt::ask_input(&format!("--{name}"), u.default.as_deref(), u.required) {
			args.push(format!("--{name}={v}"));
		}
	}
	for name in &reads.flags {
		if given_flags.contains(name) {
			continue;
		}
		if crate::prompt::ask_flag(&format!("--{name}")) {
			args.push(format!("--{name}"));
		}
	}
	for (name, u) in &reads.env {
		if std::env::var_os(name).is_some() {
			continue;
		}
		if let Some(v) = crate::prompt::ask_input(name, u.default.as_deref(), u.required) {
			// SAFETY: nothing has been dispatched yet, so this process is
			// still single-threaded. The target's environment is built on top
			// of this one, which is how the answer reaches `ENV.x` and every
			// command below it.
			unsafe { std::env::set_var(name, v) };
		}
	}
}

/// What the caller already put on the command line, so it is not asked for
/// again. Everything after a bare `--` is the target's own and is left alone.
///
/// Classified by the runner's own parser rather than by a second reading of
/// the same words: `--token given` supplies `ARG.token` exactly when the
/// runner will read it as one, so this cannot ask for something that was
/// given, or skip something that was not.
///
/// A command line the parser refuses -- a `--key` whose value is missing --
/// counts as supplying nothing. It is about to fail on that word either way,
/// and asking for one input too many is the harmless direction to be wrong in.
fn supplied(args: &[String], reads: &runfile_lang::Inputs) -> (Vec<String>, Vec<String>) {
	let (mut a, mut f) = (Vec::new(), Vec::new());
	for x in runfile_lang::args::parse(args, reads).unwrap_or_default() {
		match x {
			Arg::Arg { key, .. } => a.push(key),
			Arg::Flag(key) => f.push(key),
			Arg::Positional(_) | Arg::Unknown { .. } => {}
		}
	}
	(a, f)
}
