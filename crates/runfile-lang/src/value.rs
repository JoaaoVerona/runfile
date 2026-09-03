//! The value model. Four types, no coercion anywhere.
//!
//! `+` is arithmetic only -- `concat` joins strings, and mixing types is an
//! error rather than a silent join. `"1" == 1` is false. A string arriving from
//! `ARG.x` or `ENV.X` must go through `number(…)` before arithmetic.

use std::fmt;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
	Str(String),
	/// One numeric type. Prints as an integer when integral, so `2 + 3` is `5`.
	Num(f64),
	Bool(bool),
	List(Vec<Value>),
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum TypeError {
	#[error("expected {expected}, got {actual}")]
	Expected {
		expected: &'static str,
		actual: &'static str,
	},
	#[error("`{op}` needs numbers, got {lhs} and {rhs} -- use concat(…) to join strings")]
	BadOperands {
		op: &'static str,
		lhs: &'static str,
		rhs: &'static str,
	},
	#[error("division by zero")]
	DivideByZero,
	#[error("`{0}` is not a number")]
	NotNumeric(String),
	#[error("index {index} is out of range for a list of {len}")]
	IndexOutOfRange { index: i64, len: usize },
	#[error("condition must be true or false, got {0}")]
	NotBoolean(&'static str),
}

impl Value {
	pub fn type_name(&self) -> &'static str {
		match self {
			Value::Str(_) => "string",
			Value::Num(_) => "number",
			Value::Bool(_) => "bool",
			Value::List(_) => "list",
		}
	}

	pub fn as_str(&self) -> Result<&str, TypeError> {
		match self {
			Value::Str(s) => Ok(s),
			other => Err(TypeError::Expected {
				expected: "string",
				actual: other.type_name(),
			}),
		}
	}

	pub fn as_num(&self) -> Result<f64, TypeError> {
		match self {
			Value::Num(n) => Ok(*n),
			other => Err(TypeError::Expected {
				expected: "number",
				actual: other.type_name(),
			}),
		}
	}

	pub fn as_bool(&self) -> Result<bool, TypeError> {
		match self {
			Value::Bool(b) => Ok(*b),
			other => Err(TypeError::NotBoolean(other.type_name())),
		}
	}

	pub fn as_list(&self) -> Result<&[Value], TypeError> {
		match self {
			Value::List(v) => Ok(v),
			other => Err(TypeError::Expected {
				expected: "list",
				actual: other.type_name(),
			}),
		}
	}

	/// A usize index, rejecting negatives and non-integers.
	pub fn as_index(&self) -> Result<usize, TypeError> {
		let n = self.as_num()?;
		if n < 0.0 || n.fract() != 0.0 {
			return Err(TypeError::NotNumeric(format_num(n)));
		}
		Ok(n as usize)
	}

	/// How a value reaches shell context: one quoted argument, or N for a list.
	/// This is what removed `shell_quote` -- and why an interpolation must never
	/// be wrapped in shell quotes by hand.
	pub fn to_shell(&self) -> String {
		match self {
			Value::List(items) => items.iter().map(Value::to_shell).collect::<Vec<_>>().join(" "),
			other => shell_quote(&other.to_string()),
		}
	}
}

/// POSIX single-quoting: wrap, and close/escape/reopen around any single quote.
/// Safe for every byte, including newlines, `$` and backticks.
pub fn shell_quote(s: &str) -> String {
	if !s.is_empty()
		&& s.bytes().all(|b| {
			b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/' | b':' | b'=' | b'@' | b'+' | b',')
		}) {
		return s.to_string();
	}
	let mut out = String::with_capacity(s.len() + 2);
	out.push('\'');
	for c in s.chars() {
		if c == '\'' {
			out.push_str("'\\''");
		} else {
			out.push(c);
		}
	}
	out.push('\'');
	out
}

/// Integral values print without a fractional part; everything else uses the
/// shortest round-trip form.
pub fn format_num(n: f64) -> String {
	if n.fract() == 0.0 && n.is_finite() && n.abs() < 1e15 {
		format!("{}", n as i64)
	} else {
		format!("{n}")
	}
}

impl fmt::Display for Value {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Value::Str(s) => f.write_str(s),
			Value::Num(n) => f.write_str(&format_num(*n)),
			Value::Bool(b) => f.write_str(if *b { "true" } else { "false" }),
			Value::List(items) => {
				let joined: Vec<String> = items.iter().map(Value::to_string).collect();
				f.write_str(&joined.join(" "))
			}
		}
	}
}

/// `number("12")`. Rejects `inf`/`nan` so a typo cannot become a silent value.
pub fn parse_number(s: &str) -> Result<f64, TypeError> {
	let t = s.trim();
	match t.parse::<f64>() {
		Ok(n) if n.is_finite() => Ok(n),
		_ => Err(TypeError::NotNumeric(t.to_string())),
	}
}
