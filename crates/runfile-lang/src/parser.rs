//! Line classification, block assembly and expression parsing.

use crate::ast::*;
use crate::lexer::{self, LexError, RawPart, Spanned, Token};
use crate::span::Span;
use std::collections::BTreeSet;
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

/// `let` and reassignment both bind a name, and both used to take whatever text
/// preceded the `=` -- including nothing at all.
fn check_binding_name(name: &str, line: usize) -> Result<(), ParseError> {
	if name.is_empty() {
		return err(line, "binding has no name");
	}
	let ok = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
		&& name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
	if !ok {
		return err(line, format!("`{name}` is not a valid name"));
	}
	Ok(())
}

/// The names on the left of a `let`, a reassignment or a `for`.
///
/// One name is an ordinary binding; several unpack a list positionally. `_` is
/// a position matched and thrown away, and may be written as often as it is
/// useful -- it is not a name (an identifier starts with a letter or a letter
/// after `_`), so it cannot shadow one and two of them cannot collide.
fn binding_names(text: &str, line: usize) -> Result<Vec<String>, ParseError> {
	let names: Vec<String> = text.split(',').map(|n| n.trim().to_string()).collect();
	for n in &names {
		if n != "_" {
			check_binding_name(n, line)?;
		}
	}
	Ok(names)
}

/// One binding name, or several separated by commas.
///
/// The cheap form of [`binding_names`], for deciding whether a line is a
/// reassignment at all before committing to reading one.
fn is_name_list(text: &str) -> bool {
	!text.is_empty() && text.split(',').all(|n| is_name(n.trim()))
}

fn is_name(n: &str) -> bool {
	n == "_"
		|| (!n.is_empty()
			&& n.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
			&& n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
}

/// A condition or an iterable: either an expression or a `$` run, whose exit
/// status is the answer.
fn condition(text: &str, at: usize, no: usize) -> Result<Expr, ParseError> {
	match shell_capture(text, at, no)? {
		Some(e) => Ok(e),
		None => parse_expr(text, at, no),
	}
}

/// `rest` with `word` and the whitespace after it removed, when that is how it
/// begins. `else ifx` is not an `else if`.
fn after_keyword<'a>(rest: &'a str, word: &str) -> Option<&'a str> {
	let tail = rest.strip_prefix(word)?;
	if tail.is_empty() {
		return Some(tail);
	}
	tail.starts_with(char::is_whitespace).then(|| tail.trim_start())
}

/// Where `part` starts in the source, given where `whole` does.
///
/// `part` is a slice of `whole` -- a `trim` of it, a `strip_prefix`, a range --
/// which is what every piece of a line the parser reads is. Asked of the
/// pointers rather than worked out from lengths, since a length cannot say how
/// many blanks a `trim` took off the front: that miscount is how a name used to
/// be placed a column or three before itself.
fn offset_in(whole: &str, at: usize, part: &str) -> usize {
	let (w, p) = (whole.as_ptr() as usize, part.as_ptr() as usize);
	if p < w || p > w + whole.len() {
		return at;
	}
	at + (p - w)
}

/// The text of a statement, gathered by [`P::logical`], and where it came from.
struct Logical {
	text: String,
	/// Where `text` starts in the source: its first byte of code, past the
	/// indentation. An offset into `text` plus this is an offset into the file.
	at: usize,
	/// Where its first line starts, indentation included, which is where the
	/// statement's own span starts.
	start: usize,
	no: usize,
}

/// Whether this raw line is the `end` that closes a body opened at `indent`.
///
/// The indentation has to match the *opener's* exactly, which is what lets a
/// body carry an `end` of its own (ruby, lua) without closing the block early.
/// The `end` line itself is this language's rather than the body's, so it takes
/// a comment like any other -- and this is the one place that has to be said
/// more than once, since the formatter finds the closer by the same rule and
/// the two disagreeing would let it move a line out of a body. The language
/// server asks it too, to find a body in a document that does not parse.
pub fn closes_body(raw: &str, indent: &str) -> bool {
	let Some(rest) = raw.strip_prefix(indent) else {
		return false;
	};
	!rest.starts_with([' ', '\t']) && lexer::code(rest.trim_end()) == "end"
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
	for (i, full) in src.split('\n').enumerate() {
		// A CRLF file must parse as its LF twin: the carriage return is line
		// ending, not content. Left in, it reached exec bodies as text and made
		// an indented `end` compare unequal to its opener's indentation, so the
		// block never closed. Offsets still count it, since they index `src`.
		let raw = full.strip_suffix('\r').unwrap_or(full);
		let trimmed = raw.trim();
		let indent = &raw[..raw.len() - raw.trim_start().len()];
		out.push(Line {
			raw,
			trimmed,
			indent,
			offset,
			no: i + 1,
		});
		offset += full.len() + 1;
	}
	out
}

