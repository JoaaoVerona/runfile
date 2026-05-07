//! Tiny boolean expression language used for `if` conditions inside the
//! `commands` array.
//!
//! Grammar (informal):
//!
//! ```text
//! expr        := or
//! or          := and ( "||" and )*
//! and         := not ( "&&" not )*
//! not         := "!" not | atom
//! atom        := "(" expr ")" | comparison | truthy
//! comparison  := value ( "==" | "!=" ) value
//! truthy      := value
//! value       := substitution | quoted_string | bare_word
//! ```
//!
//! - Mixing `&&` and `||` inside a single expression requires parentheses
//!   (the parser refuses to assume a precedence). Pure `&&` or pure `||`
//!   chains are allowed.
//! - Comparisons are case-sensitive string compares.
//! - Truthiness rule: only the empty string is falsy. Every other string —
//!   including `"false"` and `"0"` — is truthy. This matches what raw
//!   shell commands see when they receive a `{{ ... }}` substitution.
//! - Substitutions inside conditions use the same `{{ ... }}` syntax as
//!   anywhere else (e.g. `{{ ARGS.env }} == prod`). The full original
//!   substring (including the leading `{{ ` and trailing ` }}`) is captured
//!   so the runtime substitutor can resolve it unchanged.
//!
//! The parser produces a [`DslExpr`] tree. Evaluation lives outside this
//! crate (in `runfile-executor`) because it needs access to the
//! substitution context.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A parsed condition expression.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DslExpr {
	/// `lhs == rhs`
	Equality(DslValue, DslValue),
	/// `lhs != rhs`
	Inequality(DslValue, DslValue),
	/// Bare value used as a truthiness test.
	Truthy(DslValue),
	/// `expr && expr` (binary chain — flattened to a vec at parse time).
	And(Vec<DslExpr>),
	/// `expr || expr` (binary chain — flattened to a vec at parse time).
	Or(Vec<DslExpr>),
	/// `!expr`
	Not(Box<DslExpr>),
}

/// A leaf value in a DSL expression.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DslValue {
	/// A raw `{{ ... }}` substitution expression. The full original substring
	/// (including the leading `{{ ` and trailing ` }}`) is stored so the
	/// existing substitution machinery can resolve it unchanged.
	Substitution(String),
	/// A literal string (either bare-word or quoted).
	Literal(String),
}

#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum DslParseError {
	#[error("Empty condition")]
	EmptyCondition,

	#[error("Unexpected character '{0}' at position {1}")]
	UnexpectedChar(char, usize),

	#[error("Unterminated string literal starting at position {0}")]
	UnterminatedString(usize),

	#[error("Unterminated `{{{{ ... }}}}` substitution starting at position {0}")]
	UnterminatedSubstitution(usize),

	#[error("Unexpected end of input — expected {0}")]
	UnexpectedEnd(&'static str),

	#[error("Expected {0} but found '{1}' at position {2}")]
	Expected(&'static str, String, usize),

	#[error(
		"Cannot mix `&&` and `||` in the same expression without parentheses (use `(a && b) || c` or `a && (b || c)`)"
	)]
	MixedAndOr,

	#[error("Empty parenthesised expression")]
	EmptyParens,

	#[error("Unbalanced parentheses")]
	UnbalancedParens,
}

/// Parse a condition string into a [`DslExpr`] AST.
pub fn parse_condition(source: &str) -> Result<DslExpr, DslParseError> {
	let trimmed = source.trim();
	if trimmed.is_empty() {
		return Err(DslParseError::EmptyCondition);
	}
	let tokens = tokenize(source)?;
	let mut parser = Parser { tokens, pos: 0 };
	let expr = parser.parse_or()?;
	if parser.pos < parser.tokens.len() {
		let tok = &parser.tokens[parser.pos];
		return Err(DslParseError::Expected(
			"end of expression",
			tok.text.clone(),
			tok.start,
		));
	}
	Ok(expr)
}

// ─── Tokenizer ───

