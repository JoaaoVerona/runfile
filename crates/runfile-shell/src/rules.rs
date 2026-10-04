//! The rules that read a script's commands.
//!
//! Each finds something bash does that the author cannot have meant, and only
//! where that is certain. Where the same text could be right -- an argument that
//! is shell code for something else to run, say -- a rule says nothing, and
//! `SHELL-CHECK-RULES.md` says why.

use crate::script::{Script, Use};
use crate::syntax::{Command, Compound, List, Part, Quote, Simple, Word, is_assignment};

/// A problem a rule found, by the characters of the script it covers.
pub(crate) struct Found {
	pub rule: &'static str,
	pub from: usize,
	pub to: usize,
	pub message: String,
	pub fix: Option<String>,
}

/// The source text of a word.
pub(crate) struct Texts<'s> {
	script: &'s Script,
	src: &'s str,
}

impl<'s> Texts<'s> {
	fn of(&self, w: &Word) -> &'s str {
		let (a, b) = self.script.bytes(w.start, w.end);
		&self.src[a..b]
	}
}

/// What surrounds a command.
#[derive(Clone, Copy, Default)]
pub(crate) struct Around {
	/// Inside the body of a shell function.
	pub function: bool,
	/// Inside a `for`, `select`, `while` or `until` of the script's own.
	pub looped: bool,
}

/// Where a word sits in a command, which decides what its text can mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pos {
	/// The program a command runs.
	Program,
	/// The file a redirection opens.
	Redirect,
	/// An argument of a command that takes plain values -- paths, patterns,
	/// names -- and never runs one as shell code. `path` when it names a file.
	Plain { path: bool },
	/// An operand of `[`, `test` or `[[`: `path` after a file test, and
	/// `paired` when another operand holds a quoted interpolation too.
	Test { path: bool, paired: bool },
	/// A `case` pattern.
	Pattern,
	/// A heredoc's delimiter.
	Delimiter,
	/// An operand bash evaluates as an **arithmetic expression** after quote
	/// removal: `[[ a -eq b ]]`, `let`, `declare -i x=…`. A value there is
	/// re-expanded whatever its quoting, so an interpolation must be a number --
	/// otherwise a `$( )` or array subscript in it runs (audit SA-009).
	Arith,
	/// Anywhere else, including every argument that may be code for something
	/// else to run: `sh -c`, `ssh`, `eval`, `echo` into a file.
	Unknown,
}

/// The `[[ … ]]` / `[ … ]` operators that force their operands to be evaluated
/// as arithmetic.
const ARITH_OPS: &[&str] = &["-eq", "-ne", "-lt", "-le", "-gt", "-ge"];

/// Whether `w` is one of [`ARITH_OPS`], written plainly.
fn is_arith_op(w: &Word) -> bool {
	w.plain().as_deref().is_some_and(|t| ARITH_OPS.contains(&t))
}

/// Whether a `[[ … ]]` / `[ … ]` / `test` operand at `j` is arithmetic: next to a
/// numeric comparison operator.
fn arith_operand(words: &[Word], j: usize) -> bool {
	(j > 0 && is_arith_op(&words[j - 1])) || words.get(j + 1).is_some_and(is_arith_op)
}

