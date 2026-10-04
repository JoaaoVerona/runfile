//! Shell text read closely enough to find what bash refuses, and nothing it
//! accepts.
//!
//! Not a shell parser in general. It follows bash's grammar as far as the rules
//! need -- where a command starts, what is quoted, which keywords open and close
//! what -- and a construct it does not follow makes it give up on the script
//! ([`Stop::Lost`]) rather than guess. Giving up costs one script its checks;
//! guessing would cost a false report, and a report stops a run.
//!
//! Every syntax error reported here is one bash reports too, which the tests
//! hold it to by asking `bash -n` about each.
//!
//! The words, quotes and expansions are read in [`crate::words`]; this module
//! reads the commands they make up.

use crate::script::Ch;
use runfile_lang::{Delimiter, Quoting, Spot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quote {
	/// Outside quotes.
	Bare,
	/// Inside `'…'`.
	Single,
	/// Inside `"…"`.
	Double,
	/// Inside `$'…'`.
	Ansi,
	/// Somewhere the checker does not judge: inside backticks, the operand of a
	/// `${…}`, arithmetic, a heredoc.
	Opaque,
}

#[derive(Debug, Clone)]
pub enum Part {
	/// A character as written, and how it is quoted. `escaped` when a backslash
	/// stood in front of it.
	Char {
		c: char,
		i: usize,
		quote: Quote,
		escaped: bool,
	},
	/// An interpolation, and how it is quoted.
	Hole { hole: usize, quote: Quote },
	/// `$name`, `$1`, `${name…}`: the parameter a reference reads, and the
	/// characters the reference covers.
	Param { name: String, i: usize, end: usize },
	/// `$(…)` and `<(…)` with what they run; `` `…` `` without.
	Subst { body: Option<List> },
	/// `$((…))`, `$[…]`.
	Arith,
}

/// One word, from the character it starts at to the one past its end.
#[derive(Debug, Clone, Default)]
pub struct Word {
	pub parts: Vec<Part>,
	pub start: usize,
	pub end: usize,
}

impl Word {
	/// The word's text, when every character of it is written plainly: nothing
	/// quoted, escaped, expanded or interpolated.
	pub fn plain(&self) -> Option<String> {
		self.parts
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
			.collect()
	}

	/// The word's text once its quotes are removed, when nothing in it expands.
	pub fn literal(&self) -> Option<String> {
		self.parts
			.iter()
			.map(|p| match p {
				Part::Char { c, .. } => Some(*c),
				_ => None,
			})
			.collect()
	}
}

#[derive(Debug, Clone)]
pub struct Redirect {
	pub op: &'static str,
	pub target: Word,
	/// Where the operator is.
	pub i: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Simple {
	/// `NAME=value` in front of the command.
	pub assigns: Vec<Word>,
	pub words: Vec<Word>,
	pub redirects: Vec<Redirect>,
}

#[derive(Debug, Clone)]
pub enum Command {
	Simple(Simple),
	Compound(Compound, Vec<Redirect>),
	/// `name() …` or `function name …`: its name, and its body.
	Function(String, Box<Command>),
}

#[derive(Debug, Clone)]
pub enum Compound {
	Group(List),
	Subshell(List),
	/// Each condition with its branch, and the `else`.
	If(Vec<(List, List)>, Option<List>),
	/// `while` or `until`: the condition, and the body.
	Loop(List, List),
	/// `for` or `select`: the name it sets -- which a `for ((…))` has none of --
	/// the words, and the body.
	For(Option<Word>, Vec<Word>, List),
	/// The subject, and each branch's patterns with its body.
	Case(Word, Vec<(Vec<Word>, List)>),
	/// `(( … ))`, with what is expanded inside.
	Arith(Vec<Part>),
	/// `[[ … ]]`.
	Test(Vec<Word>),
}

#[derive(Debug, Clone, Default)]
pub struct Pipeline {
	pub commands: Vec<Command>,
}

/// Pipelines joined by `&&` and `||`, and whether `&` ends them.
#[derive(Debug, Clone)]
pub struct Item {
	pub pipelines: Vec<Pipeline>,
	pub background: bool,
}

pub type List = Vec<Item>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
	/// Bash refuses the script here: the rule, the characters, and why.
	Refused {
		rule: &'static str,
		from: usize,
		to: usize,
		message: String,
	},
	/// Something this reading does not follow. The script is left alone.
	Lost,
}

pub type R<T> = Result<T, Stop>;

/// Read a whole script, in bash when `bash` says so, and otherwise in a shell
/// this checker reads whose `[[ … ]]` may not be bash's.
pub fn parse(chars: &[Ch], bash: bool) -> R<List> {
	let mut p = P::new(chars, bash, Vec::new());
	let list = p.list(End::Eof, None)?;
	match p.heredocs.first() {
		Some(h) => Err(p.unterminated(h)),
		None => Ok(list),
	}
}

/// Where each of a script's `holes` interpolations sits, read the way bash
/// reads it -- `None` when this reading does not follow the script. An entry
/// is `None` for an interpolation it passed without learning where it was.
pub fn spots(chars: &[Ch], holes: usize, bash: bool) -> Option<Vec<Option<Spot>>> {
	let mut p = P::new(chars, bash, vec![None; holes]);
	p.list(End::Eof, None).ok()?;
	p.heredocs.is_empty().then_some(p.spots)
}

/// A heredoc whose body starts at the next newline.
#[derive(Clone)]
pub(crate) struct Pending {
	pub delimiter: String,
	pub strip_tabs: bool,
	/// Whether the body expands: its delimiter was written without quotes.
	pub expands: bool,
	/// Where its operator is, and how long.
	pub i: usize,
	pub len: usize,
}

pub(crate) struct P<'a> {
	pub s: &'a [Ch],
	pub i: usize,
	pub heredocs: Vec<Pending>,
	pub depth: usize,
	/// Whether the shell is bash itself.
	pub bash: bool,
	/// The quoting the reader is inside, innermost last: what each
	/// interpolation it passes is recorded under.
	pub nest: Vec<Nest>,
	/// Where each interpolation sits, by its index, once it has been passed.
	/// Empty when nobody asked, which is the checker's own reading: then
	/// nothing is recorded, and nothing is read a second time to record it.
	pub spots: Vec<Option<Spot>>,
}