#[derive(Debug, Clone, PartialEq, Eq)]
enum TokKind {
	LParen,
	RParen,
	EqEq,
	NotEq,
	AndAnd,
	OrOr,
	Bang,
	Substitution, // raw `{{ ... }}` including delimiters
	String,       // quoted or bare-word; payload in `text` is the string content
}

#[derive(Debug, Clone)]
struct Token {
	kind: TokKind,
	text: String,
	start: usize,
}

fn tokenize(source: &str) -> Result<Vec<Token>, DslParseError> {
	let mut tokens = Vec::new();
	let bytes = source.as_bytes();
	let mut i = 0usize;

	while i < bytes.len() {
		let b = bytes[i];

		// Whitespace
		if b.is_ascii_whitespace() {
			i += 1;
			continue;
		}

		// Two-char operators
		if i + 1 < bytes.len() {
			let pair = &bytes[i..i + 2];
			match pair {
				b"==" => {
					tokens.push(Token {
						kind: TokKind::EqEq,
						text: "==".into(),
						start: i,
					});
					i += 2;
					continue;
				}
				b"!=" => {
					tokens.push(Token {
						kind: TokKind::NotEq,
						text: "!=".into(),
						start: i,
					});
					i += 2;
					continue;
				}
				b"&&" => {
					tokens.push(Token {
						kind: TokKind::AndAnd,
						text: "&&".into(),
						start: i,
					});
					i += 2;
					continue;
				}
				b"||" => {
					tokens.push(Token {
						kind: TokKind::OrOr,
						text: "||".into(),
						start: i,
					});
					i += 2;
					continue;
				}
				_ => {}
			}
		}

		// Single-char operators (top-level only — `(`/`)` inside a function
		// call are part of the value token; see `read_value` below).
		match b {
			b'(' => {
				tokens.push(Token {
					kind: TokKind::LParen,
					text: "(".into(),
					start: i,
				});
				i += 1;
				continue;
			}
			b')' => {
				tokens.push(Token {
					kind: TokKind::RParen,
					text: ")".into(),
					start: i,
				});
				i += 1;
				continue;
			}
			b'!' => {
				tokens.push(Token {
					kind: TokKind::Bang,
					text: "!".into(),
					start: i,
				});
				i += 1;
				continue;
			}
			_ => {}
		}

		// `{{ ... }}` substitution — capture the raw text including delimiters.
		// The substituter (in `runfile-executor::args`) handles the inner
		// content; here we just need to find the matching `}}` while tracking
		// nested `{{ }}` and quote state so we don't close on a `}}` that's
		// part of a single-quoted interpolated string.
		if b == b'{' && bytes.get(i + 1) == Some(&b'{') {
			let start = i;
			i += 2;
			let mut depth = 0i32;
			let mut in_single = false;
			let mut in_double = false;
			let mut closed = false;
			while i < bytes.len() {
				let c = bytes[i];
				if in_single {
					if c == b'\'' {
						in_single = false;
					}
					i += 1;
					continue;
				}
				if in_double {
					if c == b'"' {
						in_double = false;
					}
					i += 1;
					continue;
				}
				match c {
					b'\'' => {
						in_single = true;
						i += 1;
					}
					b'"' => {
						in_double = true;
						i += 1;
					}
					b'{' if bytes.get(i + 1) == Some(&b'{') => {
						depth += 1;
						i += 2;
					}
					b'}' if bytes.get(i + 1) == Some(&b'}') => {
						if depth == 0 {
							i += 2;
							closed = true;
							break;
						}
						depth -= 1;
						i += 2;
					}
					_ => i += 1,
				}
			}
			if !closed {
				return Err(DslParseError::UnterminatedSubstitution(start));
			}
			let text = source[start..i].to_string();
			tokens.push(Token {
				kind: TokKind::Substitution,
				text,
				start,
			});
			continue;
		}

		// Read a Value token. Values are opaque chunks of substitution-body
		// text — quoted strings, source refs (`ARGS.x`), or function calls
		// (`to_upper(...)`). The DSL evaluator will hand the raw text back
		// to the substitution machinery (see `runfile-executor::args::evaluate_arg`)
		// so the same rules apply as inside a chain or function arg:
		// single-quotes interpolate, double-quotes are kept literal, source
		// refs resolve, function calls evaluate, raw barewords error.
		let (next_i, text) = read_value(source, i)?;
		tokens.push(Token {
			kind: TokKind::String,
			text,
			start: i,
		});
		i = next_i;
	}

	Ok(tokens)
}