pub(crate) enum Visit<'t> {
	Command(&'t Simple, Around),
	Word(&'t Word, Pos, Around),
	Arith(&'t [Part], Around),
	Function(&'t str),
	/// The words inside a `[[ … ]]`.
	Test(&'t [Word]),
	/// The name a `for` or `select` sets.
	ForName(&'t Word),
}

/// Commands that take only plain values, whatever their options.
const FILE_COMMANDS: &[&str] = &[
	".", "basename", "cat", "cd", "chgrp", "chmod", "chown", "cp", "dirname", "du", "head", "install", "ln", "ls",
	"mkdir", "mv", "pushd", "readlink", "realpath", "rm", "rmdir", "source", "stat", "tail", "tee", "touch", "unlink",
	"wc",
];

/// Commands that run the command after them.
const WRAPPERS: &[&str] = &["builtin", "command", "exec", "nohup", "sudo"];

/// The tests of `[` whose operand is a file.
const FILE_TESTS: &[&str] = &[
	"-b", "-c", "-d", "-e", "-f", "-g", "-h", "-k", "-p", "-r", "-s", "-u", "-w", "-x", "-G", "-L", "-N", "-O", "-S",
];

/// The options of `docker` and `podman` whose value names a file.
const DOCKER_PATHS: &[&str] = &["-v", "--volume", "-w", "--workdir", "--env-file", "-f", "--file"];

pub(crate) fn visit<'t>(list: &'t List, around: Around, f: &mut dyn FnMut(Visit<'t>)) {
	for item in list {
		for p in &item.pipelines {
			for c in &p.commands {
				visit_command(c, around, f);
			}
		}
	}
}

fn visit_command<'t>(c: &'t Command, around: Around, f: &mut dyn FnMut(Visit<'t>)) {
	match c {
		Command::Simple(s) => {
			f(Visit::Command(s, around));
			for w in &s.assigns {
				visit_word(w, Pos::Unknown, around, f);
			}
			for (w, pos) in s.words.iter().zip(positions(s)) {
				visit_word(w, pos, around, f);
			}
			for r in &s.redirects {
				visit_word(&r.target, redirect_pos(r.op), around, f);
			}
		}
		Command::Compound(k, redirects) => {
			match k {
				Compound::Group(l) | Compound::Subshell(l) => visit(l, around, f),
				Compound::If(arms, otherwise) => {
					for (cond, body) in arms {
						visit(cond, around, f);
						visit(body, around, f);
					}
					if let Some(o) = otherwise {
						visit(o, around, f);
					}
				}
				Compound::Loop(cond, body) => {
					let inside = Around { looped: true, ..around };
					visit(cond, inside, f);
					visit(body, inside, f);
				}
				Compound::For(name, words, body) => {
					if let Some(n) = name {
						f(Visit::ForName(n));
					}
					for w in words {
						visit_word(w, Pos::Unknown, around, f);
					}
					visit(body, Around { looped: true, ..around }, f);
				}
				Compound::Case(subject, arms) => {
					visit_word(subject, Pos::Unknown, around, f);
					for (patterns, body) in arms {
						for p in patterns {
							visit_word(p, Pos::Pattern, around, f);
						}
						visit(body, around, f);
					}
				}
				Compound::Arith(parts) => {
					f(Visit::Arith(parts, around));
					visit_parts(parts, around, f);
				}
				Compound::Test(words) => {
					f(Visit::Test(words));
					let paired = words.iter().filter(|w| double_hole(w)).count() > 1;
					for (k, w) in words.iter().enumerate() {
						let pos = if arith_operand(words, k) {
							Pos::Arith
						} else {
							Pos::Test {
								path: k > 0 && is_file_test(&words[k - 1]),
								paired,
							}
						};
						visit_word(w, pos, around, f);
					}
				}
			}
			for r in redirects {
				visit_word(&r.target, redirect_pos(r.op), around, f);
			}
		}
		Command::Function(name, body) => {
			f(Visit::Function(name));
			visit_command(
				body,
				Around {
					function: true,
					looped: false,
				},
				f,
			);
		}
	}
}

fn visit_word<'t>(w: &'t Word, pos: Pos, around: Around, f: &mut dyn FnMut(Visit<'t>)) {
	f(Visit::Word(w, pos, around));
	visit_parts(&w.parts, around, f);
}

fn visit_parts<'t>(parts: &'t [Part], around: Around, f: &mut dyn FnMut(Visit<'t>)) {
	for p in parts {
		if let Part::Subst { body: Some(b), .. } = p {
			visit(b, around, f);
		}
	}
}

fn redirect_pos(op: &str) -> Pos {
	match op {
		"<<" | "<<-" => Pos::Delimiter,
		"<<<" | "<&" | ">&" => Pos::Unknown,
		_ => Pos::Redirect,
	}
}

fn double_hole(w: &Word) -> bool {
	w.parts.iter().any(|p| {
		matches!(
			p,
			Part::Hole {
				quote: Quote::Double,
				..
			}
		)
	})
}

fn is_file_test(w: &Word) -> bool {
	w.plain().is_some_and(|t| FILE_TESTS.contains(&t.as_str()))
}

/// Whether the command runs a function the script defines -- as its first word,
/// or as the program past `sudo` and the like -- rather than what that name
/// otherwise names.
fn defines(s: &Simple, functions: &[String]) -> bool {
	let named = |k: usize| {
		s.words
			.get(k)
			.and_then(Word::plain)
			.is_some_and(|w| functions.contains(&w))
	};
	named(0) || program(s).is_some_and(named)
}

/// Which word is the program: past `sudo`, `nohup` and the like, when the word
/// after one is a command rather than an option of its own.
pub(crate) fn program(s: &Simple) -> Option<usize> {
	let mut k = 0;
	loop {
		let wraps = s.words.get(k)?.plain().is_some_and(|t| WRAPPERS.contains(&t.as_str()));
		let option = s
			.words
			.get(k + 1)
			.map(|w| w.plain().is_some_and(|t| t.starts_with('-')));
		match option {
			Some(false) if wraps => k += 1,
			_ => return Some(k),
		}
	}
}

/// Where each word of a simple command sits.
pub(crate) fn positions(s: &Simple) -> Vec<Pos> {
	let n = s.words.len();
	let mut out = vec![Pos::Unknown; n];
	let Some(k) = program(s) else {
		return out;
	};
	out[k] = Pos::Program;
	let texts: Vec<Option<String>> = s.words.iter().map(Word::plain).collect();
	let option = |j: usize| texts[j].as_deref().is_some_and(|t| t.starts_with('-'));
	match texts[k].as_deref() {
		Some(name) if FILE_COMMANDS.contains(&name) => {
			for (j, pos) in out.iter_mut().enumerate().skip(k + 1) {
				*pos = Pos::Plain { path: !option(j) };
			}
		}
		Some("grep" | "egrep" | "fgrep") => {
			for pos in out.iter_mut().skip(k + 1) {
				*pos = Pos::Plain { path: false };
			}
		}
		// `[` / `test` is a builtin, not `[[ … ]]`: it compares its already
		// expanded, inert operands as integers and never re-evaluates them, so an
		// interpolation here is safe. Only `[[ … ]]`, in the visitor, evaluates
		// operands as arithmetic.
		Some(name @ ("[" | "test")) => {
			let end = match texts[n - 1].as_deref() {
				Some("]") if name == "[" && n > k + 1 => n - 1,
				_ => n,
			};
			let paired = (k + 1..end).filter(|&j| double_hole(&s.words[j])).count() > 1;
			for (j, pos) in out.iter_mut().enumerate().take(end).skip(k + 1) {
				let path = j > k + 1 && is_file_test(&s.words[j - 1]);
				*pos = Pos::Test { path, paired };
			}
		}
		// `let` takes arithmetic; `declare -i` (and `typeset`/`local`/`readonly`
		// -i) evaluates each assignment's right-hand side as arithmetic.
		Some("let") => {
			for pos in out.iter_mut().skip(k + 1) {
				*pos = Pos::Arith;
			}
		}
		Some("declare" | "typeset" | "local" | "readonly")
			if texts
				.iter()
				.skip(k + 1)
				.any(|t| t.as_deref().is_some_and(|t| t.starts_with('-') && t.contains('i'))) =>
		{
			for (j, pos) in out.iter_mut().enumerate().skip(k + 1) {
				if !option(j) {
					*pos = Pos::Arith;
				}
			}
		}
		Some("find") => {
			let mut paths = true;
			let mut j = k + 1;
			while j < n {
				match s.words[j].literal().as_deref() {
					Some("-exec" | "-execdir" | "-ok" | "-okdir") => {
						// What it runs is not find's to read.
						while j < n && !matches!(s.words[j].literal().as_deref(), Some(";" | "+")) {
							j += 1;
						}
						paths = false;
					}
					Some("-H" | "-L" | "-P") if paths => {}
					Some(t) if t.starts_with('-') || matches!(t, "(" | ")" | "!" | ",") => {
						paths = false;
						out[j] = Pos::Plain { path: false };
					}
					_ => out[j] = Pos::Plain { path: paths },
				}
				j += 1;
			}
		}
		Some("docker" | "podman") => {
			for j in k + 1..n {
				match texts[j].as_deref() {
					// A shell's script, and everything after it, is the container's.
					Some(t) if t.starts_with('-') && !t.starts_with("--") && t.ends_with('c') => break,
					Some(t) if DOCKER_PATHS.contains(&t) && j + 1 < n => out[j + 1] = Pos::Plain { path: true },
					_ => {}
				}
			}
		}
		_ => {}
	}
	out
}

/// An interpolation in a word, and where the word sits: what the value rules
/// judge.
pub(crate) struct Place {
	pub hole: usize,
	pub quote: Quote,
	/// Whether the interpolation starts its word.
	pub first: bool,
	pub pos: Pos,
}

pub(crate) fn places(list: &List) -> Vec<Place> {
	let mut out = Vec::new();
	visit(list, Around::default(), &mut |v| {
		if let Visit::Word(w, pos, _) = v {
			for (k, p) in w.parts.iter().enumerate() {
				if let Part::Hole { hole, quote, .. } = p {
					out.push(Place {
						hole: *hole,
						quote: *quote,
						first: k == 0,
						pos,
					});
				}
			}
		}
	});
	out
}

/// Every rule that reads commands, over one script.
pub(crate) fn run(list: &List, script: &Script, src: &str) -> Vec<Found> {
	let mut functions = Vec::new();
	visit(list, Around::default(), &mut |v| {
		if let Visit::Function(name) = v {
			functions.push(name.to_string());
		}
	});
	let text = Texts { script, src };
	let mut out = Vec::new();
	visit(list, Around::default(), &mut |v| match v {
		Visit::Word(w, pos, _) => {
			quoted_interpolation(w, pos, script, text.of(w), &mut out);
			tilde_in_quotes(w, pos, text.of(w), &mut out);
			dropped_backslash(w, pos, text.of(w), &mut out);
		}
		Visit::Command(s, around) => {
			// A command the script defines as a function is that function, whatever
			// it is called: `test a=b` calling a `test()` compares nothing.
			if defines(s, &functions) {
				return;
			}
			windows_command(s, &text, &mut out);
			bracket_spacing(s, &text, &mut out);
			missing_bracket(s, &text, &mut out);
			test_operands(s, &text, &mut out);
			test_redirect(s, &text, &mut out);
			outside_function(s, around, &mut out);
			outside_loop(s, around, &mut out);
			spaced_assignment(s, around, &functions, &text, &mut out);
			dollar_assignment(s, &text, &mut out);
			sudo_builtin(s, &mut out);
			unterminated_exec(s, &mut out);
			truncated_input(s, &text, &mut out);
		}
		Visit::Test(words) => {
			if let [w] = words {
				glued_comparison(w, &text, &mut out);
			}
		}
		Visit::ForName(w) => for_name(w, &text, &mut out),
		Visit::Arith(..) | Visit::Function(_) => {}
	});
	positional_parameter(list, script, &mut out);
	lost_effect(list, script, &mut out);
	out
}

fn quoted_interpolation(w: &Word, pos: Pos, script: &Script, text: &str, out: &mut Vec<Found>) {
	let judged = match pos {
		Pos::Program | Pos::Redirect | Pos::Plain { .. } => true,
		Pos::Test { paired, .. } => !paired,
		_ => false,
	};
	let quote = w.parts.iter().find_map(|p| match p {
		Part::Hole {
			quote: q @ (Quote::Single | Quote::Ansi),
			..
		} => Some(*q),
		Part::Hole {
			quote: Quote::Double, ..
		} if judged => Some(Quote::Double),
		_ => None,
	});
	let Some(quote) = quote else {
		return;
	};
	let start = script.bytes(w.start, w.end).0;
	let holes: Vec<(usize, usize)> = w
		.parts
		.iter()
		.filter_map(|p| match p {
			Part::Hole { hole, .. } => {
				let h = &script.holes[*hole];
				Some((h.start - start, h.end - start))
			}
			_ => None,
		})
		.collect();
	let fixed = unquoted(text, &holes);
	let message = match quote {
		Quote::Double => format!(
			"`{text}` puts an interpolation inside `\"…\"`, but an interpolation is already one quoted word: a value with a space or a quote in it would arrive with quotes of its own in it"
		),
		_ => format!(
			"`{text}` puts an interpolation inside `'…'`, but an interpolation is already one quoted word: a value with a space or a quote in it would end these quotes early and be split apart"
		),
	};
	out.push(Found {
		rule: "quoted-interpolation",
		from: w.start,
		to: w.end,
		message,
		fix: (fixed != text).then(|| format!("write `{fixed}`")),
	});
}

/// `text` with every interpolation inside quotes taken out of them: the quotes
/// are closed before it and opened again after, and what that leaves empty or
/// needlessly quoted is tidied away.
pub(crate) fn unquoted(text: &str, holes: &[(usize, usize)]) -> String {
	#[derive(Clone, Copy, PartialEq)]
	enum St {
		Bare,
		Single,
		Double,
		Ansi,
	}
	// Quoted segments carry whether splitting made them, since only those may
	// be tidied: an author's own `''` is an empty argument.
	enum Seg {
		Text(String),
		Quoted(St, String, bool),
	}
	let mut segs = vec![Seg::Text(String::new())];
	let mut st = St::Bare;
	let mut k = 0;
	let push = |segs: &mut Vec<Seg>, s: &str| match segs.last_mut() {
		Some(Seg::Text(t)) | Some(Seg::Quoted(_, t, _)) => t.push_str(s),
		None => {}
	};
	while k < text.len() {
		if let Some(&(from, to)) = holes.iter().find(|(from, _)| *from == k) {
			if st == St::Bare {
				push(&mut segs, &text[from..to]);
			} else {
				if let Some(Seg::Quoted(_, _, split)) = segs.last_mut() {
					*split = true;
				}
				segs.push(Seg::Text(text[from..to].to_string()));
				segs.push(Seg::Quoted(st, String::new(), true));
			}
			k = to;
			continue;
		}
		let c = text[k..].chars().next().unwrap_or_default();
		let next = text[k + c.len_utf8()..].chars().next();
		match (st, c) {
			(St::Bare, '\'') => {
				st = St::Single;
				segs.push(Seg::Quoted(st, String::new(), false));
			}
			(St::Bare, '"') => {
				st = St::Double;
				segs.push(Seg::Quoted(st, String::new(), false));
			}
			(St::Bare, '$') if next == Some('\'') => {
				st = St::Ansi;
				segs.push(Seg::Quoted(st, String::new(), false));
				k += 2;
				continue;
			}
			(St::Single | St::Ansi, '\'') | (St::Double, '"') => {
				st = St::Bare;
				segs.push(Seg::Text(String::new()));
			}
			(St::Bare | St::Double | St::Ansi, '\\') => {
				let end = k + 1 + next.map_or(0, char::len_utf8);
				push(&mut segs, &text[k..end]);
				k = end;
				continue;
			}
			_ => push(&mut segs, &text[k..k + c.len_utf8()]),
		}
		k += c.len_utf8();
	}
	let safe = |s: &str| s.chars().all(|c| c.is_ascii_alphanumeric() || "_-./:=@+,".contains(c));
	segs.iter()
		.map(|seg| match seg {
			Seg::Text(t) => t.clone(),
			Seg::Quoted(_, inner, true) if inner.is_empty() || safe(inner) => inner.clone(),
			Seg::Quoted(St::Double, inner, _) => format!("\"{inner}\""),
			Seg::Quoted(St::Ansi, inner, _) => format!("$'{inner}'"),
			Seg::Quoted(_, inner, _) => format!("'{inner}'"),
		})
		.collect()
}

fn tilde_in_quotes(w: &Word, pos: Pos, text: &str, out: &mut Vec<Found>) {
	let path = matches!(
		pos,
		Pos::Program | Pos::Redirect | Pos::Plain { path: true } | Pos::Test { path: true, .. }
	);
	let Some(Part::Char {
		c: '~',
		quote: quote @ (Quote::Single | Quote::Double),
		..
	}) = w.parts.first()
	else {
		return;
	};
	let home = match w.parts.get(1) {
		None => true,
		Some(Part::Char { c: '/', quote: q, .. }) => q == quote,
		_ => false,
	};
	if !path || !home {
		return;
	}
	let fix = match quote {
		Quote::Double => text.strip_prefix("\"~").map(|rest| format!("\"$HOME{rest}")),
		_ => text.strip_prefix("'~/").map(|rest| match rest.strip_prefix('\'') {
			Some(after) => format!("~/{after}"),
			None => format!("~/'{rest}"),
		}),
	};
	out.push(Found {
		rule: "tilde-in-quotes",
		from: w.start,
		to: w.end,
		message: format!(
			"`~` is not expanded inside quotes, so `{text}` names a directory called `~` rather than the home directory"
		),
		fix: fix.map(|f| format!("write `{f}`")),
	});
}

fn dropped_backslash(w: &Word, pos: Pos, text: &str, out: &mut Vec<Found>) {
	if matches!(pos, Pos::Program | Pos::Test { .. } | Pos::Pattern | Pos::Delimiter) {
		return;
	}
	let Some((c, i)) = w.parts.iter().find_map(|p| match p {
		Part::Char {
			c,
			i,
			quote: Quote::Bare,
			escaped: true,
		} if c.is_ascii_alphanumeric() => Some((*c, *i)),
		_ => None,
	}) else {
		return;
	};
	let arrives = match w.literal() {
		Some(l) if l.chars().count() > 1 => format!(", and `{text}` arrives as `{l}`"),
		_ => String::new(),
	};
	// Single quotes keep every backslash, which is what was meant -- as long as
	// no backslash in the word was doing a job.
	let simple = !text.contains('\'')
		&& w.parts.iter().all(|p| match p {
			Part::Char {
				quote: Quote::Bare,
				escaped,
				c,
				..
			} => !escaped || c.is_ascii_alphanumeric(),
			_ => false,
		});
	out.push(Found {
		rule: "dropped-backslash",
		from: i - 1,
		to: i + 1,
		message: format!(
			"bash drops this backslash, so `\\{c}` is just `{c}`{arrives} -- outside quotes a backslash only stops the next character being special, and `{c}` never is"
		),
		fix: simple.then(|| format!("write `'{text}'`")),
	});
}

fn windows_command(s: &Simple, text: &Texts, out: &mut Vec<Found>) {
	let Some(k) = program(s) else {
		return;
	};
	let Some(name) = s.words[k].plain() else {
		return;
	};
	let is_switch = |w: &Word| {
		w.plain().is_some_and(|t| {
			let b = t.as_bytes();
			b.len() == 2 && b[0] == b'/' && (b[1].is_ascii_alphabetic() || b[1] == b'?')
		})
	};
	let args = &s.words[k + 1..];
	let switches: Vec<&str> = args.iter().filter(|w| is_switch(w)).map(|w| text.of(w)).collect();
	let Some(first) = switches.first() else {
		return;
	};
	let recursive = switches.iter().any(|t| t.eq_ignore_ascii_case("/s"));
	let unix = match name.to_ascii_lowercase().as_str() {
		"rmdir" | "rd" if recursive => "rm -rf",
		"rmdir" | "rd" => "rmdir",
		"del" | "erase" => "rm -f",
		"copy" => "cp",
		"xcopy" => "cp -r",
		"move" | "ren" => "mv",
		_ => return,
	};
	let operands: Vec<String> = args
		.iter()
		.filter(|w| !is_switch(w))
		.map(|w| text.of(w).replace('\\', "/"))
		.collect();
	let written = std::iter::once(name.as_str())
		.chain(switches.iter().copied())
		.collect::<Vec<_>>()
		.join(" ");
	out.push(Found {
		rule: "windows-command",
		from: s.words[k].start,
		to: s.words.last().map_or(s.words[k].end, |w| w.end),
		message: format!(
			"`{written}` is cmd.exe's syntax, but a `$` line runs in a POSIX shell on every system -- Git Bash on Windows -- where `{first}` is read as a path"
		),
		fix: Some(format!("write `{unix} {}`", operands.join(" "))),
	});
}

fn bracket_spacing(s: &Simple, text: &Texts, out: &mut Vec<Found>) {
	let Some(first) = s.words.first() else {
		return;
	};
	let Some(head) = first.plain() else {
		return;
	};
	if head == "[" {
		let Some(last) = s.words.last().filter(|_| s.words.len() > 1) else {
			return;
		};
		let glued = matches!(
			last.parts.last(),
			Some(Part::Char {
				c: ']',
				quote: Quote::Bare,
				escaped: false,
				..
			})
		) && last.parts.len() > 1;
		if glued {
			let t = text.of(last);
			out.push(Found {
				rule: "bracket-spacing",
				from: last.start,
				to: last.end,
				message: format!(
					"`[` needs a blank before its closing `]`: `{t}` is one word, so `[` finds no `]` and fails"
				),
				fix: Some(format!("write `{} ]`", &t[..t.len() - 1])),
			});
		}
		return;
	}
	let bracket = if head.starts_with("[[") { "[[" } else { "[" };
	let rest = &head[bracket.len()..];
	if rest.starts_with(['-', '!']) || rest.starts_with(|c: char| c.is_ascii_alphabetic()) && head.ends_with(']') {
		out.push(Found {
			rule: "bracket-spacing",
			from: first.start,
			to: first.end,
			message: format!(
				"`{head}` is one word, so it runs a command by that name, and there is none: `{bracket}` needs a blank after it"
			),
			fix: Some(format!("write `{bracket} {rest}`")),
		});
	}
}

fn test_redirect(s: &Simple, text: &Texts, out: &mut Vec<Found>) {
	let [first, .., last] = s.words.as_slice() else {
		return;
	};
	if first.plain().as_deref() != Some("[") || last.plain().as_deref() != Some("]") {
		return;
	}
	for r in &s.redirects {
		// `2>` inside a test is a real redirection, and not this mistake.
		let bare = r.target.start > r.i && matches!(r.op, ">" | "<");
		if !bare || r.i < first.end || r.i > last.start {
			continue;
		}
		let target = text.of(&r.target);
		let (verb, numbers, words) = match r.op {
			">" => ("writes", "-gt", "\\>"),
			_ => ("reads", "-lt", "\\<"),
		};
		out.push(Found {
			rule: "test-redirect",
			from: r.i,
			to: r.target.end,
			message: format!(
				"inside `[ … ]`, `{} {target}` is a redirection, not a comparison: it {verb} a file named `{target}`",
				r.op
			),
			fix: Some(format!("compare numbers with `{numbers}`, or text with `{words}`")),
		});
	}
}

fn outside_function(s: &Simple, around: Around, out: &mut Vec<Found>) {
	let Some(w) = s.words.first() else {
		return;
	};
	if around.function {
		return;
	}
	let (message, fix) = match w.plain().as_deref() {
		Some("local") => (
			"`local` only works inside a shell function, and outside one bash fails the script here",
			"write the assignment without `local`",
		),
		Some("return") => (
			"`return` leaves a shell function, and outside one bash fails the script here",
			"`exit` ends the script",
		),
		_ => return,
	};
	out.push(Found {
		rule: "outside-function",
		from: w.start,
		to: w.end,
		message: message.into(),
		fix: Some(fix.into()),
	});
}

fn spaced_assignment(s: &Simple, around: Around, functions: &[String], text: &Texts, out: &mut Vec<Found>) {
	if !s.assigns.is_empty() {
		return;
	}
	let declares = s
		.words
		.first()
		.and_then(Word::plain)
		.filter(|b| matches!(b.as_str(), "export" | "declare" | "typeset" | "readonly" | "local"));
	if let Some(builtin) = declares {
		// Outside a function, `local` fails whatever follows it, and
		// `outside-function` says so.
		if builtin != "local" || around.function {
			declared_with_blanks(&builtin, &s.words[1..], text, out);
		}
		return;
	}
	let [name, eq, rest @ ..] = s.words.as_slice() else {
		return;
	};
	let (Some(n), Some(e)) = (name.plain(), eq.plain()) else {
		return;
	};
	let shaped = n.starts_with(|c: char| c.is_ascii_uppercase() || c == '_')
		&& n.chars()
			.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
	if !shaped || !e.starts_with('=') || e.starts_with("==") || functions.contains(&n) {
		return;
	}
	let value = match e.as_str() {
		"=" => rest.first().map_or("", |w| text.of(w)),
		_ => &text.of(eq)[1..],
	};
	out.push(Found {
		rule: "spaced-assignment",
		from: name.start,
		to: eq.end,
		message: format!("`{n} =` runs a command named `{n}`: an assignment has no blanks around its `=`"),
		fix: Some(format!("write `{n}={value}`")),
	});
}

fn sudo_builtin(s: &Simple, out: &mut Vec<Found>) {
	let [first, second, ..] = s.words.as_slice() else {
		return;
	};
	if first.plain().as_deref() != Some("sudo") {
		return;
	}
	let Some(name) = second.plain() else {
		return;
	};
	const BUILTINS: &[&str] = &[
		".", "alias", "cd", "declare", "exit", "export", "local", "popd", "pushd", "readonly", "set", "shopt",
		"source", "typeset", "ulimit", "umask", "unset",
	];
	if !BUILTINS.contains(&name.as_str()) {
		return;
	}
	out.push(Found {
		rule: "sudo-builtin",
		from: first.start,
		to: second.end,
		message: format!(
			"`{name}` is part of the shell rather than a program, so `sudo {name}` has nothing to run -- and could not change this shell if it did"
		),
		fix: None,
	});
}

/// Whether a `set` hands the shell positional parameters of its own.
fn sets_arguments(s: &Simple) -> bool {
	s.words.first().and_then(Word::plain).as_deref() == Some("set") && !only_options(&s.words[1..])
}

/// Whether the arguments of a `set` are all options: `-e`, `+x`, and `-o name`
/// or a cluster ending in `o` -- `-euo pipefail` -- with the name after it.
fn only_options(args: &[Word]) -> bool {
	let mut words = args.iter();
	while let Some(w) = words.next() {
		match w.plain().as_deref() {
			Some(t) if t.starts_with(['-', '+']) && t != "--" && t != "-" => {
				if t.ends_with('o') {
					words.next();
				}
			}
			_ => return false,
		}
	}
	true
}

fn positional_parameter(list: &List, script: &Script, out: &mut Vec<Found>) {
	if script.arguments {
		return;
	}
	let mut sets = false;
	let mut found: Vec<(String, usize, usize)> = Vec::new();
	let mut read = |parts: &[Part]| {
		for p in parts {
			if let Part::Param { name, i, end, .. } = p {
				let numbered = name.chars().all(|c| c.is_ascii_digit()) && name != "0";
				if numbered || matches!(name.as_str(), "@" | "*" | "#") {
					found.push((name.clone(), *i, *end));
				}
			}
		}
	};
	visit(list, Around::default(), &mut |v| match v {
		Visit::Command(s, _) => sets |= sets_arguments(s),
		Visit::Word(w, _, around) if !around.function => read(&w.parts),
		Visit::Arith(parts, around) if !around.function => read(parts),
		_ => {}
	});
	if sets {
		return;
	}
	for (name, from, to) in found {
		let (what, instead) = match name.as_str() {
			"@" | "*" => (
				format!("`${name}` is empty"),
				"pass the target's own arguments with `{{ ARGS }}`".to_string(),
			),
			"#" => (
				"`$#` is always 0".into(),
				"`{{ length(ARGS) }}` counts the target's own".into(),
			),
			n => {
				let index = n.parse::<usize>().unwrap_or(1).saturating_sub(1);
				(
					format!("`${n}` is empty"),
					format!("the target's own are `{{{{ ARGS[{index}] }}}}`"),
				)
			}
		};
		out.push(Found {
			rule: "positional-parameter",
			from,
			to,
			message: format!("{what}: this shell is given no arguments of its own"),
			fix: Some(instead),
		});
	}
}

fn lost_effect(list: &List, script: &Script, out: &mut Vec<Found>) {
	if script.used != Use::Statement {
		return;
	}
	let Some(item) = list.last().filter(|item| !item.background) else {
		return;
	};
	let [pipeline] = item.pipelines.as_slice() else {
		return;
	};
	let [Command::Simple(s)] = pipeline.commands.as_slice() else {
		return;
	};
	let substituted = s
		.assigns
		.iter()
		.chain(&s.words)
		.any(|w| w.parts.iter().any(|p| matches!(p, Part::Subst { .. })));
	if !s.redirects.is_empty() || substituted {
		return;
	}
	let Some((gone, fix)) = lost(s) else {
		return;
	};
	let from = s.assigns.first().or(s.words.first()).map_or(0, |w| w.start);
	let to = s.words.last().or(s.assigns.last()).map_or(from, |w| w.end);
	let what = match s.words.first().and_then(Word::plain) {
		Some(name) => format!("`{name}`"),
		None => "this assignment".into(),
	};
	out.push(Found {
		rule: "lost-effect",
		from,
		to,
		message: format!("{what} is the last command this shell runs, so {gone} the moment it has run"),
		fix: Some(fix.into()),
	});
}

/// What a command changes that ends with its shell, if that is all it does.
fn lost(s: &Simple) -> Option<(&'static str, &'static str)> {
	let assigned = "what it sets is gone";
	let env = "set `.env.NAME` instead, which every command below it receives";
	let Some(first) = s.words.first() else {
		return Some((assigned, env));
	};
	let args = &s.words[1..];
	let texts: Vec<Option<String>> = args.iter().map(Word::plain).collect();
	let identifier = |t: &str| {
		t.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
			&& t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
	};
	match first.plain()?.as_str() {
		"cd" => {
			let operands = texts
				.iter()
				.filter(|t| !t.as_deref().is_some_and(|t| t.starts_with('-')))
				.count();
			let dash = texts.iter().any(|t| t.as_deref() == Some("-"));
			(operands <= 1 && !dash).then_some((
				"the directory it changes to is gone",
				"put the commands that need it on `$` lines directly below it -- adjacent `$` lines share one shell -- or set `.workdir` on a `do` block around them",
			))
		}
		"export" => {
			let all = !args.is_empty()
				&& args
					.iter()
					.zip(&texts)
					.all(|(w, t)| is_assignment(w) || t.as_deref().is_some_and(identifier));
			all.then_some((assigned, env))
		}
		"unset" => (!args.is_empty() && texts.iter().all(|t| t.as_deref().is_some_and(identifier)))
			.then_some(("nothing it unsets is ever read again", "remove it")),
		"set" => {
			let options = !args.is_empty() && only_options(args);
			options.then_some((
				"the options it sets are gone",
				"put it at the top of the `$` lines it is for",
			))
		}
		"shopt" => {
			let changes = texts.iter().any(|t| matches!(t.as_deref(), Some("-s" | "-u")))
				&& !texts.iter().any(|t| matches!(t.as_deref(), Some("-p" | "-q")));
			changes.then_some((
				"the options it sets are gone",
				"put it at the top of the `$` lines it is for",
			))
		}
		"umask" => (texts.len() == 1 && !texts[0].as_deref().is_some_and(|t| t.starts_with('-')))
			.then_some(("the mask it sets is gone", "put it above the `$` lines it is for")),
		"alias" => (!args.is_empty() && args.iter().all(is_assignment)).then_some((
			"the alias is gone",
			"remove it: an alias is not used by a script anyway",
		)),
		_ => None,
	}
}

/// The word's text once its quotes are removed, when that is known exactly:
/// nothing in it expands, and no `$'…'` escape has to be worked out.
fn exact(w: &Word) -> Option<String> {
	w.parts
		.iter()
		.map(|p| match p {
			Part::Char {
				c,
				quote: Quote::Bare | Quote::Single | Quote::Double,
				..
			} => Some(*c),
			_ => None,
		})
		.collect()
}

fn identifier(t: &str) -> bool {
	t.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
		&& t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `export NAME = value` and its kin: a word starting with `=` handed to a
/// builtin that takes names.
fn declared_with_blanks(builtin: &str, args: &[Word], text: &Texts, out: &mut Vec<Found>) {
	for (k, name) in args.iter().enumerate() {
		let Some(eq) = args.get(k + 1) else {
			return;
		};
		let Some(n) = name.plain().filter(|n| identifier(n)) else {
			continue;
		};
		let starts = matches!(
			eq.parts.first(),
			Some(Part::Char {
				c: '=',
				quote: Quote::Bare,
				escaped: false,
				..
			})
		);
		if !starts {
			continue;
		}
		let written = text.of(eq);
		let value = match written {
			"=" => args.get(k + 2).map_or("", |w| text.of(w)),
			_ => &written[1..],
		};
		out.push(Found {
			rule: "spaced-assignment",
			from: name.start,
			to: eq.end,
			message: format!(
				"`{builtin} {n} {written}` hands `{builtin}` the word `{written}`, which is not a name, so it fails: an assignment has no blanks around its `=`"
			),
			fix: Some(format!("write `{n}={value}`")),
		});
	}
}

/// `$NAME=value`, which expands `NAME` and runs what it holds.
fn dollar_assignment(s: &Simple, text: &Texts, out: &mut Vec<Found>) {
	if !s.assigns.is_empty() {
		return;
	}
	let Some(w) = s.words.first() else {
		return;
	};
	let [
		Part::Param { name, .. },
		Part::Char {
			c: '=',
			quote: Quote::Bare,
			escaped: false,
			..
		},
		..,
	] = w.parts.as_slice()
	else {
		return;
	};
	let t = text.of(w);
	let written = [format!("${name}="), format!("${{{name}}}=")];
	let Some(rest) = written.iter().find_map(|p| t.strip_prefix(p.as_str())) else {
		return;
	};
	if !identifier(name) {
		return;
	}
	out.push(Found {
		rule: "dollar-assignment",
		from: w.start,
		to: w.end,
		message: format!(
			"`{t}` is not an assignment: the `$` reads `{name}`, so this runs a command named after what `{name}` holds"
		),
		fix: Some(format!("write `{name}={rest}`")),
	});
}

/// `for $i in …`: the name a loop sets, written as a read of it.
fn for_name(w: &Word, text: &Texts, out: &mut Vec<Found>) {
	let [Part::Param { name, .. }] = w.parts.as_slice() else {
		return;
	};
	let t = text.of(w);
	if !identifier(name) || (t != format!("${name}") && t != format!("${{{name}}}")) {
		return;
	}
	out.push(Found {
		rule: "dollar-assignment",
		from: w.start,
		to: w.end,
		message: format!(
			"the name a loop sets is written without `$`: `{t}` reads `{name}` instead, and bash refuses the loop every time"
		),
		fix: Some(format!("write `{name}`")),
	});
}

fn outside_loop(s: &Simple, around: Around, out: &mut Vec<Found>) {
	if around.function || around.looped {
		return;
	}
	let Some(w) = s.words.first() else {
		return;
	};
	let (word, does, instead) = match w.plain().as_deref() {
		Some("break") => ("break", "leaves", "leave"),
		Some("continue") => ("continue", "goes on to the next pass of", "go on to the next pass of"),
		_ => return,
	};
	out.push(Found {
		rule: "outside-loop",
		from: w.start,
		to: w.end,
		message: format!(
			"`{word}` {does} a shell loop, and there is none around it in this script, so bash complains and carries on as though it were not there -- a shell cannot see the runfile's own loops"
		),
		fix: Some(format!(
			"to {instead} a loop of the runfile, write `{word}` on a line of its own, without `$`"
		)),
	});
}

fn missing_bracket(s: &Simple, text: &Texts, out: &mut Vec<Found>) {
	let [first, rest @ ..] = s.words.as_slice() else {
		return;
	};
	if first.plain().as_deref() != Some("[") {
		return;
	}
	let last = rest.last();
	if let Some(last) = last {
		// Something that expands could be the `]`.
		if exact(last).is_none_or(|t| t == "]") {
			return;
		}
		// A `]` against the word before it is `bracket-spacing`'s to report.
		let glued = matches!(
			last.parts.last(),
			Some(Part::Char {
				c: ']',
				quote: Quote::Bare,
				escaped: false,
				..
			})
		);
		if glued {
			return;
		}
	}
	let (to, ends, fix) = match last {
		Some(w) => (
			w.end,
			format!("ends with `{}`", text.of(w)),
			format!("write `{} ]`", text.of(w)),
		),
		None => (
			first.end,
			"has nothing after it".to_string(),
			"write what to test after it, and `]` last".to_string(),
		),
	};
	out.push(Found {
		rule: "missing-bracket",
		from: first.start,
		to,
		message: format!(
			"`[` is a command whose last argument has to be `]`, and this one {ends}, so it fails every time it runs"
		),
		fix: Some(fix),
	});
}

/// The operands of `[ … ]` or `test …`, when that is the command.
fn test_args(s: &Simple) -> Option<(&'static str, &[Word])> {
	let [first, rest @ ..] = s.words.as_slice() else {
		return None;
	};
	match first.plain().as_deref() {
		Some("test") => Some(("test", rest)),
		Some("[") => match rest.split_last() {
			Some((last, inner)) if exact(last).as_deref() == Some("]") => Some(("[", inner)),
			_ => None,
		},
		_ => None,
	}
}

fn test_operands(s: &Simple, text: &Texts, out: &mut Vec<Found>) {
	let Some((program, args)) = test_args(s) else {
		return;
	};
	match args {
		[w] => glued_comparison(w, text, out),
		[flag, w] if flag.plain().as_deref() == Some("-n") => vanishing_operand(w, text, out),
		[a, op, b] => match op.plain().as_deref() {
			Some("=~") => out.push(Found {
				rule: "test-regex",
				from: op.start,
				to: op.end,
				message: format!(
					"`=~` matches a regex only inside `[[ … ]]`: to `{program}` it is not an operator, so this fails every time it runs"
				),
				fix: Some("write `[[ … =~ … ]]`, with the pattern unquoted".into()),
			}),
			Some(o @ ("-eq" | "-ne" | "-lt" | "-le" | "-gt" | "-ge")) => {
				for w in [a, b] {
					not_a_number(o, w, text, out);
				}
			}
			_ => {}
		},
		_ => {}
	}
}

/// One word tested on its own, holding an `=`: `[ "$a"="$b" ]`.
fn glued_comparison(w: &Word, text: &Texts, out: &mut Vec<Found>) {
	let written = |j: usize, want: char, bare: bool| {
		matches!(
			w.parts.get(j),
			Some(Part::Char { c, quote, escaped, .. })
				if *c == want && (if bare { *quote == Quote::Bare && !*escaped } else { *quote != Quote::Opaque })
		)
	};
	let Some(k) = (0..w.parts.len()).find(|&j| written(j, '=', false)) else {
		return;
	};
	let t = text.of(w);
	if matches!(t, "=" | "==" | "!=") {
		return;
	}
	// Inside `[[ … ]]` these split a word, so it is not the one bash sees.
	let split = w.parts.iter().any(|p| {
		matches!(
			p,
			Part::Char {
				c: '(' | ')' | '<' | '>' | '&' | '|',
				quote: Quote::Bare,
				escaped: false,
				..
			}
		)
	});
	if split {
		return;
	}
	// The operator, when it is written outside quotes, with blanks around it.
	let fix = written(k, '=', true).then(|| {
		let from = if k > 0 && written(k - 1, '!', true) { k - 1 } else { k };
		let to = if written(k + 1, '=', true) { k + 1 } else { k };
		let at = |j: usize| match &w.parts[j] {
			Part::Char { i, .. } => *i,
			_ => w.start,
		};
		let start = text.script.bytes(w.start, w.end).0;
		let a = text.script.bytes(at(from), at(from) + 1).0 - start;
		let b = text.script.bytes(at(to), at(to) + 1).1 - start;
		(a, b)
	});
	let fix = fix
		.filter(|(a, b)| *a > 0 && *b < t.len())
		.map(|(a, b)| format!("write `{} {} {}`", &t[..a], &t[a..b], &t[b..]));
	out.push(Found {
		rule: "glued-comparison",
		from: w.start,
		to: w.end,
		message: format!(
			"`{t}` is one word, and a test of one word asks only whether it is empty -- which this never is, so the test is always true"
		),
		fix,
	});
}

/// `[ -n $x ]`, where an empty `$x` leaves no word to test.
fn vanishing_operand(w: &Word, text: &Texts, out: &mut Vec<Found>) {
	let t = text.of(w);
	let unquoted = match w.parts.as_slice() {
		[Part::Param { name, .. }] => identifier(name) && (t == format!("${name}") || t == format!("${{{name}}}")),
		[Part::Subst { .. }] => t.starts_with("$(") || t.starts_with('`'),
		_ => false,
	};
	if !unquoted {
		return;
	}
	out.push(Found {
		rule: "vanishing-operand",
		from: w.start,
		to: w.end,
		message: format!(
			"unquoted, `{t}` leaves no word at all when it is empty, and `-n` with nothing after it asks only whether `-n` is empty -- so this is true whether or not `{t}` is"
		),
		fix: Some(format!("write `\"{t}\"`")),
	});
}

/// An operand of `-eq` and the like that is written out, and is not a whole number.
fn not_a_number(op: &str, w: &Word, text: &Texts, out: &mut Vec<Found>) {
	let Some(value) = exact(w) else {
		return;
	};
	// What bash's `[` reads as one: blanks, a sign, digits, blanks.
	let trimmed = value
		.trim_start_matches([' ', '\t', '\n', '\r', '\u{b}', '\u{c}'])
		.trim_end_matches([' ', '\t']);
	let digits = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
	if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
		return;
	}
	let fix = match op {
		"-eq" => Some("compare text with `=`".to_string()),
		"-ne" => Some("compare text with `!=`".to_string()),
		_ => None,
	};
	out.push(Found {
		rule: "not-a-number",
		from: w.start,
		to: w.end,
		message: format!(
			"`{op}` compares whole numbers, and `{}` is not one, so the test fails every time it runs",
			text.of(w)
		),
		fix,
	});
}

/// Primaries of `find` that take no value, so the word after one is not its value.
const FIND_BARE: &[&str] = &[
	"!",
	"(",
	")",
	",",
	"-a",
	"-and",
	"-o",
	"-or",
	"-not",
	"-print",
	"-print0",
	"-ls",
	"-delete",
	"-prune",
	"-quit",
	"-true",
	"-false",
	"-empty",
	"-depth",
	"-xdev",
	"-mount",
	"-follow",
	"-noleaf",
	"-readable",
	"-writable",
	"-executable",
	"-nouser",
	"-nogroup",
	"-daystart",
	"-H",
	"-L",
	"-P",
];

/// `find … -exec` with no `;` or `+` to end what it runs.
fn unterminated_exec(s: &Simple, out: &mut Vec<Found>) {
	let Some(k) = program(s) else {
		return;
	};
	if s.words[k].plain().as_deref() != Some("find") {
		return;
	}
	let words = &s.words[k + 1..];
	let texts: Vec<Option<String>> = words.iter().map(exact).collect();
	let mut j = 0;
	while j < words.len() {
		let Some(action @ ("-exec" | "-execdir" | "-ok" | "-okdir")) = texts[j].as_deref() else {
			j += 1;
			continue;
		};
		// An action only where a primary can start: the word before one that
		// takes a value is that value.
		let starts = match j.checked_sub(1).map(|p| texts[p].as_deref()) {
			None => true,
			Some(None) => false,
			Some(Some(p)) if p.starts_with('-') || matches!(p, "!" | "(" | ")" | ",") => FIND_BARE.contains(&p),
			Some(Some(_)) => j < 2 || texts[j - 2].as_deref() != Some("-fprintf"),
		};
		if !starts {
			j += 1;
			continue;
		}
		let plus = action.starts_with("-exec");
		let mut unknown = false;
		let mut end = None;
		for m in j + 1..words.len() {
			match texts[m].as_deref() {
				None => unknown = true,
				Some(";") => {
					end = Some(m);
					break;
				}
				Some("+") if plus && m > j + 1 && texts[m - 1].as_deref() == Some("{}") => {
					end = Some(m);
					break;
				}
				_ => {}
			}
		}
		match end {
			Some(m) => j = m + 1,
			// What expands could be the `;`.
			None if unknown => return,
			None => {
				let ends = if plus { "a `;` or a `{} +`" } else { "a `;`" };
				out.push(Found {
					rule: "unterminated-exec",
					from: words[j].start,
					to: words[j].end,
					message: format!(
						"`{action}` takes the words after it up to {ends}, and nothing here ends it, so `find` refuses to run"
					),
					fix: Some(format!(
						"end it with `\\;` -- a bare `;` ends the shell's command rather than `{action}`"
					)),
				});
				return;
			}
		}
	}
}

/// How a command that reads files takes its options and its operands, as far
/// as it is followed here. An option not listed makes the reading give up.
#[derive(Clone, Copy)]
struct Reader {
	names: &'static [&'static str],
	/// Short options that take a value, in the same word or the next.
	values: &'static str,
	/// Short options that take none.
	flags: &'static str,
	/// Whether `-5` is an option, as a count is to `head`.
	digits: bool,
	/// Long options that take a value, after `=` or in the next word.
	long_values: &'static [&'static str],
	/// Long options that take two, in the next two words: jq's `--arg`.
	long_pairs: &'static [&'static str],
	/// Long options that take none, or one only after `=`.
	long_flags: &'static [&'static str],
	/// Options that change what is read, or where the output goes, in a way
	/// this does not follow: with one of them, nothing is said.
	gives_up: &'static [&'static str],
	/// The options that give a program or a pattern, which is otherwise the
	/// first operand; `None` for a command that takes neither.
	script: Option<&'static [&'static str]>,
	/// Only the first operand is read: `uniq in out`.
	first_only: bool,
	/// No operand names a file: `tr`, `envsubst`.
	no_files: bool,
	/// An operand `name=value` sets a variable rather than naming a file.
	assignments: bool,
}

