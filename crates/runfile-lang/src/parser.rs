//! Line classification, block assembly and expression parsing.

use crate::ast::*;
use crate::lexer::{self, LexError, RawPart, Spanned, Token};
use crate::span::Span;
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum ParseError {
	#[error(transparent)]
	Lex(#[from] LexError),
	#[error("line {line}: {msg}")]
	At { line: usize, msg: String },
}

fn err<T>(line: usize, msg: impl Into<String>) -> Result<T, ParseError> {
	Err(ParseError::At { line, msg: msg.into() })
}

/// One physical line, pre-classified.
struct Line<'a> {
	raw: &'a str,
	trimmed: &'a str,
	indent: &'a str,
	offset: usize,
	no: usize,
}

fn scan_lines(src: &str) -> Vec<Line<'_>> {
	let mut out = Vec::new();
	let mut offset = 0usize;
	for (i, raw) in src.split('\n').enumerate() {
		let trimmed = raw.trim();
		let indent = &raw[..raw.len() - raw.trim_start().len()];
		out.push(Line {
			raw,
			trimmed,
			indent,
			offset,
			no: i + 1,
		});
		offset += raw.len() + 1;
	}
	out
}

pub fn parse(src: &str) -> Result<Target, ParseError> {
	let lines = scan_lines(src);
	let mut p = P { lines: &lines, i: 0 };
	let description = p.take_description();
	let body = p.block(None)?;
	if p.i < p.lines.len() {
		let l = &p.lines[p.i];
		return err(l.no, format!("unexpected `{}`", l.trimmed));
	}
	Ok(Target { description, body })
}

struct P<'a> {
	lines: &'a [Line<'a>],
	i: usize,
}

impl<'a> P<'a> {
	/// The leading comment block. Marker lines an editor or linter owns are not
	/// part of the description.
	fn take_description(&mut self) -> Option<String> {
		while self.i < self.lines.len() && self.lines[self.i].trimmed.is_empty() {
			self.i += 1;
		}
		let mut out = Vec::new();
		while self.i < self.lines.len() {
			let t = self.lines[self.i].trimmed;
			let Some(rest) = t.strip_prefix('#') else { break };
			let rest = rest.trim();
			if !rest.starts_with("shellcheck") && !rest.starts_with("vim:") && !rest.starts_with("-*-") {
				out.push(rest.to_string());
			}
			self.i += 1;
		}
		while out.last().is_some_and(|l| l.is_empty()) {
			out.pop();
		}
		(!out.is_empty()).then(|| out.join("\n"))
	}

	fn at_block_end(&self, kw: Option<&str>) -> bool {
		if self.i >= self.lines.len() {
			return true;
		}
		let t = self.lines[self.i].trimmed;
		if kw.is_none() {
			return false;
		}
		let head = t.split_whitespace().next().unwrap_or("");
		matches!(head, "end" | "else" | "case" | "default")
	}

	/// Parse statements until the block's terminator. `kw` is `None` at file level.
	fn block(&mut self, kw: Option<&str>) -> Result<Block, ParseError> {
		let mut b = Block::default();
		loop {
			while self.i < self.lines.len() {
				let t = self.lines[self.i].trimmed;
				if t.is_empty() || t.starts_with('#') {
					self.i += 1;
				} else {
					break;
				}
			}
			if self.at_block_end(kw) {
				return Ok(b);
			}
			let line = &self.lines[self.i];
			if line.trimmed.starts_with('.') {
				b.properties.push(self.property()?);
			} else {
				b.statements.push(self.statement()?);
			}
		}
	}

