//! Mistakes a runfile makes every time it runs, found before it does.
//!
//! `ARG.port + 1` fails the moment it is reached, whatever is passed: an
//! argument is a string, and the language coerces nothing. So does `sleep` given
//! a string, `for` given one, `if ENV.CI`, a call with an argument too many, a
//! regex that does not compile, and `if code_of($ make)`. None of them needs a
//! run to answer, and each used to wait for one -- in a branch that runs once a
//! month, the month it does. `RUN.os == "darwin"` fails nothing at all, and is
//! worse for it: it is false everywhere, so its block never runs and nothing says
//! why.
//!
//! **A finding stops the run**, the way a name nothing binds does, so a rule
//! here reports only what is wrong every time it is reached. Where a value's
//! type depends on the run -- `ENV.PORT ? 3000` is a string or a number -- nothing
//! is said: the check reports only what is disjoint from everything a value can
//! be (see [`crate::types`]). `LANGUAGE-CHECK-RULES.md` lists every rule, what
//! it leaves alone, and why.
//!
//! The runner asks this before a target runs (`Host::load`), the language server
//! asks it for every document, and `run :lint` asks the server's function, so a
//! run, an editor and `:lint` cannot disagree about a file.

use crate::ast::{BinaryOp, Block, Expr, InterpPart, MatchCase, Statement, Target, UnaryOp};
use crate::span::Span;
use crate::types::{self, BOOL, Kinds, LIST, NUM, Names, STR, Ty};
use std::collections::BTreeSet;

/// A rule: the name a finding carries, and what it finds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
	pub id: &'static str,
	pub summary: &'static str,
}

/// Every rule, in the order `LANGUAGE-CHECK-RULES.md` lists them.
pub const RULES: &[Rule] = &[
	Rule {
		id: "arity",
		summary: "A call with a number of arguments the function never takes.",
	},
	Rule {
		id: "wrong-type",
		summary: "A value of a type the place it reaches always refuses.",
	},
	Rule {
		id: "never-equal",
		summary: "A `==` or `!=` whose two sides can never be equal.",
	},
	Rule {
		id: "unreachable-case",
		summary: "A `case` its `match` can never take.",
	},
	Rule {
		id: "invalid-literal",
		summary: "A value written out that the place it reaches always refuses.",
	},
	Rule {
		id: "capture-position",
		summary: "A `$` run or a `run` dispatch where the runner has nothing to run it in.",
	},
];

/// Something wrong with a runfile, found before it runs: by this checker, or by
/// the shell checker, which reports in the same shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
	/// The rule it breaks.
	pub rule: &'static str,
	/// Exactly the text it is about.
	pub span: Span,
	pub message: String,
	/// What to write instead, when there is one thing to write.
	pub fix: Option<String>,
}

impl std::fmt::Display for Finding {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "line {}: {} [{}]", self.span.line, self.message, self.rule)?;
		match &self.fix {
			Some(fix) => write!(f, "; {fix}"),
			None => Ok(()),
		}
	}
}

/// Everything in `file`, whose text is `src`, that fails every time it runs.
///
/// `chain` is the `_shared.run` files above it, outermost first, for what they
/// bind; `None` when one of them could not be read, and then a name could hold
/// anything. `shared` says `file` is a `_shared.run` itself, whose `let`s are
/// worked out with nothing to run a command in.
pub fn check(src: &str, file: &Target, chain: Option<&[Target]>, shared: bool) -> Vec<Finding> {
	let mut c = Check {
		src,
		names: Names::of(file, chain),
		out: Vec::new(),
		root: Place::Inner,
	};
	match shared {
		true => c.shared(&file.body),
		false => c.block(&file.body),
	}
	c.out.sort_by_key(|f| (f.span.start, f.span.end));
	c.out.dedup();
	c.out
}

/// Where an expression is evaluated, which decides whether a `$` run can be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
	/// The whole value of a `let`, a reassignment, a line of its own or a `for`:
	/// a `$` run may be all of it, or the last argument of a call that is.
	Whole,
	/// An `if`, `while` or `until` condition, or a `match` subject: a `$` run
	/// may be all of it, and nothing else.
	Asked(&'static str),
	/// A `_shared.run`'s own `let`, worked out before any target has a process
	/// to run anything in.
	Shared,
	/// Anywhere else.
	Inner,
}

