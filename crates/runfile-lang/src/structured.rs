//! Blocks of structured text: `json … end`, and whatever joins it later.
//!
//! A format answers three questions and nothing else: how a value is written
//! in it, what stands in for an interpolation while the block is being
//! checked, and whether a document is well-formed. Adding YAML or TOML is a
//! variant and those three arms.
//!
//! The point is not the layout, it is the escaping. An interpolation renders
//! as **one value of the format**, the same way it renders as one shell word
//! in a `$` line -- so a string carrying a quote or a newline cannot break the
//! document around it, and nobody writes `\"` by hand.

use crate::value::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Structured {
	Json,
}

impl Structured {
	/// The word that opens such a block.
	pub fn keyword(self) -> &'static str {
		match self {
			Structured::Json => "json",
		}
	}

	pub fn from_keyword(word: &str) -> Option<Self> {
		match word {
			"json" => Some(Structured::Json),
			_ => None,
		}
	}

	/// Every format, for tooling that offers them.
	pub const ALL: &'static [Structured] = &[Structured::Json];

	/// The language an editor should highlight the body as.
	pub fn injection(self) -> &'static str {
		match self {
			Structured::Json => "json",
		}
	}

	/// What stands in for an interpolation while the block is checked.
	///
	/// A quoted string, because it has to be valid wherever an interpolation
	/// may stand -- as a value, and as an object key, which a bare `null`
	/// would not be. What the value turns out to be is not known until the
	/// target runs, and the shape of the document does not depend on it.
	pub fn placeholder(self) -> &'static str {
		match self {
			Structured::Json => "\"\u{2400}runfile\"",
		}
	}

	/// How a value is written in this format.
	pub fn render(self, v: &Value) -> String {
		match self {
			Structured::Json => json_value(v),
		}
	}

	/// Reject a document that is not well-formed.
	pub fn validate(self, text: &str) -> Result<(), String> {
		match self {
			Structured::Json => serde_json::from_str::<serde_json::Value>(text)
				.map(|_| ())
				.map_err(|e| e.to_string()),
		}
	}
	/// Lay a document out the one way there is, or `None` to leave it as
	/// written. See `pretty_json`.
	pub fn pretty(self, text: &str, indent: &str) -> Option<String> {
		match self {
			Structured::Json => pretty_json(text, indent),
		}
	}

	/// The document's tokens, separated so two of them cannot run together --
	/// what it says, with its layout dropped. `None` when it will not
	/// tokenise, and the caller then falls back to the text itself.
	///
	/// A format whose whitespace is significant would answer with the text
	/// unchanged rather than with this.
	pub fn canonical(self, text: &str) -> Option<String> {
		match self {
			Structured::Json => {
				let mut out = String::new();
				for t in tokens(text)? {
					out.push_str(match &t {
						Tok::Open(s) | Tok::Close(s) | Tok::Atom(s) => s,
						Tok::Comma => ",",
						Tok::Colon => ":",
					});
					out.push('\u{1}');
				}
				Some(out)
			}
		}
	}
}

fn json_value(v: &Value) -> String {
	match v {
		// serde does the escaping, so a quote, a backslash or a newline in the
		// value cannot end the string it sits in.
		Value::Str(s) => serde_json::Value::String(s.clone()).to_string(),
		Value::Bool(b) => b.to_string(),
		// One numeric type, written as an integer where it is one: `4003`
		// rather than `4003.0`, which is what anything reading the document
		// expects a port or a count to look like.
		Value::Num(n) => {
			if n.is_finite() && *n == n.trunc() && n.abs() < 9.007_199_254_740_992e15 {
				format!("{}", *n as i64)
			} else if n.is_finite() {
				format!("{n}")
			} else {
				// JSON has no infinity or NaN; null is the honest answer.
				"null".to_string()
			}
		}
		Value::List(items) => {
			let inner: Vec<String> = items.iter().map(json_value).collect();
			format!("[{}]", inner.join(", "))
		}
	}
}

