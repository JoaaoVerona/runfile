//! The Runfile language: lexer, parser and AST for `.run` files.
//!
//! Grammar reference: `GRAMMAR.ebnf` at the workspace root.
//!
//! A `.run` file is one target. Lines are classified by their first token:
//! `#` comment, `.name` property, `$ ` shell, `exec …`/`end` block, otherwise
//! a statement. Language is the default; shell is explicitly marked.

pub mod ast;
pub mod eval;
pub mod functions;
pub mod lexer;
pub mod parser;
pub mod span;
pub mod value;

pub use ast::{Block, Expr, InterpPart, Property, SourceKind, Statement, Target};
pub use eval::{EvalError, Keys, Scope, TempFiles, eval};
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
/// The cost of the coupling is one spurious re-run if that rendering ever
/// changes, which is the same cost as any other miss.
pub fn fingerprint(target: &Target) -> u64 {
	let mut text = format!("{:?}", target.body);
	// Every kind of position, or a comment that shifts the rest of the file
	// would count as a change. `Span { start: 12, end: 20, line: 3 }` becomes
	// `Span`, and `Statement::Exec`'s per-body-line source numbers go with it.
	strip_between(&mut text, "Span {", '}');
	strip_between(&mut text, "lines: [", ']');
	let mut h: u64 = 0xcbf2_9ce4_8422_2325;
	for b in text.bytes() {
		h ^= u64::from(b);
		h = h.wrapping_mul(0x1000_0000_01b3);
	}
	h
}

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
pub use span::Span;
pub use value::{TypeError, Value};

#[cfg(test)]
mod tests;