	fn property(&mut self) -> Result<Property, ParseError> {
		let line = &self.lines[self.i];
		self.i += 1;
		let t = line.trimmed;
		let (name, rest) = match t.find('=') {
			Some(k) => (t[1..k].trim(), Some(t[k + 1..].trim())),
			None => (t[1..].trim(), None),
		};
		if name.is_empty() {
			return err(line.no, "property has no name");
		}
		let path: Vec<String> = name.split('.').map(str::to_string).collect();
		let value = match rest {
			None => None,
			Some(r) => {
				let base = line.offset + (r.as_ptr() as usize - line.raw.as_ptr() as usize);
				Some(parse_expr(r, base, line.no)?)
			}
		};
		Ok(Property {
			path,
			value,
			span: Span::new(line.offset, line.offset + t.len(), line.no),
		})
	}

	/// Gather a logical line, continuing while `[` are unbalanced so a list may
	/// span lines.
	fn logical(&mut self) -> (String, usize, usize) {
		let start = &self.lines[self.i];
		let (no, offset) = (start.no, start.offset);
		let mut buf = start.trimmed.to_string();
		self.i += 1;
		while buf.matches('[').count() > buf.matches(']').count() && self.i < self.lines.len() {
			buf.push(' ');
			buf.push_str(self.lines[self.i].trimmed);
			self.i += 1;
		}
		(buf, offset, no)
	}

	fn statement(&mut self) -> Result<Statement, ParseError> {
		let line = &self.lines[self.i];
		let no = line.no;
		let indent = line.indent;

		if line.trimmed == "$" || line.trimmed.starts_with("$ ") {
			return self.shell_run();
		}
		if line.trimmed.starts_with("exec ") {
			return self.exec_block();
		}

		let (text, offset, _) = self.logical();
		let head = text.split_whitespace().next().unwrap_or("");
		let span = Span::new(offset, offset + text.len(), no);

		match head {
			"let" => {
				let rest = text[3..].trim_start();
				let Some(eq) = rest.find('=') else {
					return err(no, "`let` needs `= value`");
				};
				let name = rest[..eq].trim().to_string();
				let base = offset + (text.len() - rest.len()) + eq + 1;
				let raw_rhs = rest[eq + 1..].trim();
				let value = match self.capture_rhs(raw_rhs, indent, base, no)? {
					Some(e) => e,
					None => parse_expr(raw_rhs, base, no)?,
				};
				Ok(Statement::Let { name, value, span })
			}
			"if" => {
				let cond = parse_expr(text[2..].trim(), offset + 3, no)?;
				let then = self.block(Some("if"))?;
				let otherwise = if self.peek_kw() == Some("else") {
					self.i += 1;
					Some(self.block(Some("if"))?)
				} else {
					None
				};
				self.expect_end(no)?;
				Ok(Statement::If {
					cond,
					then,
					otherwise,
					span,
				})
			}
			"for" => {
				let rest = text[3..].trim();
				let Some(k) = rest.find(" in ") else {
					return err(no, "`for` needs `in`");
				};
				let name = rest[..k].trim().to_string();
				let iter = parse_expr(rest[k + 4..].trim(), offset, no)?;
				let body = self.block(Some("for"))?;
				self.expect_end(no)?;
				Ok(Statement::For { name, iter, body, span })
			}
			"match" => {
				let subject = parse_expr(text[5..].trim(), offset + 6, no)?;
				let (mut cases, mut default) = (Vec::new(), None);
				loop {
					match self.peek_kw() {
						Some("case") => {
							let l = &self.lines[self.i];
							let label = l.trimmed[4..].trim().trim_matches('"').to_string();
							let cs = Span::new(l.offset, l.offset + l.trimmed.len(), l.no);
							self.i += 1;
							cases.push(MatchCase {
								label,
								body: self.block(Some("match"))?,
								span: cs,
							});
						}
						Some("default") => {
							self.i += 1;
							default = Some(self.block(Some("match"))?);
						}
						_ => break,
					}
				}
				self.expect_end(no)?;
				Ok(Statement::Match {
					subject,
					cases,
					default,
					span,
				})
			}
			"run" => {
				let rest = text[3..].trim();
				let mut words = split_words(rest);
				if words.is_empty() {
					return err(no, "`run` needs a target");
				}
				let target = lexer::split_interp(&words.remove(0), offset, no)?;
				let args = words
					.iter()
					.map(|w| lexer::split_interp(w, offset, no))
					.collect::<Result<_, _>>()?;
				Ok(Statement::Run {
					target: to_parts(target, no)?,
					args: to_args(args, no)?,
					span,
				})
			}
			_ => {
				if let Some(eq) = assignment_split(&text) {
					let name = text[..eq].trim().to_string();
					let raw_rhs = text[eq + 1..].trim();
					let value = match self.capture_rhs(raw_rhs, indent, offset + eq + 1, no)? {
						Some(e) => e,
						None => parse_expr(raw_rhs, offset + eq + 1, no)?,
					};
					return Ok(Statement::Assign { name, value, span });
				}
				Ok(Statement::Call {
					expr: parse_expr(&text, offset, no)?,
					span,
				})
			}
		}
	}

