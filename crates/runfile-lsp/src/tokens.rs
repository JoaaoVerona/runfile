//! Semantic tokens: what each word of a document is, as the runner reads it.
//!
//! An editor colours a `.run` file with a grammar of its own -- the TextMate one in `editors/vscode`, the
//! tree-sitter one, or one an editor wrote for itself -- and every one of them is a second description of the
//! language, which a release can leave behind: `code_of(exec …)` reached the runner before any of them knew it.
//! These come from the parser `run` itself uses, as diagnostics do, so a client that lays them over its own colours
//! is never further behind than the `run` it has.
//!
//! What the tree places is taken from it: every name, call, source, string, number and operator in an expression,
//! and the `$`, `run` or `exec` inside a call. What it does not -- a statement's keywords, the names a `let`, a
//! `for` or an assignment binds, a `run`'s target, an `exec`'s program -- is read off the statement's own line,
//! whose kind the tree has already said. Somebody else's text gets nothing: a `$` line's shell, an `exec` body, a
//! `json` block, a `run`'s arguments -- save each `{{ … }}` in it, which is this language's. Comments, and the
//! words that close or divide a block, are read a line at a time, so they keep their colour while the document
//! does not parse.

use std::ops::Range;

use runfile_lang::ast::{BinaryOp, Block, Property, UnaryOp};
use runfile_lang::{Expr, InterpPart, SourceKind, Statement, lexer};

/// The token types, in the order their indexes say: the protocol's own names, which every client knows.
pub const TYPES: &[&str] = &[
	"keyword", "function", "variable", "property", "string", "number", "comment", "operator",
];

/// The token modifiers, a bit each, in this order.
pub const MODIFIERS: &[&str] = &["declaration", "modification", "readonly", "defaultLibrary"];

/// What a token is: an index into [`TYPES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
	Keyword,
	/// A function of the runner's, a `run`'s target, an `exec`'s program.
	Function,
	/// A binding, or a source (`ENV`, `ARG`, `FLAG`, `RUN`, `ARGS`).
	Variable,
	/// A property, or a source's key.
	Property,
	String,
	Number,
	Comment,
	Operator,
}

/// A name being bound: by a `let`, a `for` or a property.
pub const DECLARATION: u32 = 1;
/// A name assigned again, or an environment variable being set.
pub const MODIFICATION: u32 = 1 << 1;
/// What the run was given: a source, and its keys.
pub const READONLY: u32 = 1 << 2;
/// The runner's own: a function of its library, a source.
pub const DEFAULT_LIBRARY: u32 = 1 << 3;

/// One token, placed as the protocol counts: lines from zero, and columns and lengths in UTF-16 code units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
	pub line: usize,
	pub start: usize,
	pub length: usize,
	pub kind: Kind,
	pub modifiers: u32,
}

/// A token as a byte range of one line.
type Mark = (usize, Range<usize>, Kind, u32);

/// Every token in `src`, in order and apart.
pub fn tokens(src: &str) -> Vec<Token> {
	let mut marks = Vec::new();
	// Of a repaired copy when the document is mid-edit. Its byte offsets index that copy, not the document, but
	// every line it kept is the document's line of the same number.
	if let Some((tree, text)) = crate::analysis::parse_around(src, src.split('\n').count(), None) {
		let mut walk = Walk {
			text: &text,
			starts: starts(&text),
			found: Vec::new(),
		};
		walk.block(&tree.body);
		let Walk { found, starts, .. } = walk;
		for (range, kind, modifiers) in found {
			by_line(&text, &starts, range, kind, modifiers, &mut marks);
		}
	}
	read_lines(src, &mut marks);
	place(src, marks)
}

/// The protocol's encoding of `tokens`: five numbers each, its line and column relative to the one before.
pub fn encode(tokens: &[Token]) -> Vec<u32> {
	let mut out = Vec::with_capacity(tokens.len() * 5);
	let (mut line, mut start) = (0, 0);
	for t in tokens {
		let column = if t.line == line { t.start - start } else { t.start };
		out.extend([
			(t.line - line) as u32,
			column as u32,
			t.length as u32,
			t.kind as u32,
			t.modifiers,
		]);
		(line, start) = (t.line, t.start);
	}
	out
}

/// The byte each line of `text` starts at.
fn starts(text: &str) -> Vec<usize> {
	std::iter::once(0)
		.chain(text.match_indices('\n').map(|(i, _)| i + 1))
		.collect()
}

