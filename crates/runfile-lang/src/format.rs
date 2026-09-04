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

/// What a level of the block stack does to indentation. Every one of them is
/// one level, `match` included: a `case` reads as being *inside* its match.
#[derive(Clone, Copy, PartialEq)]
enum Frame {
	/// `if`, `else`, `for`, `exec`.
	Body,
	/// `match`, holding its `case`s.
	Match,
	/// `case`, `default` -- closed by the next `case` or by the `end`, never
	/// by one of its own.
	Case,
}

/// What a line is, for deciding where blank lines belong.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
	/// The leading comment block: the target's description.
	Description,
	/// A comment, which attaches to whatever follows it.
	Comment,
	/// `.name = value`.
	Property,
	/// `let name = …`.
	Let,
	/// Opens a block: `if`, `for`, `match`, `exec`.
	Open,
	/// `else`, `case`, `default`.
	Mid,
	/// `end`.
	Close,
	/// Part of the line before it -- an `exec` body, a `$` continuation.
	Opaque,
	/// Everything else: `$`, `run`, calls, reassignment.
	Other,
	Blank,
}

struct Out {
	lines: Vec<(Kind, String)>,
	stack: Vec<Frame>,
	/// Still in the leading comment block.
	heading: bool,
}

impl Out {
	fn depth(&self) -> usize {
		self.stack.len()
	}

	fn push(&mut self, depth: usize, text: &str, kind: Kind) {
		if !matches!(kind, Kind::Comment | Kind::Description | Kind::Blank) {
			self.heading = false;
		}
		if text.is_empty() {
			self.lines.push((Kind::Blank, String::new()));
		} else {
			self.lines.push((kind, format!("{}{text}", INDENT.repeat(depth))));
		}
	}

	fn at_depth(&mut self, text: &str, kind: Kind) {
		let d = self.depth();
		self.push(d, text, kind);
	}

	fn blank(&mut self) {
		// A blank line ends the description: the parser reads the *contiguous*
		// leading comment block, so a comment below the gap is an ordinary one.
		if self.lines.iter().any(|(k, _)| *k == Kind::Description) {
			self.heading = false;
		}
		self.lines.push((Kind::Blank, String::new()));
	}

	fn opaque(&mut self, text: String) {
		self.lines.push((Kind::Opaque, text));
	}
}

