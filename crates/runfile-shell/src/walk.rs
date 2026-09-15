//! The walk over a runfile: every shell script in it, checked where it stands,
//! with what each name can hold at that point.
//!
//! Two rules need values. An interpolation is one quoted word whatever it holds,
//! so a `*` in its value is never expanded (`unexpanded-glob`); and a string is
//! not shell, so one that spells `$HOME` or starts with `~/` holds exactly those
//! characters (`unexpanded-string`). Neither needs a run to answer: a string is
//! written down, a name holds what the bindings that reach it held, and `?`
//! holds either side. Only that much is followed -- strings, through names, `?`,
//! lists and the calls that pass a value through -- and anything else, `ENV.X`
//! and `ARG.x` included, holds nothing to report.

use std::collections::{BTreeMap, BTreeSet};

use runfile_lang::{Block, Expr, InterpPart, Property, Statement, Target};

use crate::Finding;
use crate::rules::{self, Pos};
use crate::script::{self, Hole, Script, Source};
use crate::syntax::{self, Quote, Stop};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
	/// `$HOME` or `${HOME}` anywhere, or a `~` that starts the value.
	Unexpanded {
		spelled: String,
		leading: bool,
	},
	Glob,
}

/// Something a value can hold, and the string it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Taint {
	kind: Kind,
	literal: String,
	start: usize,
	end: usize,
	line: usize,
	/// Whether that string is in the file being checked, rather than a shared one.
	here: bool,
}

type Env = BTreeMap<String, Vec<Taint>>;

/// The functions whose first argument names a file.
const PATH_FUNCTIONS: &[&str] = &[
	"directory_exists",
	"file_exists",
	"glob",
	"is_executable",
	"read_file",
	"write_file",
];

/// The properties whose value names a file.
const PATH_PROPERTIES: &[&str] = &["add-path", "env-file", "workdir"];

pub(crate) fn check(src: &str, file: &Target, chain: Option<&[Target]>) -> Vec<Finding> {
	let source = Source::new(src);
	let mut w = Walk {
		src: &source,
		dollar: script::dollar(file, chain),
		bash: script::dollar_bash(file, chain),
		out: Vec::new(),
		seen: BTreeSet::new(),
	};
	let mut env = Env::new();
	// A shared file binds with its top-level `let`s, in order, and nothing else.
	for shared in chain.unwrap_or_default() {
		for st in &shared.body.statements {
			if let Statement::Let { names, value, .. } = st {
				let t = taints(value, &env, false);
				bind(&mut env, names, &t);
			}
		}
	}
	w.block(&file.body, &mut env);
	w.out.sort_by_key(|f| (f.span.start, f.span.end));
	w.out
}

fn bind(env: &mut Env, names: &[String], t: &[Taint]) {
	for n in names.iter().filter(|n| *n != "_") {
		env.insert(n.clone(), t.to_vec());
	}
}

/// What either of two paths left each name holding.
fn join(mut a: Env, b: Env) -> Env {
	for (name, held) in b {
		let e = a.entry(name).or_default();
		for t in held {
			if !e.contains(&t) {
				e.push(t);
			}
		}
	}
	a
}

fn leading(t: &Taint) -> bool {
	matches!(t.kind, Kind::Unexpanded { leading: true, .. })
}