/// One piece of a document, as the layout sees it.
///
/// Every atom is a slice of the source, so a string keeps its own escapes and
/// an interpolation stays whole -- the formatter never reconstructs what it
/// can copy.
enum Tok<'a> {
	Open(&'a str),
	Close(&'a str),
	Comma,
	Colon,
	Atom(&'a str),
}

/// Split a document into tokens, `{{ … }}` counting as one.
///
/// `None` when something is not recognised, and the caller then leaves the
/// block exactly as written -- the same discipline the shell tracing follows.
fn tokens(text: &str) -> Option<Vec<Tok<'_>>> {
	let b = text.as_bytes();
	let mut out = Vec::new();
	let mut i = 0;
	while i < b.len() {
		if b[i].is_ascii_whitespace() {
			i += 1;
			continue;
		}
		if text[i..].starts_with("{{") {
			let end = crate::lexer::skip_interp(b, i, 1).ok()?;
			out.push(Tok::Atom(&text[i..end]));
			i = end;
			continue;
		}
		match b[i] {
			b'{' | b'[' => {
				out.push(Tok::Open(&text[i..=i]));
				i += 1;
			}
			b'}' | b']' => {
				out.push(Tok::Close(&text[i..=i]));
				i += 1;
			}
			b',' => {
				out.push(Tok::Comma);
				i += 1;
			}
			b':' => {
				out.push(Tok::Colon);
				i += 1;
			}
			b'"' => {
				let mut j = i + 1;
				loop {
					if j >= b.len() {
						// A string running past the end of the document: not
						// something to lay out.
						return None;
					}
					match b[j] {
						b'\\' => j += 2,
						b'"' => break,
						_ => j += 1,
					}
				}
				out.push(Tok::Atom(&text[i..=j]));
				i = j + 1;
			}
			_ => {
				let start = i;
				while i < b.len()
					&& !b[i].is_ascii_whitespace()
					&& !matches!(b[i], b'{' | b'}' | b'[' | b']' | b',' | b':' | b'"')
				{
					i += 1;
				}
				if i == start {
					return None;
				}
				out.push(Tok::Atom(&text[start..i]));
			}
		}
	}
	Some(out)
}

/// Lay a document out the one way there is: one member to a line, nested a
/// level in, and an empty object or array left on its line.
///
/// `None` when the text cannot be tokenised, and the block is then left as
/// written. `indent` is one level -- a tab, like the language around it, so a
/// file does not change indentation character halfway down.
fn pretty_json(text: &str, indent: &str) -> Option<String> {
	let toks = tokens(text)?;
	let mut out = String::new();
	let mut depth = 0usize;
	// Whether the line so far is empty, and so still owes its indentation.
	let mut fresh = true;
	let mut prev_open = false;

	for (i, t) in toks.iter().enumerate() {
		match t {
			Tok::Colon => {
				out.push_str(": ");
				fresh = false;
			}
			Tok::Comma => {
				out.push_str(",\n");
				fresh = true;
			}
			Tok::Open(o) => {
				if fresh {
					out.push_str(&indent.repeat(depth));
				}
				out.push_str(o);
				// An empty `{}` or `[]` has nothing to lay out, so it stays
				// on its line and opens no level.
				if matches!(toks.get(i + 1), Some(Tok::Close(_))) {
					fresh = false;
				} else {
					depth += 1;
					out.push('\n');
					fresh = true;
				}
			}
			Tok::Close(c) => {
				if prev_open {
					out.push_str(c);
				} else {
					depth = depth.saturating_sub(1);
					if !fresh {
						out.push('\n');
					}
					out.push_str(&indent.repeat(depth));
					out.push_str(c);
				}
				fresh = false;
			}
			Tok::Atom(a) => {
				if fresh {
					out.push_str(&indent.repeat(depth));
				}
				out.push_str(a);
				fresh = false;
			}
		}
		prev_open = matches!(t, Tok::Open(_));
	}
	Some(out)
}
