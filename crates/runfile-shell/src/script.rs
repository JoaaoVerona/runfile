//! Shell text as the runner hands it to a shell, with every character placed at
//! the byte of the runfile it came from.
//!
//! A `$` run is its lines joined by newlines, each with its marker taken off; an
//! `exec` body is its lines less their common indentation. An interpolation is
//! not rendered -- its value is not known until the run -- but kept whole, as
//! one [`Ch`] standing for everything from its `{{` to its `}}`. That is what
//! places a problem on the column the author wrote, and what keeps the checker
//! from ever reading a command nobody wrote.
//!
//! Every character is matched against the source as it is placed. A script
//! whose text cannot be found where the tree says it is -- which would be a bug
//! in the parser's positions -- is not checked at all, rather than checked with
//! its problems placed somewhere wrong.

use runfile_lang::{Expr, InterpPart, Property, Span, Statement, Target};

/// One character of a script, and the byte of the runfile it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ch {
	pub c: char,
	pub at: usize,
	/// Which of the script's interpolations this stands for, if it is one.
	pub hole: Option<usize>,
}

/// What an interpolation reads as in a script: a character no shell gives a
/// meaning to, so nothing but the checker's own rules can treat it as special.
pub const HOLE: char = '\u{FFFC}';

/// An interpolation, as the runfile spells it.
#[derive(Debug, Clone)]
pub struct Hole {
	pub expr: Expr,
	/// From its `{{` to just past its `}}`.
	pub start: usize,
	pub end: usize,
}

/// What a script's process is run for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Use {
	/// A statement: run for its effect, with a shell that ends when it does.
	Statement,
	/// A capture, a condition or a `code_of`: run for its output or status.
	Value,
}

#[derive(Debug, Clone)]
pub struct Script {
	pub chars: Vec<Ch>,
	pub holes: Vec<Hole>,
	pub used: Use,
	/// Whether the shell is handed positional parameters. A `$` run never is.
	pub arguments: bool,
}

impl Script {
	/// The bytes of the runfile that characters `from..to` came from.
	pub fn bytes(&self, from: usize, to: usize) -> (usize, usize) {
		let Some(first) = self.chars.get(from) else {
			let end = self.chars.last().map_or(0, |c| self.end_of(c));
			return (end, end);
		};
		let last = &self.chars[to.clamp(from + 1, self.chars.len()) - 1];
		(first.at, self.end_of(last).max(first.at))
	}

	fn end_of(&self, c: &Ch) -> usize {
		match c.hole {
			Some(h) => self.holes[h].end,
			None => c.at + c.c.len_utf8(),
		}
	}
}

/// A runfile's text, with where each of its lines starts.
pub struct Source<'a> {
	pub text: &'a str,
	starts: Vec<usize>,
}

impl<'a> Source<'a> {
	pub fn new(text: &'a str) -> Self {
		let mut starts = vec![0];
		starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
		Source { text, starts }
	}

	/// The 1-based line a byte is on.
	pub fn line_of(&self, at: usize) -> usize {
		self.starts.partition_point(|&s| s <= at)
	}

	pub fn span(&self, start: usize, end: usize) -> Span {
		Span::new(start, end, self.line_of(start))
	}

	fn start_of(&self, no: usize) -> Option<usize> {
		self.starts.get(no.checked_sub(1)?).copied()
	}

	/// A line's text, without its line ending.
	pub fn line(&self, no: usize) -> Option<&'a str> {
		let start = self.start_of(no)?;
		let end = self.starts.get(no).map_or(self.text.len(), |&next| next - 1);
		let l = &self.text[start..end];
		Some(l.strip_suffix('\r').unwrap_or(l))
	}
}

