//! Names nothing defines: a call to a function the language does not have, and
//! a read of a binding no line before it can have made.
//!
//! Both used to be found by running into them. `exists("x")` parsed, sat in a
//! branch that runs once a month, and failed the month it did -- and a typo in
//! a name read under a `?` never failed at all: the fallback was taken every
//! time, and the value the author meant was never read. Neither needs a run to
//! answer. Every function is listed, and the language has no way to make a
//! binding except by writing one down, so what is in scope at a line is a
//! question about the text above it.
//!
//! **What is in scope follows the runner exactly**, because an answer that
//! disagrees with it either refuses a file that runs or passes one that does
//! not:
//!
//! * Bindings are flat. A `let` inside an `if` is still bound after its `end`,
//!   so a name bound on *some* path to a line counts -- whether that path is
//!   taken is the run's question, not this one's.
//! * A loop's own names are put back when the loop ends. A sequential `for`
//!   puts them aside before its body's properties are worked out, too, and
//!   those are worked out once, before the first item is bound.
//! * A pass of a loop, or an attempt of a `retry`, sees what the one before it
//!   bound, so a name bound anywhere in the body counts anywhere in it.
//! * A parallel branch binds on a copy: nothing it binds reaches a sibling or
//!   the code after the block.
//! * A `_shared.run` binds with its top-level `let`s and nothing else, since
//!   nothing else in it runs.
//!
//! So a name is **unresolved where no binding of it can have run before it, on
//! any path**. That is exact in the direction that matters: a file whose every
//! read can be reached with its name bound is never refused. A read on a path
//! where the name happens not to be bound is still the run's to report.
//!
//! A read under a `?` is checked like any other. The chain catches a failure,
//! but a name nothing binds is not a failure that comes and goes: its fallback
//! is taken every time, which is a typo hiding rather than a name being
//! optional.

use crate::ast::{Block, Expr, InterpPart, Property, SourceKind, Statement, Target};
use crate::span::Span;
use std::collections::{BTreeMap, BTreeSet};

/// One name that does not resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
	pub kind: Kind,
	pub name: String,
	/// Where the name is written, as exactly as the tree knows it: the name
	/// itself, for a call or a read. A reassignment's names are kept without
	/// positions of their own, so for [`Kind::Rebind`] it is the statement's,
	/// whose first line the name opens.
	pub span: Span,
	/// What else is worth saying: the name this is probably a slip for, or why
	/// a name bound elsewhere in the file does not reach this line.
	pub hint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
	/// `exists(…)`: a call to a function the language does not have.
	Function,
	/// A read of a name nothing binds before it.
	Name,
	/// `x = …` with no `let x` before it: a reassignment of nothing. The runner
	/// would bind it, since it does not ask -- and that is how a slip in the
	/// name being updated goes unseen, while the name that was meant keeps its
	/// old value.
	Rebind,
	/// `RUN.oss`: a key `RUN` does not have. The runner fills in a fixed set,
	/// so which keys exist is never in doubt, the way which functions exist is
	/// not.
	RunKey,
}

impl std::fmt::Display for Unresolved {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		let name = &self.name;
		write!(f, "line {}: ", self.span.line)?;
		match self.kind {
			Kind::Function => write!(f, "unknown function `{name}`")?,
			Kind::Name => write!(f, "`{name}` is not defined")?,
			Kind::Rebind => write!(
				f,
				"`{name}` is not defined, so there is nothing to reassign -- bind it with `let {name} = …`"
			)?,
			Kind::RunKey => write!(f, "unknown `{name}`")?,
		}
		match &self.hint {
			Some(h) => write!(f, "; {h}"),
			None => Ok(()),
		}
	}
}

/// Every name a target uses that does not resolve, under the `_shared.run`
/// chain above it, outermost first.
pub fn of_chain(target: &Target, shared: &[Target]) -> Vec<Unresolved> {
	let mut w = Walk::new(true);
	let mut sc = Scope::default();
	sc.bind(shared.iter().flat_map(exported));
	w.block(&target.body, &mut sc);
	w.out
}