/// One level of what an interpolation can sit inside.
#[derive(Debug, Clone)]
pub(crate) enum Nest {
	/// `$( … )`, `<( … )`, a backtick's body: read afresh, so quoting starts over.
	Fresh,
	Single,
	Double,
	Ansi,
	/// A `${…}`'s operand, which is quoted the way the expansion is.
	Param,
	/// `$(( … ))`, `(( … ))`, `$[ … ]`.
	Arith,
	/// A heredoc's body, and whether it expands.
	Heredoc(Delimiter, bool),
	Comment,
	/// `` `…` ``, and whether it is inside double quotes.
	Backtick(bool),
}

/// The quoting a stack of [`Nest`] comes to, for a value written at its top.
fn quoting_of(nest: &[Nest]) -> Quoting {
	// A backtick's body is read again, after bash has taken one level of
	// backslashes off it -- so whatever the value is written as in there, it is
	// escaped once more for the backticks themselves.
	if let Some(k) = nest.iter().position(|n| matches!(n, Nest::Backtick(_))) {
		let Nest::Backtick(in_double) = nest[k] else {
			unreachable!()
		};
		return Quoting::Backtick(Box::new(quoting_of(&nest[k + 1..])), in_double);
	}
	let Some((k, n)) = nest.iter().enumerate().rev().find(|(_, n)| !matches!(n, Nest::Param)) else {
		return Quoting::Bare;
	};
	// Whether a `${…}` stands between that level and the value, which is where
	// bash reads a `'` as a quote however the expansion treats it.
	let braced = k + 1 < nest.len();
	match n {
		Nest::Fresh => Quoting::Bare,
		Nest::Double | Nest::Arith => Quoting::Double,
		Nest::Heredoc(_, true) if braced => Quoting::Double,
		Nest::Heredoc(_, true) => Quoting::Heredoc,
		Nest::Heredoc(_, false) => Quoting::Literal,
		Nest::Ansi => Quoting::Ansi,
		Nest::Comment => Quoting::Comment,
		// A `'` inside a `${…}` that is itself inside double quotes or a
		// heredoc, or in arithmetic, is read as a quote and kept as a character.
		Nest::Single if expands(&nest[..k]) => Quoting::SingleExpanded,
		Nest::Single => Quoting::Single,
		Nest::Param | Nest::Backtick(_) => unreachable!(),
	}
}

/// Whether the nearest level out expands what is inside it, with nothing that
/// starts quoting over coming first.
fn expands(outer: &[Nest]) -> bool {
	matches!(
		outer.iter().rev().find(|n| !matches!(n, Nest::Param)),
		Some(Nest::Double | Nest::Arith | Nest::Heredoc(_, true))
	)
}

impl<'a> P<'a> {
	pub(crate) fn new(s: &'a [Ch], bash: bool, spots: Vec<Option<Spot>>) -> Self {
		P {
			s,
			i: 0,
			heredocs: Vec::new(),
			depth: 0,
			bash,
			nest: Vec::new(),
			spots,
		}
	}

	/// Note where interpolation `hole` sits. A later reading of the same text
	/// wins, which is what makes backtracking -- `$((` that turns out to be a
	/// subshell -- record the reading that stood.
	pub(crate) fn mark(&mut self, hole: usize) {
		if hole < self.spots.len() {
			let heredocs = self
				.nest
				.iter()
				.filter_map(|n| match n {
					Nest::Heredoc(d, _) => Some(d.clone()),
					_ => None,
				})
				.collect();
			self.spots[hole] = Some(Spot {
				quoting: quoting_of(&self.nest),
				heredocs,
			});
		}
	}

	/// Go one level in, answering the depth to come back out to.
	pub(crate) fn enter(&mut self, n: Nest) -> usize {
		self.nest.push(n);
		self.nest.len() - 1
	}

	pub(crate) fn leave(&mut self, depth: usize) {
		self.nest.truncate(depth);
	}

	/// The quoting at the reader's position, for deciding how a construct that
	/// starts here is read.
	pub(crate) fn quoting_here(&self) -> Quoting {
		quoting_of(&self.nest)
	}