/// Whether a command line names a shell this checker reads, and so whether its
/// body is checked: `Some(arguments)`, where `arguments` says the shell is also
/// handed positional parameters, as `bash -s a b` is.
///
/// `None` for everything else: another language, a shell whose syntax is its
/// own (zsh and ksh both parse things bash refuses), or a command line whose
/// program is not known until it runs.
pub fn shell(command: &str) -> Option<bool> {
	if command.contains(['\'', '"', '\\', '$', '`']) {
		return None;
	}
	let words: Vec<&str> = command.split_whitespace().collect();
	let (first, rest) = words.split_first()?;
	let base = first.rsplit(['/', '\\']).next().unwrap_or(first);
	let rest = match base.strip_suffix(".exe").unwrap_or(base) {
		"sh" | "bash" | "dash" | "ash" | "brush" => rest,
		"busybox" => match rest.split_first() {
			Some((applet, rest)) if matches!(*applet, "sh" | "ash") => rest,
			_ => return None,
		},
		_ => return None,
	};
	let mut arguments = false;
	let mut words = rest.iter();
	while let Some(w) = words.next() {
		match *w {
			"-o" | "+o" | "--rcfile" | "--init-file" => {
				words.next();
			}
			w if w.starts_with("--") => {}
			// A script given as `-c` is not the body, so the body is not what runs.
			w if w.starts_with(['-', '+']) && w.contains('c') => return None,
			w if w.starts_with(['-', '+']) => {}
			_ => arguments = true,
		}
	}
	Some(arguments)
}

/// Whether the `$` runs in `file` are read: `Some(false)` when they run under a
/// shell this checker reads, and `None` when `.shell` names another one or a
/// value not known until the run -- or when the `_shared.run` files above, one
/// of which could name it, could not be read.
pub fn dollar(file: &Target, chain: Option<&[Target]>) -> Option<bool> {
	let mut named: Option<Option<String>> = None;
	for f in chain?.iter().chain(std::iter::once(file)) {
		for p in f.body.properties.iter().filter(|p| p.path == ["shell"]) {
			named = Some(literal(p));
		}
	}
	match named {
		None => Some(false),
		// `-c` is the runner's, so a shell named with arguments of its own runs
		// something other than the line.
		Some(Some(cmd)) => shell(&cmd).filter(|arguments| !arguments),
		Some(None) => None,
	}
}

fn literal(p: &Property) -> Option<String> {
	match &p.value {
		Some(Expr::Str(parts, _)) => text_of(parts),
		_ => None,
	}
}

/// Parts with no interpolation in them, as one string.
fn text_of(parts: &[InterpPart]) -> Option<String> {
	parts
		.iter()
		.map(|p| match p {
			InterpPart::Literal(s) => Some(s.as_str()),
			InterpPart::Expr(_) => None,
		})
		.collect()
}

/// The script a `$` run or an `exec` block hands its shell, when it has one this
/// checker reads. `dollar` is what [`dollar`] answered for the file.
pub fn of_statement(st: &Statement, src: &Source, dollar: Option<bool>) -> Option<Script> {
	let Statement::Exec {
		command, body, lines, ..
	} = st
	else {
		return None;
	};
	let (arguments, starts) = match command {
		None => (
			dollar?,
			lines.iter().map(|&no| after_marker(src, no)).collect::<Option<_>>()?,
		),
		Some(parts) => (shell(&text_of(parts)?)?, dedented(src, lines)?),
	};
	build(src, body, &starts, Use::Statement, arguments)
}

/// The script a capture hands its shell: `let x = $ …`, `if $ …`, `lines($ …)`,
/// `let x = exec sh … end`.
pub fn of_capture(e: &Expr, src: &Source, dollar: Option<bool>) -> Option<Script> {
	let Expr::Capture { command, body, span } = e else {
		return None;
	};
	match command {
		None => {
			let arguments = dollar?;
			// A capture is placed from its `$` by some forms and from the command
			// by others, and a command may itself start with a `$`: the text
			// decides, since only one of the two places spells the command.
			let at = span.start;
			build(src, body, &[at], Use::Value, arguments).or_else(|| {
				let after = src.text.get(at..)?.strip_prefix('$')?;
				let line = after.split('\n').next().unwrap_or_default();
				build(src, body, &[at + 1 + blanks(line)], Use::Value, arguments)
			})
		}
		Some(parts) => {
			let arguments = shell(&text_of(parts)?)?;
			let lines: Vec<usize> = (1..=body.len()).map(|i| span.line + i).collect();
			build(src, body, &dedented(src, &lines)?, Use::Value, arguments)
		}
	}
}

