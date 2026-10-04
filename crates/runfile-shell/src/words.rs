//! Words: quotes, escapes, expansions, redirections and heredocs -- the part of
//! reading shell where a character means something different depending on what
//! came before it.

use crate::syntax::{End, Mode, Nest, Opener, P, Part, Pending, Quote, R, Redirect, Stop, Word, is_meta};
use runfile_lang::{Delimiter, Quoting};

/// A quote, or the start of an expansion, with nothing to close it.
fn never_closed(i: usize, len: usize, message: &str) -> Stop {
	Stop::Refused {
		rule: "unclosed",
		from: i,
		to: i + len,
		message: message.into(),
	}
}

/// Whether a `(` here opens an extended glob -- `@(a|b)`, `!(x)` -- which
/// parses only once `shopt -s extglob` has run, and so is taken on trust.
fn extglob(parts: &[Part]) -> bool {
	matches!(
		parts.last(),
		Some(Part::Char {
			c: '@' | '!' | '+' | '*' | '?',
			quote: Quote::Bare,
			escaped: false,
			..
		})
	)
}

/// Whether the word so far is `name=` or `name+=`, so a `(` opens an array.
fn array_start(parts: &[Part]) -> bool {
	let text: Option<String> = parts
		.iter()
		.map(|p| match p {
			Part::Char {
				c,
				quote: Quote::Bare,
				escaped: false,
				..
			} => Some(*c),
			_ => None,
		})
		.collect();
	let Some(text) = text else {
		return false;
	};
	let Some(name) = text.strip_suffix("+=").or_else(|| text.strip_suffix('=')) else {
		return false;
	};
	let mut chars = name.chars();
	chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
		&& chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl P<'_> {
	pub(crate) fn word(&mut self, mode: Mode) -> R<Word> {
		let start = self.i;
		let mut parts = Vec::new();
		while let Some(ch) = self.s.get(self.i).copied() {
			let i = self.i;
			if let Some(hole) = ch.hole {
				parts.push(Part::Hole {
					hole,
					quote: Quote::Bare,
				});
				self.mark(hole);
				self.i += 1;
				continue;
			}
			match ch.c {
				' ' | '\t' | '\n' | ';' => break,
				'<' | '>' if mode == Mode::Normal && i == start && self.peek_at(1) == Some('(') => {
					let what = if ch.c == '<' { "<(" } else { ">(" };
					self.i += 2;
					let depth = self.enter(Nest::Fresh);
					let body = self.list(End::Paren, Some(Opener { i, what }))?;
					self.leave(depth);
					self.i += 1;
					parts.push(Part::Subst { body: Some(body) });
				}
				'(' if mode == Mode::Normal && extglob(&parts) => self.glob_group(&mut parts)?,
				'(' if mode == Mode::Normal && array_start(&parts) => self.array(&mut parts)?,
				c if mode == Mode::Normal && is_meta(c) => break,
				'\\' => {
					if self.at("\\\n") {
						self.i += 2;
					} else if let Some(c) = self.peek_at(1) {
						parts.push(Part::Char {
							c,
							i: i + 1,
							quote: Quote::Bare,
							escaped: true,
						});
						self.i += 2;
					} else {
						parts.push(Part::Char {
							c: '\\',
							i,
							quote: Quote::Bare,
							escaped: false,
						});
						self.i += 1;
					}
				}
				'\'' => self.single(&mut parts, Quote::Single)?,
				'"' => self.double(&mut parts, Quote::Double)?,
				'$' => self.dollar(&mut parts, Quote::Bare)?,
				'`' => self.backtick(&mut parts)?,
				c => {
					parts.push(Part::Char {
						c,
						i,
						quote: Quote::Bare,
						escaped: false,
					});
					self.i += 1;
				}
			}
		}
		Ok(Word {
			parts,
			start,
			end: self.i,
		})
	}

	/// `'…'`, with the cursor at its opening quote.
	fn single(&mut self, parts: &mut Vec<Part>, quote: Quote) -> R<()> {
		let open = self.i;
		self.i += 1;
		let depth = self.enter(Nest::Single);
		loop {
			let Some(ch) = self.s.get(self.i).copied() else {
				return Err(never_closed(
					open,
					1,
					"this `'` is never closed -- an apostrophe in a word has to be quoted, as in `\"it's\"`",
				));
			};
			let i = self.i;
			self.i += 1;
			match ch.hole {
				Some(hole) => {
					parts.push(Part::Hole { hole, quote });
					self.mark(hole);
				}
				None if ch.c == '\'' => {
					self.leave(depth);
					return Ok(());
				}
				None => parts.push(Part::Char {
					c: ch.c,
					i,
					quote,
					escaped: false,
				}),
			}
		}
	}

	/// `"…"`, with the cursor at its opening quote.
	fn double(&mut self, parts: &mut Vec<Part>, quote: Quote) -> R<()> {
		let open = self.i;
		self.i += 1;
		let depth = self.enter(Nest::Double);
		loop {
			let Some(ch) = self.s.get(self.i).copied() else {
				return Err(never_closed(open, 1, "this `\"` is never closed"));
			};
			let i = self.i;
			if let Some(hole) = ch.hole {
				parts.push(Part::Hole { hole, quote });
				self.mark(hole);
				self.i += 1;
				continue;
			}
			match ch.c {
				'"' => {
					self.i += 1;
					self.leave(depth);
					return Ok(());
				}
				'\\' => match self.peek_at(1) {
					Some('\n') => self.i += 2,
					Some(c @ ('"' | '\\' | '$' | '`')) => {
						parts.push(Part::Char {
							c,
							i: i + 1,
							quote,
							escaped: true,
						});
						self.i += 2;
					}
					_ => {
						parts.push(Part::Char {
							c: '\\',
							i,
							quote,
							escaped: false,
						});
						self.i += 1;
					}
				},
				'$' => self.dollar(parts, quote)?,
				'`' => self.backtick(parts)?,
				c => {
					parts.push(Part::Char {
						c,
						i,
						quote,
						escaped: false,
					});
					self.i += 1;
				}
			}
		}
	}

	/// `$'…'`, with the cursor at its `$`.
	fn ansi(&mut self, parts: &mut Vec<Part>) -> R<()> {
		let open = self.i;
		self.i += 2;
		let depth = self.enter(Nest::Ansi);
		loop {
			let Some(ch) = self.s.get(self.i).copied() else {
				return Err(never_closed(open, 2, "this `$'` is never closed"));
			};
			let i = self.i;
			if let Some(hole) = ch.hole {
				parts.push(Part::Hole {
					hole,
					quote: Quote::Ansi,
				});
				self.mark(hole);
				self.i += 1;
				continue;
			}
			match ch.c {
				'\'' => {
					self.i += 1;
					self.leave(depth);
					return Ok(());
				}
				'\\' => {
					if let Some(next) = self.s.get(i + 1) {
						parts.push(Part::Char {
							c: next.c,
							i: i + 1,
							quote: Quote::Ansi,
							escaped: true,
						});
					}
					self.i += 2;
				}
				c => {
					parts.push(Part::Char {
						c,
						i,
						quote: Quote::Ansi,
						escaped: false,
					});
					self.i += 1;
				}
			}
		}
	}

	/// Whatever a `$` starts, with the cursor on it.
	pub(crate) fn dollar(&mut self, parts: &mut Vec<Part>, quote: Quote) -> R<()> {
		let i = self.i;
		match self.peek_at(1) {
			Some('\'') if quote == Quote::Bare => self.ansi(parts)?,
			Some('"') if quote == Quote::Bare => {
				self.i += 1;
				self.double(parts, Quote::Double)?;
			}
			Some('(') if self.peek_at(2) == Some('(') => {
				self.i += 3;
				let (mark, pending) = (parts.len(), self.heredocs.clone());
				if let Ok(true) = self.arithmetic(parts) {
					parts.push(Part::Arith);
				} else {
					parts.truncate(mark);
					self.heredocs = pending;
					self.i = i + 1;
					self.substitution(parts, i)?;
				}
			}
			Some('(') => {
				self.i += 1;
				self.substitution(parts, i)?;
			}
			Some('{') => {
				self.i += 2;
				self.parameter(parts, i)?;
			}
			Some('[') => {
				self.i += 2;
				self.brackets()?;
				parts.push(Part::Arith);
			}
			Some(c) if c.is_ascii_alphabetic() || c == '_' => {
				self.i += 1;
				let mut name = String::new();
				while let Some(c) = self.peek_at(0).filter(|c| c.is_ascii_alphanumeric() || *c == '_') {
					name.push(c);
					self.i += 1;
				}
				parts.push(Part::Param { name, i, end: self.i });
			}
			Some(c) if c.is_ascii_digit() || matches!(c, '@' | '*' | '#' | '?' | '$' | '!' | '-') => {
				self.i += 2;
				parts.push(Part::Param {
					name: c.to_string(),
					i,
					end: self.i,
				});
			}
			_ => {
				parts.push(Part::Char {
					c: '$',
					i,
					quote,
					escaped: false,
				});
				self.i += 1;
			}
		}
		Ok(())
	}

	/// `$(…)`, with the cursor at its `(`.
	fn substitution(&mut self, parts: &mut Vec<Part>, i: usize) -> R<()> {
		self.i += 1;
		let depth = self.enter(Nest::Fresh);
		let body = self.list(End::Paren, Some(Opener { i, what: "$(" }))?;
		self.leave(depth);
		self.i += 1;
		parts.push(Part::Subst { body: Some(body) });
		Ok(())
	}

	/// Past a `((`, to the parentheses that close it. Bash reads arithmetic when
	/// the last two close together, and a subshell inside a subshell -- or a
	/// command substitution -- when they do not: `Ok(false)`.
	pub(crate) fn arithmetic(&mut self, parts: &mut Vec<Part>) -> R<bool> {
		// Both readings of a `((` are tried, so the level is left however this
		// one ends -- an error included, which a caller takes as "not arithmetic".
		let level = self.enter(Nest::Arith);
		let read = self.arithmetic_body(parts);
		self.leave(level);
		read
	}

	fn arithmetic_body(&mut self, parts: &mut Vec<Part>) -> R<bool> {
		let mut depth = 2usize;
		loop {
			let Some(ch) = self.s.get(self.i).copied() else {
				return Ok(false);
			};
			if let Some(hole) = ch.hole {
				self.mark(hole);
				self.i += 1;
				continue;
			}
			match ch.c {
				'(' => {
					depth += 1;
					self.i += 1;
				}
				')' => {
					depth -= 1;
					self.i += 1;
					if depth == 0 {
						return Ok(self.at_index(self.i - 2, ")"));
					}
				}
				'\'' => self.single(parts, Quote::Opaque)?,
				'"' => self.double(parts, Quote::Opaque)?,
				'$' => self.dollar(parts, Quote::Opaque)?,
				'`' => self.backtick(parts)?,
				'\\' => self.i += 2,
				_ => self.i += 1,
			}
		}
	}

	/// `${…}`, with the cursor past its `{`. The first `}` outside quotes closes
	/// it: bash counts no braces inside, so `${x:-{a}` is `{a`.
	fn parameter(&mut self, parts: &mut Vec<Part>, i: usize) -> R<()> {
		let level = self.enter(Nest::Param);
		let mut name = String::new();
		// `${#name}` is a length, and reads the name all the same.
		if self.at("#") && self.peek_at(1).is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
			self.i += 1;
		}
		match self.peek_at(0) {
			Some(c) if c.is_ascii_alphabetic() || c == '_' => {
				while let Some(c) = self.peek_at(0).filter(|c| c.is_ascii_alphanumeric() || *c == '_') {
					name.push(c);
					self.i += 1;
				}
			}
			Some(c) if c.is_ascii_digit() => {
				while let Some(c) = self.peek_at(0).filter(char::is_ascii_digit) {
					name.push(c);
					self.i += 1;
				}
			}
			Some(c @ ('@' | '*' | '#' | '?' | '$' | '!' | '-')) => {
				name.push(c);
				self.i += 1;
			}
			_ => {}
		}
		loop {
			let Some(ch) = self.s.get(self.i).copied() else {
				return Err(never_closed(i, 2, "`${` is never closed by `}`"));
			};
			if let Some(hole) = ch.hole {
				parts.push(Part::Hole {
					hole,
					quote: Quote::Opaque,
				});
				self.mark(hole);
				self.i += 1;
				continue;
			}
			match ch.c {
				'}' => {
					self.i += 1;
					self.leave(level);
					break;
				}
				'\\' => self.i += 2,
				'\'' => self.single(parts, Quote::Opaque)?,
				'"' => self.double(parts, Quote::Opaque)?,
				'$' => self.dollar(parts, Quote::Opaque)?,
				'`' => self.backtick(parts)?,
				_ => self.i += 1,
			}
		}
		if !name.is_empty() {
			parts.push(Part::Param { name, i, end: self.i });
		}
		Ok(())
	}

	/// `` `…` ``, with the cursor at its opening backtick. What it runs is not
	/// read: inside, a backslash means something different again.
	///
	/// Where an interpolation inside sits is read all the same, from the text
	/// bash reads again once it has taken its own backslashes off: `\\`, `` \` ``
	/// and `\$`, and `\"` too inside double quotes.
	fn backtick(&mut self, parts: &mut Vec<Part>) -> R<()> {
		let i = self.i;
		self.i += 1;
		let in_double = self.quoting_here() == Quoting::Double;
		let mut inner = Vec::new();
		loop {
			let Some(ch) = self.s.get(self.i).copied() else {
				return Err(never_closed(i, 1, "this backtick is never closed"));
			};
			if ch.hole.is_some() {
				inner.push(ch);
				self.i += 1;
				continue;
			}
			match ch.c {
				'`' => {
					self.i += 1;
					break;
				}
				'\\' => {
					match self.s.get(self.i + 1).copied() {
						Some(next)
							if next.hole.is_none()
								&& (matches!(next.c, '\\' | '`' | '$') || in_double && next.c == '"') =>
						{
							inner.push(next)
						}
						Some(next) => inner.extend([ch, next]),
						None => inner.push(ch),
					}
					self.i += 2;
				}
				_ => {
					inner.push(ch);
					self.i += 1;
				}
			}
		}
		let mut nest = self.nest.clone();
		nest.extend([Nest::Backtick(in_double), Nest::Fresh]);
		self.sandbox(&inner, nest, |p| p.list(End::Eof, None).map(drop));
		parts.push(Part::Subst { body: None });
		Ok(())
	}

	/// `$[…]`, with the cursor past its `[`.
	fn brackets(&mut self) -> R<()> {
		let level = self.enter(Nest::Arith);
		let mut depth = 1usize;
		loop {
			let Some(ch) = self.s.get(self.i).copied() else {
				return Err(Stop::Lost);
			};
			self.i += 1;
			if let Some(hole) = ch.hole {
				self.mark(hole);
				continue;
			}
			match ch.c {
				'[' => depth += 1,
				']' => {
					depth -= 1;
					if depth == 0 {
						self.leave(level);
						return Ok(());
					}
				}
				_ => {}
			}
		}
	}

	/// An extended glob's `(…)`, with the cursor at the `(`.
	fn glob_group(&mut self, parts: &mut Vec<Part>) -> R<()> {
		let mut depth = 0usize;
		loop {
			let Some(ch) = self.s.get(self.i).copied() else {
				return Err(Stop::Lost);
			};
			let i = self.i;
			if let Some(hole) = ch.hole {
				parts.push(Part::Hole {
					hole,
					quote: Quote::Bare,
				});
				self.mark(hole);
				self.i += 1;
				continue;
			}
			match ch.c {
				'(' => {
					depth += 1;
					self.i += 1;
				}
				')' => {
					self.i += 1;
					depth -= 1;
					if depth == 0 {
						return Ok(());
					}
				}
				' ' | '\t' | '\n' | ';' | '&' | '<' | '>' | '\\' => return Err(Stop::Lost),
				'\'' => self.single(parts, Quote::Single)?,
				'"' => self.double(parts, Quote::Double)?,
				'$' => self.dollar(parts, Quote::Bare)?,
				'`' => self.backtick(parts)?,
				c => {
					parts.push(Part::Char {
						c,
						i,
						quote: Quote::Bare,
						escaped: false,
					});
					self.i += 1;
				}
			}
		}
	}

	/// `name=(…)`, with the cursor at the `(`. Its words are kept as parts of
	/// the assignment's.
	fn array(&mut self, parts: &mut Vec<Part>) -> R<()> {
		let open = self.i;
		self.i += 1;
		loop {
			self.linebreak()?;
			match self.peek() {
				None => return Err(never_closed(open, 1, "`(` is never closed by `)`")),
				Some(')') => {
					self.i += 1;
					return Ok(());
				}
				Some(c) if is_meta(c) => return Err(Stop::Lost),
				_ => {
					let w = self.word(Mode::Normal)?;
					if w.start == w.end {
						return Err(Stop::Lost);
					}
					parts.extend(w.parts);
				}
			}
		}
	}

	/// The redirection at the cursor, if one is there: its length -- with any
	/// file descriptor in front -- and its operator.
	pub(crate) fn redirect_op(&self) -> Option<(usize, &'static str)> {
		let mut j = self.i;
		while self.s.get(j).is_some_and(|c| c.hole.is_none() && c.c.is_ascii_digit()) {
			j += 1;
		}
		if j == self.i && self.at("{") {
			// `{name}>`: a descriptor bash picks, and names.
			let mut k = j + 1;
			while self
				.s
				.get(k)
				.is_some_and(|c| c.hole.is_none() && (c.c.is_ascii_alphanumeric() || c.c == '_'))
			{
				k += 1;
			}
			if k > j + 1 && self.at_index(k, "}") && (self.at_index(k + 1, "<") || self.at_index(k + 1, ">")) {
				j = k + 1;
			}
		}
		const OPS: [&str; 12] = ["<<<", "<<-", "<<", "<>", "<&", "<", ">>", ">|", ">&", ">", "&>>", "&>"];
		let op = OPS.into_iter().find(|op| self.at_index(j, op))?;
		if j > self.i && op.starts_with('&') {
			return None;
		}
		if (op == "<" || op == ">") && self.at_index(j + 1, "(") {
			return None;
		}
		Some((j - self.i + op.len(), op))
	}

	pub(crate) fn redirect(&mut self, len: usize, op: &'static str) -> R<Redirect> {
		let i = self.i;
		self.i += len;
		self.blank();
		let missing = match self.peek() {
			None => true,
			_ if self.is_hole(self.i) => false,
			Some('<' | '>') if self.peek_at(1) == Some('(') => false,
			Some(c) => is_meta(c) || c == '#',
		};
		if missing {
			let text: String = self.s[i..i + len].iter().map(|c| c.c).collect();
			return Err(Stop::Refused {
				rule: "unexpected",
				from: i,
				to: i + len,
				message: format!("`{text}` has nothing after it to redirect to"),
			});
		}
		let target = self.word(Mode::Normal)?;
		if target.start == target.end {
			return Err(Stop::Lost);
		}
		if op == "<<" || op == "<<-" {
			let Some(delimiter) = target.literal() else {
				return Err(Stop::Lost);
			};
			self.heredocs.push(Pending {
				delimiter,
				strip_tabs: op == "<<-",
				// Any quoting at all in the delimiter makes the body literal.
				expands: target.plain().is_some(),
				i,
				len,
			});
		}
		Ok(Redirect { op, target, i })
	}

	/// The bodies of the heredocs the line just ended opened, in order.
	pub(crate) fn read_heredocs(&mut self) -> R<()> {
		for h in std::mem::take(&mut self.heredocs) {
			let body = self.i;
			loop {
				if self.eof() {
					return Err(self.unterminated(&h));
				}
				let start = self.i;
				while self.s.get(self.i).is_some_and(|c| c.hole.is_some() || c.c != '\n') {
					self.i += 1;
				}
				let text: Option<String> = self.s[start..self.i]
					.iter()
					.map(|c| c.hole.is_none().then_some(c.c))
					.collect();
				if self.i < self.s.len() {
					self.i += 1;
				}
				let text = text.map(|t| match h.strip_tabs {
					true => t.trim_start_matches('\t').to_string(),
					false => t,
				});
				if text.as_deref() == Some(h.delimiter.as_str()) {
					self.heredoc_body(&h, body, start);
					break;
				}
			}
		}
		Ok(())
	}

	/// Note where the interpolations in a heredoc's body, characters
	/// `from..to`, sit.
	fn heredoc_body(&mut self, h: &Pending, from: usize, to: usize) {
		if self.spots.is_empty() {
			return;
		}
		let mut nest = self.nest.clone();
		nest.push(Nest::Heredoc(
			Delimiter {
				text: h.delimiter.clone(),
				strip_tabs: h.strip_tabs,
			},
			h.expands,
		));
		let body = self.s[from..to].to_vec();
		if h.expands {
			self.sandbox(&body, nest, |p| p.heredoc_text());
		} else {
			let outer = std::mem::replace(&mut self.nest, nest);
			for hole in body.iter().filter_map(|c| c.hole) {
				self.mark(hole);
			}
			self.nest = outer;
		}
	}

	/// An expanding heredoc's body, read the way bash expands it: `$`,
	/// backticks and `\` mean what they do inside double quotes, and a `"` is a
	/// character like any other.
	pub(crate) fn heredoc_text(&mut self) -> R<()> {
		let mut parts = Vec::new();
		while let Some(ch) = self.s.get(self.i).copied() {
			if let Some(hole) = ch.hole {
				self.mark(hole);
				self.i += 1;
				continue;
			}
			match ch.c {
				'\\' if self.s.get(self.i + 1).is_some_and(|n| n.hole.is_none()) => self.i += 2,
				'$' => self.dollar(&mut parts, Quote::Opaque)?,
				'`' => self.backtick(&mut parts)?,
				_ => self.i += 1,
			}
		}
		Ok(())
	}
}
