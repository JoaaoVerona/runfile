//! The Runfile language: lexer, parser and AST for `.run` files.
//!
//! Grammar reference: `GRAMMAR.ebnf` at the workspace root.
//!
//! A `.run` file is one target. Lines are classified by their first token:
//! `#` comment, `.name` property, `$ ` shell, `exec …`/`end` block, otherwise
//! a statement. Language is the default; shell is explicitly marked.

pub mod args;
pub mod ast;
pub mod eval;
pub mod format;
pub mod functions;
pub mod inputs;
pub mod keywords;
pub mod lexer;
pub mod parser;
pub mod resolve;
pub mod span;
pub mod structured;
pub mod value;

pub use args::Arg;
pub use ast::{Block, Constant, Expr, InterpPart, Property, SourceKind, Statement, Target};
pub use eval::{EvalError, Keys, Scope, TempFiles, eval};
pub use format::format;
pub use parser::{ParseError, parse};

/// A fingerprint of what a target *does*, ignoring where it says it.
///
/// The prepare gate re-triggers when its setup target changes, and hashing the
/// file text meant reflowing a comment counted as a change. This hashes the
/// parsed tree instead, with source positions stripped -- adding a comment
/// shifts every span after it, so leaving them in would defeat the point.
///
/// Stripping is done on the `Debug` rendering rather than by walking every
/// variant, so a new expression kind cannot silently escape the fingerprint.
/// The cost of the coupling is a spurious re-run whenever that rendering
/// changes -- and it is paid once in **every project on the machine**, since a
/// runner upgrade changes the rendering for all of them at once. Adding
/// `detach` to `Statement::Exec` did exactly that: every `setup.run` holding a
/// command asked for `run setup` again, 83 of the author's 88, for a change none
/// of them had made. `UNASKED` is what keeps a new field from doing it again.
pub fn fingerprint(target: &Target) -> u64 {
	let mut text = format!("{:?}", target.body);
	// Every kind of position, or a comment that shifts the rest of the file
	// would count as a change. `Span { start: 12, end: 20, line: 3 }` becomes
	// `Span`, and `Statement::Exec`'s per-body-line source numbers go with it.
	strip_between(&mut text, "Span {", '}');
	strip_between(&mut text, "lines: [", ']');
	for field in UNASKED {
		text = text.replace(field, "");
	}
	let mut h: u64 = 0xcbf2_9ce4_8422_2325;
	for b in text.bytes() {
		h ^= u64::from(b);
		h = h.wrapping_mul(0x1000_0000_01b3);
	}
	h
}

/// Fields added to the tree after files were already being fingerprinted, as
/// they render at the value a file gets **without asking for them** -- the
/// field and its separator, so dropping the text leaves the rendering the field
/// did not yet exist in. Such a field changes nothing about what an existing
/// file does, so it must not change that file's fingerprint either; a file that
/// *does* ask for it renders differently, and still counts as changed.
///
/// A field added to the tree with a default that means "as before" belongs
/// here, in the same change. A test pins a real `setup.run` to the fingerprint
/// the runner recorded for it before `detach` existed, so forgetting shows.
const UNASKED: &[&str] = &["detach: false, ", "parallel: false, "];

/// Replace every `open …close` run with `open`, so positional detail drops out
/// of the rendering without a walk over every variant.
fn strip_between(text: &mut String, open: &str, close: char) {
	let mut from = 0;
	while let Some(at) = text[from..].find(open).map(|i| i + from) {
		let after = at + open.len();
		let end = text[after..].find(close).map(|e| after + e + 1).unwrap_or(text.len());
		text.replace_range(at..end, open);
		from = at + open.len();
	}
}
pub use inputs::Inputs;
pub use keywords::{KEYWORDS, Keyword};
pub use resolve::Unresolved;
pub use span::Span;
pub use structured::Structured;
pub use value::{TypeError, Value};

#[cfg(test)]
mod tests;
