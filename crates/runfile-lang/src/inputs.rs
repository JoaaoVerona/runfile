//! What a target actually reads: `ARG.x`, `FLAG.x`, `ENV.X`, `ARGS` -- and for
//! each one, whether it can fail for want of a value, and what it falls back
//! to when it cannot.
//!
//! Walked from the **tree**, not the text. Scanning for `ARG.` was exact
//! enough to be tempting -- the keys have no dynamic form, so every use is
//! spelled out -- but text is not only the program: a name mentioned in a
//! comment, in a string, or in the literal half of a `$` line counted as a
//! use. That made `--help` list inputs the target never reads, and, worse, it
//! *suppressed* the unread-flag warning: a comment saying `ARG.legacy` was
//! enough for a mistyped `--legacy` to pass without a word.
//!
//! There is nothing to guess. `ARG[k]`, `ARG.{{ k }}` and `ARG."k"` are all
//! refused by the parser, so a key is a literal in the tree or it is not a key
//! at all. That is what lets `--help` describe a target's inputs, and
//! `--stdin-args` ask for them **before** anything runs rather than when a
//! statement happens to reach one.

use crate::ast::{Block, Expr, InterpPart, SourceKind, Statement, Target};
use std::collections::{BTreeMap, BTreeSet};

/// How one name is read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Use {
	/// What a `?` chain falls back to, when it ends in a literal.
	/// `ARG.port ? ENV.PORT ? "3000"` defaults to `3000`: the outermost
	/// fallback is what is reached once everything before it is missing.
	pub default: Option<String>,
	/// Whether some use of it has **no** fallback, so a run can fail on it.
	/// One guarded use does not make a name optional while another is bare.
	pub required: bool,
	/// What a `one_of` checking it accepts, when every option is written out
	/// as a string: `one_of(ARG.env, "staging", "production")` is the file
	/// saying which values the name takes, so `--help` lists them and
	/// `--stdin-args` offers them. Empty when two checks list different values,
	/// since neither list is then the answer.
	pub choices: Option<Vec<String>>,
}

/// The inputs a target reads, sorted and without repeats.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Inputs {
	pub args: BTreeMap<String, Use>,
	/// Flags are never required and have no default: an absent one is `false`.
	pub flags: BTreeSet<String>,
	pub env: BTreeMap<String, Use>,
	/// Whether the positional arguments are read at all.
	pub positional: bool,
	/// What a `one_of` around `first(ARGS)` or `ARGS[0]` accepts, as
	/// [`Use::choices`] says -- the idiom `run release minor` is written in.
	pub positional_choices: Option<Vec<String>>,
}

fn merge(into: &mut BTreeMap<String, Use>, name: &str, u: Use) {
	let slot = into.entry(name.to_string()).or_default();
	slot.required |= u.required;
	if slot.default.is_none() {
		slot.default = u.default;
	}
	narrow(&mut slot.choices, u.choices);
}

/// Add one check's options to what a name is known to accept. A use with no
/// `one_of` around it says nothing either way; two that list different values
/// leave no list to show, because a value one accepts may be one the other
/// refuses -- and showing either would be choosing between them.
fn narrow(known: &mut Option<Vec<String>>, with: Option<Vec<String>>) {
	match (known.as_ref(), with) {
		(_, None) => {}
		(None, Some(c)) => *known = Some(c),
		(Some(k), Some(c)) if k.len() != c.len() || !c.iter().all(|o| k.contains(o)) => *known = Some(Vec::new()),
		(Some(_), Some(_)) => {}
	}
}

/// Environment names set where a read happens, on **every** path to it.
///
/// A read of one is not an input at all: the assignment beats whatever the
/// caller exports, so nothing they pass can reach it -- listing it would tell
/// someone to supply a value that is then ignored. Matched exactly, as the line
/// sets it: `ENV.X` looks the name up exactly before it tries any other case,
/// so a caller's `PORT` still reaches a read of `ENV.PORT` under an
/// `ENV.port = …`, and that read stays an input.
type Set = BTreeSet<String>;

/// What the walk knows at one point of a file.
///
/// Every name in `set` has been set on every path that reaches the point. That
/// is the one direction this can afford to be wrong in: a name left out is
/// listed as an input the caller need not pass, while a name wrongly in it
/// would hide one they must.
#[derive(Clone, Default)]
struct Flow {
	set: Set,
	/// Whether `ignore_errors()` may be in effect. Under it, an `ENV.X = …`
	/// whose value fails is forgiven and sets nothing, so only one that cannot
	/// fail -- a value written out -- counts as setting its name.
	forgiving: bool,
}