/// A byte range of `text` as marks on the lines it crosses.
fn by_line(text: &str, starts: &[usize], range: Range<usize>, kind: Kind, modifiers: u32, out: &mut Vec<Mark>) {
	let first = starts.partition_point(|&s| s <= range.start) - 1;
	for (line, &start) in starts.iter().enumerate().skip(first) {
		if start >= range.end {
			break;
		}
		let end = starts.get(line + 1).map_or(text.len(), |next| next - 1);
		let (from, to) = (range.start.max(start), range.end.min(end));
		if from < to {
			out.push((line, from - start..to - start, kind, modifiers));
		}
	}
}

/// What each line says of itself, outside somebody else's text: a comment, and the word at its start that closes
/// or divides a block -- `end`, `else`, `case`, `default` -- which is all such a line can be, as the parser reads it.
fn read_lines(src: &str, out: &mut Vec<Mark>) {
	let foreign: Vec<Range<usize>> = crate::analysis::bodies(src).into_iter().map(|b| b.lines).collect();
	for (no, raw) in src.split('\n').enumerate() {
		if foreign.iter().any(|lines| lines.contains(&no)) {
			continue;
		}
		let line = raw.strip_suffix('\r').unwrap_or(raw);
		let end = line.trim_end().len();
		let at = line.len() - line.trim_start().len();
		if line[at..].starts_with('#') {
			out.push((no, at..end, Kind::Comment, 0));
			continue;
		}
		if let Some(comment) = lexer::comment_at(line) {
			out.push((no, comment..end, Kind::Comment, 0));
		}
		let head = line[at..].split_whitespace().next().unwrap_or_default();
		if matches!(head, "end" | "else" | "case" | "default") {
			out.push((no, at..at + head.len(), Kind::Keyword, 0));
		}
	}
}

/// `marks` in order, apart, and counted in UTF-16: the first of two that overlap is the one kept.
fn place(src: &str, mut marks: Vec<Mark>) -> Vec<Token> {
	marks.sort_by_key(|(line, range, ..)| (*line, range.start));
	let lines: Vec<&str> = src.split('\n').collect();
	let mut out: Vec<Token> = Vec::with_capacity(marks.len());
	let mut reached = (0, 0);
	for (no, range, kind, modifiers) in marks {
		let Some(line) = lines.get(no) else {
			continue;
		};
		if (no, range.start) < reached || range.end > line.len() || !line.is_char_boundary(range.start) {
			continue;
		}
		let Some(text) = line.get(range.clone()) else {
			continue;
		};
		reached = (no, range.end);
		out.push(Token {
			line: no,
			start: line[..range.start].encode_utf16().count(),
			length: text.encode_utf16().count(),
			kind,
			modifiers,
		});
	}
	out
}

fn is_name_byte(b: u8) -> bool {
	b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// The tree, read for its tokens against the text it was parsed from.
struct Walk<'a> {
	text: &'a str,
	starts: Vec<usize>,
	found: Vec<(Range<usize>, Kind, u32)>,
}