	/// Read `chars` -- text bash reads again on its own, a backtick's body or a
	/// heredoc's -- only to learn where its interpolations sit, from `nest`
	/// down. Nothing it finds wrong is reported: this reading has not been held
	/// to bash's there, so it may add a place to write a value for, never a
	/// refusal. A reading that does not get to the end adds nothing.
	pub(crate) fn sandbox(&mut self, chars: &[Ch], nest: Vec<Nest>, read: impl FnOnce(&mut P<'_>) -> R<()>) {
		if self.spots.is_empty() {
			return;
		}
		let mut p = P::new(chars, self.bash, vec![None; self.spots.len()]);
		p.depth = self.depth;
		p.nest = nest;
		if read(&mut p).is_ok() && p.heredocs.is_empty() {
			for (k, spot) in p.spots.into_iter().enumerate() {
				if spot.is_some() {
					self.spots[k] = spot;
				}
			}
		}
	}
}

/// What ends the list being read.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum End {
	Eof,
	Words(&'static [&'static str]),
	Paren,
	CaseItem,
}

/// Which characters end a word: a command's, or one inside `[[ … ]]`, where
/// `(`, `<`, `&&` and the rest are the test's own.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
	Normal,
	Test,
}

/// What opened the construct being read, for when it is never closed.
#[derive(Clone, Copy)]
pub(crate) struct Opener {
	pub i: usize,
	pub what: &'static str,
}

const RESERVED: &[&str] = &[
	"!", "[[", "case", "coproc", "do", "done", "elif", "else", "esac", "fi", "for", "function", "if", "in", "select",
	"then", "time", "until", "while", "{", "}",
];

/// The characters that end a word outside quotes.
pub(crate) fn is_meta(c: char) -> bool {
	matches!(c, ' ' | '\t' | '\n' | ';' | '&' | '|' | '(' | ')' | '<' | '>')
}

/// `NAME=…`, `NAME+=…` or `NAME[…]=…`, written plainly up to its `=`.
pub(crate) fn is_assignment(w: &Word) -> bool {
	let lead: Vec<char> = w
		.parts
		.iter()
		.map_while(|p| match p {
			Part::Char {
				c,
				quote: Quote::Bare,
				escaped: false,
				..
			} => Some(*c),
			_ => None,
		})
		.collect();
	let Some(&first) = lead.first() else {
		return false;
	};
	if !(first.is_ascii_alphabetic() || first == '_') {
		return false;
	}
	let mut k = 1;
	while k < lead.len() && (lead[k].is_ascii_alphanumeric() || lead[k] == '_') {
		k += 1;
	}
	if lead.get(k) == Some(&'[') {
		while k < lead.len() && lead[k] != ']' {
			k += 1;
		}
		k += 1;
	}
	matches!(lead.get(k..k + 1), Some(['='])) || matches!(lead.get(k..k + 2), Some(['+', '=']))
}

impl<'a> P<'a> {
	pub fn peek(&self) -> Option<char> {
		self.s.get(self.i).map(|c| c.c)
	}

	pub fn peek_at(&self, k: usize) -> Option<char> {
		self.s.get(self.i + k).filter(|c| c.hole.is_none()).map(|c| c.c)
	}

	pub fn is_hole(&self, j: usize) -> bool {
		self.s.get(j).is_some_and(|c| c.hole.is_some())
	}

	pub fn eof(&self) -> bool {
		self.i >= self.s.len()
	}

	/// Whether the text at `j` is exactly `lit`.
	pub fn at_index(&self, j: usize, lit: &str) -> bool {
		lit.chars()
			.enumerate()
			.all(|(k, c)| self.s.get(j + k).is_some_and(|ch| ch.hole.is_none() && ch.c == c))
	}

	pub fn at(&self, lit: &str) -> bool {
		self.at_index(self.i, lit)
	}

	/// Whether a word ends at `j`: the text ends there, or a character that
	/// ends one does.
	pub fn boundary(&self, j: usize) -> bool {
		self.s.get(j).is_none_or(|c| c.hole.is_none() && is_meta(c.c))
	}

	pub fn blank(&mut self) {
		loop {
			if self.at(" ") || self.at("\t") {
				self.i += 1;
			} else if self.at("\\\n") {
				self.i += 2;
			} else {
				return;
			}
		}
	}

	/// A `#` where a word would start opens a comment, to the end of the line.
	pub fn comment(&mut self) {
		if self.at("#") {
			// An interpolation in a comment does nothing -- until its value holds
			// a line break, which ends the comment early.
			let depth = self.enter(Nest::Comment);
			while !self.eof() && !self.at("\n") {
				if let Some(hole) = self.s[self.i].hole {
					self.mark(hole);
				}
				self.i += 1;
			}
			self.leave(depth);
		}
	}

	pub fn newline(&mut self) -> R<()> {
		self.i += 1;
		self.read_heredocs()
	}

	/// Blanks, comments and newlines, wherever a list lets them go.
	pub fn linebreak(&mut self) -> R<()> {
		loop {
			self.blank();
			self.comment();
			if !self.at("\n") {
				return Ok(());
			}
			self.newline()?;
		}
	}

