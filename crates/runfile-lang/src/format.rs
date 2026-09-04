//! Pretty-printing `.run` source.
//!
//! Line-oriented, like the language itself. An AST printer was the other
//! option and is the wrong one here: the tree keeps no comments, and it folds
//! a run of `$` lines into one statement, so printing from it would delete
//! what a person wrote. Walking the lines keeps every one of them and changes
//! only their shape.
//!
//! Three regions are never touched:
//!
//! * **Strings.** Rendering is done from the lexer's own token spans, so a
//!   string is copied out of the source verbatim -- raw prefix, escapes,
//!   interpolations and all. Nothing has to be reconstructed, so nothing can
//!   be reconstructed wrongly.
//! * **Shell.** The text after `$ ` is the shell's, not ours. A continuation
//!   line reaches the shell *raw*, indentation included, so those are copied
//!   byte for byte.
//! * **`exec` bodies.** The body is data -- a Python script, a heredoc, an SQL
//!   statement. Only its base indentation moves, which the parser strips
//!   anyway, and relative indentation and trailing space survive untouched.
//!
//! There are no options. A formatter that can be configured is a formatter
//! every project configures differently.

use crate::lexer::{self, Token};
use crate::parser::ParseError;

/// One tab per block level, which is what the corpus already uses.
const INDENT: &str = "\t";

/// Format a whole file.
///
/// Fails on source that does not parse: reindenting a file whose blocks do not
/// close is guesswork, and guessing at a broken file is how a formatter eats
/// someone's work.
pub fn format(src: &str) -> Result<String, ParseError> {
	let before = crate::parse(src)?;
	let out = render(src)?;
	// The formatter must not be able to change what a file means. Re-parsing
	// and comparing the tree is cheap next to being wrong once.
	let after = crate::parse(&out)?;
	if crate::fingerprint(&before) != crate::fingerprint(&after) {
		return Err(ParseError::At {
			line: 0,
			msg: "formatting would have changed this file's meaning; it was left alone".into(),
		});
	}
	Ok(out)
}

/// What a level of the block stack does to indentation.
#[derive(Clone, Copy, PartialEq)]
enum Frame {
	/// `if`, `else`, `for`, `exec` -- its body is one level in.
	Body,
	/// `match` -- its `case`s sit at the *same* level, so it adds nothing.
	Match,
	/// `case`, `default` -- one level in, and closed by the next `case` or the
	/// `end`, never by one of its own.
	Case,
}

struct Out {
	lines: Vec<String>,
	stack: Vec<Frame>,
}

impl Out {
	fn depth(&self) -> usize {
		self.stack.iter().filter(|f| **f != Frame::Match).count()
	}

	fn push(&mut self, depth: usize, text: &str) {
		if text.is_empty() {
			self.lines.push(String::new());
		} else {
			self.lines.push(format!("{}{text}", INDENT.repeat(depth)));
		}
	}

	fn at_depth(&mut self, text: &str) {
		let d = self.depth();
		self.push(d, text);
	}
}

fn render(src: &str) -> Result<String, ParseError> {
	let raw: Vec<&str> = src.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
	let mut o = Out {
		lines: Vec::new(),
		stack: Vec::new(),
	};
	let mut i = 0;
	while i < raw.len() {
		let line = raw[i];
		let trimmed = line.trim();
		let no = i + 1;

		if trimmed.is_empty() {
			o.lines.push(String::new());
			i += 1;
			continue;
		}
		if trimmed.starts_with('#') {
			o.at_depth(trimmed);
			i += 1;
			continue;
		}
		// A `$` line, and the raw continuation lines that belong to it.
		if trimmed == "$" || trimmed.starts_with("$ ") {
			let body = trimmed.strip_prefix('$').unwrap().trim_start();
			let shell = if body.is_empty() {
				"$".to_string()
			} else {
				format!("$ {body}")
			};
			o.at_depth(&shell);
			let mut last = trimmed.to_string();
			while last.ends_with('\\') && i + 1 < raw.len() {
				i += 1;
				o.lines.push(raw[i].to_string());
				last = raw[i].to_string();
			}
			i += 1;
			continue;
		}
		if let Some(cmd) = trimmed.strip_prefix("exec ") {
			let d = o.depth();
			o.push(d, &format!("exec {}", cmd.trim()));
			i = exec_body(&raw, i + 1, line, d, &mut o);
			continue;
		}

		let head = trimmed.split_whitespace().next().unwrap_or("");
		match head {
			"end" => {
				if o.stack.last() == Some(&Frame::Case) {
					o.stack.pop();
				}
				o.stack.pop();
				o.at_depth("end");
			}
			"else" => {
				o.stack.pop();
				o.at_depth("else");
				o.stack.push(Frame::Body);
			}
			"case" | "default" => {
				if o.stack.last() == Some(&Frame::Case) {
					o.stack.pop();
				}
				// The label is matched textually by the parser, quotes and
				// all, so it is passed through rather than re-rendered.
				let rest = trimmed[head.len()..].trim();
				o.at_depth(&if rest.is_empty() {
					head.to_string()
				} else {
					format!("{head} {rest}")
				});
				o.stack.push(Frame::Case);
			}
			_ => {
				i = statement(&raw, i, trimmed, no, &mut o)?;
				continue;
			}
		}
		i += 1;
	}
	Ok(finish(o.lines))
}