/// Where two paths meet: set on both, and forgiving on either.
fn meet(a: Option<Flow>, b: Option<Flow>) -> Option<Flow> {
	match (a, b) {
		(None, x) | (x, None) => x,
		(Some(a), Some(b)) => Some(Flow {
			set: a.set.intersection(&b.set).cloned().collect(),
			forgiving: a.forgiving || b.forgiving,
		}),
	}
}

/// Where a source sits: whether a failure there is caught, what the chain
/// around it falls back to, which names are already set in the environment it
/// reads, and what a `one_of` around it accepts.
#[derive(Clone, Copy)]
struct At<'a> {
	guarded: bool,
	default: Option<&'a str>,
	set: &'a Set,
	/// Only ever the `one_of`'s subject itself, or a `?` chain that is: a
	/// value worked out *from* an input (`lower(ARG.x)`) is what the options
	/// describe, not the input.
	choices: Option<&'a [String]>,
}

impl<'a> At<'a> {
	fn plain(set: &'a Set) -> Self {
		At {
			guarded: false,
			default: None,
			set,
			choices: None,
		}
	}

	/// The same environment, with no guard or fallback of its own: what a
	/// sub-expression that is not on a `?` chain's left spine is read under.
	fn unguarded(self) -> Self {
		At::plain(self.set)
	}
}

pub fn of(target: &Target) -> Inputs {
	of_chain(target, &[])
}

/// What a target reads under the `_shared.run` chain above it, outermost
/// first -- the one description of it, so `--help`, `--stdin-args` and the
/// command line cannot disagree about what a target takes.
///
/// A `_shared.run` runs ahead of the target, so what it sets is set for the
/// target too: the walk goes straight through the chain and into the file,
/// following the runner's rules -- a change lasts past the block it is written
/// in, a `do` puts the environment back at its `end`, and a branch changes a
/// copy -- since `--help` saying a name is not needed when the runner then
/// reads the caller's would be a lie.
pub fn of_chain(target: &Target, shared: &[Target]) -> Inputs {
	let mut out = Inputs::default();
	let mut flow = Flow::default();
	for s in shared {
		flow = walk(&s.body, &mut out, flow.clone()).unwrap_or(flow);
	}
	walk(&target.body, &mut out, flow);
	out
}

/// Whether a value is written out, and so cannot fail.
fn constant(e: &Expr) -> bool {
	match e {
		Expr::Number(..) | Expr::Bool(..) => true,
		Expr::Str(ps, _) => ps.iter().all(|p| matches!(p, InterpPart::Literal(_))),
		_ => false,
	}
}

/// Whether a statement ends the block it is in, every time: an `exit()` or
/// `error()` of its own, a `break` or a `continue`. Only what is certain, since
/// a path wrongly called a dead end is one whose names would be counted as set.
fn leaves(s: &Statement) -> bool {
	match s {
		Statement::Break { .. } | Statement::Continue { .. } => true,
		Statement::Call {
			expr: Expr::Call { name, .. },
			..
		} => matches!(name.as_str(), "exit" | "error"),
		_ => false,
	}
}

/// One block, the way the runner walks it: its properties, then every
/// statement in order, each reading what the lines above it set on every path.
/// Hands back what is known as the block finishes, or `None` when it never
/// does -- every path through it leaves first.
fn walk(b: &Block, out: &mut Inputs, mut flow: Flow) -> Option<Flow> {
	for p in &b.properties {
		if let Some(e) = &p.value {
			expr(e, out, At::plain(&flow.set));
		}
	}
	for st in &b.statements {
		flow = statement(st, out, flow)?;
	}
	Some(flow)
}