/// Read a single Value token (an opaque substitution-body fragment) starting
/// at byte `start`. Handles:
/// - `'...'` and `"..."` quoted strings (consumed verbatim, including quotes)
/// - bareword identifiers (`ARGS.x`, `RUN.os`, `prod`, etc.)
/// - function calls (`<ident>(...)` — the entire call including balanced
///   parens is part of the value)
///
/// Returns the byte position after the consumed value and the raw text.
fn read_value(source: &str, start: usize) -> Result<(usize, String), DslParseError> {
	let bytes = source.as_bytes();
	if start >= bytes.len() {
		return Err(DslParseError::UnexpectedEnd("a value"));
	}
	let mut i = start;
	let first = bytes[i];

	// Quoted string — read until matching quote.
	if first == b'"' || first == b'\'' {
		i += 1;
		while i < bytes.len() && bytes[i] != first {
			i += 1;
		}
		if i >= bytes.len() {
			return Err(DslParseError::UnterminatedString(start));
		}
		i += 1; // closing quote
		return Ok((i, source[start..i].to_string()));
	}

	// Bareword (or source-ref or function-call name).
	if is_bareword_byte(first) {
		while i < bytes.len() && is_bareword_byte(bytes[i]) {
			i += 1;
		}
		// If followed by `(`, this is a function call — read the balanced
		// argument list as part of the value.
		if i < bytes.len() && bytes[i] == b'(' {
			i += 1;
			let mut depth = 1i32;
			let mut in_single = false;
			let mut in_double = false;
			while i < bytes.len() && depth > 0 {
				let c = bytes[i];
				if in_single {
					if c == b'\'' {
						in_single = false;
					}
					i += 1;
					continue;
				}
				if in_double {
					if c == b'"' {
						in_double = false;
					}
					i += 1;
					continue;
				}
				match c {
					b'\'' => {
						in_single = true;
						i += 1;
					}
					b'"' => {
						in_double = true;
						i += 1;
					}
					b'(' => {
						depth += 1;
						i += 1;
					}
					b')' => {
						depth -= 1;
						i += 1;
					}
					_ => i += 1,
				}
			}
			if depth != 0 {
				return Err(DslParseError::UnbalancedParens);
			}
		}
		return Ok((i, source[start..i].to_string()));
	}

	Err(DslParseError::UnexpectedChar(first as char, start))
}

fn is_bareword_byte(b: u8) -> bool {
	b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'/' | b':' | b'-')
}

// ─── Parser ───

struct Parser {
	tokens: Vec<Token>,
	pos: usize,
}

impl Parser {
	fn peek(&self) -> Option<&Token> {
		self.tokens.get(self.pos)
	}

	fn eat(&mut self) -> Option<Token> {
		if self.pos >= self.tokens.len() {
			return None;
		}
		let t = self.tokens[self.pos].clone();
		self.pos += 1;
		Some(t)
	}

	/// Top-level: a chain of `not` atoms joined by either `&&`s or `||`s
	/// — but not both in the same level. The caller must use parentheses
	/// to mix them.
	fn parse_or(&mut self) -> Result<DslExpr, DslParseError> {
		let first = self.parse_not()?;

		match self.peek().map(|t| t.kind.clone()) {
			Some(TokKind::AndAnd) => {
				let mut parts = vec![first];
				while matches!(self.peek().map(|t| &t.kind), Some(TokKind::AndAnd)) {
					self.pos += 1;
					parts.push(self.parse_not()?);
				}
				if matches!(self.peek().map(|t| &t.kind), Some(TokKind::OrOr)) {
					return Err(DslParseError::MixedAndOr);
				}
				Ok(DslExpr::And(parts))
			}
			Some(TokKind::OrOr) => {
				let mut parts = vec![first];
				while matches!(self.peek().map(|t| &t.kind), Some(TokKind::OrOr)) {
					self.pos += 1;
					parts.push(self.parse_not()?);
					if matches!(self.peek().map(|t| &t.kind), Some(TokKind::AndAnd)) {
						return Err(DslParseError::MixedAndOr);
					}
				}
				Ok(DslExpr::Or(parts))
			}
			_ => Ok(first),
		}
	}

