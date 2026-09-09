//! Tokens, string scanning and interpolation splitting.
//!
//! Three rules here cannot be expressed in the EBNF, and a prototype parser run
//! against the corpus found each one the hard way:
//!
//! 1. A string literal cannot be lexed by regex. Scanning must recursively skip
//!    `{{ … }}`, so a quote inside an interpolation does not end the string.
//! 2. In a raw string the backslash is kept in the value but still prevents
//!    termination -- Python's rule. Fully inert backslashes would make a quote
//!    impossible in a regex, which real targets need.
//! 3. `exec` bodies terminate on an `end` at the opener's indentation, handled
//!    in the parser where the block structure lives.

use crate::span::Span;
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum LexError {
	#[error("line {line}: unterminated string literal")]
	UnterminatedString { line: usize },
	#[error("line {line}: unterminated `{{{{ … }}}}` interpolation")]
	UnterminatedInterp { line: usize },
	#[error("line {line}: unexpected character `{ch}`")]
	UnexpectedChar { ch: char, line: usize },
	#[error("line {line}: unknown escape `\\{ch}`")]
	UnknownEscape { ch: char, line: usize },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
	Number(f64),
	/// Literal text and interpolation slices, unresolved -- the parser turns the
	/// slices into expressions so the lexer stays free of recursion into it.
	Str(Vec<RawPart>),
	Ident(String),
	Punct(&'static str),
}

/// A piece of interpolated text: literal, or the source of a `{{ … }}` body.
#[derive(Debug, Clone, PartialEq)]
pub enum RawPart {
	Literal(String),
	Expr { text: String, span: Span },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Spanned {
	pub token: Token,
	pub span: Span,
}

const PUNCT: &[&str] = &[
	"==", "!=", "<=", ">=", "&&", "||", "+", "-", "*", "/", "%", "<", ">", "!", "?", "[", "]", "(", ")", ",", ".",
];

/// Index just past the `}}` matching the `{{` at `i`. Nested blocks are opaque:
/// quotes, parens and commas inside them cannot affect the enclosing scanner.
pub fn skip_interp(s: &[u8], mut i: usize, line: usize) -> Result<usize, LexError> {
	let mut depth = 0usize;
	while i < s.len() {
		if s[i..].starts_with(b"{{") {
			depth += 1;
			i += 2;
		} else if s[i..].starts_with(b"}}") {
			depth -= 1;
			i += 2;
			if depth == 0 {
				return Ok(i);
			}
		} else {
			i += 1;
		}
	}
	Err(LexError::UnterminatedInterp { line })
}

/// Split text into literal runs and interpolation slices. Used for shell lines,
/// `exec` bodies and the inside of string literals alike -- they interpolate by
/// the same rule. `\{{` escapes a literal `{{`.
pub fn split_interp(text: &str, base: usize, line: usize) -> Result<Vec<RawPart>, LexError> {
	let b = text.as_bytes();
	let (mut parts, mut lit, mut i) = (Vec::new(), String::new(), 0usize);
	while i < b.len() {
		if b[i] == b'\\' && b[i + 1..].starts_with(b"{{") {
			lit.push_str("{{");
			i += 3;
			continue;
		}
		if b[i..].starts_with(b"{{") {
			let end = skip_interp(b, i, line)?;
			if !lit.is_empty() {
				parts.push(RawPart::Literal(std::mem::take(&mut lit)));
			}
			// trim the delimiters, then the single space the syntax requires
			let inner = text[i + 2..end - 2].trim().to_string();
			parts.push(RawPart::Expr {
				text: inner,
				span: Span::new(base + i, base + end, line),
			});
			i = end;
			continue;
		}
		lit.push(text[i..].chars().next().unwrap());
		i += text[i..].chars().next().unwrap().len_utf8();
	}
	if !lit.is_empty() {
		parts.push(RawPart::Literal(lit));
	}
	Ok(parts)
}

/// Scan a `"…"` or `r"…"` literal starting at `i`. Returns the parts and the
/// index just past the closing quote.
pub fn scan_string(s: &str, i: usize, line: usize, raw: bool) -> Result<(Vec<RawPart>, usize), LexError> {
	let b = s.as_bytes();
	let open = if raw { i + 2 } else { i + 1 };
	let mut j = open;
	while j < b.len() {
		if b[j..].starts_with(b"{{") {
			j = skip_interp(b, j, line)?;
			continue;
		}
		match b[j] {
			// In both forms a backslash prevents termination. In a raw string it
			// is also kept in the value, so a regex can contain a quote.
			b'\\' => j += 2,
			b'"' => {
				let body = &s[open..j];
				let parts = if raw {
					split_interp(body, open, line)?
				} else {
					unescape_parts(split_interp(body, open, line)?, line)?
				};
				return Ok((parts, j + 1));
			}
			_ => j += 1,
		}
	}
	Err(LexError::UnterminatedString { line })
}

fn unescape_parts(parts: Vec<RawPart>, line: usize) -> Result<Vec<RawPart>, LexError> {
	parts
		.into_iter()
		.map(|p| match p {
			RawPart::Literal(t) => unescape(&t, line).map(RawPart::Literal),
			other => Ok(other),
		})
		.collect()
}

fn unescape(t: &str, line: usize) -> Result<String, LexError> {
	let mut out = String::with_capacity(t.len());
	let mut it = t.chars();
	while let Some(c) = it.next() {
		if c != '\\' {
			out.push(c);
			continue;
		}
		match it.next() {
			Some('"') => out.push('"'),
			Some('\\') => out.push('\\'),
			Some('n') => out.push('\n'),
			Some('t') => out.push('\t'),
			Some('r') => out.push('\r'),
			// ESC. Colour is what `printf` is for, and without this every
			// coloured line had to stay a shell line.
			Some('e') => out.push('\u{1b}'),
			Some(other) => return Err(LexError::UnknownEscape { ch: other, line }),
			None => return Err(LexError::UnterminatedString { line }),
		}
	}
	Ok(out)
}

/// Tokenize one expression. `base` is the byte offset of `s` within the file.
pub fn tokenize(s: &str, base: usize, line: usize) -> Result<Vec<Spanned>, LexError> {
	let b = s.as_bytes();
	let mut out = Vec::new();
	let mut i = 0usize;
	while i < b.len() {
		let c = b[i];
		if c.is_ascii_whitespace() {
			i += 1;
			continue;
		}
		let start = i;
		if c == b'r' && b[i + 1..].starts_with(b"\"") {
			let (parts, next) = scan_string(s, i, line, true)?;
			out.push(sp(Token::Str(parts), base, start, next, line));
			i = next;
			continue;
		}
		if c == b'"' {
			let (parts, next) = scan_string(s, i, line, false)?;
			out.push(sp(Token::Str(parts), base, start, next, line));
			i = next;
			continue;
		}
		if c.is_ascii_digit() {
			let mut j = i;
			while j < b.len() && (b[j].is_ascii_digit() || b[j] == b'.') {
				j += 1;
			}
			let n: f64 = s[i..j]
				.parse()
				.map_err(|_| LexError::UnexpectedChar { ch: c as char, line })?;
			out.push(sp(Token::Number(n), base, start, j, line));
			i = j;
			continue;
		}
		if c.is_ascii_alphabetic() || c == b'_' {
			let mut j = i;
			while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] == b'-') {
				j += 1;
			}
			out.push(sp(Token::Ident(s[i..j].to_string()), base, start, j, line));
			i = j;
			continue;
		}
		if let Some(p) = PUNCT.iter().find(|p| s[i..].starts_with(**p)) {
			out.push(sp(Token::Punct(p), base, start, i + p.len(), line));
			i += p.len();
			continue;
		}
		return Err(LexError::UnexpectedChar { ch: c as char, line });
	}
	Ok(out)
}