	/// The reserved word at the cursor, if one is there.
	pub fn keyword(&self) -> Option<&'static str> {
		let mut w = String::new();
		for ch in self.s.get(self.i..).unwrap_or_default() {
			if ch.hole.is_some() {
				return None;
			}
			if is_meta(ch.c) {
				break;
			}
			if matches!(ch.c, '\'' | '"' | '\\' | '$' | '`') || w.len() > 8 {
				return None;
			}
			w.push(ch.c);
		}
		RESERVED.iter().copied().find(|k| *k == w)
	}

	pub fn take(&mut self, word: &str) {
		self.i += word.chars().count();
	}

	fn at_end(&self, end: End) -> bool {
		match end {
			End::Eof => self.eof(),
			End::Words(words) => self.keyword().is_some_and(|k| words.contains(&k)),
			End::Paren => self.at(")"),
			End::CaseItem => self.at(";;") || self.at(";&") || self.keyword() == Some("esac"),
		}
	}

	pub fn list(&mut self, end: End, opener: Option<Opener>) -> R<List> {
		let mut items = Vec::new();
		loop {
			self.linebreak()?;
			if self.at_end(end) {
				return Ok(items);
			}
			if self.eof() {
				return Err(self.unclosed(opener));
			}
			let pipelines = self.and_or(opener)?;
			self.blank();
			let background = if self.at("&") && !self.at("&&") {
				self.i += 1;
				true
			} else if self.at(";") && !self.at(";;") && !self.at(";&") {
				self.i += 1;
				false
			} else if self.eof() || self.at("\n") || self.at("#") || self.at_end(end) {
				false
			} else {
				return Err(self.unexpected());
			};
			items.push(Item { pipelines, background });
		}
	}

	fn and_or(&mut self, opener: Option<Opener>) -> R<Vec<Pipeline>> {
		let mut pipelines = vec![self.pipeline(opener)?];
		loop {
			self.blank();
			if !(self.at("&&") || self.at("||")) {
				return Ok(pipelines);
			}
			let op = self.i;
			self.i += 2;
			self.linebreak()?;
			if self.eof() {
				return Err(self.dangling(op, 2));
			}
			pipelines.push(self.pipeline(opener)?);
		}
	}

	fn pipeline(&mut self, opener: Option<Opener>) -> R<Pipeline> {
		self.blank();
		let mut prefixed = false;
		loop {
			match self.keyword() {
				Some("time") => {
					self.take("time");
					self.blank();
					if self.at("-p") && self.boundary(self.i + 2) {
						self.i += 2;
					}
				}
				Some("!") => self.i += 1,
				_ => break,
			}
			prefixed = true;
			self.blank();
		}
		if prefixed && (self.eof() || self.at("\n") || self.at(";") || self.at("&") || self.at(")")) {
			return Ok(Pipeline::default());
		}
		let mut commands = vec![self.command(opener)?];
		loop {
			self.blank();
			let len = if self.at("|&") {
				2
			} else if self.at("|") && !self.at("||") {
				1
			} else {
				return Ok(Pipeline { commands });
			};
			let op = self.i;
			self.i += len;
			self.linebreak()?;
			if self.eof() {
				return Err(self.dangling(op, len));
			}
			commands.push(self.command(opener)?);
		}
	}

	fn command(&mut self, opener: Option<Opener>) -> R<Command> {
		self.depth += 1;
		if self.depth > 64 {
			return Err(Stop::Lost);
		}
		let c = self.command_inner(opener);
		self.depth -= 1;
		c
	}

	fn command_inner(&mut self, opener: Option<Opener>) -> R<Command> {
		self.blank();
		let compound = match self.keyword() {
			Some("if") => self.if_clause()?,
			Some(k @ ("while" | "until")) => self.loop_clause(k)?,
			Some(k @ ("for" | "select")) => self.for_clause(k)?,
			Some("case") => self.case_clause()?,
			Some("{") => self.group()?,
			Some("[[") => self.test()?,
			Some("function") => return self.function_keyword(),
			Some("coproc" | "!" | "time") => return Err(Stop::Lost),
			Some(_) => return Err(self.unexpected()),
			None if self.at("((") => self.arith_or_subshell()?,
			None if self.at("(") => self.subshell()?,
			None if self.eof() => return Err(self.unclosed(opener)),
			None => {
				if let Some(f) = self.function_def()? {
					return Ok(f);
				}
				return self.simple();
			}
		};
		let mut redirects = Vec::new();
		loop {
			self.blank();
			let Some((len, op)) = self.redirect_op() else {
				return Ok(Command::Compound(compound, redirects));
			};
			redirects.push(self.redirect(len, op)?);
		}
	}

	fn if_clause(&mut self) -> R<Compound> {
		let o = Some(Opener { i: self.i, what: "if" });
		self.take("if");
		let mut arms = Vec::new();
		let mut after = "if";
		loop {
			let cond = self.list(End::Words(&["then"]), o)?;
			if cond.is_empty() {
				return Err(self.empty(after));
			}
			self.take("then");
			let body = self.list(End::Words(&["elif", "else", "fi"]), o)?;
			if body.is_empty() {
				return Err(self.empty("then"));
			}
			arms.push((cond, body));
			match self.keyword() {
				Some("elif") => {
					self.take("elif");
					after = "elif";
				}
				Some("else") => {
					self.take("else");
					let otherwise = self.list(End::Words(&["fi"]), o)?;
					if otherwise.is_empty() {
						return Err(self.empty("else"));
					}
					self.take("fi");
					return Ok(Compound::If(arms, Some(otherwise)));
				}
				_ => {
					self.take("fi");
					return Ok(Compound::If(arms, None));
				}
			}
		}
	}

	fn loop_clause(&mut self, k: &'static str) -> R<Compound> {
		let o = Some(Opener { i: self.i, what: k });
		self.take(k);
		let cond = self.list(End::Words(&["do"]), o)?;
		if cond.is_empty() {
			return Err(self.empty(k));
		}
		self.take("do");
		Ok(Compound::Loop(cond, self.do_body(o)?))
	}

	fn do_body(&mut self, o: Option<Opener>) -> R<List> {
		let body = self.list(End::Words(&["done"]), o)?;
		if body.is_empty() {
			return Err(self.empty("do"));
		}
		self.take("done");
		Ok(body)
	}

	fn for_clause(&mut self, k: &'static str) -> R<Compound> {
		let o = Some(Opener { i: self.i, what: k });
		self.take(k);
		self.blank();
		let mut words = Vec::new();
		let mut name = None;
		if self.at("((") {
			self.i += 2;
			if !self.arithmetic(&mut Vec::new())? {
				return Err(Stop::Lost);
			}
			self.blank();
			if self.at(";") {
				self.i += 1;
			}
		} else {
			if self.eof() {
				return Err(self.unclosed(o));
			}
			if self.peek().is_some_and(is_meta) {
				return Err(Stop::Lost);
			}
			name = Some(self.word(Mode::Normal)?);
			self.linebreak()?;
			if self.keyword() == Some("in") {
				self.take("in");
				loop {
					self.blank();
					self.comment();
					if self.eof() || self.at("\n") || self.at(";") {
						break;
					}
					if self.peek().is_some_and(is_meta) {
						return Err(Stop::Lost);
					}
					words.push(self.word(Mode::Normal)?);
				}
				if self.at(";") {
					self.i += 1;
				}
			} else if self.at(";") {
				self.i += 1;
			}
		}
		self.linebreak()?;
		match self.keyword() {
			Some("do") => {
				self.take("do");
				Ok(Compound::For(name, words, self.do_body(o)?))
			}
			Some("{") => match self.group()? {
				Compound::Group(body) => Ok(Compound::For(name, words, body)),
				_ => Err(Stop::Lost),
			},
			_ if self.eof() => Err(self.unclosed(o)),
			_ => Err(Stop::Lost),
		}
	}

	fn case_clause(&mut self) -> R<Compound> {
		let o = Some(Opener {
			i: self.i,
			what: "case",
		});
		self.take("case");
		self.blank();
		if self.eof() {
			return Err(self.unclosed(o));
		}
		if self.peek().is_some_and(is_meta) {
			return Err(Stop::Lost);
		}
		let subject = self.word(Mode::Normal)?;
		self.linebreak()?;
		match self.keyword() {
			Some("in") => self.take("in"),
			_ if self.eof() => return Err(self.unclosed(o)),
			_ => return Err(Stop::Lost),
		}
		let mut arms = Vec::new();
		loop {
			self.linebreak()?;
			if self.keyword() == Some("esac") {
				self.take("esac");
				return Ok(Compound::Case(subject, arms));
			}
			if self.at("(") {
				self.i += 1;
			}
			let mut patterns = Vec::new();
			loop {
				self.blank();
				if self.eof() {
					return Err(self.unclosed(o));
				}
				if self.peek().is_some_and(is_meta) {
					return Err(Stop::Lost);
				}
				patterns.push(self.word(Mode::Normal)?);
				self.blank();
				if !self.at("|") {
					break;
				}
				self.i += 1;
			}
			if self.eof() {
				return Err(self.unclosed(o));
			}
			if !self.at(")") {
				return Err(Stop::Lost);
			}
			self.i += 1;
			let body = self.list(End::CaseItem, o)?;
			arms.push((patterns, body));
			if self.at(";;&") {
				self.i += 3;
			} else if self.at(";;") || self.at(";&") {
				self.i += 2;
			}
		}
	}

	fn group(&mut self) -> R<Compound> {
		let o = Some(Opener { i: self.i, what: "{" });
		self.take("{");
		let body = self.list(End::Words(&["}"]), o)?;
		if body.is_empty() {
			return Err(self.empty("{"));
		}
		self.take("}");
		Ok(Compound::Group(body))
	}

	fn subshell(&mut self) -> R<Compound> {
		let o = Some(Opener { i: self.i, what: "(" });
		self.i += 1;
		let body = self.list(End::Paren, o)?;
		if body.is_empty() {
			return Err(self.empty("("));
		}
		self.i += 1;
		Ok(Compound::Subshell(body))
	}

	/// `((` is arithmetic when a `))` closes it, and a subshell in a subshell
	/// when the parentheses close some other way -- which is how bash tells.
	fn arith_or_subshell(&mut self) -> R<Compound> {
		let start = self.i;
		self.i += 2;
		let mut parts = Vec::new();
		if let Ok(true) = self.arithmetic(&mut parts) {
			return Ok(Compound::Arith(parts));
		}
		self.i = start;
		self.subshell()
	}

	fn test(&mut self) -> R<Compound> {
		let open = self.i;
		let o = Some(Opener { i: open, what: "[[" });
		self.take("[[");
		let mut words = Vec::new();
		let mut lines = false;
		loop {
			self.blank();
			if self.eof() {
				return Err(self.unclosed(o));
			}
			if self.at("\n") {
				lines = true;
				self.newline()?;
				continue;
			}
			if self.at(";") {
				return Err(Stop::Lost);
			}
			if self.at("]]") && self.boundary(self.i + 2) {
				let close = self.i;
				self.i += 2;
				// Bash skips a newline in some places inside `[[ … ]]` and refuses
				// one in others; a test spread over lines is left to it. Another
				// shell's `[[` is its own -- busybox reads one as `test` -- and is
				// left to that shell.
				if !lines && self.bash {
					conditional(self.s, &words, open, close)?;
				}
				return Ok(Compound::Test(words));
			}
			words.push(self.word(Mode::Test)?);
		}
	}

	fn function_keyword(&mut self) -> R<Command> {
		let o = Opener {
			i: self.i,
			what: "function",
		};
		self.take("function");
		self.blank();
		if self.eof() || self.peek().is_some_and(is_meta) {
			return Err(Stop::Lost);
		}
		self.word(Mode::Normal)?;
		self.blank();
		if self.at("(") {
			self.i += 1;
			self.blank();
			if !self.at(")") {
				return Err(Stop::Lost);
			}
			self.i += 1;
		}
		self.function_body(o)
	}

	/// `name()`, if the cursor is at one: a plain name, then `(` and `)`.
	fn function_def(&mut self) -> R<Option<Command>> {
		let start = self.i;
		let mut j = start;
		while self.s.get(j).is_some_and(|ch| {
			ch.hole.is_none() && (ch.c.is_alphanumeric() || matches!(ch.c, '_' | '-' | '.' | ':' | '@' | '+'))
		}) {
			j += 1;
		}
		if j == start {
			return Ok(None);
		}
		let k = self.blanks_from(j);
		if !self.at_index(k, "(") {
			return Ok(None);
		}
		let k = self.blanks_from(k + 1);
		if !self.at_index(k, ")") {
			return Ok(None);
		}
		self.i = k + 1;
		self.function_body(Opener {
			i: start,
			what: "function",
		})
		.map(Some)
	}

	fn blanks_from(&self, mut k: usize) -> usize {
		while self.at_index(k, " ") || self.at_index(k, "\t") {
			k += 1;
		}
		k
	}

	fn function_body(&mut self, o: Opener) -> R<Command> {
		self.linebreak()?;
		if self.eof() {
			return Err(Stop::Refused {
				rule: "unclosed",
				from: o.i,
				to: o.i + 1,
				message: "the function defined here has no body".into(),
			});
		}
		let compound = matches!(
			self.keyword(),
			Some("{" | "if" | "while" | "until" | "for" | "select" | "case" | "[[")
		) || self.at("(");
		if !compound {
			return Err(Stop::Lost);
		}
		// Named by the word after `function`, or by the word before `()`.
		let mut j = o.i;
		if self.at_index(j, "function ") || self.at_index(j, "function\t") {
			j = self.blanks_from(j + 8);
		}
		let name: String = self.s[j.min(self.s.len())..]
			.iter()
			.take_while(|c| {
				c.hole.is_none() && (c.c.is_alphanumeric() || matches!(c.c, '_' | '-' | '.' | ':' | '@' | '+'))
			})
			.map(|c| c.c)
			.collect();
		Ok(Command::Function(name, Box::new(self.command(None)?)))
	}

	fn simple(&mut self) -> R<Command> {
		let mut cmd = Simple::default();
		loop {
			self.blank();
			if let Some((len, op)) = self.redirect_op() {
				cmd.redirects.push(self.redirect(len, op)?);
				continue;
			}
			match self.peek() {
				None | Some('\n' | ';' | '&' | '|' | ')') => break,
				Some('(') => return Err(Stop::Lost),
				Some('#') => {
					self.comment();
					break;
				}
				_ => {}
			}
			let word = self.word(Mode::Normal)?;
			if word.start == word.end {
				return Err(Stop::Lost);
			}
			if cmd.words.is_empty() && is_assignment(&word) {
				cmd.assigns.push(word);
			} else {
				cmd.words.push(word);
			}
		}
		if cmd.words.is_empty() && cmd.assigns.is_empty() && cmd.redirects.is_empty() {
			return Err(self.unexpected());
		}
		Ok(Command::Simple(cmd))
	}

	/// The token at the cursor, and what bash says when it finds it there.
	fn describe(&self) -> (usize, String) {
		if let Some(k) = self.keyword() {
			let message = match k {
				"fi" => "`fi` has no `if` to close".into(),
				"done" => "`done` has no `do` to close".into(),
				"esac" => "`esac` has no `case` to close".into(),
				"}" => "`}` has no `{` to close".into(),
				"then" => "`then` is not after the condition of an `if` or an `elif`".into(),
				"do" => "`do` is not after the header of a `for`, `while` or `until`".into(),
				"in" => "`in` is not part of a `for` or a `case`".into(),
				_ => format!("`{k}` is not inside an `if`"),
			};
			return (k.chars().count(), message);
		}
		for op in [";;&", ";;", ";&", "&&", "||", "|&", ";", "&", "|", ")"] {
			if self.at(op) {
				let message = match op {
					";;&" | ";;" | ";&" => format!("`{op}` ends a branch of a `case`, and there is no `case` here"),
					")" => "`)` has no `(` to close".into(),
					_ => format!("`{op}` has no command before it"),
				};
				return (op.chars().count(), message);
			}
		}
		(1, "bash cannot read the script from here".into())
	}

	pub fn unexpected(&self) -> Stop {
		let (len, message) = self.describe();
		Stop::Refused {
			rule: "unexpected",
			from: self.i,
			to: self.i + len,
			message,
		}
	}

	/// A block with no command in it, which bash refuses at the word that ends it.
	fn empty(&self, after: &str) -> Stop {
		let (len, _) = self.describe();
		let token: String = self.s[self.i..(self.i + len).min(self.s.len())]
			.iter()
			.map(|c| c.c)
			.collect();
		Stop::Refused {
			rule: "unexpected",
			from: self.i,
			to: self.i + len,
			message: format!(
				"nothing runs between `{after}` and `{token}`, and bash needs a command there -- `:` is the one that does nothing"
			),
		}
	}

	pub fn unclosed(&self, opener: Option<Opener>) -> Stop {
		let Some(o) = opener else {
			return Stop::Lost;
		};
		let close = match o.what {
			"if" => "`fi`",
			"while" | "until" | "for" | "select" => "`done`",
			"case" => "`esac`",
			"{" => "`}`",
			"[[" => "`]]` -- which has to be a word of its own",
			_ => "`)`",
		};
		Stop::Refused {
			rule: "unclosed",
			from: o.i,
			to: o.i + o.what.chars().count(),
			message: format!("`{}` is never closed by {close}", o.what),
		}
	}

	/// An operator the script ends on, with nothing after it to run.
	fn dangling(&self, op: usize, len: usize) -> Stop {
		let text: String = self.s[op..op + len].iter().map(|c| c.c).collect();
		Stop::Refused {
			rule: "unclosed",
			from: op,
			to: op + len,
			message: format!("the script ends after `{text}`, with no command for it to run"),
		}
	}

	pub fn unterminated(&self, h: &Pending) -> Stop {
		let tabs = if h.strip_tabs {
			" once its leading tabs are removed"
		} else {
			""
		};
		Stop::Refused {
			rule: "unterminated-heredoc",
			from: h.i,
			to: h.i + h.len,
			message: format!(
				"no line is exactly `{}`{tabs}, so this heredoc takes in every line after it, and those lines never run",
				h.delimiter
			),
		}
	}
}

