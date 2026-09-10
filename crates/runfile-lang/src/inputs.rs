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

impl Inputs {
	/// Fold another file's inputs in -- a `_shared.run` reads for every target
	/// under it.
	pub fn extend(&mut self, other: Inputs) {
		for (k, v) in other.args {
			merge(&mut self.args, &k, v);
		}
		for (k, v) in other.env {
			merge(&mut self.env, &k, v);
		}
		self.flags.extend(other.flags);
		self.positional |= other.positional;
	}
}

fn merge(into: &mut BTreeMap<String, Use>, name: &str, u: Use) {
	let slot = into.entry(name.to_string()).or_default();
	slot.required |= u.required;
	if slot.default.is_none() {
		slot.default = u.default;
	}
}

/// Where a source sits: whether a failure there is caught, and what the chain
/// around it falls back to.
#[derive(Clone, Copy, Default)]
struct At<'a> {
	guarded: bool,
	default: Option<&'a str>,
}

pub fn of(target: &Target) -> Inputs {
	let mut out = Inputs::default();
	block(&target.body, &mut out);
	out
}

fn block(b: &Block, out: &mut Inputs) {
	for p in &b.properties {
		if let Some(e) = &p.value {
			expr(e, out, At::default());
		}
	}
	for s in &b.statements {
		statement(s, out);
	}
}

fn statement(s: &Statement, out: &mut Inputs) {
	let plain = At::default();
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
			parts(target, out);
			for a in args {
				parts(a, out);
			}
		}
		Statement::Exec { command, body, .. } => {
			if let Some(c) = command {
				parts(c, out);
			}
			for line in body {
				parts(line, out);
			}
		}
	}
}

/// The interpolations in a run of text. The literal halves are the shell's, or
/// somebody else's language, and are not this one's to read names out of.
fn parts(ps: &[InterpPart], out: &mut Inputs) {
	for p in ps {
		if let InterpPart::Expr(e) = p {
			expr(e, out, At::default());
		}
	}
}

fn expr(e: &Expr, out: &mut Inputs, at: At<'_>) {
	let plain = At::default();
	match e {
		Expr::Number(..) | Expr::Bool(..) | Expr::Ident(..) => {}
		Expr::Str(ps, _) => parts(ps, out),
		Expr::List(items, _) => items.iter().for_each(|i| expr(i, out, plain)),
		Expr::Source { kind, key, .. } => {
			let u = Use {
				default: at.default.map(str::to_string),
				required: !at.guarded,
			};
			match (kind, key) {
				(SourceKind::Arg, Some(k)) => merge(&mut out.args, k, u),
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
					},
				);
			}
		}
		Expr::Call { args, .. } => args.iter().for_each(|a| expr(a, out, plain)),
		Expr::Structured { body, .. } => body.lines.iter().for_each(|l| parts(l, out)),
		Expr::Capture { command, body, .. } => {
			if let Some(c) = command {
				parts(c, out);
			}
			body.iter().for_each(|l| parts(l, out));
		}
		Expr::Dispatch { target, args, .. } => {
			parts(target, out);
			args.iter().for_each(|a| parts(a, out));
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