	fn parse_not(&mut self) -> Result<DslExpr, DslParseError> {
		if let Some(tok) = self.peek() {
			if tok.kind == TokKind::Bang {
				self.pos += 1;
				let inner = self.parse_not()?;
				return Ok(DslExpr::Not(Box::new(inner)));
			}
		}
		self.parse_atom()
	}

	fn parse_atom(&mut self) -> Result<DslExpr, DslParseError> {
		let tok = self
			.peek()
			.cloned()
			.ok_or(DslParseError::UnexpectedEnd("an expression"))?;

		// Parenthesised group — must contain a full expression, no leading op.
		if tok.kind == TokKind::LParen {
			self.pos += 1;
			if matches!(self.peek().map(|t| &t.kind), Some(TokKind::RParen)) {
				return Err(DslParseError::EmptyParens);
			}
			let inner = self.parse_or()?;
			match self.eat() {
				Some(t) if t.kind == TokKind::RParen => Ok(inner),
				Some(t) => Err(DslParseError::Expected("')'", t.text, t.start)),
				None => Err(DslParseError::UnbalancedParens),
			}
		} else {
			// Otherwise must start with a value.
			let lhs = self.parse_value()?;

			match self.peek().map(|t| t.kind.clone()) {
				Some(TokKind::EqEq) => {
					self.pos += 1;
					let rhs = self.parse_value()?;
					Ok(DslExpr::Equality(lhs, rhs))
				}
				Some(TokKind::NotEq) => {
					self.pos += 1;
					let rhs = self.parse_value()?;
					Ok(DslExpr::Inequality(lhs, rhs))
				}
				_ => Ok(DslExpr::Truthy(lhs)),
			}
		}
	}

	fn parse_value(&mut self) -> Result<DslValue, DslParseError> {
		let tok = self.eat().ok_or(DslParseError::UnexpectedEnd("a value"))?;
		match tok.kind {
			TokKind::Substitution => Ok(DslValue::Substitution(tok.text)),
			TokKind::String => Ok(DslValue::Literal(tok.text)),
			TokKind::LParen | TokKind::RParen => Err(DslParseError::Expected("a value", tok.text, tok.start)),
			TokKind::EqEq | TokKind::NotEq | TokKind::AndAnd | TokKind::OrOr | TokKind::Bang => {
				Err(DslParseError::Expected("a value", tok.text, tok.start))
			}
		}
	}
}

#[cfg(test)]
mod dsl_unit_tests {
	use super::*;

	#[test]
	fn parses_truthy_substitution() {
		let ast = parse_condition("{{ ARGS.x }}").unwrap();
		assert_eq!(ast, DslExpr::Truthy(DslValue::Substitution("{{ ARGS.x }}".into())));
	}

	#[test]
	fn parses_equality() {
		let ast = parse_condition("{{ ARGS.env }} == production").unwrap();
		assert_eq!(
			ast,
			DslExpr::Equality(
				DslValue::Substitution("{{ ARGS.env }}".into()),
				DslValue::Literal("production".into()),
			)
		);
	}

	#[test]
	fn parses_inequality() {
		let ast = parse_condition("{{ ARGS.env }} != \"staging\"").unwrap();
		assert_eq!(
			ast,
			DslExpr::Inequality(
				DslValue::Substitution("{{ ARGS.env }}".into()),
				DslValue::Literal("\"staging\"".into()),
			)
		);
	}

	#[test]
	fn parses_and_chain() {
		let ast = parse_condition("a && b && c").unwrap();
		match ast {
			DslExpr::And(parts) => assert_eq!(parts.len(), 3),
			_ => panic!("expected And"),
		}
	}

