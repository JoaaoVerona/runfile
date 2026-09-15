//! A shell checker for runfiles.
//!
//! The shell in a `.run` file is not a file of its own. A `$` line's text reaches
//! bash with every `{{ … }}` replaced by a quoted word, and the language around
//! it decides what those words hold. A checker that sees only the shell --
//! ShellCheck, with a placeholder where each interpolation was -- cannot tell an
//! interpolation from a word nobody wrote, and reports on both; one that sees
//! only the runfile cannot see the shell. This one reads both.
//!
//! **It reports only what is wrong every time.** A finding stops a run, so a rule
//! that could be wrong about a script that works is not a rule here: where the
//! same text can be right, a rule says nothing. `SHELL-CHECK-RULES.md` lists
//! every rule, what each leaves alone, and why.

mod rules;
mod script;
mod syntax;
mod walk;
mod words;

#[cfg(test)]
mod tests;

use runfile_lang::{Span, Target};

/// A rule: the name a finding carries, and what it finds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
	pub id: &'static str,
	pub summary: &'static str,
}

/// Every rule, in the order `SHELL-CHECK-RULES.md` lists them.
pub const RULES: &[Rule] = &[
	Rule {
		id: "unclosed",
		summary: "A quote, an expansion or a block that is never closed.",
	},
	Rule {
		id: "unexpected",
		summary: "A keyword or an operator where bash cannot take one.",
	},
	Rule {
		id: "unterminated-heredoc",
		summary: "A heredoc that no line ends.",
	},
	Rule {
		id: "quoted-interpolation",
		summary: "An interpolation wrapped in shell quotes.",
	},
	Rule {
		id: "unexpanded-string",
		summary: "A string that spells `$HOME` or `~`, used as a path.",
	},
	Rule {
		id: "unexpanded-glob",
		summary: "An interpolation holding a `*` the shell never expands.",
	},
	Rule {
		id: "tilde-in-quotes",
		summary: "A quoted `~`, which never means the home directory.",
	},
	Rule {
		id: "positional-parameter",
		summary: "`$1` or `$@` in a shell that is given no arguments.",
	},
	Rule {
		id: "lost-effect",
		summary: "`cd`, `export` or `set` as the last command of its shell.",
	},
	Rule {
		id: "dropped-backslash",
		summary: "A backslash bash removes, in front of a letter or a digit.",
	},
	Rule {
		id: "windows-command",
		summary: "cmd.exe's syntax, in a shell that is not cmd.exe.",
	},
	Rule {
		id: "bracket-spacing",
		summary: "A `[` or `]` written against the word beside it.",
	},
	Rule {
		id: "test-redirect",
		summary: "`>` or `<` inside `[ … ]`, where it redirects.",
	},
	Rule {
		id: "outside-function",
		summary: "`local` or `return` outside a shell function.",
	},
	Rule {
		id: "spaced-assignment",
		summary: "`NAME = value`, which runs a command called `NAME`.",
	},
	Rule {
		id: "sudo-builtin",
		summary: "`sudo cd` and the like, which have no program to run.",
	},
];

/// Something wrong with the shell in a runfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
	/// The [`Rule`] it breaks.
	pub rule: &'static str,
	/// Exactly the text it is about.
	pub span: Span,
	pub message: String,
	/// What to write instead, when there is one thing to write.
	pub fix: Option<String>,
}

impl std::fmt::Display for Finding {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "line {}: {} [{}]", self.span.line, self.message, self.rule)?;
		match &self.fix {
			Some(fix) => write!(f, "; {fix}"),
			None => Ok(()),
		}
	}
}

/// Everything wrong with the shell in `file`, whose text is `src`.
///
/// `chain` is the `_shared.run` files above it, outermost first: what they bind,
/// and whether one names the shell `$` lines run in. `None` when one of them
/// could not be read, and then neither is known -- so `$` lines are left alone,
/// and nothing a shared file binds is judged.
pub fn check(src: &str, file: &Target, chain: Option<&[Target]>) -> Vec<Finding> {
	walk::check(src, file, chain)
}