/// Every name a `_shared.run` uses that does not resolve, under the ones above
/// it, outermost first.
///
/// Folded rather than walked, the way the runner applies one: its properties
/// and its top-level `let`s, in order. Everything else in it is still checked
/// -- a slip there is a slip -- but never runs, so nothing it binds reaches the
/// line after it.
pub fn of_shared(file: &Target, above: &[Target]) -> Vec<Unresolved> {
	let mut w = Walk::new(true);
	let mut sc = Scope::default();
	sc.bind(above.iter().flat_map(exported));
	for p in file.body.declaration() {
		w.property(p, &sc);
	}
	w.body(&file.body, &mut sc, |w, st, sc| match st {
		Statement::Let { .. } => w.statement(st, sc),
		_ => w.statement(st, &mut sc.clone()),
	});
	w.out
}

/// Only the calls to functions that do not exist.
///
/// For a file whose chain cannot be read -- a `_shared.run` above it that does
/// not parse -- so which names reach it is not known, and reporting them would
/// be a guess. Which functions exist is never in doubt.
pub fn functions(file: &Target) -> Vec<Unresolved> {
	let mut w = Walk::new(false);
	w.block(&file.body, &mut Scope::default());
	w.out
}

/// What a `_shared.run` binds for the files below it: its top-level `let`s.
fn exported(file: &Target) -> impl Iterator<Item = &str> {
	file.body
		.statements
		.iter()
		.flat_map(|st| match st {
			Statement::Let { names, .. } => names.as_slice(),
			_ => &[],
		})
		.map(String::as_str)
}

/// What a loop's names were bound as, after the loop.
const AFTER_LOOP: &str = "a loop's names end with the loop";

/// What a parallel branch bound, after the block.
const AFTER_BRANCH: &str = "it is bound inside a `parallel` branch, and what a branch binds ends with it";

/// A loop's name, read by a property at the top of its body.
const BEFORE_ITEM: &str = "a property at the top of a loop's body is worked out once, before the loop binds \
                           its names -- write it below the body's first statement";

/// What can be bound at one point of a walk.
#[derive(Clone, Default)]
struct Scope {
	/// Every name some path to this point binds.
	bound: BTreeSet<String>,
	/// Names bound elsewhere that do not reach this point, and why not -- a
	/// more useful thing to say than that the name does not exist.
	gone: BTreeMap<String, &'static str>,
}

impl Scope {
	fn has(&self, name: &str) -> bool {
		self.bound.contains(name)
	}

	fn bind<'a>(&mut self, names: impl IntoIterator<Item = &'a str>) {
		for n in names.into_iter().filter(|n| *n != "_") {
			self.gone.remove(n);
			self.bound.insert(n.to_string());
		}
	}

	/// Take `name` out of scope, and say why when it is read.
	fn put_aside(&mut self, name: &str, why: &'static str) {
		self.bound.remove(name);
		self.gone.insert(name.to_string(), why);
	}

	/// Everything some path through `other` binds is bound here too: `other`
	/// is one way the code before this point can have gone.
	fn absorb(&mut self, other: Scope) {
		for (n, why) in other.gone {
			if !self.bound.contains(&n) {
				self.gone.entry(n).or_insert(why);
			}
		}
		for n in other.bound {
			self.gone.remove(&n);
			self.bound.insert(n);
		}
	}

	/// What `branch` bound that is not bound here ends with the branch.
	fn forget(&mut self, branch: &Scope, why: &'static str) {
		for n in branch.bound.difference(&self.bound) {
			self.gone.entry(n.clone()).or_insert(why);
		}
	}
}

struct Walk {
	out: Vec<Unresolved>,
	/// Whether reads are checked at all. Off when what the chain above the file
	/// binds is not known.
	names: bool,
}

impl Walk {
	fn new(names: bool) -> Self {
		Walk { out: Vec::new(), names }
	}

	/// A block as the runner walks one: the properties above its first
	/// statement against the scope it is entered with, then everything else in
	/// source order.
	fn block(&mut self, b: &Block, sc: &mut Scope) {
		for p in b.declaration() {
			self.property(p, sc);
		}
		self.body(b, sc, |w, st, sc| w.statement(st, sc));
	}

	/// A block's statements, with any property written between them read where
	/// it sits. `each` walks one statement.
	fn body(&mut self, b: &Block, sc: &mut Scope, each: impl Fn(&mut Walk, &Statement, &mut Scope)) {
		let mut trailing = b.trailing().iter().peekable();
		for st in &b.statements {
			while let Some(p) = trailing.next_if(|p| p.span.line < st.span().line) {
				self.property(p, sc);
			}
			each(self, st, sc);
		}
		for p in trailing {
			self.property(p, sc);
		}
	}

