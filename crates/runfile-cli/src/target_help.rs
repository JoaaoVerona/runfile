//! `run <target> --help`.
//!
//! A target's description used to be visible only as its first line, in
//! `run :list` -- so forty of them in the corpus had grown past three hundred
//! characters with nowhere to be read, one to sixteen hundred. And asking for
//! help was worse than useless: `--help` after a target name is the target's
//! argument, so `run deploy --help` warned about an unread flag and then
//! **deployed**. This is both halves of that.

use crate::help;
use runfile_discovery::{Catalog, Target};

/// Whether these target arguments are asking for help.
///
/// Only before a `--`: past that everything is the target's, verbatim, which
/// is how a wrapper forwards a command line that has its own `--help`.
pub(crate) fn wants_help(args: &[String]) -> bool {
	args.iter()
		.take_while(|a| *a != "--")
		.any(|a| a == "--help" || a == "-h")
}

/// The inputs a target reads: the target's own, plus every `_shared.run`
/// above it, since a name a shared file reads is read for every target under
/// it.
///
/// Walked from the tree. Reading the text found names in comments, in strings
/// and in the literal halves of `$` lines, so this listed inputs a target
/// never reads.
pub(crate) fn inputs(cat: &Catalog, target: &Target) -> runfile_lang::Inputs {
	let parse = |p: &std::path::Path| {
		std::fs::read_to_string(p)
			.ok()
			.and_then(|src| runfile_lang::parse(&src).ok())
	};
	// A file that does not parse contributes nothing, as before: it will say
	// what is wrong with it when the target runs, and `--help` must still
	// answer with what the rest of the chain reads.
	let own = parse(&target.path).unwrap_or_else(|| runfile_lang::Target {
		description: None,
		body: Default::default(),
	});
	let shared: Vec<_> = cat.shared_chain(target).iter().filter_map(|p| parse(p)).collect();
	runfile_lang::inputs::of_chain(&own, &shared)
}

pub(crate) fn render(cat: &Catalog, target: &Target) -> String {
	let src = std::fs::read_to_string(&target.path).unwrap_or_default();
	let description = runfile_lang::parse(&src)
		.ok()
		.and_then(|t| t.description)
		.unwrap_or_default();
	let reads = inputs(cat, target);

	// The usage line names what a caller has to supply. Required first, since
	// that is the half a run fails without.
	let mut usage = format!("run {}", target.name);
	for (name, u) in &reads.args {
		usage.push_str(&if u.required {
			format!(" --{name}=<value>")
		} else {
			format!(" [--{name}=<value>]")
		});
	}
	for name in &reads.flags {
		usage.push_str(&format!(" [--{name}]"));
	}
	if reads.positional {
		usage.push_str(" [args…]");
	}

	let mut out = format!("{}\n", help::bold(&usage));

	// The whole description, not its first line. That is the point of this.
	// Wrapped, because these run to sixteen hundred characters on one line --
	// each source line on its own, so a description that laid itself out keeps
	// its shape.
	if !description.is_empty() {
		out.push('\n');
		for line in description.lines() {
			for wrapped in wrap(line, 92) {
				// A blank line in the description stays blank rather than
				// becoming two spaces.
				out.push_str(&if wrapped.is_empty() {
					"\n".to_string()
				} else {
					format!("  {wrapped}\n")
				});
			}
		}
	}

	if !reads.args.is_empty() || !reads.flags.is_empty() || reads.positional {
		out.push_str(&format!("\n{}\n", help::bold("Arguments")));
		for (name, u) in &reads.args {
			out.push_str(&format!("  {}\n", note(&format!("--{name}=<value>"), u)));
		}
		for name in &reads.flags {
			out.push_str(&format!("  --{name:<28}off unless passed\n"));
		}
		if reads.positional {
			out.push_str(&format!("  {:<30}read as `ARGS`\n", "<positional…>"));
		}
	}

	// Read from the environment, so worth naming even though a caller does not
	// pass them on the command line -- an `.env-file` or the shell supplies
	// them, and a missing required one fails the same way.
	if !reads.env.is_empty() {
		out.push_str(&format!("\n{}\n", help::bold("Environment")));
		for (name, u) in &reads.env {
			out.push_str(&format!("  {}\n", note(name, u)));
		}
	}

	out.push_str(&format!(
		"\n{}\n  {}\n",
		help::bold("Defined in"),
		target.path.display()
	));
	out.push_str("\n  Pass `--` first to hand `--help` to the target instead: run ");
	out.push_str(&format!("{} -- --help\n", target.name));
	out
}

/// One input, with what happens when it is not supplied.
///
/// Both facts come from the tree: a `?` chain catches the failure, so a name
/// with one is optional, and the literal the chain ends in is what it falls
/// back to. A name read bare can fail, and says so.
fn note(label: &str, u: &runfile_lang::inputs::Use) -> String {
	let tail = match (u.default.as_deref(), u.required) {
		(Some(""), _) => "defaults to empty".to_string(),
		(Some(d), _) => format!("defaults to {d}"),
		(None, true) => "required".to_string(),
		(None, false) => "optional".to_string(),
	};
	format!("{label:<30}{tail}")
}

/// Break a line on spaces at `width`, keeping its leading indentation on the
/// lines it spills onto. A word longer than the width is left whole rather
/// than cut: it is usually a path or a URL, and half of one is no use.
fn wrap(line: &str, width: usize) -> Vec<String> {
	let indent: String = line.chars().take_while(|c| *c == ' ').collect();
	let mut out = Vec::new();
	let mut cur = indent.clone();
	for word in line.split_whitespace() {
		if cur.trim().is_empty() {
			cur.push_str(word);
		} else if cur.chars().count() + 1 + word.chars().count() <= width {
			cur.push(' ');
			cur.push_str(word);
		} else {
			out.push(std::mem::replace(&mut cur, format!("{indent}{word}")));
		}
	}
	if !cur.trim().is_empty() {
		out.push(cur);
	}
	if out.is_empty() { vec![String::new()] } else { out }
}