const NOTHING: Reader = Reader {
	names: &[],
	values: "",
	flags: "",
	digits: false,
	long_values: &[],
	long_pairs: &[],
	long_flags: &[],
	gives_up: &[],
	script: None,
	first_only: false,
	no_files: false,
	assignments: false,
};

const READERS: &[Reader] = &[
	Reader {
		names: &["cat"],
		flags: "AbeEnstTuvl",
		long_flags: &[
			"show-all",
			"number-nonblank",
			"show-ends",
			"number",
			"squeeze-blank",
			"show-tabs",
			"show-nonprinting",
		],
		..NOTHING
	},
	Reader {
		names: &["sort"],
		values: "ktST",
		flags: "bdfgiMhnRrVcCmsuz",
		long_values: &[
			"key",
			"field-separator",
			"buffer-size",
			"temporary-directory",
			"parallel",
			"batch-size",
			"compress-program",
			"random-source",
			"sort",
		],
		long_flags: &[
			"ignore-leading-blanks",
			"dictionary-order",
			"ignore-case",
			"general-numeric-sort",
			"ignore-nonprinting",
			"month-sort",
			"human-numeric-sort",
			"numeric-sort",
			"random-sort",
			"reverse",
			"version-sort",
			"check",
			"merge",
			"stable",
			"unique",
			"zero-terminated",
			"debug",
		],
		gives_up: &["-o", "--output", "--files0-from"],
		..NOTHING
	},
	Reader {
		names: &["head"],
		values: "nc",
		flags: "qvz",
		digits: true,
		long_values: &["lines", "bytes"],
		long_flags: &["quiet", "silent", "verbose", "zero-terminated"],
		..NOTHING
	},
	Reader {
		names: &["tail"],
		values: "ncs",
		flags: "fFqvzr",
		digits: true,
		long_values: &["lines", "bytes", "sleep-interval", "pid", "max-unchanged-stats"],
		long_flags: &["follow", "retry", "quiet", "silent", "verbose", "zero-terminated"],
		..NOTHING
	},
	Reader {
		names: &["grep", "egrep", "fgrep"],
		values: "efmABC",
		flags: "EFGPiywxvclLoqsbHhnTZzaIUu",
		digits: true,
		long_values: &[
			"regexp",
			"file",
			"max-count",
			"after-context",
			"before-context",
			"context",
			"label",
			"binary-files",
			"group-separator",
		],
		long_flags: &[
			"extended-regexp",
			"fixed-strings",
			"basic-regexp",
			"perl-regexp",
			"ignore-case",
			"no-ignore-case",
			"word-regexp",
			"line-regexp",
			"null-data",
			"no-messages",
			"invert-match",
			"byte-offset",
			"line-number",
			"line-buffered",
			"with-filename",
			"no-filename",
			"only-matching",
			"quiet",
			"silent",
			"files-without-match",
			"files-with-matches",
			"count",
			"initial-tab",
			"null",
			"text",
			"color",
			"colour",
			"no-group-separator",
		],
		gives_up: &[
			"-r",
			"-R",
			"-d",
			"-D",
			"--recursive",
			"--dereference-recursive",
			"--directories",
			"--devices",
			"--include",
			"--exclude",
			"--exclude-from",
			"--exclude-dir",
		],
		script: Some(&["-e", "-f", "--regexp", "--file"]),
		..NOTHING
	},
	Reader {
		names: &["sed"],
		values: "ef",
		flags: "nErsuzb",
		long_values: &["expression", "file", "line-length"],
		long_flags: &[
			"quiet",
			"silent",
			"regexp-extended",
			"separate",
			"unbuffered",
			"null-data",
			"posix",
			"debug",
			"sandbox",
			"follow-symlinks",
			"binary",
		],
		gives_up: &["-i", "-l", "--in-place"],
		script: Some(&["-e", "-f", "--expression", "--file"]),
		..NOTHING
	},
	Reader {
		names: &["awk", "gawk", "mawk", "nawk"],
		values: "Fvf",
		long_values: &["field-separator", "assign", "file"],
		script: Some(&["-f", "--file"]),
		assignments: true,
		..NOTHING
	},
	Reader {
		names: &["jq"],
		values: "fL",
		flags: "rcsjaCMSeR",
		long_values: &["from-file", "indent", "library-path"],
		long_pairs: &["arg", "argjson", "slurpfile", "rawfile"],
		long_flags: &[
			"raw-output",
			"compact-output",
			"slurp",
			"join-output",
			"ascii-output",
			"color-output",
			"monochrome-output",
			"sort-keys",
			"exit-status",
			"raw-input",
			"tab",
			"stream",
			"stream-errors",
			"seq",
			"unbuffered",
			"raw-output0",
		],
		gives_up: &["-n", "--null-input", "--args", "--jsonargs"],
		script: Some(&["-f", "--from-file"]),
		..NOTHING
	},
	Reader {
		names: &["cut"],
		values: "bcfd",
		flags: "snz",
		long_values: &["bytes", "characters", "fields", "delimiter", "output-delimiter"],
		long_flags: &["complement", "only-delimited", "zero-terminated"],
		..NOTHING
	},
	Reader {
		names: &["uniq"],
		values: "fsw",
		flags: "cdDiuz",
		long_values: &["skip-fields", "skip-chars", "check-chars"],
		long_flags: &[
			"count",
			"repeated",
			"all-repeated",
			"unique",
			"ignore-case",
			"zero-terminated",
			"group",
		],
		first_only: true,
		..NOTHING
	},
	Reader {
		names: &["wc"],
		flags: "clmwL",
		long_flags: &["bytes", "chars", "lines", "words", "max-line-length"],
		gives_up: &["--files0-from"],
		..NOTHING
	},
	Reader {
		names: &["tac"],
		values: "s",
		flags: "br",
		long_values: &["separator"],
		long_flags: &["before", "regex"],
		..NOTHING
	},
	Reader {
		names: &["base64"],
		values: "w",
		flags: "d",
		long_values: &["wrap"],
		long_flags: &["decode", "ignore-garbage"],
		gives_up: &["-i", "-o", "-b", "-D"],
		..NOTHING
	},
	Reader {
		names: &[
			"md5sum",
			"sha1sum",
			"sha224sum",
			"sha256sum",
			"sha384sum",
			"sha512sum",
			"b2sum",
		],
		flags: "bctwz",
		long_flags: &[
			"binary",
			"check",
			"tag",
			"text",
			"ignore-missing",
			"quiet",
			"status",
			"strict",
			"warn",
			"zero",
		],
		..NOTHING
	},
	Reader {
		names: &["tr"],
		flags: "cdsCt",
		long_flags: &["complement", "delete", "squeeze-repeats", "truncate-set1"],
		no_files: true,
		..NOTHING
	},
	Reader {
		names: &["envsubst"],
		gives_up: &["-v", "--variables"],
		no_files: true,
		..NOTHING
	},
];