	fn property(&mut self, p: &Property, sc: &Scope) {
		if let Some(v) = &p.value {
			self.expr(v, sc);
		}
	}

	fn statement(&mut self, st: &Statement, sc: &mut Scope) {
		match st {
			Statement::Let { names, value, .. } => {
				self.expr(value, sc);
				sc.bind(names.iter().map(String::as_str));
			}
			Statement::Assign { names, value, span } => {
				self.expr(value, sc);
				for n in names.iter().filter(|n| *n != "_") {
					if self.names && !sc.has(n) {
						let hint = match sc.gone.get(n.as_str()) {
							Some(why) => Some((*why).to_string()),
							None => suggest(n, sc.bound.iter().map(String::as_str)),
						};
						self.report(Kind::Rebind, n, *span, hint);
					}
				}
				// Bound from here on regardless, so one missing `let` is one
				// message rather than one for every line below that reads it.
				sc.bind(names.iter().map(String::as_str));
			}
			Statement::Call { expr, .. } => self.expr(expr, sc),
			Statement::Do {
				body, parallel: false, ..
			} => self.block(body, sc),
			// Every statement directly inside is a branch, set up in order and
			// walked on a copy of the block's scope.
			Statement::Do {
				body, parallel: true, ..
			} => {
				for p in body.declaration() {
					self.property(p, sc);
				}
				self.body(body, sc, |w, st, sc| {
					let mut branch = sc.clone();
					w.statement(st, &mut branch);
					sc.forget(&branch, AFTER_BRANCH);
				});
			}
			Statement::If {
				cond, then, otherwise, ..
			} => {
				self.expr(cond, sc);
				let mut taken = sc.clone();
				self.block(then, &mut taken);
				if let Some(o) = otherwise {
					let mut other = sc.clone();
					self.block(o, &mut other);
					sc.absorb(other);
				}
				sc.absorb(taken);
			}
			Statement::Match {
				subject,
				cases,
				default,
				..
			} => {
				self.expr(subject, sc);
				let before = sc.clone();
				for body in cases.iter().map(|c| &c.body).chain(default) {
					let mut case = before.clone();
					self.block(body, &mut case);
					sc.absorb(case);
				}
			}
			Statement::Retry {
				attempts,
				delay,
				body,
				otherwise,
				..
			} => {
				self.expr(attempts, sc);
				if let Some(d) = delay {
					self.expr(d, sc);
				}
				// Worked out once, before the first attempt.
				for p in body.declaration() {
					self.property(p, sc);
				}
				let mut attempt = sc.clone();
				attempt.bind(carried(body).iter().map(String::as_str));
				self.body(body, &mut attempt, |w, st, sc| w.statement(st, sc));
				sc.absorb(attempt);
				// Once every attempt has failed, after at least one of them, so
				// whatever they bound may be there.
				if let Some(o) = otherwise {
					let mut other = sc.clone();
					self.block(o, &mut other);
					sc.absorb(other);
				}
			}
			Statement::For {
				names,
				iter,
				body,
				parallel,
				..
			} => self.for_loop(names, iter, body, *parallel, sc),
			Statement::Loop { test, body, .. } => {
				// Worked out once, before the condition is first asked.
				for p in body.declaration() {
					self.property(p, sc);
				}
				let mut pass = sc.clone();
				pass.bind(carried(body).iter().map(String::as_str));
				// Asked again after every pass, of what the passes bound.
				if let Some(c) = test.cond() {
					self.expr(c, &pass);
				}
				self.body(body, &mut pass, |w, st, sc| w.statement(st, sc));
				sc.absorb(pass);
			}
			Statement::Break { .. } | Statement::Continue { .. } => {}
			Statement::Run { target, args, .. } => {
				self.parts(target, sc);
				for a in args {
					self.parts(a, sc);
				}
			}
			Statement::Exec { command, body, .. } => {
				if let Some(c) = command {
					self.parts(c, sc);
				}
				for line in body {
					self.parts(line, sc);
				}
			}
		}
	}