/// Copy an `exec` body through, moving only its base indentation.
///
/// The terminator is found the way the parser finds it -- an `end` at the
/// opener's own indentation -- so a body containing its own `end` (ruby, lua)
/// is left whole here too.
fn exec_body(raw: &[&str], from: usize, opener: &str, depth: usize, o: &mut Out) -> usize {
	let indent = &opener[..opener.len() - opener.trim_start().len()];
	let closer = format!("{indent}end");
	let mut j = from;
	let mut body: Vec<&str> = Vec::new();
	while j < raw.len() {
		if raw[j] == closer || (indent.is_empty() && raw[j].trim_end() == "end") {
			break;
		}
		body.push(raw[j]);
		j += 1;
	}
	let base = body
		.iter()
		.filter(|l| !l.trim().is_empty())
		.map(|l| l.len() - l.trim_start().len())
		.min()
		.unwrap_or(0);
	for l in &body {
		let text = if l.len() >= base { &l[base..] } else { "" };
		if text.trim().is_empty() {
			o.lines.push(String::new());
		} else {
			o.lines.push(format!("{}{text}", INDENT.repeat(depth + 1)));
		}
	}
	if j < raw.len() {
		o.push(depth, "end");
	}
	j + 1
}

/// A statement line, plus any lines a list literal spills onto.
fn statement(raw: &[&str], i: usize, trimmed: &str, no: usize, o: &mut Out) -> Result<usize, ParseError> {
	let head = trimmed.split_whitespace().next().unwrap_or("");
	let (text, opens) = match head {
		"if" => (format!("if {}", spaced(trimmed[2..].trim(), no)?), Some(Frame::Body)),
		"match" => (
			format!("match {}", spaced(trimmed[5..].trim(), no)?),
			Some(Frame::Match),
		),
		"for" => {
			let rest = trimmed[3..].trim();
			match rest.find(" in ") {
				Some(k) => (
					format!("for {} in {}", rest[..k].trim(), spaced(rest[k + 4..].trim(), no)?),
					Some(Frame::Body),
				),
				None => (trimmed.to_string(), Some(Frame::Body)),
			}
		}
		// A `run` argument is one whitespace-delimited word; the words are
		// separated by exactly one space and otherwise left as written.
		"run" => (format!("run {}", spaced_words(trimmed[3..].trim())), None),
		_ => {
			if trimmed.starts_with('.') {
				(property(trimmed, no)?, None)
			} else {
				assignment(trimmed, no)?
			}
		}
	};

	// A capture with a body opens a block whose contents are opaque.
	if let Some(rest) = text.split(" = ").nth(1)
		&& let Some(cmd) = rest.strip_prefix("exec ")
	{
		let d = o.depth();
		let head = text[..text.len() - rest.len()].to_string();
		o.push(d, &format!("{head}exec {}", cmd.trim()));
		return Ok(exec_body(raw, i + 1, raw[i], d, o));
	}

	let d = o.depth();
	o.push(d, &text);
	// `let x = [` and its continuation lines are one logical line to the
	// parser, so they are indented as a run rather than as statements -- and
	// against the *opener's* depth, since `for x in [` spills its list before
	// the loop body starts, not inside it.
	let mut open = brackets(&text, no)?;
	let mut j = i + 1;
	while open > 0 && j < raw.len() {
		let t = raw[j].trim();
		let rendered = if t.is_empty() { String::new() } else { spaced(t, no)? };
		let delta = brackets(&rendered, no)?;
		o.push(if open + delta <= 0 { d } else { d + 1 }, &rendered);
		open += delta;
		j += 1;
	}
	if let Some(f) = opens {
		o.stack.push(f);
	}
	Ok(j)
}

/// `[` minus `]` among a line's tokens -- strings and interpolations excluded,
/// since a bracket inside one is text.
fn brackets(text: &str, no: usize) -> Result<i32, ParseError> {
	let Some(rhs) = text.split_once(" = ").map(|(_, r)| r).or(Some(text)) else {
		return Ok(0);
	};
	let Ok(toks) = lexer::tokenize(rhs, 0, no) else {
		return Ok(0);
	};
	Ok(toks
		.iter()
		.map(|t| match &t.token {
			Token::Punct("[") => 1,
			Token::Punct("]") => -1,
			_ => 0,
		})
		.sum())
}

fn property(trimmed: &str, no: usize) -> Result<String, ParseError> {
	match trimmed.find('=') {
		Some(k) => {
			let name = trimmed[1..k].trim();
			let value = trimmed[k + 1..].trim();
			Ok(format!(".{name} = {}", rhs(value, no)?))
		}
		None => Ok(format!(".{}", trimmed[1..].trim())),
	}
}