fn statement(s: &Statement, out: &mut Inputs, flow: Flow) -> Option<Flow> {
	let plain = At::plain(&flow.set);
	match s {
		Statement::Let { value, .. } | Statement::Assign { value, .. } => expr(value, out, plain),
		Statement::SetEnv { name, value, .. } => {
			expr(value, out, plain);
			let mut flow = flow.clone();
			if !flow.forgiving || constant(value) {
				flow.set.insert(name.clone());
			}
			return Some(flow);
		}
		Statement::Call { expr: e, .. } => {
			expr(e, out, plain);
			if leaves(s) {
				return None;
			}
			if let Some(("ignore_errors", _)) = s.effect() {
				return Some(Flow {
					forgiving: true,
					..flow
				});
			}
		}
		// A `do` puts the environment back at its `end`, so what it sets is not
		// set after it -- but a block every path through which leaves is one
		// nothing after it is reached from.
		Statement::Do {
			body, parallel: false, ..
		} => {
			walk(body, out, flow.clone())?;
		}
		// Every branch works on a copy.
		Statement::Do {
			body, parallel: true, ..
		} => {
			for st in &body.statements {
				statement(st, out, flow.clone());
			}
		}
		Statement::If {
			cond, then, otherwise, ..
		} => {
			expr(cond, out, plain);
			let taken = walk(then, out, flow.clone());
			let other = match otherwise {
				Some(b) => walk(b, out, flow.clone()),
				None => Some(flow.clone()),
			};
			return meet(taken, other);
		}
		Statement::Match {
			subject,
			cases,
			default,
			..
		} => {
			expr(subject, out, plain);
			let mut after = None;
			for c in cases {
				after = meet(after, walk(&c.body, out, flow.clone()));
			}
			after = meet(
				after,
				match default {
					Some(b) => walk(b, out, flow.clone()),
					// No case matching fails the target, unless that is forgiven.
					None => flow.forgiving.then(|| flow.clone()),
				},
			);
			return after;
		}
		// Every attempt starts from the state the `retry` began with, and the one
		// that succeeds is kept -- so the body is walked once, from here. When
		// none does, the `else` runs from here too; without one the `retry`
		// fails, unless that is forgiven.
		Statement::Retry {
			attempts,
			delay,
			body,
			otherwise,
			..
		} => {
			expr(attempts, out, plain);
			if let Some(d) = delay {
				expr(d, out, plain);
			}
			let attempt = walk(
				body,
				out,
				Flow {
					forgiving: false,
					..flow.clone()
				},
			)
			.map(|f| Flow {
				forgiving: flow.forgiving,
				..f
			});
			let failed = match otherwise {
				Some(b) => walk(b, out, flow.clone()),
				None => flow.forgiving.then(|| flow.clone()),
			};
			return meet(attempt, failed);
		}
		// A loop may run no passes at all, so nothing it sets is certain after
		// it. Its first pass reads what was set before it, which is the pass a
		// missing name fails on.
		Statement::For { iter, body, .. } => {
			expr(iter, out, plain);
			walk(body, out, flow.clone());
		}
		Statement::Loop { test, body, .. } => {
			if let Some(c) = test.cond() {
				expr(c, out, plain);
			}
			walk(body, out, flow.clone());
		}
		Statement::Break { .. } | Statement::Continue { .. } => return None,
		Statement::Run { target, args, .. } => {
			parts(target, out, &flow.set);
			for a in args {
				parts(a, out, &flow.set);
			}
		}
		Statement::Exec { command, body, .. } => {
			if let Some(c) = command {
				parts(c, out, &flow.set);
			}
			for line in body {
				parts(line, out, &flow.set);
			}
		}
	}
	Some(flow)
}

/// The interpolations in a run of text. The literal halves are the shell's, or
/// somebody else's language, and are not this one's to read names out of.
fn parts(ps: &[InterpPart], out: &mut Inputs, set: &Set) {
	for p in ps {
		if let InterpPart::Expr(e) = p {
			expr(e, out, At::plain(set));
		}
	}
}