struct Check<'a> {
	src: &'a str,
	names: Names,
	out: Vec<Finding>,
	/// Where the statement being walked evaluates its expression.
	root: Place,
}

/// The text of a string literal with nothing interpolated into it.
fn literal_str(e: &Expr) -> Option<String> {
	match e {
		Expr::Str(parts, _) => parts
			.iter()
			.map(|p| match p {
				InterpPart::Literal(t) => Some(t.as_str()),
				InterpPart::Expr(_) => None,
			})
			.collect::<Option<Vec<_>>>()
			.map(|t| t.concat()),
		_ => None,
	}
}

/// A number written out, with its sign.
fn literal_num(e: &Expr) -> Option<f64> {
	match e {
		Expr::Number(n, _) => Some(*n),
		Expr::Unary {
			op: UnaryOp::Neg, rhs, ..
		} => match rhs.as_ref() {
			Expr::Number(n, _) => Some(-n),
			_ => None,
		},
		_ => None,
	}
}

fn ordinal(n: usize) -> String {
	match n {
		1 => "1st".into(),
		2 => "2nd".into(),
		3 => "3rd".into(),
		n => format!("{n}th"),
	}
}

/// The names a sentence gives a set of strings: `` `a`, `b` or `c` ``.
fn listed(values: &BTreeSet<&str>) -> String {
	let quoted: Vec<String> = values.iter().map(|v| format!("`{v}`")).collect();
	match quoted.as_slice() {
		[one] => one.clone(),
		[init @ .., last] => format!("{} or {last}", init.join(", ")),
		[] => "nothing".into(),
	}
}

/// What someone writing a platform's other name for a `RUN.os` or `RUN.arch`
/// value meant, when it is one of those.
fn meant<'v>(written: &str, values: &BTreeSet<&'v str>) -> Option<&'v str> {
	let lower = written.to_ascii_lowercase();
	let alias = match lower.as_str() {
		"macos" | "darwin" | "osx" => "mac",
		"win" | "win32" | "win64" => "windows",
		"x86_64" | "amd64" | "x64" => "x86-64",
		"aarch64" => "arm64",
		other => other,
	};
	values.get(alias).copied().filter(|v| *v != written)
}

/// Whether a `case` label can be a number's text, as a `match` renders one.
fn number_text(label: &str) -> bool {
	label.parse::<f64>().is_ok_and(|x| crate::value::format_num(x) == label)
}