	fn peek_kw(&self) -> Option<&'a str> {
		let l = self.lines.get(self.i)?;
		l.trimmed.split_whitespace().next()
	}

	fn expect_end(&mut self, opened: usize) -> Result<(), ParseError> {
		if self.peek_kw() == Some("end") {
			self.i += 1;
			Ok(())
		} else {
			err(opened, "block opened here is never closed by `end`")
		}
	}

	/// A run of `$` lines becomes one process. Blank lines and comments are
	/// transparent -- reformatting a file must not change what shares a shell.
	fn shell_run(&mut self) -> Result<Statement, ParseError> {
		let first = &self.lines[self.i];
		let (no, offset) = (first.no, first.offset);
		let mut body = Vec::new();
		let mut last = self.i;
		let mut j = self.i;
		while j < self.lines.len() {
			let l = &self.lines[j];
			if l.trimmed == "$" || l.trimmed.starts_with("$ ") {
				let mut text = l.trimmed.strip_prefix('$').unwrap().trim_start().to_string();
				// A trailing backslash continues the shell line. The backslash and
				// newline are kept so the shell sees the continuation it expects,
				// and the author's formatting survives into --dry-run output.
				while text.ends_with('\\') && j + 1 < self.lines.len() {
					j += 1;
					text.push('\n');
					text.push_str(self.lines[j].raw);
				}
				body.push(to_parts(lexer::split_interp(&text, l.offset, l.no)?, l.no)?);
				last = j;
				j += 1;
			} else if l.trimmed.is_empty() || l.trimmed.starts_with('#') {
				j += 1;
			} else {
				break;
			}
		}
		self.i = last + 1;
		let end = self.lines[last].offset + self.lines[last].raw.len();
		Ok(Statement::Exec {
			command: None,
			body,
			span: Span::new(offset, end, no),
		})
	}

	/// Read an `exec` body: lines until an `end` at `indent`, dedented by their
	/// own base indentation.
	fn exec_body(&mut self, indent: &str, no: usize) -> Result<(Vec<Vec<InterpPart>>, usize), ParseError> {
		let mut j = self.i;
		let mut raw: Vec<&Line> = Vec::new();
		loop {
			if j >= self.lines.len() {
				return err(no, "`exec` block never closed by an `end` at its own indentation");
			}
			let l = &self.lines[j];
			if l.raw == format!("{indent}end") || (indent.is_empty() && l.raw.trim_end() == "end") {
				break;
			}
			raw.push(l);
			j += 1;
		}
		let base = raw
			.iter()
			.filter(|l| !l.trimmed.is_empty())
			.map(|l| l.indent.len())
			.min()
			.unwrap_or(0);
		let body = raw
			.iter()
			.map(|l| {
				let text = if l.raw.len() >= base { &l.raw[base..] } else { "" };
				to_parts(lexer::split_interp(text, l.offset + base, l.no)?, l.no)
			})
			.collect::<Result<_, _>>()?;
		let end = self.lines[j].offset + self.lines[j].raw.len();
		self.i = j + 1;
		Ok((body, end))
	}

	/// A `$ cmd` or `exec cmd … end` used as a value.
	fn capture_rhs(&mut self, rhs: &str, indent: &str, offset: usize, no: usize) -> Result<Option<Expr>, ParseError> {
		if let Some(cmd) = rhs.strip_prefix("$ ") {
			let parts = to_parts(lexer::split_interp(cmd.trim(), offset, no)?, no)?;
			let span = Span::new(offset, offset + rhs.len(), no);
			return Ok(Some(Expr::Capture {
				command: None,
				body: vec![parts],
				span,
			}));
		}
		if let Some(cmd) = rhs.strip_prefix("exec ") {
			let command = to_parts(lexer::split_interp(cmd.trim(), offset, no)?, no)?;
			let (body, end) = self.exec_body(indent, no)?;
			let span = Span::new(offset, end, no);
			return Ok(Some(Expr::Capture {
				command: Some(command),
				body,
				span,
			}));
		}
		Ok(None)
	}

	/// `exec <command…>` … `end`, where the terminator must sit at the opener's
	/// indentation: a body containing its own `end` (ruby, lua) would otherwise
	/// close the block early. The body is dedented by its own base indentation,
	/// so `exec sudo tee file` writes a file without leading tabs.
	fn exec_block(&mut self) -> Result<Statement, ParseError> {
		let open = &self.lines[self.i];
		let (no, offset, indent) = (open.no, open.offset, open.indent);
		let cmd_text = open.trimmed.strip_prefix("exec ").unwrap().trim();
		let command = to_parts(lexer::split_interp(cmd_text, offset, no)?, no)?;
		self.i += 1;
		let indent = indent.to_string();
		let (body, end) = self.exec_body(&indent, no)?;
		Ok(Statement::Exec {
			command: Some(command),
			body,
			span: Span::new(offset, end, no),
		})
	}
}