pub fn parse(src: &str) -> Result<Target, ParseError> {
	let lines = scan_lines(src);
	let mut p = P {
		lines: &lines,
		i: 0,
		loops: 0,
		unfold_next: false,
		branch: None,
		fence: None,
	};
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
	/// How many loop bodies enclose the line being parsed.
	///
	/// `break` and `continue` are refused outside one *here* rather than at
	/// run time, so an editor underlines them: whether a line sits inside a
	/// loop is a question about the text, and a run that finds out has already
	/// done half the work.
	loops: usize,
	/// Set for the one statement being parsed directly inside a `parallel do`,
	/// where a `$` line is a branch of its own rather than the first of a run:
	/// folded, `$ lint` and `$ test` were one process, run one after the other,
	/// under a block that said they were not.
	unfold_next: bool,
	/// The names the parallel branch being parsed binds itself, when there is
	/// one. Every branch works on its own copy of the scope, so assigning a
	/// name bound outside it would be lost when the block ends -- which is
	/// refused here, where an editor can say so, rather than silently dropped.
	branch: Option<BTreeSet<String>>,
	/// The innermost parallel block, as the loop depth it opened at. `break`
	/// and `continue` cannot reach a loop outside it: its branches run at once,
	/// so there is no order for them to leave.
	fence: Option<Fence>,
}