impl Walk<'_> {
	fn mark(&mut self, range: Range<usize>, kind: Kind, modifiers: u32) {
		if range.start < range.end && range.end <= self.text.len() {
			self.found.push((range, kind, modifiers));
		}
	}

	/// Whether `word` is written at `at`.
	fn at(&self, at: usize, word: &str) -> bool {
		self.text.get(at..at + word.len()) == Some(word)
	}

	/// Past the blanks from `at`.
	fn blanks(&self, at: usize) -> usize {
		let b = self.text.as_bytes();
		let mut at = at;
		while b.get(at).is_some_and(|c| matches!(c, b' ' | b'\t')) {
			at += 1;
		}
		at
	}

	/// `word` after the blanks from `at`, as a word of its own: marked a keyword, and where it ends.
	fn keyword(&mut self, at: usize, word: &str) -> Option<usize> {
		let at = self.blanks(at);
		let end = at + word.len();
		let alone = !self.text.as_bytes().get(end).is_some_and(|b| is_name_byte(*b));
		(self.at(at, word) && alone).then(|| {
			self.mark(at..end, Kind::Keyword, 0);
			end
		})
	}

	/// Where `word` first stands alone in `range`.
	fn find_word(&self, range: Range<usize>, word: &str) -> Option<usize> {
		let b = self.text.as_bytes();
		let hay = self.text.get(range.clone())?;
		hay.match_indices(word).map(|(i, _)| range.start + i).find(|&at| {
			(at == 0 || !is_name_byte(b[at - 1])) && !b.get(at + word.len()).is_some_and(|c| is_name_byte(*c))
		})
	}

	/// The names written in `range` -- `a, b, _` -- each a variable with `modifiers`; `_` binds nothing.
	fn names(&mut self, range: Range<usize>, modifiers: u32) {
		let Some(list) = self.text.get(range.clone()) else {
			return;
		};
		let mut at = range.start;
		let mut spans = Vec::new();
		for piece in list.split(',') {
			let lead = piece.len() - piece.trim_start().len();
			let name = piece.trim();
			if !name.is_empty() && name != "_" && name.bytes().all(is_name_byte) {
				spans.push(at + lead..at + lead + name.len());
			}
			at += piece.len() + 1;
		}
		for span in spans {
			self.mark(span, Kind::Variable, modifiers);
		}
	}

	/// What a binding binds, from `from` to its `=` -- the names, each with `modifiers` -- and the `=`.
	fn binding(&mut self, from: usize, value: &Expr, modifiers: u32) {
		let Some(eq) = self.text.get(from..value.span().start).and_then(|t| t.find('=')) else {
			return;
		};
		self.names(from..from + eq, modifiers);
		self.mark(from + eq..from + eq + 1, Kind::Operator, 0);
	}

	/// A word from the blanks after `at` to the next blank -- or, inside a call, its `)` -- with every `{{ … }}`
	/// in it whole, as the parser splits a `run`'s words and an `exec`'s command.
	fn word(&self, at: usize, in_call: bool) -> Range<usize> {
		let b = self.text.as_bytes();
		let start = self.blanks(at);
		let mut end = start;
		while end < b.len() && !b[end].is_ascii_whitespace() && !(in_call && b[end] == b')') {
			end = match b[end..].starts_with(b"{{").then(|| lexer::skip_interp(b, end, 0)) {
				Some(Ok(next)) => next,
				_ => end + 1,
			};
		}
		start..end
	}

	/// `range` as `kind`, around each `{{ … }}` in it, which is an expression of its own.
	fn around_interps(&mut self, range: Range<usize>, kind: Kind, modifiers: u32) {
		let b = self.text.as_bytes();
		let (mut from, mut at) = (range.start, range.start);
		while at < range.end {
			if b[at..range.end].starts_with(b"{{") {
				let end = lexer::skip_interp(b, at, 0).unwrap_or(range.end).min(range.end);
				self.mark(from..at, kind, modifiers);
				(from, at) = (end, end);
			} else {
				at += 1;
			}
		}
		self.mark(from..range.end, kind, modifiers);
	}

	/// The `{{ … }}` among somebody else's text.
	fn parts(&mut self, parts: &[InterpPart]) {
		for part in parts {
			if let InterpPart::Expr(e) = part {
				self.expr(e);
			}
		}
	}

	/// The first word after `at` as what an `exec` starts or a `run` dispatches.
	fn callee(&mut self, at: usize, in_call: bool) {
		let word = self.word(at, in_call);
		self.around_interps(word, Kind::Function, 0);
	}

	/// The `=` between `from` and `to`, the last one there.
	fn equals(&mut self, from: usize, to: usize) {
		if let Some(eq) = self.text.get(from..to).and_then(|t| t.rfind('=')) {
			self.mark(from + eq..from + eq + 1, Kind::Operator, 0);
		}
	}

	/// The operator written between two operands.
	fn operator(&mut self, between: Range<usize>, symbol: &str) {
		if let Some(at) = self.text.get(between.clone()).and_then(|t| t.find(symbol)) {
			let at = between.start + at;
			self.mark(at..at + symbol.len(), Kind::Operator, 0);
		}
	}

	/// The `$` that opens a capture: where its span starts, or, for one inside a call, whose span is the command
	/// alone, just before it.
	fn dollar(&mut self, start: usize) {
		let at = if self.at(start, "$") {
			Some(start)
		} else {
			let before = self.text[..start].trim_end_matches([' ', '\t']);
			before.ends_with('$').then(|| before.len() - 1)
		};
		if let Some(at) = at {
			self.mark(at..at + 1, Kind::Keyword, 0);
		}
	}

	fn block(&mut self, b: &Block) {
		for p in &b.properties {
			self.property(p);
		}
		for st in &b.statements {
			self.statement(st);
		}
	}

	fn property(&mut self, p: &Property) {
		let dot = self.blanks(p.span.start);
		if !self.at(dot, ".") {
			return;
		}
		let mut at = dot + 1;
		for part in &p.path {
			if !self.at(at, part) {
				break;
			}
			self.mark(at..at + part.len(), Kind::Property, DECLARATION);
			at += part.len() + 1;
		}
		if let Some(value) = &p.value {
			self.equals(dot, value.span().start);
			self.expr(value);
		}
	}

	fn statement(&mut self, st: &Statement) {
		let start = st.span().start;
		match st {
			Statement::Let { value, .. } => {
				if let Some(after) = self.keyword(start, "let") {
					self.binding(after, value, DECLARATION);
				}
				self.expr(value);
			}
			Statement::Assign { value, .. } => {
				self.binding(start, value, MODIFICATION);
				self.expr(value);
			}
			Statement::SetEnv { name, value, .. } => {
				let at = self.blanks(start);
				if self.at(at, "ENV.") {
					self.mark(at..at + 3, Kind::Variable, READONLY | DEFAULT_LIBRARY);
					if self.at(at + 4, name) {
						self.mark(at + 4..at + 4 + name.len(), Kind::Property, MODIFICATION);
					}
				}
				self.equals(at, value.span().start);
				self.expr(value);
			}
			Statement::Call { expr, .. } => self.expr(expr),
			Statement::Do { body, parallel, .. } => {
				let at = if *parallel {
					self.keyword(start, "parallel")
				} else {
					Some(start)
				};
				if let Some(at) = at {
					self.keyword(at, "do");
				}
				self.block(body);
			}
			Statement::If {
				cond, then, otherwise, ..
			} => {
				// An `else if`'s own `If` starts at its `if`; the `else` before it is the line's.
				self.keyword(start, "if");
				self.expr(cond);
				self.block(then);
				if let Some(otherwise) = otherwise {
					self.block(otherwise);
				}
			}
			Statement::Retry {
				attempts,
				delay,
				body,
				otherwise,
				..
			} => {
				self.keyword(start, "retry");
				self.expr(attempts);
				if let Some(delay) = delay {
					if let Some(every) = self.find_word(attempts.span().end..delay.span().start, "every") {
						self.keyword(every, "every");
					}
					self.expr(delay);
				}
				self.block(body);
				if let Some(otherwise) = otherwise {
					self.block(otherwise);
				}
			}
			Statement::For {
				iter, body, parallel, ..
			} => {
				let at = if *parallel {
					self.keyword(start, "parallel")
				} else {
					Some(start)
				};
				if let Some(after) = at.and_then(|at| self.keyword(at, "for"))
					&& let Some(word) = self.find_word(after..iter.span().start, "in")
				{
					self.names(after..word, DECLARATION);
					self.keyword(word, "in");
				}
				self.expr(iter);
				self.block(body);
			}
			Statement::Loop { test, body, .. } => {
				self.keyword(start, test.keyword());
				if let Some(cond) = test.cond() {
					self.expr(cond);
				}
				self.block(body);
			}
			Statement::Break { .. } => {
				self.keyword(start, "break");
			}
			Statement::Continue { .. } => {
				self.keyword(start, "continue");
			}
			Statement::Match {
				subject,
				cases,
				default,
				..
			} => {
				self.keyword(start, "match");
				self.expr(subject);
				for case in cases {
					if let Some(after) = self.keyword(case.span.start, "case") {
						// A label is a quoted string with nothing in it to interpolate, as the parser takes one.
						let label = self.blanks(after);
						let end = label + case.label.len() + 2;
						if self.at(label, "\"") && self.at(end - 1, "\"") {
							self.mark(label..end, Kind::String, 0);
						}
					}
					self.block(&case.body);
				}
				if let Some(default) = default {
					self.block(default);
				}
			}
			Statement::Run { target, args, .. } => {
				if let Some(after) = self.keyword(start, "run") {
					self.callee(after, false);
				}
				self.parts(target);
				for arg in args {
					self.parts(arg);
				}
			}
			Statement::Exec {
				command,
				body,
				detach,
				lines,
				..
			} => {
				match command {
					Some(command) => {
						let at = if *detach {
							self.keyword(start, "detach")
						} else {
							Some(start)
						};
						if let Some(after) = at.and_then(|at| self.keyword(at, "exec")) {
							self.callee(after, false);
						}
						self.parts(command);
					}
					// A run of `$` lines, each with its own marker; only the first may be detached.
					None => {
						for (i, no) in lines.iter().enumerate() {
							let Some(&line) = self.starts.get(no.wrapping_sub(1)) else {
								continue;
							};
							let at = if *detach && i == 0 {
								self.keyword(line, "detach")
							} else {
								Some(line)
							};
							if let Some(at) = at {
								self.keyword(at, "$");
							}
						}
					}
				}
				for line in body {
					self.parts(line);
				}
			}
		}
	}

	fn expr(&mut self, e: &Expr) {
		match e {
			Expr::Number(_, s) => self.mark(s.start..s.end, Kind::Number, 0),
			// The protocol has no type for a boolean, and calling one a keyword would take it off the boolean
			// colour an editor's grammar gives it -- Neovim lays these over tree-sitter's. Left to the grammar.
			Expr::Bool(..) => {}
			Expr::Str(parts, s) => {
				self.around_interps(s.start..s.end, Kind::String, 0);
				self.parts(parts);
			}
			Expr::List(items, _) => {
				for item in items {
					self.expr(item);
				}
			}
			Expr::Ident(name, s) => {
				if self.at(s.start, name) {
					self.mark(s.start..s.start + name.len(), Kind::Variable, 0);
				}
			}
			Expr::Source { kind, key, span } => {
				let word = match kind {
					SourceKind::Arg => "ARG",
					SourceKind::Env => "ENV",
					SourceKind::Flag => "FLAG",
					SourceKind::Run => "RUN",
					SourceKind::Args => "ARGS",
				};
				if self.at(span.start, word) {
					self.mark(
						span.start..span.start + word.len(),
						Kind::Variable,
						READONLY | DEFAULT_LIBRARY,
					);
					let at = span.start + word.len() + 1;
					if let Some(key) = key
						&& self.at(at, key)
					{
						self.mark(at..at + key.len(), Kind::Property, READONLY);
					}
				}
			}
			Expr::Unary { op, rhs, span } => {
				let symbol = match op {
					UnaryOp::Not => "!",
					UnaryOp::Neg => "-",
				};
				if self.at(span.start, symbol) {
					self.mark(span.start..span.start + 1, Kind::Operator, 0);
				}
				self.expr(rhs);
			}
			Expr::Binary { op, lhs, rhs, .. } => {
				self.expr(lhs);
				self.operator(lhs.span().end..rhs.span().start, symbol(*op));
				self.expr(rhs);
			}
			Expr::Chain { lhs, rhs, .. } => {
				self.expr(lhs);
				self.operator(lhs.span().end..rhs.span().start, "?");
				self.expr(rhs);
			}
			Expr::Index { base, index, .. } => {
				self.expr(base);
				self.expr(index);
			}
			Expr::Call { name, args, span } => {
				// Only a function a call reaches, by the runner's own test: any other is a mistake the diagnostics
				// underline, and keeps the colour of a plain name, as the tree-sitter grammar leaves it.
				if self.at(span.start, name) && runfile_lang::functions::exists(name) {
					self.mark(span.start..span.start + name.len(), Kind::Function, DEFAULT_LIBRARY);
				}
				for arg in args {
					self.expr(arg);
				}
			}
			Expr::Structured { body, span } => {
				self.keyword(span.start, body.format.keyword());
				for line in &body.lines {
					self.parts(line);
				}
			}
			Expr::Capture { command, body, span } => {
				match command {
					Some(command) => {
						if let Some(after) = self.keyword(span.start, "exec") {
							self.callee(after, false);
						}
						self.parts(command);
					}
					None => self.dollar(span.start),
				}
				for line in body {
					self.parts(line);
				}
			}
			Expr::Dispatch { target, args, span } => {
				if let Some(after) = self.keyword(span.start, "run") {
					self.callee(after, true);
				}
				self.parts(target);
				for arg in args {
					self.parts(arg);
				}
			}
			Expr::Program { command, span } => {
				if let Some(after) = self.keyword(span.start, "exec") {
					self.callee(after, true);
				}
				self.parts(command);
			}
		}
	}
}