	fn for_loop(&mut self, names: &[String], iter: &Expr, body: &Block, parallel: bool, sc: &mut Scope) {
		self.expr(iter, sc);
		let own: Vec<&str> = names.iter().map(String::as_str).filter(|n| *n != "_").collect();
		let before = sc.clone();
		// The body's properties are worked out once, before any item is bound.
		// A sequential `for` puts its names aside first, so not even a binding
		// of one from above the loop reaches them; a `parallel for` leaves them
		// as they were.
		let mut header = sc.clone();
		for n in &own {
			if !parallel || !header.has(n) {
				header.put_aside(n, BEFORE_ITEM);
			}
		}
		for p in body.declaration() {
			self.property(p, &header);
		}
		let mut pass = sc.clone();
		pass.bind(own.iter().copied());
		if !parallel {
			// A pass sees what the passes before it bound. An iteration of a
			// `parallel for` is a branch of its own, and sees no other's.
			pass.bind(carried(body).iter().map(String::as_str));
		}
		self.body(body, &mut pass, |w, st, sc| w.statement(st, sc));
		// The loop's names are put back as it ends: bound again if they were
		// bound before it, and gone if they were not.
		let ended: Vec<&str> = own.into_iter().filter(|n| !before.has(n)).collect();
		if parallel {
			for n in &ended {
				pass.bound.remove(*n);
				sc.gone.entry((*n).to_string()).or_insert(AFTER_LOOP);
			}
			sc.forget(&pass, AFTER_BRANCH);
		} else {
			sc.absorb(pass);
			for n in ended {
				sc.put_aside(n, AFTER_LOOP);
			}
		}
	}

	fn parts(&mut self, ps: &[InterpPart], sc: &Scope) {
		for p in ps {
			if let InterpPart::Expr(e) = p {
				self.expr(e, sc);
			}
		}
	}

	fn expr(&mut self, e: &Expr, sc: &Scope) {
		match e {
			Expr::Number(..) | Expr::Bool(..) => {}
			Expr::Source {
				kind: SourceKind::Run,
				key: Some(key),
				span,
			} if !crate::eval::RUN_KEYS.contains(&key.as_str()) => {
				let name = format!("RUN.{key}");
				let keys: Vec<String> = crate::eval::RUN_KEYS.iter().map(|k| format!("RUN.{k}")).collect();
				let hint = suggest(&name, keys.iter().map(String::as_str));
				self.report(Kind::RunKey, &name, *span, hint);
			}
			Expr::Source { .. } => {}
			Expr::Ident(name, span) => {
				if self.names && !sc.has(name) {
					let hint = read_hint(name, sc);
					self.report(Kind::Name, name, *span, hint);
				}
			}
			Expr::Str(ps, _) => self.parts(ps, sc),
			Expr::List(items, _) => items.iter().for_each(|i| self.expr(i, sc)),
			Expr::Unary { rhs, .. } => self.expr(rhs, sc),
			Expr::Binary { lhs, rhs, .. } | Expr::Chain { lhs, rhs, .. } => {
				self.expr(lhs, sc);
				self.expr(rhs, sc);
			}
			Expr::Index { base, index, .. } => {
				self.expr(base, sc);
				self.expr(index, sc);
			}
			Expr::Call { name, args, span } => {
				if !crate::functions::exists(name) {
					let at = Span::new(span.start, span.start + name.len(), span.line);
					let hint = suggest(name, crate::functions::names());
					self.report(Kind::Function, name, at, hint);
				}
				args.iter().for_each(|a| self.expr(a, sc));
			}
			Expr::Structured { body, .. } => body.lines.iter().for_each(|l| self.parts(l, sc)),
			Expr::Capture { command, body, .. } => {
				if let Some(c) = command {
					self.parts(c, sc);
				}
				body.iter().for_each(|l| self.parts(l, sc));
			}
			Expr::Dispatch { target, args, .. } => {
				self.parts(target, sc);
				args.iter().for_each(|a| self.parts(a, sc));
			}
		}
	}

	fn report(&mut self, kind: Kind, name: &str, span: Span, hint: Option<String>) {
		self.out.push(Unresolved {
			kind,
			name: name.to_string(),
			span,
			hint,
		});
	}
}

/// Every name a loop's body can leave bound, which a later pass may read
/// before the line that binds it has come round again.
fn carried(body: &Block) -> BTreeSet<String> {
	let mut out = BTreeSet::new();
	binds(body, &mut out);
	out
}