/// One of the tokens bash reads inside `[[ … ]]`, from a word's text as
/// written: a quoted `"("` or `"-f"` is only a word to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tok {
	Word,
	Bang,
	Open,
	Close,
	And,
	Or,
	/// `<` and `>`, which only compare.
	Angle,
	End,
}

/// The unary tests of `[[ … ]]`: bash's `test_unop`.
const UNARY: &[&str] = &[
	"-a", "-b", "-c", "-d", "-e", "-f", "-g", "-h", "-k", "-n", "-o", "-p", "-r", "-s", "-t", "-u", "-v", "-w", "-x",
	"-z", "-G", "-L", "-N", "-O", "-R", "-S",
];

/// Its binary operators that are words: bash's `test_binop`, and `=~`.
const BINARY: &[&str] = &[
	"=", "==", "!=", "=~", "!~", "-nt", "-ot", "-ef", "-eq", "-ne", "-lt", "-le", "-gt", "-ge",
];

fn tok(text: Option<&str>) -> Tok {
	match text {
		Some("!") => Tok::Bang,
		Some("(") => Tok::Open,
		Some(")") => Tok::Close,
		Some("&&") => Tok::And,
		Some("||") => Tok::Or,
		Some("<" | ">") => Tok::Angle,
		_ => Tok::Word,
	}
}