/// Where a parallel block sits among the loops around it.
#[derive(Clone, Copy)]
struct Fence {
	/// `P.loops` when the block opened.
	loops: usize,
	/// `parallel for`, whose own iterations `continue` may end.
	iterates: bool,
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
		self.block_of(kw, false)
	}

	/// A block, whose statements are each a branch of their own when it is the
	/// body of a `parallel do`.
	fn block_of(&mut self, kw: Option<&str>, branches: bool) -> Result<Block, ParseError> {
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
			// At file level `at_block_end` says no, so a closer would otherwise
			// fall through and parse as an expression statement -- deferring a
			// plain typo to a confusing "not defined" at run time.
			if kw.is_none() {
				let head = line.trimmed.split_whitespace().next().unwrap_or("");
				if matches!(head, "end" | "else" | "case" | "default") {
					return err(line.no, format!("`{head}` closes a block, but none is open"));
				}
			}
			if line.trimmed.starts_with('.') {
				b.properties.push(self.property()?);
			} else {
				let mut st = if branches {
					self.branch_statement()?
				} else {
					self.statement()?
				};
				// Whatever the statement read, the last line of it is the one
				// before where parsing resumes: a block's `end`, a spilled list's
				// `]`, a run's last `$` line.
				let last = &self.lines[self.i - 1];
				st.span_mut().end = last.offset + last.raw.len();
				b.statements.push(st);
			}
		}
	}

	fn property(&mut self) -> Result<Property, ParseError> {
		let line = &self.lines[self.i];
		self.i += 1;
		// A property is one physical line -- it never spills the way a list
		// does -- so it takes its comment off itself rather than through
		// `logical`. `t` is still a slice of `raw`, which the offsets index.
		let t = lexer::code(line.trimmed);
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
	/// span lines -- and nest, since the count is what says where it ends.
	///
	/// Each *physical* line loses its comment before it is joined, which is
	/// what lets one sit beside an element of a spilled list, or on a line of
	/// its own between two of them.
	///
	/// What sits between two lines' code -- the first one's comment, its line
	/// ending, the next one's indentation -- is joined as blanks of the same
	/// length, with the newline kept. So every byte of the text is at its own
	/// offset from [`Logical::at`], and every token after a newline is on its
	/// own line: a name on the third line of a list is reported on the third
	/// line, and underlined where it is. Joined with one space, as this was,
	/// both were placed by the first line.
	fn logical(&mut self) -> Logical {
		let lines = self.lines;
		let first = &lines[self.i];
		let code = lexer::code(first.trimmed);
		let at = first.offset + first.indent.len();
		let mut text = code.to_string();
		// Just past the last byte of code joined so far, in the source.
		let mut end = at + code.len();
		self.i += 1;
		while lexer::brackets(&text, first.no).0 > 0 && self.i < lines.len() {
			let (prev, line) = (&lines[self.i - 1], &lines[self.i]);
			let prev_end = prev.offset + prev.raw.len();
			// A `\r\n` is two bytes and one line ending, so its `\r` is a blank.
			text.extend(std::iter::repeat_n(
				' ',
				(prev_end - end) + (line.offset - prev_end - 1),
			));
			text.push('\n');
			text.extend(std::iter::repeat_n(' ', line.indent.len()));
			let code = lexer::code(line.trimmed);
			text.push_str(code);
			end = line.offset + line.indent.len() + code.len();
			self.i += 1;
		}
		Logical {
			text,
			at,
			start: first.offset,
			no: first.no,
		}
	}

	fn statement(&mut self) -> Result<Statement, ParseError> {
		// Taken here, so it applies to this statement and to nothing nested in it.
		let unfold = std::mem::take(&mut self.unfold_next);
		let line = &self.lines[self.i];
		let no = line.no;
		let indent = line.indent;

		if line.trimmed == "$" || line.trimmed.starts_with("$ ") {
			return self.shell_run(false, unfold);
		}
		if line.trimmed.starts_with("exec ") {
			return self.exec_block(false);
		}
		// `detach` marks the command it prefixes. Claimed only in front of the
		// two forms that start a process, so it stays an ordinary name
		// everywhere else -- `detach = 5` is still a reassignment.
		if let Some(rest) = line.trimmed.strip_prefix("detach ").map(str::trim_start) {
			if rest == "$" || rest.starts_with("$ ") {
				return self.shell_run(true, true);
			}
			if rest.starts_with("exec ") {
				return self.exec_block(true);
			}
			if rest.starts_with("run ") {
				return err(
					no,
					"`detach` marks a command, and `run` dispatches in-process -- there is no process to detach",
				);
			}
		}

		let Logical { text, at, start, .. } = self.logical();
		let head = text.split_whitespace().next().unwrap_or("");
		let span = Span::new(start, start + text.len(), no);
		// Where a piece of `text` is in the file.
		let pos = |part: &str| offset_in(&text, at, part);

		match head {
			// Claimed only in front of a keyword, so it stays an ordinary name
			// everywhere else -- `parallel = 5` is still a binding.
			"parallel"
				if matches!(
					text[8..].split_whitespace().next(),
					Some(
						"do" | "for"
							| "if" | "while" | "until"
							| "loop" | "match" | "retry"
							| "exec" | "run" | "detach"
							| "$"
					)
				) =>
			{
				let rest = text[8..].trim_start();
				match rest.split_whitespace().next() {
					Some("do") => self.parallel_do(rest, no, span),
					Some("for") => self.parallel_for(rest, pos(rest), no, span),
					Some(other) => err(
						no,
						format!(
							"`parallel` takes `do` or `for`, not `{other}`: `parallel do` makes each statement inside a branch, `parallel for` each iteration"
						),
					),
					None => unreachable!("the guard saw a word"),
				}
			}
			"let" => {
				let rest = text[3..].trim_start();
				let Some(eq) = rest.find('=') else {
					return err(no, "`let` needs `= value`");
				};
				let names = binding_names(rest[..eq].trim(), no)?;
				// Inside a parallel branch, a name bound here is the branch's own.
				if let Some(own) = &mut self.branch {
					own.extend(names.iter().filter(|n| *n != "_").cloned());
				}
				let raw_rhs = rest[eq + 1..].trim();
				let base = pos(raw_rhs);
				let value = match self.capture_rhs(raw_rhs, indent, base, no)? {
					Some(e) => e,
					None => parse_expr(raw_rhs, base, no)?,
				};
				Ok(Statement::Let { names, value, span })
			}
			"do" => {
				if !text[2..].trim().is_empty() {
					return err(
						no,
						"`do` takes nothing; it opens a block so a property has somewhere to go",
					);
				}
				let body = self.block(Some("do"))?;
				self.expect_end(no)?;
				Ok(Statement::Do {
					body,
					parallel: false,
					span,
				})
			}
			"if" => {
				let rest = text[2..].trim();
				let cond = condition(rest, pos(rest), no)?;
				let st = self.if_tail(cond, span)?;
				self.expect_end(no)?;
				Ok(st)
			}
			// `while cond`, `until cond` -- the same body, opposite questions.
			// `until` is the shape a wait loop wants, and the corpus wrote four
			// of them as shell `until … do sleep … done`.
			"while" | "until" => {
				let rest = text[head.len()..].trim();
				if rest.is_empty() {
					return err(no, format!("`{head}` needs a condition, as `{head} n < 10`"));
				}
				let cond = condition(rest, pos(rest), no)?;
				let test = if head == "while" {
					LoopTest::While(cond)
				} else {
					LoopTest::Until(cond)
				};
				let body = self.loop_body(head)?;
				self.expect_end(no)?;
				Ok(Statement::Loop { test, body, span })
			}
			// Takes nothing, and says so: anything after it would read as a
			// condition, which is the one thing a `loop` does not have.
			"loop" => {
				if !text[4..].trim().is_empty() {
					return err(
						no,
						"`loop` takes nothing; it runs until a `break`, and `while` is the form that asks a question",
					);
				}
				let body = self.loop_body("loop")?;
				self.expect_end(no)?;
				Ok(Statement::Loop {
					test: LoopTest::Forever,
					body,
					span,
				})
			}
			"break" | "continue" => {
				if !text[head.len()..].trim().is_empty() {
					return err(no, format!("`{head}` takes nothing after it"));
				}
				if self.loops == 0 {
					return err(
						no,
						format!("`{head}` is only meaningful inside a `for`, `while`, `until` or `loop`"),
					);
				}
				if let Some(fence) = self.fence {
					// Loops opened inside the parallel block, `parallel for` included.
					let opened = self.loops - fence.loops;
					if opened == 0 {
						return err(
							no,
							format!(
								"`{head}` cannot reach a loop outside a `parallel` block: its branches run at once, so there is no order to leave"
							),
						);
					}
					if fence.iterates && opened == 1 && head == "break" {
						return err(
							no,
							"`break` cannot stop a `parallel for`: every iteration is already running -- `continue` ends this one",
						);
					}
				}
				Ok(if head == "break" {
					Statement::Break { span }
				} else {
					Statement::Continue { span }
				})
			}
			// `retry n [every secs]` … `[else …]` `end`
			"retry" => {
				let rest = text[5..].trim();
				let (count, wait) = match rest.split_once(" every ") {
					Some((a, d)) => (a.trim(), Some(d.trim())),
					None => (rest, None),
				};
				if count.is_empty() {
					return err(no, "`retry` needs a number of attempts, as `retry 30 every 1`");
				}
				let attempts = parse_expr(count, pos(count), no)?;
				let delay = match wait {
					Some(d) => Some(parse_expr(d, pos(d), no)?),
					None => None,
				};
				let body = self.block(Some("retry"))?;
				let otherwise = if self.peek_kw() == Some("else") {
					self.i += 1;
					Some(self.block(Some("retry"))?)
				} else {
					None
				};
				self.expect_end(no)?;
				Ok(Statement::Retry {
					attempts,
					delay,
					body,
					otherwise,
					span,
				})
			}
			"for" => {
				let (names, iter) = for_header(&text[3..], pos(&text[3..]), no)?;
				let body = self.for_body(&names)?;
				self.expect_end(no)?;
				Ok(Statement::For {
					names,
					iter,
					body,
					parallel: false,
					span,
				})
			}
			"match" => {
				let rest = text[5..].trim();
				let subject = match shell_capture(rest, pos(rest), no)? {
					Some(e) => e,
					None => parse_expr(rest, pos(rest), no)?,
				};
				let (mut cases, mut default) = (Vec::new(), None);
				loop {
					match self.peek_kw() {
						Some("case") => {
							let l = &self.lines[self.i];
							let label = case_label(l.trimmed, l.no)?;
							let cs = Span::new(l.offset, l.offset + lexer::code(l.trimmed).len(), l.no);
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
				let (target, args) = dispatch_words(split_words(rest), pos(rest), no)?;
				Ok(Statement::Run { target, args, span })
			}
			_ => {
				// `code_of($ cmd)` on its own: run it, ignore how it went.
				if let Some(e) = shell_capture(&text, at, no)? {
					return Ok(Statement::Call { expr: e, span });
				}
				if let Some(eq) = assignment_split(&text) {
					let names = binding_names(text[..eq].trim(), no)?;
					self.check_reassign(&names, no)?;
					let raw_rhs = text[eq + 1..].trim();
					let value = match self.capture_rhs(raw_rhs, indent, pos(raw_rhs), no)? {
						Some(e) => e,
						None => parse_expr(raw_rhs, pos(raw_rhs), no)?,
					};
					return Ok(Statement::Assign { names, value, span });
				}
				let expr = parse_expr(&text, at, no)?;
				check_has_effect(&expr, no)?;
				Ok(Statement::Call { expr, span })
			}
		}
	}

	/// An `if` and every `else`/`else if` after it, down to but **not**
	/// including the `end` that closes the chain.
	///
	/// A chain is one block with one `end`, so only the caller consumes it.
	/// `else if` builds a nested `If` here rather than going back through
	/// `statement`, which would want an `end` of its own -- which means
	/// nothing downstream has to tell a chain from a nest: `else` + a nested
	/// `if` and `else if` parse to the same tree, and the formatter, being
	/// line-oriented, prints back whichever was written.
	fn if_tail(&mut self, cond: Expr, span: Span) -> Result<Statement, ParseError> {
		let then = self.block(Some("if"))?;
		let otherwise = if self.peek_kw() == Some("else") {
			let Logical { text, at, no, .. } = self.logical();
			let rest = text[4..].trim();
			if rest.is_empty() {
				Some(self.block(Some("if"))?)
			} else {
				// Trailing text used to be dropped without a word, so an
				// `else x > 1` ran its block unconditionally.
				let Some(tail) = after_keyword(rest, "if") else {
					return err(
						no,
						format!("`else` takes nothing after it, or `if <condition>`: `{rest}`"),
					);
				};
				if tail.is_empty() {
					return err(no, "`else if` needs a condition");
				}
				let inner = Span::new(offset_in(&text, at, rest), at + text.len(), no);
				let icond = condition(tail, offset_in(&text, at, tail), no)?;
				Some(Block {
					properties: Vec::new(),
					statements: vec![self.if_tail(icond, inner)?],
				})
			}
		} else {
			None
		};
		Ok(Statement::If {
			cond,
			then,
			otherwise,
			span,
		})
	}

	/// One statement directly inside a `parallel do`: a branch of its own.
	fn branch_statement(&mut self) -> Result<Statement, ParseError> {
		self.unfold_next = true;
		let outer = self.branch.replace(BTreeSet::new());
		let st = self.statement();
		self.branch = outer;
		self.unfold_next = false;
		let st = st?;
		if let Statement::Let { names, .. } = &st
			&& names.iter().any(|n| n != "_")
		{
			return err(
				st.span().line,
				"a `let` directly inside `parallel do` binds nothing anyone can read: each statement there is a branch of its own, and what a branch binds ends with it -- bind it before the block, or inside the branch that uses it",
			);
		}
		Ok(st)
	}

	/// `parallel do` and its body. `rest` starts at `do`.
	fn parallel_do(&mut self, rest: &str, no: usize, span: Span) -> Result<Statement, ParseError> {
		if !rest[2..].trim().is_empty() {
			return err(
				no,
				"`parallel do` takes nothing after it; each statement inside is a branch",
			);
		}
		let fence = self.fence.replace(Fence {
			loops: self.loops,
			iterates: false,
		});
		let body = self.block_of(Some("do"), true);
		self.fence = fence;
		let body = body?;
		self.expect_end(no)?;
		Ok(Statement::Do {
			body,
			parallel: true,
			span,
		})
	}

	/// `parallel for` and its body, which is one branch per iteration. `rest`
	/// starts at `for`, at byte `base` of the source.
	fn parallel_for(&mut self, rest: &str, base: usize, no: usize, span: Span) -> Result<Statement, ParseError> {
		let (names, iter) = for_header(&rest[3..], base + 3, no)?;
		// The loop's names are each iteration's own, and nothing an iteration
		// binds reaches the next one or the code after the loop.
		let branch = self
			.branch
			.replace(names.iter().filter(|n| *n != "_").cloned().collect());
		let fence = self.fence.replace(Fence {
			loops: self.loops,
			iterates: true,
		});
		let body = self.loop_body("for");
		self.branch = branch;
		self.fence = fence;
		let body = body?;
		self.expect_end(no)?;
		Ok(Statement::For {
			names,
			iter,
			body,
			parallel: true,
			span,
		})
	}

	/// A plain `for`'s body. Inside a parallel branch its names are the
	/// branch's own for the extent of the loop, and gone again after it: the
	/// runner puts a loop variable back when the loop ends, so a later
	/// assignment to one reaches whatever it named outside.
	fn for_body(&mut self, names: &[String]) -> Result<Block, ParseError> {
		let added: Vec<String> = match &mut self.branch {
			Some(own) => names
				.iter()
				.filter(|n| *n != "_" && own.insert((*n).clone()))
				.cloned()
				.collect(),
			None => Vec::new(),
		};
		let body = self.loop_body("for");
		if let Some(own) = &mut self.branch {
			for n in &added {
				own.remove(n);
			}
		}
		body
	}

	/// Refuse assigning, inside a parallel branch, a name the branch did not
	/// bind itself. Each branch works on its own copy of the scope, so the
	/// value would be gone when the block ends -- a lost write rather than a
	/// race, but lost all the same.
	fn check_reassign(&self, names: &[String], no: usize) -> Result<(), ParseError> {
		let Some(own) = &self.branch else {
			return Ok(());
		};
		match names.iter().find(|n| *n != "_" && !own.contains(*n)) {
			Some(n) => err(
				no,
				format!(
					"`{n}` is bound outside this parallel branch, and every branch works on its own copy -- what it assigns here is gone when the block ends; compute it before the block, or `let` a name inside the branch"
				),
			),
			None => Ok(()),
		}
	}

	/// A loop's body, with the depth `break` and `continue` are checked
	/// against raised for its extent.
	fn loop_body(&mut self, kw: &str) -> Result<Block, ParseError> {
		self.loops += 1;
		let out = self.block(Some(kw));
		self.loops -= 1;
		out
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
	/// `detach` marks the run's opening line; `single` ends the run with it --
	/// true for a detached command, and for a `$` line that is a parallel
	/// branch of its own.
	fn shell_run(&mut self, detach: bool, single: bool) -> Result<Statement, ParseError> {
		let first = &self.lines[self.i];
		let (no, offset) = (first.no, first.offset);
		let mut body = Vec::new();
		let mut lines = Vec::new();
		let mut last = self.i;
		let mut j = self.i;
		while j < self.lines.len() {
			let l = &self.lines[j];
			// The opening line carries the marker; everything after it is a
			// plain `$` line, and a `detach $` line further down is a statement
			// of its own rather than more of this one.
			let trimmed = match detach && j == self.i {
				true => l.trimmed.strip_prefix("detach").unwrap().trim_start(),
				false => l.trimmed,
			};
			if trimmed == "$" || trimmed.starts_with("$ ") {
				let first = trimmed.strip_prefix('$').unwrap().trim_start();
				let mut parts = to_parts(lexer::split_interp(first, offset_in(l.raw, l.offset, first), l.no)?)?;
				// A trailing backslash continues the shell line. The backslash and
				// newline are kept so the shell sees the continuation it expects,
				// and the author's formatting survives into --dry-run output. Each
				// line is split on its own, so what one interpolates is placed on
				// it: a `\r\n` is two bytes where the joined text has one, so the
				// text as a whole has no single offset to count from.
				let mut ends = first;
				while ends.ends_with('\\') && j + 1 < self.lines.len() {
					j += 1;
					let next = &self.lines[j];
					parts.push(InterpPart::Literal("\n".into()));
					parts.extend(to_parts(lexer::split_interp(next.raw, next.offset, next.no)?)?);
					ends = next.raw;
				}
				body.push(joined(parts));
				lines.push(l.no);
				last = j;
				j += 1;
				// One command, so the run ends with its own line.
				if single {
					break;
				}
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
			lines,
			detach,
			span: Span::new(offset, end, no),
		})
	}

	/// Read an `exec` body: lines until an `end` at `indent`, dedented by their
	/// own base indentation. See [`closes_body`] for what counts as one.
	#[allow(clippy::type_complexity)]
	fn exec_body(&mut self, indent: &str, no: usize) -> Result<(Vec<Vec<InterpPart>>, Vec<usize>, usize), ParseError> {
		let mut j = self.i;
		let mut raw: Vec<&Line> = Vec::new();
		loop {
			if j >= self.lines.len() {
				return err(no, "`exec` block never closed by an `end` at its own indentation");
			}
			let l = &self.lines[j];
			if closes_body(l.raw, indent) {
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
				to_parts(lexer::split_interp(text, l.offset + base, l.no)?)
			})
			.collect::<Result<_, _>>()?;
		let lines = raw.iter().map(|l| l.no).collect();
		let end = self.lines[j].offset + self.lines[j].raw.len();
		self.i = j + 1;
		Ok((body, lines, end))
	}

	/// A `$ cmd` or `exec cmd … end` used as a value.
	fn capture_rhs(&mut self, rhs: &str, indent: &str, offset: usize, no: usize) -> Result<Option<Expr>, ParseError> {
		// A call whose last argument is a `$` run -- `lines($ git ls-files)`,
		// and `code_of($ cmd)`, which is that same shape wearing a name people
		// already know -- or a `run` dispatch, which only `code_of` may hold.
		// `shell_capture` answers `None` when the `$` turns out to be inside a
		// string, so this only has to be a cheap first look.
		if !rhs.starts_with("$ ")
			&& (rhs.contains("$ ") || rhs.contains("run"))
			&& let Some(e) = shell_capture(rhs, offset, no)?
		{
			return Ok(Some(e));
		}
		if let Some(cmd) = rhs.strip_prefix("$ ") {
			let cmd = cmd.trim();
			let parts = to_parts(lexer::split_interp(cmd, offset_in(rhs, offset, cmd), no)?)?;
			let span = Span::new(offset, offset + rhs.len(), no);
			return Ok(Some(Expr::Capture {
				command: None,
				body: vec![parts],
				span,
			}));
		}
		// `json … end`, and whatever formats join it: a block of structured
		// text, closing on an `end` at the opener's own indentation like any
		// other body.
		if let Some(format) = crate::Structured::from_keyword(rhs) {
			let (body, _, end) = self.exec_body(indent, no)?;
			// Checked here, with each interpolation standing in as a value of
			// the format: a missing brace is then an error in the editor
			// rather than one the far end reports hours later. What the values
			// turn out to be cannot change the shape.
			let probe = body
				.iter()
				.map(|parts| {
					parts
						.iter()
						.map(|p| match p {
							InterpPart::Literal(t) => t.as_str(),
							InterpPart::Expr(_) => format.placeholder(),
						})
						.collect::<String>()
				})
				.collect::<Vec<_>>()
				.join("\n");
			if let Err(e) = format.validate(&probe) {
				return err(
					no,
					format!(
						"this `{}` block is not valid {}: {e}",
						format.keyword(),
						format.keyword()
					),
				);
			}
			return Ok(Some(Expr::Structured {
				body: crate::ast::StructuredBody { format, lines: body },
				span: Span::new(offset, end, no),
			}));
		}
		if let Some(cmd) = rhs.strip_prefix("exec ") {
			let cmd = cmd.trim();
			let command = to_parts(lexer::split_interp(cmd, offset_in(rhs, offset, cmd), no)?)?;
			// A capture is an expression: its value is what the command prints,
			// so per-line positions have nothing to report against.
			let (body, _, end) = self.exec_body(indent, no)?;
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
	fn exec_block(&mut self, detach: bool) -> Result<Statement, ParseError> {
		let open = &self.lines[self.i];
		let (no, offset, indent) = (open.no, open.offset, open.indent);
		let head = match detach {
			true => open.trimmed.strip_prefix("detach").unwrap().trim_start(),
			false => open.trimmed,
		};
		let cmd_text = lexer::code(head.strip_prefix("exec ").unwrap().trim());
		let command = to_parts(lexer::split_interp(
			cmd_text,
			offset_in(open.raw, offset, cmd_text),
			no,
		)?)?;
		self.i += 1;
		let indent = indent.to_string();
		let (body, lines, end) = self.exec_body(&indent, no)?;
		Ok(Statement::Exec {
			command: Some(command),
			body,
			lines,
			detach,
			span: Span::new(offset, end, no),
		})
	}
}

/// The part of a `for` line after the keyword -- its names, and its list --
/// which `for` and `parallel for` share. `base` is where `rest` starts.
fn for_header(header: &str, base: usize, no: usize) -> Result<(Vec<String>, Expr), ParseError> {
	let rest = header.trim();
	let Some(k) = rest.find(" in ") else {
		return err(no, "`for` needs `in`");
	};
	let names = binding_names(rest[..k].trim(), no)?;
	// `for f in lines($ git ls-files)` -- the same rule as a `let`, and the
	// shape a loop over a command's output actually wants.
	let list = rest[k + 4..].trim();
	let iter = condition(list, offset_in(header, base, list), no)?;
	Ok((names, iter))
}

/// Whether an expression can do anything when evaluated for effect.
///
/// Only a call can: everything else computes a value, and a statement throws
/// that value away. `write_file(…)` is a statement; `write_file(…) ? "x"` and
/// `f() && g()` are too, since a call is still reached. `35` is not.
fn has_effect(e: &Expr) -> bool {
	match e {
		Expr::Call { .. } | Expr::Capture { .. } | Expr::Dispatch { .. } => true,
		Expr::Unary { rhs, .. } => has_effect(rhs),
		Expr::Binary { lhs, rhs, .. } | Expr::Chain { lhs, rhs, .. } => has_effect(lhs) || has_effect(rhs),
		Expr::Index { base, index, .. } => has_effect(base) || has_effect(index),
		Expr::List(items, _) => items.iter().any(has_effect),
		Expr::Structured { .. }
		| Expr::Number(..)
		| Expr::Bool(..)
		| Expr::Str(..)
		| Expr::Ident(..)
		| Expr::Source { .. } => false,
	}
}

/// Reject a statement that computes a value and discards it.
///
/// A line like `abc`, `35` or `"hi"` is always a mistake -- most often a call
/// with the parentheses left off. Caught here rather than at evaluation so an
/// editor underlines it, and because at statement level it is inert whether or
/// not the name happens to be bound, which is not true inside an expression.
fn check_has_effect(e: &Expr, no: usize) -> Result<(), ParseError> {
	if has_effect(e) {
		return Ok(());
	}
	if let Expr::Ident(name, _) = e
		&& crate::functions::exists(name)
	{
		return err(no, format!("`{name}` is a function; call it as `{name}()`"));
	}
	let what = match e {
		Expr::Ident(name, _) => format!("`{name}` is a value, not something to run"),
		Expr::Source { .. } => "an input is a value, not something to run".to_string(),
		_ => "it computes a value and discards it".to_string(),
	};
	err(no, format!("this line does nothing: {what}"))
}

/// `$ cmd` used as a value, and a call whose **last argument** is one.
///
/// Only a `$` run, never an `exec` block: an `exec` closes on an `end` at its
/// opener's indentation, which is the same `end` an `if` around it would want.
///
/// A capture runs to the end of its line, so it can only ever be the last
/// argument, and the `)` that closes the call has to be the last character of
/// the line. That leaves one thing it cannot do -- a command containing a `)`
/// -- and that is refused rather than guessed at. `code_of` was the only call
/// allowed to hold one; the rule is the same for every call now, so
/// `lines($ git ls-files)` reads the way it looks.
fn shell_capture(text: &str, offset: usize, no: usize) -> Result<Option<Expr>, ParseError> {
	if let Some(cmd) = text.strip_prefix("$ ") {
		let cmd = cmd.trim();
		return Ok(Some(capture_of(cmd, offset_in(text, offset, cmd), no)?));
	}
	let Some(open) = text.find('(') else {
		return Ok(None);
	};
	let name = &text[..open];
	if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
		return Ok(None);
	}
	let Some(inner) = text[open + 1..].strip_suffix(')') else {
		// Only worth a message when a `$` is plainly what was meant.
		return if text[open..].contains("$ ") {
			err(
				no,
				format!(
					"`{name}(` is never closed: a `$` run has to be the last argument, and the `)` the last character of the line"
				),
			)
		} else {
			Ok(None)
		};
	};
	let Some(parts) = split_args(inner) else {
		return Ok(None);
	};
	let Some(last) = parts.last() else {
		return Ok(None);
	};
	// `code_of(run test)`: the same dispatch the statement spells, scored.
	// Only `code_of` may hold one. A dispatched target writes to the terminal
	// like any other, so its status is the only value there is to take and
	// `lines(run x)` would have nothing to read. Refused here rather than at
	// evaluation, so an editor underlines it while it is being written.
	if let Some(tail) = dispatch_arg(&inner[last.clone()])
		// A bare `run` is a dispatch missing its target only where a dispatch
		// could go. `length(run)` is a binding called `run` -- oddly named,
		// but legal -- and must not be read as a keyword.
		&& !(tail.is_empty() && name != "code_of")
	{
		if name != "code_of" {
			return err(
				no,
				format!(
					"`{name}(` cannot take a `run` dispatch: a target reports a status, not \
					 output, so `code_of(run …)` is the one call that takes one"
				),
			);
		}
		if parts.len() != 1 {
			return err(no, "`code_of(run …)` takes the dispatch on its own");
		}
		let at = offset + open + 1 + last.start;
		let (target, args) = dispatch_words(split_words(tail), offset_in(text, offset, tail), no)?;
		return Ok(Some(Expr::Call {
			name: name.into(),
			args: vec![Expr::Dispatch {
				target,
				args,
				span: Span::new(at, at + inner[last.clone()].len(), no),
			}],
			span: Span::new(offset, offset + text.len(), no),
		}));
	}
	let Some(cmd) = inner[last.clone()].trim().strip_prefix("$ ") else {
		// A `$` anywhere but last is the mistake worth naming: the capture
		// would swallow the arguments after it, so there is no reading of it
		// that works.
		return if parts.iter().any(|r| inner[r.clone()].trim().starts_with("$ ")) {
			err(
				no,
				format!(
					"a `$` run has to be the last argument of `{name}(`, because it runs to the \
					 end of the line; bind it first and pass the binding"
				),
			)
		} else {
			Ok(None)
		};
	};
	if cmd.contains(')') {
		return err(
			no,
			format!(
				"a `)` inside this `$` run cannot be told from the one that closes `{name}(`; \
				 put the command on a line of its own and pass the binding"
			),
		);
	}
	let mut args = Vec::with_capacity(parts.len());
	for r in &parts[..parts.len() - 1] {
		let arg = inner[r.clone()].trim();
		args.push(parse_expr(arg, offset_in(text, offset, arg), no)?);
	}
	let cmd = cmd.trim();
	args.push(capture_of(cmd, offset_in(text, offset, cmd), no)?);
	Ok(Some(Expr::Call {
		name: name.into(),
		args,
		span: Span::new(offset, offset + text.len(), no),
	}))
}

/// The `run …` tail of a dispatch argument, if that is what this text is.
///
/// Shared with the formatter, which spaces a dispatch's words the way it
/// spaces a `run` statement's, because they are the same words.
pub(crate) fn dispatch_arg(text: &str) -> Option<&str> {
	let rest = text.trim().strip_prefix("run")?;
	// A bare `run` is still one: `code_of(run)` is a dispatch missing its
	// target, and saying so beats reporting a stray token. `running` is not.
	if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
		return None;
	}
	Some(rest.trim())
}

/// A `run` target and its arguments, from the words after the keyword -- each
/// with where it starts in the text they were split from, which starts at
/// `offset`.
fn dispatch_words(words: Vec<(usize, String)>, offset: usize, no: usize) -> Result<Dispatched, ParseError> {
	let mut words = words.into_iter().map(|(at, word)| {
		lexer::split_interp(&word, offset + at, no)
			.map_err(ParseError::from)
			.and_then(to_parts)
	});
	let Some(target) = words.next() else {
		return err(no, "`run` needs a target");
	};
	Ok((target?, words.collect::<Result<_, _>>()?))
}

type Dispatched = (Vec<InterpPart>, Vec<Vec<InterpPart>>);

/// Whether this text is a call whose last argument is a `$` run or a `run`
/// dispatch.
///
/// The formatter asks: both run to the end of the line and are rendered from
/// their own text, so the call around them is not re-spaced as an expression.
pub(crate) fn is_capture_call(text: &str) -> bool {
	let Some(open) = text.find('(') else {
		return false;
	};
	let name = &text[..open];
	if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
		return false;
	}
	let Some(inner) = text[open + 1..].strip_suffix(')') else {
		return false;
	};
	split_args(inner)
		.and_then(|parts| {
			parts
				.last()
				.map(|r| inner[r.clone()].trim().starts_with("$ ") || dispatch_arg(&inner[r.clone()]).is_some())
		})
		.unwrap_or(false)
}

/// The top-level argument ranges of `inner`, or `None` when it does not look
/// like an argument list at all.
///
/// Scanned rather than lexed: everything from the `$` on is a shell line, and
/// no expression lexer can read one. Strings are skipped so a comma or a `$`
/// inside one is left where it is.
fn split_args(inner: &str) -> Option<Vec<std::ops::Range<usize>>> {
	let b = inner.as_bytes();
	let (mut out, mut start, mut depth, mut i) = (Vec::new(), 0usize, 0i32, 0usize);
	while i < b.len() {
		match b[i] {
			// `r"…"` keeps its backslashes; an unescaped `"` still ends it.
			b'"' => {
				let raw = i > 0 && b[i - 1] == b'r';
				i += 1;
				while i < b.len() && b[i] != b'"' {
					i += if !raw && b[i] == b'\\' { 2 } else { 1 };
				}
				i += 1;
			}
			b'(' | b'[' => {
				depth += 1;
				i += 1;
			}
			b')' | b']' => {
				depth -= 1;
				if depth < 0 {
					return None;
				}
				i += 1;
			}
			b',' if depth == 0 => {
				out.push(start..i);
				i += 1;
				start = i;
			}
			_ => i += 1,
		}
	}
	if depth != 0 {
		return None;
	}
	out.push(start..inner.len());
	Some(out)
}

fn capture_of(cmd: &str, offset: usize, no: usize) -> Result<Expr, ParseError> {
	Ok(Expr::Capture {
		command: None,
		body: vec![to_parts(lexer::split_interp(cmd, offset, no)?)?],
		span: Span::new(offset, offset + cmd.len(), no),
	})
}

/// The label of a `case`, which must be written as a quoted string.
///
/// A subject is a value and a label is compared against it, so `case linux`
/// asks about a string while looking like a bare word -- and `RUN.os` is a
/// string like any other. Requiring the quotes keeps one rule: a string is
/// always in quotes, wherever it appears.
fn case_label(trimmed: &str, no: usize) -> Result<String, ParseError> {
	let rest = lexer::code(trimmed[4..].trim());
	let inner = rest
		.strip_prefix('"')
		.and_then(|r| r.strip_suffix('"'))
		.filter(|_| rest.len() >= 2);
	match inner {
		Some(label) => Ok(label.to_string()),
		None if rest.is_empty() => err(no, "`case` needs a label, as `case \"linux\"`"),
		None => err(
			no,
			format!("a `case` label is a string and must be quoted: write `case \"{rest}\"`"),
		),
	}
}

/// Split on whitespace, keeping `{{ … }}` blocks intact, with where each word
/// starts.
///
/// A word is sliced out of the text rather than rebuilt a byte at a time, which
/// read every byte as a character of its own: an `ñ` arrived as `Ã±`.
fn split_words(s: &str) -> Vec<(usize, String)> {
	let b = s.as_bytes();
	let (mut out, mut start, mut i) = (Vec::new(), None, 0usize);
	while i < b.len() {
		if b[i].is_ascii_whitespace() {
			if let Some(from) = start.take() {
				out.push((from, s[from..i].to_string()));
			}
			i += 1;
			continue;
		}
		start.get_or_insert(i);
		match b[i..].starts_with(b"{{").then(|| lexer::skip_interp(b, i, 0)) {
			Some(Ok(end)) => i = end,
			_ => i += 1,
		}
	}
	if let Some(from) = start {
		out.push((from, s[from..].to_string()));
	}
	out
}

/// Adjacent literal runs as one, so a text read a line at a time comes out as
/// the same parts it would have been read whole.
fn joined(parts: Vec<InterpPart>) -> Vec<InterpPart> {
	let mut out: Vec<InterpPart> = Vec::with_capacity(parts.len());
	for p in parts {
		if let (InterpPart::Literal(t), Some(InterpPart::Literal(prev))) = (&p, out.last_mut()) {
			prev.push_str(t);
			continue;
		}
		out.push(p);
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
	is_name_list(text[..eq].trim()).then_some(eq)
}

fn to_parts(raw: Vec<RawPart>) -> Result<Vec<InterpPart>, ParseError> {
	raw.into_iter()
		.map(|p| match p {
			RawPart::Literal(t) => Ok(InterpPart::Literal(t)),
			RawPart::Expr { text, span } => Ok(InterpPart::Expr(parse_expr(&text, span.start, span.line)?)),
		})
		.collect()
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
				Ok(Expr::Str(to_parts(parts)?, span))
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