/// Split on whitespace, keeping `{{ … }}` blocks and `"…"` intact.
fn split_words(s: &str) -> Vec<String> {
	let b = s.as_bytes();
	let (mut out, mut cur, mut i) = (Vec::new(), String::new(), 0usize);
	while i < b.len() {
		if b[i..].starts_with(b"{{")
			&& let Ok(end) = lexer::skip_interp(b, i, 0)
		{
			cur.push_str(&s[i..end]);
			i = end;
			continue;
		}
		if b[i].is_ascii_whitespace() {
			if !cur.is_empty() {
				out.push(std::mem::take(&mut cur));
			}
			i += 1;
			continue;
		}
		cur.push(b[i] as char);
		i += 1;
	}
	if !cur.is_empty() {
		out.push(cur);
	}
	out
}

/// A bare `name = value` reassignment, as opposed to a call or comparison.
fn assignment_split(text: &str) -> Option<usize> {
	let eq = text.find('=')?;
	if text[eq..].starts_with("==") || eq == 0 {
		return None;
	}
	let before = text[..eq].trim_end();
	if before.ends_with(['!', '<', '>']) {
		return None;
	}
	let name = text[..eq].trim();
	(!name.is_empty()
		&& name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
		&& name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
	.then_some(eq)
}

fn to_parts(raw: Vec<RawPart>, line: usize) -> Result<Vec<InterpPart>, ParseError> {
	raw.into_iter()
		.map(|p| match p {
			RawPart::Literal(t) => Ok(InterpPart::Literal(t)),
			RawPart::Expr { text, span } => Ok(InterpPart::Expr(parse_expr(&text, span.start, line)?)),
		})
		.collect()
}

fn to_args(raw: Vec<Vec<RawPart>>, line: usize) -> Result<Vec<Vec<InterpPart>>, ParseError> {
	raw.into_iter().map(|w| to_parts(w, line)).collect()
}

// ---------------------------------------------------------------- expressions

pub fn parse_expr(s: &str, base: usize, line: usize) -> Result<Expr, ParseError> {
	let toks = lexer::tokenize(s, base, line)?;
	if toks.is_empty() {
		return err(line, "expected an expression");
	}
	let mut e = E { t: &toks, i: 0, line };
	let out = e.chain()?;
	if e.i != e.t.len() {
		return err(
			line,
			format!("trailing tokens after expression: `{}`", &s[..s.len().min(60)]),
		);
	}
	Ok(out)
}

struct E<'a> {
	t: &'a [Spanned],
	i: usize,
	line: usize,
}