fn expr(e: &Expr, out: &mut Inputs, at: At<'_>) {
	if first_positional(e) {
		narrow(&mut out.positional_choices, at.choices.map(<[String]>::to_vec));
	}
	let plain = at.unguarded();
	match e {
		Expr::Number(..) | Expr::Bool(..) | Expr::Ident(..) => {}
		Expr::Str(ps, _) => parts(ps, out, at.set),
		Expr::List(items, _) => items.iter().for_each(|i| expr(i, out, plain)),
		Expr::Source { kind, key, .. } => {
			let u = Use {
				default: at.default.map(str::to_string),
				required: !at.guarded,
				choices: at.choices.map(<[String]>::to_vec),
			};
			match (kind, key) {
				(SourceKind::Arg, Some(k)) => merge(&mut out.args, k, u),
				// Set above it on every path, which the caller cannot reach.
				(SourceKind::Env, Some(k)) if at.set.contains(k) => {}
				(SourceKind::Env, Some(k)) => merge(&mut out.env, k, u),
				(SourceKind::Flag, Some(k)) => {
					out.flags.insert(k.clone());
				}
				// `ARGS` is the positional list, and carries no key. A `one_of`
				// around the whole list compares a list with strings and never
				// matches, so its options are not the list's; `first_positional`
				// is where they are taken.
				(SourceKind::Args, _) => out.positional = true,
				_ => {}
			}
		}
		Expr::Unary { rhs, .. } => expr(rhs, out, plain),
		Expr::Binary { lhs, rhs, .. } => {
			expr(lhs, out, plain);
			expr(rhs, out, plain);
		}
		// `a ? b` catches a failure in `a`, so everything down the left spine
		// is guarded and falls back to whatever the chain ends in. The
		// right-hand side is only as guarded as the chain itself: if it fails,
		// so does the expression.
		Expr::Chain { lhs, rhs, .. } => {
			let fallback = literal(rhs).or_else(|| at.default.map(str::to_string));
			// Whichever side the value comes from is what a `one_of` around the
			// chain checks, so both keep its options.
			expr(
				lhs,
				out,
				At {
					guarded: true,
					default: fallback.as_deref(),
					set: at.set,
					choices: at.choices,
				},
			);
			expr(rhs, out, at);
		}
		Expr::Index { base, index, .. } => {
			expr(base, out, plain);
			expr(index, out, plain);
		}
		// `try(x)` is the other way a failure is forgiven.
		Expr::Call { name, args, .. } if name == "try" => {
			for a in args {
				expr(
					a,
					out,
					At {
						guarded: true,
						default: at.default,
						set: at.set,
						choices: at.choices,
					},
				);
			}
		}
		// The subject is checked against the options, which are read like any
		// other argument. Nothing else hands its arguments anything.
		Expr::Call { name, args, .. } if name == "one_of" && !args.is_empty() => {
			let choices = options(&args[1..]);
			expr(
				&args[0],
				out,
				At {
					choices: choices.as_deref(),
					..plain
				},
			);
			args[1..].iter().for_each(|a| expr(a, out, plain));
		}
		Expr::Call { args, .. } => args.iter().for_each(|a| expr(a, out, plain)),
		Expr::Structured { body, .. } => body.lines.iter().for_each(|l| parts(l, out, at.set)),
		Expr::Capture { command, body, .. } => {
			if let Some(c) = command {
				parts(c, out, at.set);
			}
			body.iter().for_each(|l| parts(l, out, at.set));
		}
		Expr::Dispatch { target, args, .. } => {
			parts(target, out, at.set);
			args.iter().for_each(|a| parts(a, out, at.set));
		}
	}
}

/// The text a literal stands for, so a default can be shown. Only a literal:
/// `ARG.x ? ENV.Y` has a fallback but no value to print for it.
fn literal(e: &Expr) -> Option<String> {
	match e {
		Expr::Number(n, _) => Some(crate::value::format_num(*n)),
		Expr::Bool(b, _) => Some(b.to_string()),
		_ => string_literal(e),
	}
}

fn string_literal(e: &Expr) -> Option<String> {
	match e {
		Expr::Str(parts, _) => match parts.as_slice() {
			[] => Some(String::new()),
			[InterpPart::Literal(s)] => Some(s.clone()),
			// An interpolated string is not a fixed value.
			_ => None,
		},
		_ => None,
	}
}

/// A `one_of`'s options, when every one is a string written out -- one by one,
/// or as a single list, which is how `one_of` reads a list. One computed option
/// and there is no list to show; a number or a bool is never equal to the
/// string an input arrives as, so it is not one a caller could pass.
fn options(args: &[Expr]) -> Option<Vec<String>> {
	let items = match args {
		[Expr::List(items, _)] => items.as_slice(),
		_ => args,
	};
	items.iter().map(string_literal).collect()
}

/// `first(ARGS)` or `ARGS[0]`: the first positional, the word a `one_of`
/// around either one is checking.
fn first_positional(e: &Expr) -> bool {
	let args = |e: &Expr| {
		matches!(
			e,
			Expr::Source {
				kind: SourceKind::Args,
				..
			}
		)
	};
	match e {
		Expr::Call { name, args: a, .. } => name == "first" && matches!(a.as_slice(), [x] if args(x)),
		Expr::Index { base, index, .. } => args(base) && matches!(**index, Expr::Number(n, _) if n == 0.0),
		_ => false,
	}
}