/// What an expression can hold, as far as the text says.
fn taints(e: &Expr, env: &Env, here: bool) -> Vec<Taint> {
	let mut out = Vec::new();
	match e {
		Expr::Str(parts, span) => {
			let literal: String = parts
				.iter()
				.map(|p| match p {
					InterpPart::Literal(t) => t.as_str(),
					InterpPart::Expr(_) => "{{ … }}",
				})
				.collect();
			let taint = |kind| Taint {
				kind,
				literal: format!("\"{literal}\""),
				start: span.start,
				end: span.end,
				line: span.line,
				here,
			};
			for (k, p) in parts.iter().enumerate() {
				match p {
					InterpPart::Literal(s) => {
						for spelled in variables(s) {
							out.push(taint(Kind::Unexpanded {
								spelled,
								leading: false,
							}));
						}
						if k == 0 && (s == "~" || s.starts_with("~/") || s.starts_with("~\\")) {
							out.push(taint(Kind::Unexpanded {
								spelled: "~".into(),
								leading: true,
							}));
						}
						if s.contains('*') {
							out.push(taint(Kind::Glob));
						}
					}
					InterpPart::Expr(inner) => {
						out.extend(taints(inner, env, here).into_iter().filter(|t| k == 0 || !leading(t)));
					}
				}
			}
		}
		Expr::Ident(name, _) => out.extend(env.get(name).cloned().unwrap_or_default()),
		Expr::Chain { lhs, rhs, .. } => {
			out.extend(taints(lhs, env, here));
			out.extend(taints(rhs, env, here));
		}
		Expr::List(items, _) => {
			for item in items {
				out.extend(taints(item, env, here));
			}
		}
		Expr::Index { base, .. } => out.extend(taints(base, env, here)),
		Expr::Call { name, args, .. } => match name.as_str() {
			"concat" | "join_path" => {
				for (k, a) in args.iter().enumerate() {
					out.extend(taints(a, env, here).into_iter().filter(|t| k == 0 || !leading(t)));
				}
			}
			"append" | "concat_lists" | "dirname" | "first" | "flatten" | "last" | "one_of" | "prepend" | "reverse"
			| "slice" | "sort" | "trim" | "trim_end" | "unique" => {
				for a in args {
					out.extend(taints(a, env, here));
				}
			}
			_ => {}
		},
		_ => {}
	}
	out
}

/// `$NAME` and `${NAME}` as a shell would expand them in a path: a name in
/// capitals, ending the string or followed by a separator, so `$RECYCLE.BIN`
/// and a `$5` price are left alone.
fn variables(s: &str) -> Vec<String> {
	let b = s.as_bytes();
	let mut out = Vec::new();
	let mut k = 0;
	while k < b.len() {
		if b[k] != b'$' {
			k += 1;
			continue;
		}
		let braced = b.get(k + 1) == Some(&b'{');
		let start = k + 1 + usize::from(braced);
		let mut end = start;
		while end < b.len() && (b[end].is_ascii_uppercase() || b[end].is_ascii_digit() || b[end] == b'_') {
			end += 1;
		}
		let name = &s[start..end];
		let named = name.len() >= 2 && name.starts_with(|c: char| c.is_ascii_uppercase() || c == '_');
		let ends = match braced {
			true => b.get(end) == Some(&b'}'),
			false => matches!(b.get(end), None | Some(b'/' | b'\\' | b':' | b';')),
		};
		if named && ends {
			let to = end + usize::from(braced);
			out.push(s[k..to].to_string());
			k = to;
		} else {
			k += 1;
		}
	}
	out
}

struct Walk<'a> {
	src: &'a Source<'a>,
	dollar: Option<bool>,
	/// Whether the `$` runs are bash's.
	bash: bool,
	out: Vec<Finding>,
	/// What has been reported, since a loop's body is walked twice.
	seen: BTreeSet<(&'static str, usize, usize)>,
}