impl Check<'_> {
	fn report(&mut self, rule: &'static str, span: Span, message: String, fix: Option<String>) {
		self.out.push(Finding {
			rule,
			span,
			message,
			fix,
		});
	}

	/// The source of `span` as a message quotes it: on one line, and cut short
	/// when it is long.
	fn text(&self, span: Span) -> String {
		let raw = self.src.get(span.start..span.end).unwrap_or_default();
		let one: String = raw.lines().map(str::trim).collect::<Vec<_>>().join(" ");
		match one.chars().count() > 60 {
			true => format!("{}…", one.chars().take(59).collect::<String>()),
			false => one,
		}
	}

	fn block(&mut self, b: &Block) {
		for p in &b.properties {
			if let Some(v) = &p.value {
				self.value(v, Place::Inner);
			}
		}
		for st in &b.statements {
			self.statement(st);
		}
	}

	/// A `_shared.run`: its top-level `let`s are folded into every target below
	/// it before any of them runs. The rest of it never runs, and is checked as
	/// the runner would have run it anyway -- a slip there is still a slip.
	fn shared(&mut self, b: &Block) {
		for p in &b.properties {
			if let Some(v) = &p.value {
				self.value(v, Place::Inner);
			}
		}
		for st in &b.statements {
			match st {
				Statement::Let { .. } => self.bind(st, Place::Shared),
				_ => self.statement(st),
			}
		}
	}

	fn statement(&mut self, st: &Statement) {
		match st {
			Statement::Let { .. } | Statement::Assign { .. } => self.bind(st, Place::Whole),
			Statement::Call { expr, .. } => {
				self.value(expr, Place::Whole);
			}
			Statement::Do { body, .. } => self.block(body),
			Statement::If {
				cond, then, otherwise, ..
			} => {
				self.condition(cond, "if");
				self.block(then);
				if let Some(o) = otherwise {
					self.block(o);
				}
			}
			Statement::Loop { test, body, .. } => {
				if let Some(c) = test.cond() {
					self.condition(c, test.keyword());
				}
				self.block(body);
			}
			Statement::Retry {
				attempts,
				delay,
				body,
				otherwise,
				..
			} => {
				self.number(attempts, "`retry` counts attempts in a number");
				if let Some(d) = delay {
					self.number(d, "`every` waits a number of seconds");
				}
				self.block(body);
				if let Some(o) = otherwise {
					self.block(o);
				}
			}
			Statement::For { names, iter, body, .. } => {
				self.for_loop(names.len(), iter);
				self.block(body);
			}
			Statement::Match {
				subject,
				cases,
				default,
				..
			} => {
				self.matched(subject, cases);
				for body in cases.iter().map(|c| &c.body).chain(default) {
					self.block(body);
				}
			}
			Statement::Break { .. } | Statement::Continue { .. } => {}
			Statement::Run { target, args, .. } => {
				self.root = Place::Inner;
				self.parts(target);
				for a in args {
					self.parts(a);
				}
			}
			Statement::Exec { command, body, .. } => {
				self.root = Place::Inner;
				if let Some(c) = command {
					self.parts(c);
				}
				for line in body {
					self.parts(line);
				}
			}
		}
	}

	/// Walk an expression a statement evaluates, from where it is evaluated.
	/// Answers whether it fails every time.
	fn value(&mut self, e: &Expr, place: Place) -> bool {
		self.root = place;
		self.expr(e, true)
	}

	fn parts(&mut self, parts: &[InterpPart]) -> bool {
		let mut fails = false;
		for p in parts {
			if let InterpPart::Expr(e) = p {
				fails |= self.expr(e, false);
			}
		}
		fails
	}

	/// A `let` or a reassignment.
	fn bind(&mut self, st: &Statement, place: Place) {
		let (Statement::Let { names, value, .. } | Statement::Assign { names, value, .. }) = st else {
			return;
		};
		if self.value(value, place) || names.len() < 2 {
			return;
		}
		let t = self.names.ty(value);
		if t.refused(LIST) {
			let text = self.text(value.span());
			self.report(
				"wrong-type",
				value.span(),
				format!(
					"unpacking {} names takes a list, and `{text}` is {}",
					names.len(),
					types::named(t.kinds)
				),
				None,
			);
		} else if let Expr::List(items, span) = value
			&& items.len() < names.len()
		{
			self.report(
				"invalid-literal",
				*span,
				format!(
					"{} names to unpack, but this list has {}, and a name with no value to take fails every time",
					names.len(),
					items.len()
				),
				None,
			);
		}
	}

	fn for_loop(&mut self, names: usize, iter: &Expr) {
		if self.value(iter, Place::Whole) {
			return;
		}
		let t = self.names.ty(iter);
		let text = self.text(iter.span());
		if t.refused(LIST) {
			let fix = (t.kinds == STR).then(|| format!("`lines({text})` or `split({text}, \",\")` makes a list of it"));
			self.report(
				"wrong-type",
				iter.span(),
				format!("`for` walks a list, and `{text}` is {}", types::named(t.kinds)),
				fix,
			);
			return;
		}
		if names < 2 {
			return;
		}
		let item = t.item();
		if item.refused(LIST) {
			self.report(
				"wrong-type",
				iter.span(),
				format!(
					"unpacking {names} names takes each item to be a list, and every item of `{text}` is {}",
					types::named(item.kinds)
				),
				None,
			);
		} else if let Expr::List(items, _) = iter
			&& let Some(Expr::List(first, span)) = items.first()
			&& first.len() < names
		{
			self.report(
				"invalid-literal",
				*span,
				format!(
					"{names} names to unpack, but the first item has {}, and a name with no value to take fails every time",
					first.len()
				),
				None,
			);
		}
	}

	/// A value the statement needs a number of: `retry`'s attempts, `every`'s
	/// seconds.
	fn number(&mut self, e: &Expr, what: &str) {
		if self.value(e, Place::Inner) {
			return;
		}
		let t = self.names.ty(e);
		if t.refused(NUM) {
			let text = self.text(e.span());
			let fix = (t.kinds == STR && literal_str(e).is_none()).then(|| format!("`number({text})` reads it as one"));
			self.report(
				"wrong-type",
				e.span(),
				format!("{what}, and `{text}` is {}", types::named(t.kinds)),
				fix,
			);
		}
	}

	fn condition(&mut self, cond: &Expr, keyword: &'static str) {
		if let Expr::Capture { command, body, .. } = cond {
			self.root = Place::Asked(keyword);
			if let Some(c) = command {
				self.parts(c);
			}
			for line in body {
				self.parts(line);
			}
			return;
		}
		if self.value(cond, Place::Asked(keyword)) {
			return;
		}
		let t = self.names.ty(cond);
		if !t.refused(BOOL) {
			return;
		}
		let text = self.text(cond.span());
		let fix = match t.kinds {
			STR => Some(format!("compare it, as `{text} == \"true\"`")),
			NUM => Some(format!("compare it, as `{text} != 0`")),
			LIST => Some(format!("ask about its length, as `length({text}) > 0`")),
			_ => None,
		};
		self.report(
			"wrong-type",
			cond.span(),
			format!(
				"`{keyword}` needs `true` or `false`, and `{text}` is {}",
				types::named(t.kinds)
			),
			fix,
		);
	}

	fn matched(&mut self, subject: &Expr, cases: &[MatchCase]) {
		let status = matches!(subject, Expr::Capture { .. });
		let fails = match subject {
			Expr::Capture { command, body, .. } => {
				self.root = Place::Asked("match");
				let mut fails = command.as_ref().is_some_and(|c| self.parts(c));
				for line in body {
					fails |= self.parts(line);
				}
				fails
			}
			_ => self.value(subject, Place::Asked("match")),
		};
		let t = self.names.ty(subject);
		let values = self.names.values(subject);
		let text = self.text(subject.span());
		let mut seen: Vec<(&str, usize)> = Vec::new();
		for c in cases {
			let at = self.label(c);
			if let Some((_, line)) = seen.iter().find(|(l, _)| *l == c.label) {
				self.report(
					"unreachable-case",
					at,
					format!(
						"`case \"{}\"` repeats the one on line {line}, which is always taken first",
						c.label
					),
					None,
				);
				continue;
			}
			seen.push((&c.label, c.span.line));
			if fails {
				continue;
			}
			let label = c.label.as_str();
			let (message, fix) = if status {
				if label.parse::<i32>().is_ok_and(|n| n.to_string() == label) {
					continue;
				}
				(
					format!(
						"`match $ …` takes the command's exit status, which is a whole number written plainly, so `case \"{label}\"` is never taken"
					),
					None,
				)
			} else if let Some(values) = &values {
				if values.contains(label) {
					continue;
				}
				(
					format!(
						"`{text}` is only ever {}, so `case \"{label}\"` is never taken",
						listed(values)
					),
					meant(label, values).map(|m| format!("write `case \"{m}\"`")),
				)
			} else {
				let reachable = t.kinds & (STR | LIST) != 0
					|| t.kinds & NUM != 0 && number_text(label)
					|| t.kinds & BOOL != 0 && matches!(label, "true" | "false");
				if t.kinds == 0 || reachable {
					continue;
				}
				let seen_as = match t.kinds {
					BOOL => "which a `case` sees as `true` or `false`",
					NUM => "which a `case` sees written the way `print` writes it, as `3` or `2.5`",
					_ => "which a `case` sees as `true`, `false` or a number written plainly",
				};
				(
					format!(
						"`{text}` is {}, {seen_as}, so `case \"{label}\"` is never taken",
						types::named(t.kinds)
					),
					None,
				)
			};
			self.report("unreachable-case", at, message, fix);
		}
	}

	/// Where a `case`'s label is written: from its opening quote to its closing
	/// one.
	fn label(&self, c: &MatchCase) -> Span {
		let line = self
			.src
			.get(c.span.start..)
			.and_then(|rest| rest.split('\n').next())
			.unwrap_or_default();
		let Some(open) = line.find('"') else {
			return c.span;
		};
		let close = open + 1 + c.label.len();
		match line.get(open + 1..close) == Some(c.label.as_str()) && line.get(close..close + 1) == Some("\"") {
			true => Span::new(c.span.start + open, c.span.start + close + 1, c.span.line),
			false => c.span,
		}
	}

	/// Walk an expression, answering whether it fails every time it is
	/// evaluated. `root` says it is the whole of what its statement evaluates.
	///
	/// What is found inside a value that always fails is reported; what would
	/// be asked of that value afterwards is not, since the run never gets that
	/// far with it.
	fn expr(&mut self, e: &Expr, root: bool) -> bool {
		match e {
			Expr::Number(..) | Expr::Bool(..) | Expr::Ident(..) | Expr::Source { .. } => false,
			Expr::Str(parts, _) => self.parts(parts),
			Expr::Structured { body, .. } => body.lines.iter().fold(false, |f, l| self.parts(l) | f),
			Expr::List(items, _) => items.iter().fold(false, |f, i| self.expr(i, false) | f),
			Expr::Chain { lhs, rhs, .. } => {
				let (a, b) = (self.expr(lhs, false), self.expr(rhs, false));
				a && b
			}
			Expr::Unary { op, rhs, .. } => {
				if self.expr(rhs, false) {
					return true;
				}
				let (needs, what) = match op {
					UnaryOp::Not => (BOOL, "`!` needs `true` or `false`"),
					UnaryOp::Neg => (NUM, "`-` needs a number"),
				};
				self.operand(rhs, needs, what, None)
			}
			Expr::Binary { op, lhs, rhs, span } => self.binary(*op, lhs, rhs, *span),
			Expr::Index { base, index, .. } => {
				if self.expr(base, false) | self.expr(index, false) {
					return true;
				}
				if self.operand(index, NUM, "an index is a number", None) {
					return true;
				}
				if let Some(x) = literal_num(index)
					&& (x < 0.0 || x.fract() != 0.0)
				{
					let text = self.text(index.span());
					let fix = (x == -1.0).then(|| format!("`last({})` is the last item", self.text(base.span())));
					self.report(
						"invalid-literal",
						index.span(),
						format!("an index counts whole items from 0, so `{text}` is never one"),
						fix,
					);
					return true;
				}
				let fix =
					(self.names.ty(base).kinds == STR).then(|| "`substring(…)` takes part of a string".to_string());
				self.operand(base, LIST, "only a list can be indexed", fix)
			}
			Expr::Call { name, args, span } => self.call(name, args, *span, root),
			Expr::Capture { command, body, span } => {
				let mut fails = command.as_ref().is_some_and(|c| self.parts(c));
				for line in body {
					fails |= self.parts(line);
				}
				if root && self.root == Place::Whole {
					return fails;
				}
				self.unrunnable(self.reaching_marker(*span), "a `$` run");
				true
			}
			Expr::Dispatch { target, args, span } => {
				let mut fails = self.parts(target);
				for a in args {
					fails |= self.parts(a);
				}
				if root && self.root == Place::Whole {
					return fails;
				}
				self.unrunnable(*span, "a `run` dispatch");
				true
			}
		}
	}

	/// `e` has to be one of `needs`; report it when it never is. Answers
	/// whether it was reported.
	fn operand(&mut self, e: &Expr, needs: Kinds, what: &str, fix: Option<String>) -> bool {
		let t = self.names.ty(e);
		if !t.refused(needs) {
			return false;
		}
		let text = self.text(e.span());
		self.report(
			"wrong-type",
			e.span(),
			format!("{what}, and `{text}` is {}", types::named(t.kinds)),
			fix,
		);
		true
	}

	fn binary(&mut self, op: BinaryOp, lhs: &Expr, rhs: &Expr, span: Span) -> bool {
		let (left, right) = (self.expr(lhs, false), self.expr(rhs, false));
		match op {
			BinaryOp::And | BinaryOp::Or => {
				let word = if op == BinaryOp::And { "&&" } else { "||" };
				if left {
					return true;
				}
				let what = format!("`{word}` needs `true` or `false` on each side");
				if self.operand(lhs, BOOL, &what, None) {
					return true;
				}
				// The right side is asked only when the left does not settle it.
				let settled = matches!(lhs, Expr::Bool(b, _) if *b == (op == BinaryOp::Or));
				!settled && !right && self.operand(rhs, BOOL, &what, None)
			}
			BinaryOp::Eq | BinaryOp::Ne => {
				if !(left || right) {
					self.equality(op, lhs, rhs, span);
				}
				left || right
			}
			_ => {
				if left || right {
					return true;
				}
				let sign = match op {
					BinaryOp::Add => "+",
					BinaryOp::Sub => "-",
					BinaryOp::Mul => "*",
					BinaryOp::Div => "/",
					BinaryOp::Rem => "%",
					BinaryOp::Lt => "<",
					BinaryOp::Le => "<=",
					BinaryOp::Gt => ">",
					_ => ">=",
				};
				let joined = op == BinaryOp::Add
					&& [lhs, rhs]
						.iter()
						.any(|e| matches!(e, Expr::Str(..)) && self.names.ty(e).kinds == STR);
				let mut found = false;
				for side in [lhs, rhs] {
					let t = self.names.ty(side);
					let fix = if joined {
						Some(format!(
							"`concat({}, {})` joins text; `+` only adds numbers",
							self.text(lhs.span()),
							self.text(rhs.span())
						))
					} else if t.kinds == STR && literal_str(side).is_none() {
						Some(format!("`number({})` reads it as one", self.text(side.span())))
					} else {
						None
					};
					found |= self.operand(side, NUM, &format!("`{sign}` needs numbers"), fix);
				}
				if !found && matches!(op, BinaryOp::Div | BinaryOp::Rem) && literal_num(rhs) == Some(0.0) {
					self.report(
						"invalid-literal",
						rhs.span(),
						format!("`{sign}` by zero fails every time"),
						None,
					);
					found = true;
				}
				found
			}
		}
	}

	fn equality(&mut self, op: BinaryOp, lhs: &Expr, rhs: &Expr, span: Span) {
		let always = if op == BinaryOp::Eq { "false" } else { "true" };
		let whole = self.text(span);
		let (l, r) = (self.names.ty(lhs), self.names.ty(rhs));
		if l.kinds != 0 && r.kinds != 0 && l.kinds & r.kinds == 0 {
			let fix = self.equality_fix(lhs, l, rhs, r);
			self.report(
				"never-equal",
				span,
				format!(
					"`{whole}` is always {always}: `{}` is {} and `{}` is {}, and a value never equals one of another type",
					self.text(lhs.span()),
					types::named(l.kinds),
					self.text(rhs.span()),
					types::named(r.kinds)
				),
				fix,
			);
			return;
		}
		for (side, other) in [(lhs, rhs), (rhs, lhs)] {
			let (Some(values), Some(written)) = (self.names.values(side), literal_str(other)) else {
				continue;
			};
			if values.contains(written.as_str()) {
				continue;
			}
			self.report(
				"never-equal",
				span,
				format!(
					"`{whole}` is always {always}: `{}` is only ever {}",
					self.text(side.span()),
					listed(&values)
				),
				meant(&written, &values).map(|m| format!("write `\"{m}\"`")),
			);
			return;
		}
	}

	/// What to write instead of a comparison across types, when the mistake is
	/// plain: quotes around a number or a bool, or a string never read as a
	/// number.
	fn equality_fix(&self, lhs: &Expr, l: Ty, rhs: &Expr, r: Ty) -> Option<String> {
		for (side, st, other, ot) in [(lhs, l, rhs, r), (rhs, r, lhs, l)] {
			if let Some(written) = literal_str(side) {
				if ot.kinds == NUM && number_text(&written)
					|| ot.kinds == BOOL && matches!(written.as_str(), "true" | "false")
				{
					return Some(format!("write `{written}` without the quotes"));
				}
			} else if st.kinds == STR && ot.kinds == NUM && !matches!(side, Expr::Str(..)) {
				return Some(format!("`number({})` reads it as a number", self.text(side.span())));
			}
			let _ = other;
		}
		None
	}

	fn call(&mut self, name: &str, args: &[Expr], span: Span, root: bool) -> bool {
		// A `$` run can be the last argument of a call that is the whole value
		// of its statement, and a dispatch the one argument of `code_of` there:
		// that is where the runner runs one before applying the call.
		let whole = root && self.root == Place::Whole;
		let mut fails = false;
		for (k, a) in args.iter().enumerate() {
			let last = k + 1 == args.len();
			fails |= match a {
				Expr::Capture { .. } => self.expr(a, whole && last),
				Expr::Dispatch { .. } => self.expr(a, whole && last && name == "code_of"),
				_ => self.expr(a, false),
			};
		}
		let Some(sig) = types::signature(name) else {
			// A function that does not exist is the name check's to report.
			return true;
		};
		if !sig.takes(args.len()) {
			self.report(
				"arity",
				Span::new(span.start, span.start + name.len(), span.line),
				format!("`{name}` takes {}, and is given {}", sig.arity(), args.len()),
				None,
			);
			return true;
		}
		if fails {
			return true;
		}
		if name == "code_of" && !matches!(args.first(), Some(Expr::Capture { .. } | Expr::Dispatch { .. })) {
			let text = args.first().map(|a| self.text(a.span())).unwrap_or_default();
			self.report(
				"wrong-type",
				args.first().map_or(span, Expr::span),
				format!(
					"`code_of` takes a `$` run or a `run` dispatch, as `code_of($ make)` or `code_of(run build)`, and `{text}` is neither"
				),
				None,
			);
			return true;
		}
		let tys: Vec<Ty> = args.iter().map(|a| self.names.ty(a)).collect();
		let mut found = false;
		for (k, (a, t)) in args.iter().zip(&tys).enumerate() {
			let accepted = sig.accepts(k, &tys);
			if !t.refused(accepted) {
				continue;
			}
			let text = self.text(a.span());
			let place = match sig.max {
				Some(1) => format!("`{name}` takes {}", types::named(accepted)),
				_ => format!(
					"the {} argument of `{name}` has to be {}",
					ordinal(k + 1),
					types::named(accepted)
				),
			};
			let fix = self.argument_fix(name, args, &tys, k, accepted);
			self.report(
				"wrong-type",
				a.span(),
				format!("{place}, and `{text}` is {}", types::named(t.kinds)),
				fix,
			);
			found = true;
		}
		found || self.literals(name, args)
	}

	fn argument_fix(&self, name: &str, args: &[Expr], tys: &[Ty], k: usize, accepted: Kinds) -> Option<String> {
		let t = tys[k];
		let text = self.text(args[k].span());
		if name == "join" && k == 0 && args.len() == 2 && t.kinds == LIST && tys[1].kinds & STR != 0 {
			return Some(format!(
				"the separator comes first: `join({}, {text})`",
				self.text(args[1].span())
			));
		}
		match (t.kinds, accepted) {
			(STR, a) if a & NUM != 0 && literal_str(&args[k]).is_none() => {
				Some(format!("`number({text})` reads it as one"))
			}
			(NUM | BOOL, a) if a & STR != 0 => Some(format!("`\"{{{{ {text} }}}}\"` writes it as text")),
			(LIST, STR) => Some(format!("`join(\" \", {text})` makes one string of it")),
			(STR, LIST) => Some(format!("`lines({text})` or `split({text}, \",\")` makes a list of it")),
			_ => None,
		}
	}

	/// What a call's own arguments can be seen to do wrong, written out.
	fn literals(&mut self, name: &str, args: &[Expr]) -> bool {
		match name {
			"regex_matches" | "regex_replace" | "regex_remove" | "regex_capture" | "regex_capture_all" => {
				if let Some(pattern) = args.get(1).and_then(literal_str)
					&& let Err(e) = regex::Regex::new(&pattern)
				{
					let detail = e.to_string();
					let detail = detail.lines().rfind(|l| !l.trim().is_empty()).unwrap_or_default();
					let detail = detail.trim().trim_start_matches("error: ");
					self.report(
						"invalid-literal",
						args[1].span(),
						format!("`{pattern}` is not a regex that compiles ({detail}), so `{name}` fails every time"),
						None,
					);
					return true;
				}
				if name.starts_with("regex_capture") {
					return self.whole(args.get(2), name, "group", false);
				}
				false
			}
			"glob" => {
				let Some(pattern) = args.first().and_then(literal_str) else {
					return false;
				};
				let Err(e) = crate::functions::glob_plan(&pattern) else {
					return false;
				};
				let detail = e.kind().to_string();
				self.report(
					"invalid-literal",
					args[0].span(),
					format!("`{pattern}` is not a glob that compiles ({detail}), so `glob` fails every time"),
					None,
				);
				true
			}
			"printf" => self.format(args),
			"now" => {
				let Some(format) = args.first().and_then(literal_str) else {
					return false;
				};
				if crate::functions::now_formatted(&format).is_some() {
					return false;
				}
				self.report(
					"invalid-literal",
					args[0].span(),
					format!(
						"`{format}` is not a time format; `now` takes unix, unix-ms, iso, iso-date, iso-time, year, month, day, hour, minute or second"
					),
					None,
				);
				true
			}
			"substring" => {
				self.whole(args.get(1), name, "start", false) | self.whole(args.get(2), name, "length", false)
			}
			"repeat" => self.whole(args.get(1), name, "count", false),
			"range" => self.whole(args.first(), name, "bound", true) | self.whole(args.get(1), name, "bound", true),
			"sleep" => {
				let Some(x) = args.first().and_then(literal_num).filter(|x| *x < 0.0) else {
					return false;
				};
				self.report(
					"invalid-literal",
					args[0].span(),
					format!(
						"`sleep` waits a number of seconds that is not negative, and `{}` is",
						crate::value::format_num(x)
					),
					None,
				);
				true
			}
			_ => false,
		}
	}

	/// An argument that has to be a whole number -- and not a negative one,
	/// unless `signed` -- when it is written out as one that is not.
	fn whole(&mut self, arg: Option<&Expr>, name: &str, what: &str, signed: bool) -> bool {
		let Some((arg, x)) = arg.and_then(|a| literal_num(a).map(|x| (a, x))) else {
			return false;
		};
		if x.fract() == 0.0 && (signed || x >= 0.0) {
			return false;
		}
		let kind = if signed {
			"a whole number"
		} else {
			"a whole number that is not negative"
		};
		self.report(
			"invalid-literal",
			arg.span(),
			format!(
				"the {what} `{name}` takes is {kind}, and `{}` is not one",
				self.text(arg.span())
			),
			None,
		);
		true
	}

	/// A `printf` whose format is written out: what it substitutes, and the
	/// values handed to it.
	fn format(&mut self, args: &[Expr]) -> bool {
		let Some(fmt) = args.first().and_then(literal_str) else {
			return false;
		};
		let at = args[0].span();
		let pieces = match crate::functions::parse_format(&fmt) {
			Ok(p) => p,
			Err(why) => {
				self.report(
					"invalid-literal",
					at,
					format!("{why}, so `printf` fails every time"),
					None,
				);
				return true;
			}
		};
		let subs: Vec<char> = pieces
			.iter()
			.filter_map(|p| match p {
				crate::functions::Piece::Sub(verb, _) => Some(*verb),
				crate::functions::Piece::Text(_) => None,
			})
			.collect();
		if let Some(why) = crate::functions::format_count(subs.len(), args.len() - 1) {
			self.report(
				"invalid-literal",
				at,
				format!("{why}, so `printf` fails every time"),
				None,
			);
			return true;
		}
		let mut found = false;
		for (verb, arg) in subs.iter().zip(&args[1..]) {
			if *verb == 's' {
				continue;
			}
			let fix = (self.names.ty(arg).kinds == STR && literal_str(arg).is_none())
				.then(|| format!("`number({})` reads it as one", self.text(arg.span())));
			if self.operand(arg, NUM, &format!("`%{verb}` puts in a number"), fix) {
				found = true;
			} else if *verb == 'd'
				&& let Some(x) = literal_num(arg).filter(|x| x.fract() != 0.0)
			{
				self.report(
					"invalid-literal",
					arg.span(),
					format!(
						"`%d` puts in a whole number, and `{}` is not one",
						crate::value::format_num(x)
					),
					Some("`%f` puts in any number".into()),
				);
				found = true;
			}
		}
		found
	}

	/// A capture's span, reaching back to its `$` when the tree placed it at the
	/// command -- which some forms do -- so the underline covers what was written.
	fn reaching_marker(&self, span: Span) -> Span {
		let before = self.src.get(..span.start).unwrap_or_default();
		let blanks = before.len() - before.trim_end_matches([' ', '\t']).len();
		match before[..before.len() - blanks].ends_with('$') {
			true => Span::new(span.start - blanks - 1, span.end, span.line),
			false => span,
		}
	}

	fn unrunnable(&mut self, span: Span, what: &str) {
		let (message, fix) = match self.root {
			Place::Asked(keyword) => (
				format!(
					"`{keyword}` runs a `$` run only when it is the whole of what it asks, as `{keyword} $ …`: inside a call there is nothing to run {what} in, so this fails every time"
				),
				Some(format!(
					"bind it on a line above -- `let out = …` -- and ask about that here; `{keyword} $ …` alone asks about the command's exit status"
				)),
			),
			Place::Shared => (
				format!(
					"a `_shared.run` works out its `let`s before any target runs, with nothing to run {what} in, so this fails for every target below it"
				),
				Some("do it in the targets that need it".into()),
			),
			Place::Whole | Place::Inner => (
				format!(
					"there is nothing to run {what} in here: the runner runs one only as the whole value of a `let`, a line of its own or a `for`, or as the last argument of a call that is"
				),
				Some("bind it on a line above, and use the name here".into()),
			),
		};
		self.report("capture-position", span, message, fix);
	}
}
