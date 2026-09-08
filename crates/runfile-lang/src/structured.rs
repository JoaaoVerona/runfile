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