/// Whether bash splits this word inside `[[ … ]]` where the checker did not:
/// at a `(`, `)`, `<`, `>`, `&` or `|` outside quotes, which `[[` reads as its
/// own even with no blank around them. The pattern after `=~` has rules of its
/// own, where a `(` and a `|` belong to it -- as long as its parentheses
/// balance, since a blank inside them splits it here and not there.
fn splits(w: &Word, pattern: bool) -> bool {
	if matches!(w.plain().as_deref(), Some("(" | ")" | "<" | ">" | "&&" | "||")) {
		return false;
	}
	let bare = w.parts.iter().filter_map(|p| match p {
		Part::Char {
			c,
			quote: Quote::Bare,
			escaped: false,
			..
		} => Some(*c),
		_ => None,
	});
	if !pattern {
		return bare.into_iter().any(|c| matches!(c, '(' | ')' | '<' | '>' | '&' | '|'));
	}
	let mut depth = 0i32;
	for c in bare {
		match c {
			'(' => depth += 1,
			')' if depth == 0 => return true,
			')' => depth -= 1,
			'<' | '>' | '&' => return true,
			_ => {}
		}
	}
	depth != 0
}

/// What is wrong, as the word it is about -- `None` for `]]` -- and why.
type Refusal = (Option<usize>, String);