/// The files a command reads, as its words name them, and whether it reads
/// its standard input -- or `None` when an option is one this does not know,
/// or a word may become one once it expands.
fn inputs<'w>(r: &Reader, words: &'w [Word]) -> Option<(Vec<&'w Word>, bool)> {
	let mut operands: Vec<&Word> = Vec::new();
	let mut scripted = false;
	let mut options = true;
	let mut k = 0;
	while k < words.len() {
		let w = &words[k];
		k += 1;
		if !options {
			operands.push(w);
			continue;
		}
		let Some(t) = exact(w) else {
			// Starting with a character other than `-`, it cannot become an option.
			match w.parts.first() {
				Some(Part::Char { c, .. }) if *c != '-' => {
					operands.push(w);
					continue;
				}
				_ => return None,
			}
		};
		if t == "--" {
			options = false;
			continue;
		}
		if t == "-" || !t.starts_with('-') {
			operands.push(w);
			continue;
		}
		if let Some(long) = t.strip_prefix("--") {
			let (name, glued) = match long.split_once('=') {
				Some((n, _)) => (n, true),
				None => (long, false),
			};
			let option = format!("--{name}");
			if r.gives_up.contains(&option.as_str()) {
				return None;
			}
			scripted |= r.script.is_some_and(|s| s.contains(&option.as_str()));
			if r.long_values.contains(&name) {
				k += usize::from(!glued);
			} else if r.long_pairs.contains(&name) && !glued {
				k += 2;
			} else if !r.long_flags.contains(&name) {
				return None;
			}
			continue;
		}
		let letters: Vec<char> = t[1..].chars().collect();
		for (m, c) in letters.iter().enumerate() {
			let option = format!("-{c}");
			if r.gives_up.contains(&option.as_str()) {
				return None;
			}
			scripted |= r.script.is_some_and(|s| s.contains(&option.as_str()));
			if r.values.contains(*c) {
				k += usize::from(m + 1 == letters.len());
				break;
			}
			if !(r.flags.contains(*c) || r.digits && c.is_ascii_digit()) {
				return None;
			}
		}
	}
	if r.no_files {
		return Some((Vec::new(), true));
	}
	if r.script.is_some() && !scripted {
		if operands.is_empty() {
			return None;
		}
		operands.remove(0);
	}
	if r.assignments {
		operands.retain(|w| !is_assignment(w));
	}
	if r.first_only {
		operands.truncate(1);
	}
	let stdin = operands.is_empty();
	Some((operands, stdin))
}