/// Every name a block can leave bound behind it: its `let`s and reassignments,
/// and those of every block inside it that runs in order.
///
/// Not a loop's own names, which the runner puts back as the loop ends, and
/// nothing a parallel block binds, which is bound on copies.
fn binds(b: &Block, out: &mut BTreeSet<String>) {
	for st in &b.statements {
		match st {
			Statement::Let { names, .. } | Statement::Assign { names, .. } => {
				out.extend(names.iter().filter(|n| *n != "_").cloned());
			}
			Statement::Do {
				body, parallel: false, ..
			}
			| Statement::Loop { body, .. } => binds(body, out),
			Statement::For {
				names,
				body,
				parallel: false,
				..
			} => {
				let mut inner = BTreeSet::new();
				binds(body, &mut inner);
				out.extend(inner.into_iter().filter(|n| !names.contains(n)));
			}
			Statement::If { then, otherwise, .. }
			| Statement::Retry {
				body: then, otherwise, ..
			} => {
				binds(then, out);
				if let Some(o) = otherwise {
					binds(o, out);
				}
			}
			Statement::Match { cases, default, .. } => {
				for body in cases.iter().map(|c| &c.body).chain(default) {
					binds(body, out);
				}
			}
			Statement::Do { parallel: true, .. }
			| Statement::For { parallel: true, .. }
			| Statement::Call { .. }
			| Statement::Break { .. }
			| Statement::Continue { .. }
			| Statement::Run { .. }
			| Statement::Exec { .. } => {}
		}
	}
}

/// What to say about a read of a name that does not reach it.
fn read_hint(name: &str, sc: &Scope) -> Option<String> {
	// Named without its parentheses: a one-word fix rather than a missing
	// binding, and the same thing the evaluator says.
	if crate::functions::exists(name) {
		return Some(format!("`{name}` is a function; call it as `{name}()`"));
	}
	if let Some(why) = sc.gone.get(name) {
		return Some((*why).to_string());
	}
	suggest(
		name,
		sc.bound.iter().map(String::as_str).chain(["ARGS", "true", "false"]),
	)
}

/// "did you mean …?", when something is near enough to be worth the guess.
///
/// Near is, best first: the same word in another case; one letter out; one
/// word of a longer name, for a name long enough to mean something on its own
/// -- `exists` is how `file_exists` gets misremembered, and no small count of
/// edits reaches it; then two letters out, for a longer name. Only the best of
/// those that turn anything up is offered: `exit` is two letters from `exists`,
/// and a worse guess beside a good one only makes the good one harder to see.
pub fn suggest<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<String> {
	let len = name.chars().count();
	let mut near: Vec<(u8, &str)> = candidates
		.into_iter()
		.filter(|c| *c != name)
		.filter_map(|c| {
			if c.eq_ignore_ascii_case(name) {
				return Some((0, c));
			}
			match edits(name, c) {
				1 if len >= 3 => Some((1, c)),
				_ if len >= 4 && c.split('_').any(|word| word == name) => Some((2, c)),
				2 if len >= 6 => Some((3, c)),
				_ => None,
			}
		})
		.collect();
	near.sort();
	near.dedup();
	let best = near.first()?.0;
	let shown: Vec<String> = near
		.iter()
		.take_while(|(tier, _)| *tier == best)
		.take(3)
		.map(|(_, c)| format!("`{c}`"))
		.collect();
	match shown.as_slice() {
		[one] => Some(format!("did you mean {one}?")),
		[init @ .., last] => Some(format!("did you mean {} or {last}?", init.join(", "))),
		[] => None,
	}
}

/// How many single-character edits turn `a` into `b`.
pub fn edits(a: &str, b: &str) -> usize {
	let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
	let mut prev: Vec<usize> = (0..=b.len()).collect();
	let mut cur = vec![0; b.len() + 1];
	for i in 1..=a.len() {
		cur[0] = i;
		for j in 1..=b.len() {
			let sub = prev[j - 1] + usize::from(a[i - 1] != b[j - 1]);
			cur[j] = sub.min(prev[j] + 1).min(cur[j - 1] + 1);
		}
		std::mem::swap(&mut prev, &mut cur);
	}
	prev[b.len()]
}