/// Bash's grammar for what is inside `[[ … ]]` (`cond_term` in its parser),
/// over the words read there.
struct Cond<'t> {
	toks: &'t [Tok],
	texts: &'t [String],
	at: usize,
	/// Nesting of `( … )` groups, so a pathologically deep test gives up rather
	/// than overflowing the stack (audit SA-022). `||`/`&&` width is handled by
	/// the loops in `or`/`and`, which do not recurse.
	depth: usize,
}

impl Cond<'_> {
	fn peek(&self) -> Tok {
		self.toks.get(self.at).copied().unwrap_or(Tok::End)
	}

	fn or(&mut self) -> Result<(), Refusal> {
		// A loop rather than right-recursion, so `[[ x || x || … ]]` tens of
		// thousands wide validates without a stack frame per operator (SA-022).
		// `or` is also the entry for each `( … )` group (via `term`), so its
		// depth bounds parenthesis nesting; decremented on success so sibling
		// groups do not accumulate.
		self.depth += 1;
		if self.depth > 64 {
			return Err((None, "`[[ … ]]` is nested too deeply".into()));
		}
		loop {
			self.and()?;
			if self.peek() == Tok::Or {
				self.at += 1;
			} else {
				break;
			}
		}
		self.depth -= 1;
		Ok(())
	}

	fn and(&mut self) -> Result<(), Refusal> {
		loop {
			self.term()?;
			if self.peek() == Tok::And {
				self.at += 1;
			} else {
				break;
			}
		}
		Ok(())
	}

	/// `!` and a term, `( … )`, a unary test and its word, a word, an operator
	/// and a word -- or one word alone, which asks whether it is empty.
	fn term(&mut self) -> Result<(), Refusal> {
		let k = self.at;
		match self.peek() {
			Tok::End => Err(match k.checked_sub(1) {
				None => (None, "`[[ ]]` has nothing in it to test, and bash refuses it".into()),
				Some(before) => (
					Some(before),
					format!("`{}` has nothing after it to test", self.texts[before]),
				),
			}),
			Tok::Open => {
				self.at += 1;
				self.or()?;
				if self.peek() != Tok::Close {
					return Err((Some(k), "this `(` is never closed by a `)` of its own".into()));
				}
				self.at += 1;
				Ok(())
			}
			Tok::Bang => {
				self.at += 1;
				self.term()
			}
			Tok::Word if UNARY.contains(&self.texts[k].as_str()) => {
				self.at += 1;
				match self.peek() {
					Tok::Word | Tok::Bang => {
						self.at += 1;
						Ok(())
					}
					_ => Err((
						Some(k),
						format!("`{}` tests the word after it, and there is none", self.texts[k]),
					)),
				}
			}
			Tok::Word => {
				self.at += 1;
				match self.peek() {
					Tok::End | Tok::And | Tok::Or | Tok::Close => return Ok(()),
					Tok::Angle => {}
					Tok::Word if BINARY.contains(&self.texts[self.at].as_str()) => {}
					_ => return Err(self.misplaced(self.at, "follows a word")),
				}
				let op = self.at;
				self.at += 1;
				match self.peek() {
					Tok::Word | Tok::Bang => {
						self.at += 1;
						Ok(())
					}
					_ => Err((
						Some(op),
						format!(
							"`{}` compares the word before it with a word after it, and there is none",
							self.texts[op]
						),
					)),
				}
			}
			Tok::Close | Tok::And | Tok::Or | Tok::Angle => {
				Err((Some(k), format!("`{}` has nothing before it to test", self.texts[k])))
			}
		}
	}

	/// A word where `[[ … ]]` needs an operator, `&&`, `||` or `]]`.
	fn misplaced(&self, k: usize, place: &str) -> Refusal {
		let t = &self.texts[k];
		let message = match t.as_str() {
			"-a" | "-o" => format!(
				"`{t}` joins tests inside `[ … ]`, but inside `[[ … ]]` it is not an operator -- write `{}`",
				if t == "-a" { "&&" } else { "||" }
			),
			_ => format!(
				"`{t}` {place}, where `[[ … ]]` needs an operator such as `==`, or `&&`, `||` or `]]` -- a blank in a value has to be quoted"
			),
		};
		(Some(k), message)
	}
}