fn assignment(trimmed: &str, no: usize) -> Result<(String, Option<Frame>), ParseError> {
	if let Some(rest) = trimmed.strip_prefix("let ")
		&& let Some(k) = rest.find('=')
	{
		let name = rest[..k].trim();
		return Ok((format!("let {name} = {}", rhs(rest[k + 1..].trim(), no)?), None));
	}
	if let Some(k) = assignment_split(trimmed) {
		let name = trimmed[..k].trim();
		if is_name(name) {
			return Ok((format!("{name} = {}", rhs(trimmed[k + 1..].trim(), no)?), None));
		}
	}
	Ok((spaced(trimmed, no)?, None))
}

/// The right-hand side of a binding: a capture runs to end of line and is the
/// shell's text, so only a plain expression is re-rendered.
fn rhs(text: &str, no: usize) -> Result<String, ParseError> {
	if let Some(cmd) = text.strip_prefix("$ ") {
		return Ok(format!("$ {}", cmd.trim()));
	}
	if let Some(cmd) = text.strip_prefix("exec ") {
		return Ok(format!("exec {}", cmd.trim()));
	}
	spaced(text, no)
}

/// A `=` that is an assignment rather than part of `==`, `!=`, `<=` or `>=`.
fn assignment_split(text: &str) -> Option<usize> {
	let b = text.as_bytes();
	let mut i = 0;
	while i < b.len() {
		match b[i] {
			b'"' => {
				// Skip the string, so an `=` inside one is not an assignment.
				let Ok((_, next)) = lexer::scan_string(text, i, 1, false) else {
					return None;
				};
				i = next;
			}
			b'=' if b.get(i + 1) != Some(&b'=')
				&& !matches!(b.get(i.wrapping_sub(1)), Some(b'=' | b'!' | b'<' | b'>')) =>
			{
				return Some(i);
			}
			_ => i += 1,
		}
	}
	None
}

fn is_name(s: &str) -> bool {
	!s.is_empty()
		&& s.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
		&& s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Words separated by one space, each left exactly as written.
///
/// Only for `run`, whose arguments the parser re-splits on whitespace. An
/// `exec` command is *not* re-split -- its text reaches the shell as written --
/// so collapsing the spaces inside one would change what runs.
fn spaced_words(s: &str) -> String {
	s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Clone, Copy, PartialEq)]
enum Cls {
	Atom,
	Open,
	Close,
	Comma,
	Dot,
	Op,
}

fn classify(t: &Token) -> Cls {
	match t {
		Token::Punct("(" | "[") => Cls::Open,
		Token::Punct(")" | "]") => Cls::Close,
		Token::Punct(",") => Cls::Comma,
		Token::Punct(".") => Cls::Dot,
		Token::Punct(_) => Cls::Op,
		_ => Cls::Atom,
	}
}

/// Re-render an expression with canonical spacing.
///
/// Each token is copied out of the source by its own span, so a string arrives
/// exactly as written; only what sits *between* tokens is decided here.
fn spaced(expr: &str, no: usize) -> Result<String, ParseError> {
	let toks = lexer::tokenize(expr, 0, no).map_err(ParseError::Lex)?;
	let mut out = String::new();
	let mut prev: Option<(Cls, bool)> = None;
	for t in &toks {
		let text = &expr[t.span.start..t.span.end];
		let cls = classify(&t.token);
		let unary = cls == Cls::Op
			&& matches!(text, "-" | "!")
			&& prev.is_none_or(|(c, _)| matches!(c, Cls::Op | Cls::Open | Cls::Comma));
		if let Some(p) = prev
			&& space_between(p, cls)
		{
			out.push(' ');
		}
		out.push_str(text);
		prev = Some((cls, unary));
	}
	Ok(out)
}

fn space_between((pc, punary): (Cls, bool), cur: Cls) -> bool {
	match cur {
		Cls::Close | Cls::Comma | Cls::Dot => false,
		// A `(` or `[` right after a value is a call or an index, not a group
		// or a list, and those bind tight.
		Cls::Open => !matches!(pc, Cls::Atom | Cls::Close | Cls::Open | Cls::Dot),
		_ => !(pc == Cls::Dot || pc == Cls::Open || (pc == Cls::Op && punary)),
	}
}

/// One trailing newline, no leading blank lines, and never two blanks running.
fn finish(lines: Vec<String>) -> String {
	let mut out: Vec<String> = Vec::with_capacity(lines.len());
	for l in lines {
		if l.is_empty() && (out.is_empty() || out.last().is_some_and(String::is_empty)) {
			continue;
		}
		out.push(l);
	}
	while out.last().is_some_and(String::is_empty) {
		out.pop();
	}
	if out.is_empty() {
		return String::new();
	}
	let mut s = out.join("\n");
	s.push('\n');
	s
}
