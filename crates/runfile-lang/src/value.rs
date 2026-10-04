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

/// Where an interpolation sits in the shell text around it, which is what its
/// value has to be written for to arrive as one inert word.
///
/// [`Value::to_shell`] single-quotes, and single quotes are inert only where
/// they are quotes. Inside the author's `"…"` they are plain characters and the
/// `$`, backticks and `\` of the value still expand -- so a value read from a
/// branch name or a PR title ran as code in `echo "Deploying {{ v }}"` and in
/// `ssh host "cd {{ dir }}"`, two shapes the rules call correct. The shell
/// checker reads where each interpolation is, the way bash does, and the
/// runner writes each value for that place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Quoting {
	/// Outside quotes, or where quoting starts again, as inside `$( … )`: the
	/// word as it is.
	Bare,
	/// Inside the author's `'…'`, which the word's own quotes would end: the
	/// quote is closed around the word and opened again after it.
	Single,
	/// Inside a `'…'` that bash reads as a quote but keeps in the text -- in a
	/// `${…}` inside double quotes or a heredoc, or in arithmetic: closed and
	/// reopened around the word, which is written as [`Quoting::Double`].
	SingleExpanded,
	/// Inside `$'…'`, closed and reopened the same way.
	Ansi,
	/// Where this shell expands `$`, backticks and `\` and keeps quotes as
	/// plain characters: `"…"`, a `${…}` inside them or inside a heredoc,
	/// arithmetic. The word keeps its quotes -- a command string for another
	/// shell needs them -- and what this shell would expand is escaped inside
	/// them. The `\'` between two quoted pieces is left as it is: inside a
	/// `${…}` bash still reads `'` as a quote while it looks for the `}`, and an
	/// escaped backslash there would leave the quotes unbalanced.
	Double,
	/// An expanding heredoc's body: as [`Quoting::Double`], but `"` is a plain
	/// character there, so it is left alone.
	Heredoc,
	/// Text nothing expands, a quoted heredoc's body: the word as it is.
	Literal,
	/// A shell comment, which a line break would end.
	Comment,
	/// Inside `` `…` ``: written for where it sits inside them, then escaped
	/// for the backticks themselves -- `"` too when they are inside `"…"`.
	Backtick(Box<Quoting>, bool),
}

/// The line that ends a heredoc, which no line of a value inside its body may
/// be: the body would end there, and what followed would run as commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delimiter {
	pub text: String,
	/// `<<-`, which takes a line's leading tabs off before comparing it.
	pub strip_tabs: bool,
}

impl Delimiter {
	/// Whether `line` would end the heredoc.
	pub fn ends(&self, line: &str) -> bool {
		let line = line.strip_suffix('\r').unwrap_or(line);
		match self.strip_tabs {
			true => line.trim_start_matches('\t') == self.text,
			false => line == self.text,
		}
	}
}

/// Where one interpolation sits: its quoting, and every heredoc whose body it
/// is inside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spot {
	pub quoting: Quoting,
	pub heredocs: Vec<Delimiter>,
}

impl Value {
	/// [`Value::to_shell`], written for where it goes. `Err` says why when no
	/// way of writing this value is safe there.
	pub fn to_shell_at(&self, spot: &Spot) -> Result<String, String> {
		written(&self.to_shell(), &spot.quoting)
	}
}

fn written(word: &str, quoting: &Quoting) -> Result<String, String> {
	Ok(match quoting {
		Quoting::Bare | Quoting::Literal => word.to_string(),
		Quoting::Single => format!("'{word}'"),
		Quoting::SingleExpanded => format!("'{}'", in_quotes(word, &['\\', '$', '`', '"'])),
		Quoting::Ansi => format!("'{word}$'"),
		Quoting::Double => in_quotes(word, &['\\', '$', '`', '"']),
		Quoting::Heredoc => escaped(word, &['\\', '$', '`']),
		Quoting::Comment if word.contains(['\n', '\r']) => {
			return Err(
				"a value holding a line break cannot go in a shell comment: the break would end the \
			            comment, and the rest of the value would run as commands"
					.into(),
			);
		}
		Quoting::Comment => word.to_string(),
		Quoting::Backtick(inner, in_double) => {
			let inner = written(word, inner)?;
			match in_double {
				true => escaped(&inner, &['\\', '`', '$', '"']),
				false => escaped(&inner, &['\\', '`', '$']),
			}
		}
	})
}

/// `word` with a backslash in front of each of `special` inside its single
/// quotes -- and nothing outside them, where a quoted word has only the `\'`
/// joining two quoted pieces, the blank between two words of a list, and
/// characters no shell gives a meaning to.
fn in_quotes(word: &str, special: &[char]) -> String {
	let mut out = String::with_capacity(word.len() + 2);
	let mut quoted = false;
	let mut chars = word.chars();
	while let Some(c) = chars.next() {
		match c {
			'\'' => {
				quoted = !quoted;
				out.push(c);
			}
			'\\' if !quoted => {
				out.push(c);
				out.extend(chars.next());
			}
			c if quoted && special.contains(&c) => {
				out.push('\\');
				out.push(c);
			}
			c => out.push(c),
		}
	}
	out
}

/// `word` with a backslash in front of each of `special`.
fn escaped(word: &str, special: &[char]) -> String {
	let mut out = String::with_capacity(word.len() + 2);
	for c in word.chars() {
		if special.contains(&c) {
			out.push('\\');
		}
		out.push(c);
	}
	out
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