fn blanks(s: &str) -> usize {
	s.len() - s.trim_start().len()
}

/// Where the command on a `$` line starts: past its indentation, a `detach`,
/// the marker, and the blanks after it.
fn after_marker(src: &Source, no: usize) -> Option<usize> {
	let line = src.line(no)?;
	let mut rest = &line[blanks(line)..];
	if let Some(after) = rest.strip_prefix("detach")
		&& after.starts_with([' ', '\t'])
	{
		rest = &after[blanks(after)..];
	}
	let after = rest.strip_prefix('$')?;
	let command = &after[blanks(after)..];
	Some(src.start_of(no)? + line.len() - command.len())
}

/// Where each line of an `exec` body starts once the body's own indentation is
/// taken off -- the parser's rule, which blank lines have no say in.
fn dedented(src: &Source, lines: &[usize]) -> Option<Vec<usize>> {
	let texts: Vec<&str> = lines.iter().map(|&no| src.line(no)).collect::<Option<_>>()?;
	let base = texts
		.iter()
		.filter(|t| !t.trim().is_empty())
		.map(|t| blanks(t))
		.min()
		.unwrap_or(0);
	lines
		.iter()
		.zip(&texts)
		.map(|(&no, t)| Some(src.start_of(no)? + base.min(t.len())))
		.collect()
}

fn build(src: &Source, body: &[Vec<InterpPart>], starts: &[usize], used: Use, arguments: bool) -> Option<Script> {
	if body.len() != starts.len() {
		return None;
	}
	let mut s = Script {
		chars: Vec::new(),
		holes: Vec::new(),
		used,
		arguments,
	};
	let mut end = 0;
	for (k, (parts, &start)) in body.iter().zip(starts).enumerate() {
		if k > 0 {
			// The runner joins a body's lines with newlines.
			s.chars.push(Ch {
				c: '\n',
				at: end,
				hole: None,
			});
		}
		end = place(src, parts, start, &mut s)?;
	}
	Some(s)
}

/// Lay one line's parts out from `at`, checking each character against the
/// source. Answers where the line ended.
fn place(src: &Source, parts: &[InterpPart], mut at: usize, s: &mut Script) -> Option<usize> {
	let text = src.text;
	for part in parts {
		match part {
			InterpPart::Literal(lit) => {
				for c in lit.chars() {
					if c == '\n' {
						// A backslash continuation: the text goes on at the start of
						// the next line, however that line's ending was written.
						s.chars.push(Ch { c, at, hole: None });
						at = src.start_of(src.line_of(at) + 1)?;
						continue;
					}
					// `\{{` is how a literal `{{` is written.
					if c == '{' && text.get(at..)?.starts_with("\\{{") {
						at += 1;
					}
					if !text.get(at..)?.starts_with(c) {
						return None;
					}
					s.chars.push(Ch { c, at, hole: None });
					at += c.len_utf8();
				}
			}
			InterpPart::Expr(e) => {
				if !text.get(at..)?.starts_with("{{") {
					return None;
				}
				let end = runfile_lang::lexer::skip_interp(text.as_bytes(), at, 0).ok()?;
				s.chars.push(Ch {
					c: HOLE,
					at,
					hole: Some(s.holes.len()),
				});
				s.holes.push(Hole {
					expr: e.clone(),
					start: at,
					end,
				});
				at = end;
			}
		}
	}
	Some(at)
}