fn render(src: &str) -> Result<String, ParseError> {
	let raw: Vec<&str> = src.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
	let mut o = Out {
		lines: Vec::new(),
		stack: Vec::new(),
		heading: true,
	};
	let mut i = 0;
	while i < raw.len() {
		let line = raw[i];
		let trimmed = line.trim();
		let no = i + 1;

		if trimmed.is_empty() {
			o.blank();
			i += 1;
			continue;
		}
		if trimmed.starts_with('#') {
			let kind = if o.heading { Kind::Description } else { Kind::Comment };
			o.at_depth(trimmed, kind);
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
			o.at_depth(&shell, Kind::Other);
			let mut last = trimmed.to_string();
			while last.ends_with('\\') && i + 1 < raw.len() {
				i += 1;
				o.opaque(raw[i].to_string());
				last = raw[i].to_string();
			}
			i += 1;
			continue;
		}
		if let Some(cmd) = trimmed.strip_prefix("exec ") {
			let d = o.depth();
			o.push(d, &format!("exec {}", cmd.trim()), Kind::Open);
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
				o.at_depth("end", Kind::Close);
			}
			"else" => {
				o.stack.pop();
				o.at_depth("else", Kind::Mid);
				o.stack.push(Frame::Body);
			}
			"case" | "default" => {
				if o.stack.last() == Some(&Frame::Case) {
					o.stack.pop();
				}
				// The label is matched textually by the parser, quotes and
				// all, so it is passed through rather than re-rendered.
				let rest = trimmed[head.len()..].trim();
				let text = if rest.is_empty() {
					head.to_string()
				} else {
					format!("{head} {rest}")
				};
				o.at_depth(&text, Kind::Mid);
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
			o.opaque(String::new());
		} else {
			o.opaque(format!("{}{text}", INDENT.repeat(depth + 1)));
		}
	}
	if j < raw.len() {
		o.push(depth, "end", Kind::Close);
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
		o.push(d, &format!("{head}exec {}", cmd.trim()), Kind::Open);
		return Ok(exec_body(raw, i + 1, raw[i], d, o));
	}

	let d = o.depth();
	o.push(d, &text, kind_of(head, trimmed, opens));
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
		o.push(if open + delta <= 0 { d } else { d + 1 }, &rendered, Kind::Opaque);
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

/// What a statement line counts as when blank lines are being placed.
fn kind_of(head: &str, trimmed: &str, opens: Option<Frame>) -> Kind {
	if opens.is_some() {
		return Kind::Open;
	}
	if trimmed.starts_with('.') {
		return Kind::Property;
	}
	if head == "let" {
		return Kind::Let;
	}
	Kind::Other
}

/// A statement together with the comments written above it.
///
/// Comments are not free-standing: one written directly above a statement is
/// about that statement, so a blank line belongs *before* the comment and
/// never between the two. Grouping them here is what makes that fall out.
struct Unit {
	kind: Kind,
	lines: Vec<String>,
	/// The author already left a blank line here, which is kept whatever the
	/// rules say.
	spaced: bool,
}

/// Whether a blank line belongs between two units.
///
/// Only ever *adds* one. Removing the author's own blank lines would be
/// arguing with them about the shape of their file; this is about the places
/// where a missing one makes a file harder to read.
fn wants_blank(prev: Kind, cur: Kind) -> bool {
	use Kind::*;
	match (prev, cur) {
		// Nothing is pushed away from the block it closes or continues, and
		// nothing is pushed away from the line that opened one.
		(_, Close | Mid) => false,
		(Open | Mid, _) => false,
		// The description is a paragraph of its own, then the properties.
		(Description, _) => true,
		// After a block has closed, the next thing is a new thought.
		(Close, _) => true,
		// Blocks stand out from what runs before them.
		(_, Open) => true,
		// A run of `let`s is one group; anything either side of it is not.
		(Let, x) => x != Let,
		(_, Let) => true,
		(Property, x) => x != Property,
		_ => false,
	}
}

/// Group the emitted lines, place the blank lines, and end the file properly.
fn finish(lines: Vec<(Kind, String)>) -> String {
	let mut units: Vec<Unit> = Vec::new();
	let mut pending: Vec<String> = Vec::new();
	let mut spaced = false;

	for (kind, text) in lines {
		match kind {
			Kind::Blank => {
				// A blank between a comment and what follows means the comment
				// was a remark, not a heading for the next statement.
				if !pending.is_empty() {
					units.push(Unit {
						kind: Kind::Comment,
						lines: std::mem::take(&mut pending),
						spaced,
					});
					spaced = false;
				}
				spaced = spaced || !units.is_empty();
			}
			// The description is one paragraph, however many `#` lines it took.
			Kind::Description => match units.last_mut() {
				Some(u) if u.kind == Kind::Description => u.lines.push(text),
				_ => units.push(Unit {
					kind: Kind::Description,
					lines: vec![text],
					spaced: false,
				}),
			},
			Kind::Comment => pending.push(text),
			Kind::Opaque => match units.last_mut() {
				Some(u) => u.lines.push(text),
				None => units.push(Unit {
					kind: Kind::Other,
					lines: vec![text],
					spaced: false,
				}),
			},
			_ => {
				let mut group = std::mem::take(&mut pending);
				group.push(text);
				units.push(Unit {
					kind,
					lines: group,
					spaced,
				});
				spaced = false;
			}
		}
	}
	if !pending.is_empty() {
		units.push(Unit {
			kind: Kind::Comment,
			lines: pending,
			spaced,
		});
	}

	let mut out: Vec<String> = Vec::new();
	let mut prev: Option<Kind> = None;
	for u in units {
		if let Some(p) = prev
			&& (u.spaced || wants_blank(p, u.kind))
		{
			out.push(String::new());
		}
		out.extend(u.lines);
		prev = Some(u.kind);
	}
	if out.is_empty() {
		return String::new();
	}
	let mut s = out.join("\n");
	s.push('\n');
	s
}