/// `sort f > f`: a file the command reads, emptied by its own redirection
/// before it starts.
fn truncated_input(s: &Simple, text: &Texts, out: &mut Vec<Found>) {
	let Some(k) = program(s) else {
		return;
	};
	let Some(name) = s.words[k].plain() else {
		return;
	};
	let base = name.rsplit('/').next().unwrap_or_default().to_string();
	let Some(reader) = READERS.iter().find(|r| r.names.contains(&base.as_str())) else {
		return;
	};
	let emptied: Vec<(&crate::syntax::Redirect, String)> = s
		.redirects
		.iter()
		.filter(|r| matches!(r.op, ">" | ">|" | "&>"))
		.filter_map(|r| exact(&r.target).map(|t| (r, t)))
		.filter(|(_, t)| t != "-" && !t.starts_with("/dev/") && !t.starts_with("/proc/"))
		.collect();
	if emptied.is_empty() {
		return;
	}
	let Some((files, stdin)) = inputs(reader, &s.words[k + 1..]) else {
		return;
	};
	for (redirect, target) in emptied {
		let named = files.iter().any(|w| exact(w).as_deref() == Some(target.as_str()));
		let fed = stdin
			&& s.redirects
				.iter()
				.any(|i| i.op == "<" && exact(&i.target).as_deref() == Some(target.as_str()));
		if !(named || fed) {
			continue;
		}
		let (a, b) = text.script.bytes(redirect.i, redirect.target.end);
		let written = &text.src[a..b];
		let fix = match base.as_str() {
			"sed" => "`sed -i` edits the file in place".to_string(),
			_ => format!("write to another file and move it over: `… > {target}.tmp && mv {target}.tmp {target}`"),
		};
		out.push(Found {
			rule: "truncated-input",
			from: redirect.i,
			to: redirect.target.end,
			message: format!(
				"`{written}` empties `{target}` before `{base}` starts, so `{base}` reads an empty file and `{target}` is left empty"
			),
			fix: Some(fix),
		});
	}
}