impl Walk<'_> {
	fn report(&mut self, rule: &'static str, (start, end): (usize, usize), message: String, fix: Option<String>) {
		if self.seen.insert((rule, start, end)) {
			self.out.push(Finding {
				rule,
				span: self.src.span(start, end),
				message,
				fix,
			});
		}
	}

	fn block(&mut self, b: &Block, env: &mut Env) {
		for p in b.declaration() {
			self.property(p, env);
		}
		let mut trailing = b.trailing().iter().peekable();
		for st in &b.statements {
			while let Some(p) = trailing.next_if(|p| p.span.line < st.span().line) {
				self.property(p, env);
			}
			self.statement(st, env);
		}
		for p in trailing {
			self.property(p, env);
		}
	}

	fn property(&mut self, p: &Property, env: &Env) {
		let Some(v) = &p.value else {
			return;
		};
		self.expr(v, env);
		if p.path.len() == 1 && PATH_PROPERTIES.contains(&p.path[0].as_str()) {
			self.path_value(v, env);
		}
	}

	fn statement(&mut self, st: &Statement, env: &mut Env) {
		match st {
			Statement::Let { names, value, .. } | Statement::Assign { names, value, .. } => {
				self.expr(value, env);
				let t = taints(value, env, true);
				bind(env, names, &t);
			}
			Statement::Call { expr, .. } => self.expr(expr, env),
			Statement::Do {
				body, parallel: false, ..
			} => self.block(body, env),
			Statement::Do {
				body, parallel: true, ..
			} => {
				// A branch binds on a copy, which its siblings never see.
				for p in body.properties.iter() {
					self.property(p, env);
				}
				for branch in &body.statements {
					self.statement(branch, &mut env.clone());
				}
			}
			Statement::If {
				cond, then, otherwise, ..
			} => {
				self.expr(cond, env);
				let mut taken = env.clone();
				self.block(then, &mut taken);
				let mut other = env.clone();
				if let Some(o) = otherwise {
					self.block(o, &mut other);
				}
				*env = join(taken, other);
			}
			Statement::Match {
				subject,
				cases,
				default,
				..
			} => {
				self.expr(subject, env);
				let mut after = if default.is_some() { Env::new() } else { env.clone() };
				for body in cases.iter().map(|c| &c.body).chain(default) {
					let mut case = env.clone();
					self.block(body, &mut case);
					after = join(after, case);
				}
				*env = after;
			}
			Statement::Retry {
				attempts,
				delay,
				body,
				otherwise,
				..
			} => {
				self.expr(attempts, env);
				if let Some(d) = delay {
					self.expr(d, env);
				}
				// An attempt sees what the one before it bound.
				self.block(body, env);
				self.block(body, env);
				if let Some(o) = otherwise {
					let mut other = env.clone();
					self.block(o, &mut other);
					*env = join(env.clone(), other);
				}
			}
			Statement::For {
				names,
				iter,
				body,
				parallel,
				..
			} => {
				self.expr(iter, env);
				let item = taints(iter, env, true);
				let before = env.clone();
				let mut pass = env.clone();
				bind(&mut pass, names, &item);
				self.block(body, &mut pass);
				if *parallel {
					return;
				}
				bind(&mut pass, names, &item);
				self.block(body, &mut pass);
				let mut after = join(before.clone(), pass);
				for n in names {
					match before.get(n) {
						Some(t) => after.insert(n.clone(), t.clone()),
						None => after.remove(n),
					};
				}
				*env = after;
			}
			Statement::Loop { test, body, .. } => {
				let before = env.clone();
				for _ in 0..2 {
					if let Some(c) = test.cond() {
						self.expr(c, env);
					}
					self.block(body, env);
				}
				*env = join(before, env.clone());
			}
			Statement::Break { .. } | Statement::Continue { .. } => {}
			Statement::Run { target, args, .. } => {
				self.parts(target, env);
				for a in args {
					self.parts(a, env);
				}
			}
			Statement::Exec { command, body, .. } => {
				if let Some(c) = command {
					self.parts(c, env);
				}
				for line in body {
					self.parts(line, env);
				}
				if let Some(s) = script::of_statement(st, self.src, self.dollar, self.bash) {
					self.script(&s, env);
				}
			}
		}
	}

	fn parts(&mut self, parts: &[InterpPart], env: &Env) {
		for p in parts {
			if let InterpPart::Expr(e) = p {
				self.expr(e, env);
			}
		}
	}

	fn expr(&mut self, e: &Expr, env: &Env) {
		match e {
			Expr::Capture { command, body, .. } => {
				if let Some(c) = command {
					self.parts(c, env);
				}
				for line in body {
					self.parts(line, env);
				}
				if let Some(s) = script::of_capture(e, self.src, self.dollar, self.bash) {
					self.script(&s, env);
				}
			}
			Expr::Call { name, args, .. } => {
				for a in args {
					self.expr(a, env);
				}
				if PATH_FUNCTIONS.contains(&name.as_str())
					&& let Some(path) = args.first()
				{
					self.path_value(path, env);
				}
			}
			Expr::Str(parts, _) => self.parts(parts, env),
			Expr::List(items, _) => {
				for item in items {
					self.expr(item, env);
				}
			}
			Expr::Unary { rhs, .. } => self.expr(rhs, env),
			Expr::Binary { lhs, rhs, .. } | Expr::Chain { lhs, rhs, .. } => {
				self.expr(lhs, env);
				self.expr(rhs, env);
			}
			Expr::Index { base, index, .. } => {
				self.expr(base, env);
				self.expr(index, env);
			}
			Expr::Structured { body, .. } => {
				for line in &body.lines {
					self.parts(line, env);
				}
			}
			Expr::Dispatch { target, args, .. } => {
				self.parts(target, env);
				for a in args {
					self.parts(a, env);
				}
			}
			Expr::Number(..) | Expr::Bool(..) | Expr::Ident(..) | Expr::Source { .. } => {}
		}
	}

	/// A value the runner uses as a path itself: a property's, or a function's
	/// first argument.
	fn path_value(&mut self, e: &Expr, env: &Env) {
		let at = e.span();
		for t in taints(e, env, true) {
			if let Kind::Unexpanded { spelled, .. } = &t.kind {
				self.unexpanded(&t, spelled, (at.start, at.end));
			}
		}
	}

	fn script(&mut self, s: &Script, env: &Env) {
		let list = match syntax::parse(&s.chars, s.bash) {
			Ok(list) => list,
			Err(Stop::Lost) => return,
			Err(Stop::Refused {
				rule,
				from,
				to,
				message,
			}) => {
				self.report(rule, s.bytes(from, to), message, None);
				return;
			}
		};
		for f in rules::run(&list, s, self.src.text) {
			self.report(f.rule, s.bytes(f.from, f.to), f.message, f.fix);
		}
		for place in rules::places(&list) {
			let hole = &s.holes[place.hole];
			for t in taints(&hole.expr, env, true) {
				match &t.kind {
					Kind::Unexpanded { spelled, leading } => {
						let path = matches!(
							place.pos,
							Pos::Program | Pos::Redirect | Pos::Plain { path: true } | Pos::Test { path: true, .. }
						);
						if path && (place.first || !leading) {
							self.unexpanded(&t, spelled, (hole.start, hole.end));
						}
					}
					Kind::Glob => {
						let operand = matches!(
							place.pos,
							Pos::Redirect | Pos::Plain { path: true } | Pos::Test { path: true, .. }
						);
						if operand && place.quote == Quote::Bare {
							self.glob(&t, hole);
						}
					}
				}
			}
		}
	}

	fn unexpanded(&mut self, t: &Taint, spelled: &str, sink: (usize, usize)) {
		let name = spelled.trim_start_matches(['$', '{']).trim_end_matches('}');
		let instead = match spelled {
			"~" => "{{ ENV.HOME }}".to_string(),
			_ => format!("{{{{ ENV.{name} }}}}"),
		};
		if t.here {
			let line = self.src.line_of(sink.0);
			let literal = &self.src.text[t.start..t.end];
			self.report(
				"unexpanded-string",
				(t.start, t.end),
				format!(
					"`{spelled}` in `{literal}` is kept as written, since a string is not shell -- and on line {line} this value is used as a path, which then starts with `{spelled}` itself"
				),
				Some(format!("write `{instead}` in its place")),
			);
		} else {
			let text = &self.src.text[sink.0..sink.1];
			self.report(
				"unexpanded-string",
				sink,
				format!(
					"`{text}` can hold `{}`, from line {} of a `_shared.run` above this file, where `{spelled}` is kept as written since a string is not shell -- so this path starts with `{spelled}` itself",
					t.literal, t.line
				),
				Some(format!("write `{instead}` in that string")),
			);
		}
	}

	fn glob(&mut self, t: &Taint, hole: &Hole) {
		let text = &self.src.text[hole.start..hole.end];
		let literal = match t.here {
			true => &self.src.text[t.start..t.end],
			false => t.literal.as_str(),
		};
		self.report(
			"unexpanded-glob",
			(hole.start, hole.end),
			format!(
				"`{text}` can hold `{literal}`, but an interpolation is one quoted word, so the shell never expands its `*`: the command is handed the pattern itself"
			),
			Some("loop over `glob(…)` in the runfile instead, or write the pattern on the `$` line, outside the interpolation".into()),
		);
	}
}