fn sp(token: Token, base: usize, start: usize, end: usize, line: usize) -> Spanned {
	Spanned {
		token,
		span: Span::new(base + start, base + end, line),
	}
}

/// How a line moves the `[` depth, and how many of the brackets it closes were
/// opened before it.
///
/// Counted from the *tokens*, so a bracket inside a string is text rather than
/// structure: `["x[y"]` is one complete line, not the start of one. Counting
/// characters was close enough while a list held only short words, and stops
/// being so the moment one holds a path or a glob.
///
/// The second number is what lets a nested list be laid out: a line starting
/// with `]` belongs to the level it closes, not to the one its contents were
/// in.
///
/// A whole line often will not tokenise -- a bare `=` is not an operator this
/// language has -- so the right-hand side is tried next, and the characters
/// last, which is what this always did.
pub fn brackets(text: &str, no: usize) -> (i32, i32) {
	let toks = tokenize(text, 0, no)
		.ok()
		.or_else(|| text.split_once(" = ").and_then(|(_, r)| tokenize(r, 0, no).ok()));
	let Some(toks) = toks else {
		let open = i32::try_from(text.matches('[').count()).unwrap_or(0);
		let close = i32::try_from(text.matches(']').count()).unwrap_or(0);
		return (open - close, 0);
	};
	let (mut delta, mut leading, mut still_leading) = (0, 0, true);
	for t in &toks {
		match &t.token {
			Token::Punct("[") => {
				delta += 1;
				still_leading = false;
			}
			Token::Punct("]") => {
				delta -= 1;
				if still_leading {
					leading += 1;
				}
			}
			// A `,` after a closer is part of closing it, not a new thing.
			Token::Punct(",") => {}
			_ => still_leading = false,
		}
	}
	(delta, leading)
}
