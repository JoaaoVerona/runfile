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

use crate::ast::{Block, Expr, InterpPart, Property, SourceKind, Statement, Target};
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
}

fn merge(into: &mut BTreeMap<String, Use>, name: &str, u: Use) {
	let slot = into.entry(name.to_string()).or_default();
	slot.required |= u.required;
	if slot.default.is_none() {
		slot.default = u.default;
	}
}

/// Environment names a `.env` property has already set where a read happens.
///
/// A read of one is not an input at all: the property beats whatever the
/// caller exports, so nothing they pass can reach it -- listing it would tell
/// someone to supply a value that is then ignored. Matched exactly, as the
/// property sets it: `ENV.X` looks the name up exactly before it tries any
/// other case, so a caller's `PORT` still reaches a read of `ENV.PORT` under a
/// `.env.port`, and that read stays an input.
type Set = BTreeSet<String>;

/// Where a source sits: whether a failure there is caught, what the chain
/// around it falls back to, and which names are already set in the
/// environment it reads.
#[derive(Clone, Copy)]
struct At<'a> {
	guarded: bool,
	default: Option<&'a str>,
	set: &'a Set,
}

impl<'a> At<'a> {
	fn plain(set: &'a Set) -> Self {
		At {
			guarded: false,
			default: None,
			set,
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
/// A `.env` is in place for everything below it, in source order, straight
/// through the chain and into the target: a shared file's `let`, the target's
/// own header, its statements. That is the runner's rule too -- a value reads
/// the environment as it stands at its own line -- and this walk follows it
/// exactly, since `--help` saying a name is not needed when the runner then
/// reads the caller's would be a lie.
pub fn of_chain(target: &Target, shared: &[Target]) -> Inputs {
	let mut out = Inputs::default();
	let mut set = Set::new();
	for s in shared {
		// Folded rather than walked, but every property in it applies, below a
		// `let` or not -- which is what the walk hands back.
		set = walk(&s.body, &mut out, &set);
	}
	walk(&target.body, &mut out, &set);
	out
}

/// The name a `.env.NAME` sets. `.env-file` sets names too, but which ones is
/// in a file that is read at run time and may well not exist -- a project
/// whose `.env` is git-ignored still has to run -- so a read it might answer
/// stays an input.
fn sets(p: &Property) -> Option<String> {
	match p.path.as_slice() {
		[head, name] if head == "env" => Some(name.clone()),
		_ => None,
	}
}

/// One block, the way the runner applies it: every property in source order,
/// reading the names set above it and supplying them to what follows. Hands
/// back what is set by the end of the block, which a nested block's caller
/// drops -- a block's own `.env` is undone when it closes -- and a shared
/// file's caller carries on into the next file.
fn walk(b: &Block, out: &mut Inputs, set: &Set) -> Set {
	let mut here = set.clone();
	let property = |p: &Property, out: &mut Inputs, here: &mut Set| {
		if let Some(e) = &p.value {
			expr(e, out, At::plain(here));
		}
		here.extend(sets(p));
	};
	for p in b.declaration() {
		property(p, out, &mut here);
	}
	let mut trailing = b.trailing().iter().peekable();
	for st in &b.statements {
		while let Some(p) = trailing.next_if(|p| p.span.line < st.span().line) {
			property(p, out, &mut here);
		}
		statement(st, out, &here);
	}
	for p in trailing {
		property(p, out, &mut here);
	}
	here
}

fn statement(s: &Statement, out: &mut Inputs, set: &Set) {
	let plain = At::plain(set);
	// A nested block is entered with the environment as it stands here; what
	// it sets is its own, and gone when it closes.
	let block = |b: &Block, out: &mut Inputs| {
		walk(b, out, set);
	};
	match s {
		Statement::Let { value, .. } | Statement::Assign { value, .. } => expr(value, out, plain),
		Statement::Call { expr: e, .. } => expr(e, out, plain),
		Statement::Do { body, .. } => block(body, out),
		Statement::If {
			cond, then, otherwise, ..
		} => {
			expr(cond, out, plain);
			block(then, out);
			if let Some(b) = otherwise {
				block(b, out);
			}
		}
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
			block(body, out);
			if let Some(b) = otherwise {
				block(b, out);
			}
		}
		Statement::For { iter, body, .. } => {
			expr(iter, out, plain);
			block(body, out);
		}
		// The condition is read under the enclosing environment. The runner
		// actually asks it inside the body's -- a `while ENV.X` sees a `.env.X`
		// the loop body declares -- but reading it as the enclosing one can
		// only list a name the caller need not pass, never hide one they must.
		Statement::Loop { test, body, .. } => {
			if let Some(c) = test.cond() {
				expr(c, out, plain);
			}
			block(body, out);
		}
		// Neither reads anything; both are here so a new statement form cannot
		// be added without this walk being asked about it.
		Statement::Break { .. } | Statement::Continue { .. } => {}
		Statement::Match {
			subject,
			cases,
			default,
			..
		} => {
			expr(subject, out, plain);
			for c in cases {
				block(&c.body, out);
			}
			if let Some(b) = default {
				block(b, out);
			}
		}
		Statement::Run { target, args, .. } => {
			parts(target, out, set);
			for a in args {
				parts(a, out, set);
			}
		}
		Statement::Exec { command, body, .. } => {
			if let Some(c) = command {
				parts(c, out, set);
			}
			for line in body {
				parts(line, out, set);
			}
		}
	}
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
	let plain = at.unguarded();
	match e {
		Expr::Number(..) | Expr::Bool(..) | Expr::Ident(..) => {}
		Expr::Str(ps, _) => parts(ps, out, at.set),
		Expr::List(items, _) => items.iter().for_each(|i| expr(i, out, plain)),
		Expr::Source { kind, key, .. } => {
			let u = Use {
				default: at.default.map(str::to_string),
				required: !at.guarded,
			};
			match (kind, key) {
				(SourceKind::Arg, Some(k)) => merge(&mut out.args, k, u),
				// Set by a property above it, which the caller cannot reach.
				(SourceKind::Env, Some(k)) if at.set.contains(k) => {}
				(SourceKind::Env, Some(k)) => merge(&mut out.env, k, u),
				(SourceKind::Flag, Some(k)) => {
					out.flags.insert(k.clone());
				}
				// `ARGS` is the positional list, and carries no key.
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
			expr(
				lhs,
				out,
				At {
					guarded: true,
					default: fallback.as_deref(),
					set: at.set,
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
					},
				);
			}
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
		Expr::Str(parts, _) => match parts.as_slice() {
			[] => Some(String::new()),
			[InterpPart::Literal(s)] => Some(s.clone()),
			// An interpolated string is not a fixed value.
			_ => None,
		},
		_ => None,
	}
}