impl<'a> E<'a> {
	fn peek(&self) -> Option<&'a Token> {
		self.t.get(self.i).map(|s| &s.token)
	}
	fn eat(&mut self, p: &str) -> bool {
		if matches!(self.peek(), Some(Token::Punct(x)) if *x == p) {
			self.i += 1;
			return true;
		}
		false
	}
	fn span(&self, from: usize) -> Span {
		let a = self.t[from].span;
		let b = self.t[self.i.saturating_sub(1).max(from)].span;
		a.to(b)
	}

	fn chain(&mut self) -> Result<Expr, ParseError> {
		let from = self.i;
		let mut lhs = self.or()?;
		while self.eat("?") {
			let rhs = self.or()?;
			lhs = Expr::Chain {
				lhs: Box::new(lhs),
				rhs: Box::new(rhs),
				span: self.span(from),
			};
		}
		Ok(lhs)
	}
	fn or(&mut self) -> Result<Expr, ParseError> {
		let from = self.i;
		let mut lhs = self.and()?;
		while self.eat("||") {
			let rhs = self.and()?;
			lhs = bin(BinaryOp::Or, lhs, rhs, self.span(from));
		}
		Ok(lhs)
	}
	fn and(&mut self) -> Result<Expr, ParseError> {
		let from = self.i;
		let mut lhs = self.cmp()?;
		while self.eat("&&") {
			let rhs = self.cmp()?;
			lhs = bin(BinaryOp::And, lhs, rhs, self.span(from));
		}
		Ok(lhs)
	}
	fn cmp(&mut self) -> Result<Expr, ParseError> {
		let from = self.i;
		let mut lhs = self.add()?;
		loop {
			let op = match self.peek() {
				Some(Token::Punct("==")) => BinaryOp::Eq,
				Some(Token::Punct("!=")) => BinaryOp::Ne,
				Some(Token::Punct("<=")) => BinaryOp::Le,
				Some(Token::Punct(">=")) => BinaryOp::Ge,
				Some(Token::Punct("<")) => BinaryOp::Lt,
				Some(Token::Punct(">")) => BinaryOp::Gt,
				_ => return Ok(lhs),
			};
			self.i += 1;
			let rhs = self.add()?;
			lhs = bin(op, lhs, rhs, self.span(from));
		}
	}
	fn add(&mut self) -> Result<Expr, ParseError> {
		let from = self.i;
		let mut lhs = self.mul()?;
		loop {
			let op = match self.peek() {
				Some(Token::Punct("+")) => BinaryOp::Add,
				Some(Token::Punct("-")) => BinaryOp::Sub,
				_ => return Ok(lhs),
			};
			self.i += 1;
			let rhs = self.mul()?;
			lhs = bin(op, lhs, rhs, self.span(from));
		}
	}
	fn mul(&mut self) -> Result<Expr, ParseError> {
		let from = self.i;
		let mut lhs = self.unary()?;
		loop {
			let op = match self.peek() {
				Some(Token::Punct("*")) => BinaryOp::Mul,
				Some(Token::Punct("/")) => BinaryOp::Div,
				Some(Token::Punct("%")) => BinaryOp::Rem,
				_ => return Ok(lhs),
			};
			self.i += 1;
			let rhs = self.unary()?;
			lhs = bin(op, lhs, rhs, self.span(from));
		}
	}
	fn unary(&mut self) -> Result<Expr, ParseError> {
		let from = self.i;
		for (p, op) in [("!", UnaryOp::Not), ("-", UnaryOp::Neg)] {
			if self.eat(p) {
				let rhs = self.unary()?;
				return Ok(Expr::Unary {
					op,
					rhs: Box::new(rhs),
					span: self.span(from),
				});
			}
		}
		self.postfix()
	}
	fn postfix(&mut self) -> Result<Expr, ParseError> {
		let from = self.i;
		let mut e = self.primary()?;
		loop {
			if self.eat("[") {
				let idx = self.chain()?;
				if !self.eat("]") {
					return err(self.line, "expected `]`");
				}
				e = Expr::Index {
					base: Box::new(e),
					index: Box::new(idx),
					span: self.span(from),
				};
			} else if matches!(self.peek(), Some(Token::Punct("("))) {
				let Expr::Ident(name, _) = e else {
					return err(self.line, "only a named function can be called");
				};
				self.i += 1;
				let mut args = Vec::new();
				if !self.eat(")") {
					loop {
						args.push(self.chain()?);
						if self.eat(",") {
							if self.eat(")") {
								break;
							}
							continue;
						}
						if self.eat(")") {
							break;
						}
						return err(self.line, "expected `,` or `)` in argument list");
					}
				}
				e = Expr::Call {
					name,
					args,
					span: self.span(from),
				};
			} else {
				return Ok(e);
			}
		}
	}
	fn primary(&mut self) -> Result<Expr, ParseError> {
		let from = self.i;
		let Some(tok) = self.peek().cloned() else {
			return err(self.line, "expected a value");
		};
		match tok {
			Token::Number(n) => {
				self.i += 1;
				Ok(Expr::Number(n, self.span(from)))
			}
			Token::Str(parts) => {
				self.i += 1;
				let span = self.span(from);
				Ok(Expr::Str(to_parts(parts, self.line)?, span))
			}
			Token::Punct("[") => {
				self.i += 1;
				let mut items = Vec::new();
				if !self.eat("]") {
					loop {
						items.push(self.chain()?);
						if self.eat(",") {
							if self.eat("]") {
								break;
							}
							continue;
						}
						if self.eat("]") {
							break;
						}
						return err(self.line, "expected `,` or `]` in list");
					}
				}
				Ok(Expr::List(items, self.span(from)))
			}
			Token::Punct("(") => {
				self.i += 1;
				let e = self.chain()?;
				if !self.eat(")") {
					return err(self.line, "expected `)`");
				}
				Ok(e)
			}
			Token::Ident(name) => {
				self.i += 1;
				let kind = match name.as_str() {
					"ARG" => Some(SourceKind::Arg),
					"ENV" => Some(SourceKind::Env),
					"FLAG" => Some(SourceKind::Flag),
					"RUN" => Some(SourceKind::Run),
					_ => None,
				};
				if let Some(kind) = kind {
					if !self.eat(".") {
						return err(self.line, format!("`{name}` needs a `.key`"));
					}
					let Some(Token::Ident(key)) = self.peek().cloned() else {
						return err(self.line, format!("`{name}.` needs a key"));
					};
					self.i += 1;
					return Ok(Expr::Source {
						kind,
						key: Some(key),
						span: self.span(from),
					});
				}
				match name.as_str() {
					"ARGS" => Ok(Expr::Source {
						kind: SourceKind::Args,
						key: None,
						span: self.span(from),
					}),
					"true" => Ok(Expr::Bool(true, self.span(from))),
					"false" => Ok(Expr::Bool(false, self.span(from))),
					_ => Ok(Expr::Ident(name, self.span(from))),
				}
			}
			other => err(self.line, format!("unexpected {other:?}")),
		}
	}
}

fn bin(op: BinaryOp, lhs: Expr, rhs: Expr, span: Span) -> Expr {
	Expr::Binary {
		op,
		lhs: Box::new(lhs),
		rhs: Box::new(rhs),
		span,
	}
}