fn symbol(op: BinaryOp) -> &'static str {
	match op {
		BinaryOp::Add => "+",
		BinaryOp::Sub => "-",
		BinaryOp::Mul => "*",
		BinaryOp::Div => "/",
		BinaryOp::Rem => "%",
		BinaryOp::Eq => "==",
		BinaryOp::Ne => "!=",
		BinaryOp::Lt => "<",
		BinaryOp::Le => "<=",
		BinaryOp::Gt => ">",
		BinaryOp::Ge => ">=",
		BinaryOp::And => "&&",
		BinaryOp::Or => "||",
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The tokens on line `no` of `src`, each as its text and its type, with its modifiers after a dot.
	fn on(src: &str, no: usize) -> Vec<(String, String)> {
		let line: Vec<u16> = src.split('\n').nth(no).unwrap().encode_utf16().collect();
		tokens(src)
			.into_iter()
			.filter(|t| t.line == no)
			.map(|t| {
				let text = String::from_utf16(&line[t.start..t.start + t.length]).unwrap();
				let mut kind = TYPES[t.kind as usize].to_string();
				for (bit, modifier) in MODIFIERS.iter().enumerate() {
					if t.modifiers & (1 << bit) != 0 {
						kind = format!("{kind}.{modifier}");
					}
				}
				(text, kind)
			})
			.collect()
	}

	fn want(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
		pairs.iter().map(|(t, k)| (t.to_string(), k.to_string())).collect()
	}

	#[test]
	fn a_binding_its_names_and_its_value() {
		let src = "let name, _ = split(ARG.who ? \"world\", \",\")\n";
		assert_eq!(
			on(src, 0),
			want(&[
				("let", "keyword"),
				("name", "variable.declaration"),
				("=", "operator"),
				("split", "function.defaultLibrary"),
				("ARG", "variable.readonly.defaultLibrary"),
				("who", "property.readonly"),
				("?", "operator"),
				("\"world\"", "string"),
				("\",\"", "string"),
			])
		);
	}

	#[test]
	fn a_string_s_interpolation_is_an_expression_and_the_rest_of_it_string() {
		let src = "print(\"hi {{ name }}, {{ n + 1 }}!\")\n";
		assert_eq!(
			on(src, 0),
			want(&[
				("print", "function.defaultLibrary"),
				("\"hi ", "string"),
				("name", "variable"),
				(", ", "string"),
				("n", "variable"),
				("+", "operator"),
				("1", "number"),
				("!\"", "string"),
			])
		);
	}

	#[test]
	fn control_flow_s_keywords_on_every_line_that_has_one() {
		let src = "\
for f, _ in pairs  # each
	if f == \"x\"
		break
	else if !ok
		continue
	else
		n = n - 1
	end
end
retry 3 every 2
	print(1)
end
match RUN.os
	case \"linux\"
		print(true)
	default
		print(false)
end
";
		assert_eq!(
			on(src, 0),
			want(&[
				("for", "keyword"),
				("f", "variable.declaration"),
				("in", "keyword"),
				("pairs", "variable"),
				("# each", "comment"),
			])
		);
		assert_eq!(
			on(src, 1),
			want(&[
				("if", "keyword"),
				("f", "variable"),
				("==", "operator"),
				("\"x\"", "string")
			])
		);
		assert_eq!(on(src, 2), want(&[("break", "keyword")]));
		assert_eq!(
			on(src, 3),
			want(&[
				("else", "keyword"),
				("if", "keyword"),
				("!", "operator"),
				("ok", "variable")
			])
		);
		assert_eq!(on(src, 5), want(&[("else", "keyword")]));
		assert_eq!(
			on(src, 6),
			want(&[
				("n", "variable.modification"),
				("=", "operator"),
				("n", "variable"),
				("-", "operator"),
				("1", "number"),
			])
		);
		assert_eq!(on(src, 7), want(&[("end", "keyword")]));
		assert_eq!(
			on(src, 9),
			want(&[
				("retry", "keyword"),
				("3", "number"),
				("every", "keyword"),
				("2", "number")
			])
		);
		assert_eq!(
			on(src, 12),
			want(&[
				("match", "keyword"),
				("RUN", "variable.readonly.defaultLibrary"),
				("os", "property.readonly"),
			])
		);
		assert_eq!(on(src, 13), want(&[("case", "keyword"), ("\"linux\"", "string")]));
		assert_eq!(
			on(src, 14),
			want(&[("print", "function.defaultLibrary")]),
			"a boolean is the grammar's: the protocol has no type for one"
		);
		assert_eq!(on(src, 15), want(&[("default", "keyword")]));
	}

	#[test]
	fn somebody_else_s_text_gets_nothing_but_its_interpolations() {
		let src = "\
$ if true; then echo \"{{ who }}\"; fi  # the shell's
detach $ npm start
run build:{{ os }} --fast \"x\" {{ ARGS }}
exec python3 -u  # a comment
	import os  # {{ x }}
	end = 1
end
let data = json
	{\"a\": {{ n }}}
end
";
		assert_eq!(on(src, 0), want(&[("$", "keyword"), ("who", "variable")]));
		assert_eq!(on(src, 1), want(&[("detach", "keyword"), ("$", "keyword")]));
		assert_eq!(
			on(src, 2),
			want(&[
				("run", "keyword"),
				("build:", "function"),
				("os", "variable"),
				("ARGS", "variable.readonly.defaultLibrary"),
			])
		);
		assert_eq!(
			on(src, 3),
			want(&[("exec", "keyword"), ("python3", "function"), ("# a comment", "comment")])
		);
		assert_eq!(on(src, 4), want(&[("x", "variable")]), "a body is the program's");
		assert_eq!(on(src, 5), want(&[]), "an `end` the body indents is the body's");
		assert_eq!(on(src, 6), want(&[("end", "keyword")]));
		assert_eq!(
			on(src, 7),
			want(&[
				("let", "keyword"),
				("data", "variable.declaration"),
				("=", "operator"),
				("json", "keyword")
			])
		);
		assert_eq!(on(src, 8), want(&[("n", "variable")]));
	}

	#[test]
	fn what_a_call_holds_a_capture_a_dispatch_or_a_program() {
		let src = "\
let ok = code_of(exec make -j{{ jobs }} all)
let up = code_of(run deploy {{ env }})
let found = lines($ git ls-files)
let out = $ git log -1
if $ test -f x
	print(nope(1))
end
";
		assert_eq!(
			on(src, 0),
			want(&[
				("let", "keyword"),
				("ok", "variable.declaration"),
				("=", "operator"),
				("code_of", "function.defaultLibrary"),
				("exec", "keyword"),
				("make", "function"),
				("jobs", "variable"),
			])
		);
		assert_eq!(
			on(src, 1),
			want(&[
				("let", "keyword"),
				("up", "variable.declaration"),
				("=", "operator"),
				("code_of", "function.defaultLibrary"),
				("run", "keyword"),
				("deploy", "function"),
				("env", "variable"),
			])
		);
		assert_eq!(
			on(src, 2),
			want(&[
				("let", "keyword"),
				("found", "variable.declaration"),
				("=", "operator"),
				("lines", "function.defaultLibrary"),
				("$", "keyword"),
			])
		);
		assert_eq!(
			on(src, 3),
			want(&[
				("let", "keyword"),
				("out", "variable.declaration"),
				("=", "operator"),
				("$", "keyword")
			])
		);
		assert_eq!(on(src, 4), want(&[("if", "keyword"), ("$", "keyword")]));
		assert_eq!(
			on(src, 5),
			want(&[("print", "function.defaultLibrary"), ("1", "number")]),
			"a function the runner does not have keeps a plain name's colour"
		);
	}

	#[test]
	fn properties_the_environment_and_parallel_blocks() {
		let src = "\
# Builds it.
.shell = \"bash\"
.watch

ENV.PATH = \"bin\"
parallel do
	$ lint
	$ test
end
parallel for i in [1, 2]
	print(i)
end
";
		assert_eq!(on(src, 0), want(&[("# Builds it.", "comment")]));
		assert_eq!(
			on(src, 1),
			want(&[
				("shell", "property.declaration"),
				("=", "operator"),
				("\"bash\"", "string")
			])
		);
		assert_eq!(on(src, 2), want(&[("watch", "property.declaration")]));
		assert_eq!(
			on(src, 4),
			want(&[
				("ENV", "variable.readonly.defaultLibrary"),
				("PATH", "property.modification"),
				("=", "operator"),
				("\"bin\"", "string"),
			])
		);
		assert_eq!(on(src, 5), want(&[("parallel", "keyword"), ("do", "keyword")]));
		assert_eq!(on(src, 6), want(&[("$", "keyword")]));
		assert_eq!(on(src, 7), want(&[("$", "keyword")]));
		assert_eq!(
			on(src, 9),
			want(&[
				("parallel", "keyword"),
				("for", "keyword"),
				("i", "variable.declaration"),
				("in", "keyword"),
				("1", "number"),
				("2", "number"),
			])
		);
	}

	#[test]
	fn a_document_mid_edit_keeps_its_colours_around_the_line_being_typed() {
		let src = "let x = 1\nprint(re\nlet y = x + 1\nif x\n";
		assert_eq!(on(src, 0).len(), 4, "{:?}", on(src, 0));
		assert_eq!(on(src, 1), want(&[]));
		assert_eq!(
			on(src, 2),
			want(&[
				("let", "keyword"),
				("y", "variable.declaration"),
				("=", "operator"),
				("x", "variable"),
				("+", "operator"),
				("1", "number"),
			])
		);
		assert_eq!(
			on(src, 3),
			want(&[("if", "keyword"), ("x", "variable")]),
			"a block not closed yet"
		);
	}

	#[test]
	fn columns_and_lengths_are_utf_16_code_units() {
		let src = "let s = \"ñ😀\" + t\n";
		let found = tokens(src);
		let string = found.iter().find(|t| t.kind == Kind::String).unwrap();
		assert_eq!((string.start, string.length), (8, 5), "the emoji is two units");
		let plus = found.iter().find(|t| t.kind == Kind::Operator && t.start > 8).unwrap();
		assert_eq!(plus.start, 14);
		assert_eq!(on(src, 0).last(), Some(&("t".into(), "variable".into())));
	}

	#[test]
	fn a_crlf_file_is_coloured_as_its_lf_twin() {
		let lf = "let x = 1 # one\nif x\n\tprint(x)\nend\n";
		let crlf = lf.replace('\n', "\r\n");
		assert_eq!(tokens(&crlf), tokens(lf));
	}

	#[test]
	fn the_encoding_is_relative_to_the_token_before() {
		let src = "let x = 1\n  print(x)\n";
		assert_eq!(
			encode(&tokens(src)),
			[
				0,
				0,
				3,
				Kind::Keyword as u32,
				0, // let
				0,
				4,
				1,
				Kind::Variable as u32,
				DECLARATION, // x
				0,
				2,
				1,
				Kind::Operator as u32,
				0, // =
				0,
				2,
				1,
				Kind::Number as u32,
				0, // 1
				1,
				2,
				5,
				Kind::Function as u32,
				DEFAULT_LIBRARY, // print
				0,
				6,
				1,
				Kind::Variable as u32,
				0, // x
			]
		);
	}

	/// Every token in every `.run` file here is apart from the others, on its line, and spells what its type says
	/// it is -- the check that holds the line-reading half to the text, as `spans.rs` holds the tree.
	#[test]
	fn every_token_in_every_file_in_this_repository_spells_what_it_is() {
		let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
		let mut files = Vec::new();
		let mut dirs = vec![root];
		while let Some(d) = dirs.pop() {
			for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
				let p = e.path();
				let name = e.file_name().to_string_lossy().into_owned();
				if p.is_dir() {
					if !matches!(name.as_str(), "target" | "node_modules" | ".git" | ".claude")
						&& !name.starts_with("target-")
					{
						dirs.push(p);
					}
				} else if p.extension().is_some_and(|x| x == "run") {
					files.push(p);
				}
			}
		}
		assert!(files.len() > 20, "the sweep found almost nothing: {}", files.len());
		let keywords: Vec<&str> = runfile_lang::KEYWORDS
			.iter()
			.map(|k| k.name)
			.chain(["every", "in", "parallel", "detach"])
			.collect();
		let operators = [
			"=", "+", "-", "*", "/", "%", "==", "!=", "<", "<=", ">", ">=", "&&", "||", "!", "?",
		];
		let mut wrong = Vec::new();
		let mut seen = 0;
		for f in files {
			let src = std::fs::read_to_string(&f).unwrap();
			if runfile_lang::parse(&src).is_err() {
				continue;
			}
			let lines: Vec<Vec<u16>> = src.split('\n').map(|l| l.encode_utf16().collect()).collect();
			let found = tokens(&src);
			// And none is missed where a line opens with one: a statement's keyword, a closer, a `$`.
			let foreign: Vec<Range<usize>> = crate::analysis::bodies(&src).into_iter().map(|b| b.lines).collect();
			for (no, line) in src.split('\n').enumerate() {
				let at = line.len() - line.trim_start().len();
				let head = line[at..].split_whitespace().next().unwrap_or_default();
				let opens = matches!(
					head,
					"let"
						| "if" | "else" | "for"
						| "while" | "until"
						| "loop" | "break" | "continue"
						| "match" | "case" | "default"
						| "retry" | "do" | "end"
						| "run" | "exec" | "parallel"
						| "detach" | "$"
				);
				let marked = found
					.iter()
					.any(|t| t.line == no && t.start == at && t.kind == Kind::Keyword);
				if opens && !marked && !foreign.iter().any(|lines| lines.contains(&no)) {
					wrong.push(format!("{}:{}: `{head}` is not coloured", f.display(), no + 1));
				}
			}
			let mut reached = (0, 0);
			for t in found {
				seen += 1;
				let line = &lines[t.line];
				if (t.line, t.start) < reached || t.start + t.length > line.len() || t.length == 0 {
					wrong.push(format!(
						"{}:{}: {t:?} overlaps or leaves its line",
						f.display(),
						t.line + 1
					));
					continue;
				}
				reached = (t.line, t.start + t.length);
				let text = String::from_utf16_lossy(&line[t.start..t.start + t.length]);
				let name = |s: &str| s.bytes().all(is_name_byte);
				let fits = match t.kind {
					Kind::Keyword => keywords.contains(&text.as_str()),
					Kind::Variable | Kind::Property => name(&text),
					Kind::Function => !text.contains(char::is_whitespace),
					Kind::Number => text.parse::<f64>().is_ok(),
					Kind::Comment => text.starts_with('#'),
					Kind::Operator => operators.contains(&text.as_str()),
					// A `}}` is text, as JSON in a string has; a `{{` always opens an interpolation.
					Kind::String => !text.contains("{{"),
				};
				if !fits {
					wrong.push(format!("{}:{}: `{text}` is no {:?}", f.display(), t.line + 1, t.kind));
				}
			}
		}
		assert!(seen > 1000, "the sweep coloured almost nothing: {seen}");
		assert!(wrong.is_empty(), "{}", wrong.join("\n"));
	}
}