	#[test]
	fn parses_or_chain() {
		let ast = parse_condition("a || b || c").unwrap();
		match ast {
			DslExpr::Or(parts) => assert_eq!(parts.len(), 3),
			_ => panic!("expected Or"),
		}
	}

	#[test]
	fn rejects_mixed_and_or() {
		let err = parse_condition("a && b || c").unwrap_err();
		assert_eq!(err, DslParseError::MixedAndOr);
	}

	#[test]
	fn allows_grouped_mix() {
		assert!(parse_condition("(a && b) || c").is_ok());
		assert!(parse_condition("a || (b && c)").is_ok());
	}

	#[test]
	fn parses_negation() {
		let ast = parse_condition("!a").unwrap();
		match ast {
			DslExpr::Not(_) => {}
			_ => panic!(),
		}
	}

	#[test]
	fn parses_double_negation() {
		let ast = parse_condition("!!a").unwrap();
		match ast {
			DslExpr::Not(inner) => match *inner {
				DslExpr::Not(_) => {}
				_ => panic!(),
			},
			_ => panic!(),
		}
	}

	#[test]
	fn parses_quoted_strings() {
		// Under the new tokenizer, value tokens keep their RAW text (including
		// the surrounding quotes). The evaluator (in `runfile-executor::args`)
		// then unwraps single-quoted strings (interpolated) or keeps double-
		// quoted strings verbatim — that decision happens at evaluation time,
		// not parse time.
		let ast = parse_condition("'foo bar' == \"baz\"").unwrap();
		assert_eq!(
			ast,
			DslExpr::Equality(
				DslValue::Literal("'foo bar'".into()),
				DslValue::Literal("\"baz\"".into()),
			)
		);
	}

	#[test]
	fn rejects_empty_condition() {
		assert_eq!(parse_condition(""), Err(DslParseError::EmptyCondition));
		assert_eq!(parse_condition("   "), Err(DslParseError::EmptyCondition));
	}

	#[test]
	fn rejects_unterminated_string() {
		match parse_condition("\"oops").unwrap_err() {
			DslParseError::UnterminatedString(_) => {}
			e => panic!("got {e:?}"),
		}
	}

	#[test]
	fn rejects_unterminated_substitution() {
		match parse_condition("{{ ARGS.x").unwrap_err() {
			DslParseError::UnterminatedSubstitution(_) => {}
			e => panic!("got {e:?}"),
		}
	}

	#[test]
	fn rejects_empty_parens() {
		assert_eq!(parse_condition("()"), Err(DslParseError::EmptyParens));
	}

	#[test]
	fn rejects_unbalanced_parens() {
		match parse_condition("(a").unwrap_err() {
			DslParseError::UnbalancedParens | DslParseError::UnexpectedEnd(_) => {}
			e => panic!("got {e:?}"),
		}
	}

	#[test]
	fn parses_flag_ternary_substitution() {
		// `{{ FLAGS.x ? on : off }}` — colon inside but the inner content is
		// captured as a single substitution token by the DSL tokenizer.
		let ast = parse_condition("{{ FLAGS.x ? on : off }} == on").unwrap();
		assert!(matches!(ast, DslExpr::Equality(..)));
	}

	#[test]
	fn parses_complex_expression() {
		let ast = parse_condition(
			"{{ ARGS.env }} == production && ({{ FLAGS.confirm }} == true || {{ ENV.CI }} == \"true\")",
		)
		.unwrap();
		match ast {
			DslExpr::And(parts) => assert_eq!(parts.len(), 2),
			_ => panic!(),
		}
	}

	#[test]
	fn parses_negated_truthy() {
		let ast = parse_condition("!{{ ARGS.skip }}").unwrap();
		match ast {
			DslExpr::Not(inner) => assert!(matches!(*inner, DslExpr::Truthy(_))),
			_ => panic!(),
		}
	}

	#[test]
	fn parses_paths_as_barewords() {
		let ast = parse_condition("{{ ENV.PATH }} != /usr/local/bin").unwrap();
		assert!(matches!(ast, DslExpr::Inequality(..)));
	}
}