/// Hold what is inside `[[ … ]]` to bash's grammar for it. `open` and `close`
/// are where its `[[` and `]]` are.
///
/// A word this reading could get wrong is left to bash rather than guessed at:
/// one holding an interpolation, which could become an operator once rendered,
/// and one holding a character `[[` splits words at, which the checker kept
/// inside it.
fn conditional(chars: &[Ch], words: &[Word], open: usize, close: usize) -> R<()> {
	for (k, w) in words.iter().enumerate() {
		if w.parts.iter().any(|p| matches!(p, Part::Hole { .. })) {
			return Ok(());
		}
		let pattern = k > 0 && matches!(words[k - 1].plain().as_deref(), Some("=~" | "!~"));
		if splits(w, pattern) {
			return Ok(());
		}
	}
	let toks: Vec<Tok> = words.iter().map(|w| tok(w.plain().as_deref())).collect();
	// A word's text as written, so `"-f"` is not taken for `-f`.
	let texts: Vec<String> = words
		.iter()
		.map(|w| chars[w.start..w.end].iter().map(|c| c.c).collect())
		.collect();
	let mut c = Cond {
		toks: &toks,
		texts: &texts,
		at: 0,
		depth: 0,
	};
	let refused = match c.or() {
		Err(r) => r,
		Ok(()) if c.peek() == Tok::End => return Ok(()),
		Ok(()) => c.misplaced(c.at, "comes after a complete test"),
	};
	let (from, to) = match refused.0 {
		Some(k) => (words[k].start, words[k].end),
		None if words.is_empty() => (open, open + 2),
		None => (close, close + 2),
	};
	Err(Stop::Refused {
		rule: "unexpected",
		from,
		to,
		message: refused.1,
	})
}
