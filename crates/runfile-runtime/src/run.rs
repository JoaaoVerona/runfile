//! The statement walker.

use crate::exec::{self, ExecError, Spawn};
use crate::props::{PropError, Props};
use runfile_lang::Value;
use runfile_lang::ast::*;
use runfile_lang::eval::{EvalError, Scope, eval_boundary, interpolate_shell};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum RunError {
	#[error(transparent)]
	Eval(#[from] EvalError),
	#[error(transparent)]
	Prop(#[from] PropError),
	#[error(transparent)]
	Exec(#[from] ExecError),
	#[error("line {line}: `for` needs a list, got {actual}")]
	ForNeedsList { actual: &'static str, line: usize },
	#[error("line {line}: no case matched `{subject}`; valid cases: {cases}")]
	NoCase {
		subject: String,
		cases: String,
		line: usize,
	},
	#[error("cancelled")]
	Cancelled,
	#[error("line {line}: `run` is not wired to a target resolver here")]
	NoResolver { line: usize },
}

/// How a nested `run <target>` is dispatched. In-process, so cycle detection
/// and the step counter are ordinary data rather than an env-var protocol.
/// Writing `$ run <target>` instead re-execs the binary, which is just a shell
/// line and never reaches this.
pub trait Dispatch {
	fn run(&mut self, target: &str, args: &[String]) -> Result<(), RunError>;
}

pub struct NoDispatch;
impl Dispatch for NoDispatch {
	fn run(&mut self, _t: &str, _a: &[String]) -> Result<(), RunError> {
		Err(RunError::NoResolver { line: 0 })
	}
}

pub struct Runner<'a> {
	pub scope: Scope,
	/// The anchor: the directory containing `runfiles/`. Everything relative
	/// resolves against it.
	pub anchor: PathBuf,
	pub dispatch: &'a mut dyn Dispatch,
	pub assume_yes: bool,
	/// Collected so a caller can show what ran without re-deriving it.
	pub trace: Vec<String>,
}

pub fn run_target(target: &Target, r: &mut Runner<'_>) -> Result<(), RunError> {
	let base = Props::default();
	let props = base.extend(&target.body, &mut r.scope, false)?;
	if let Some(msg) = props.confirm.clone().filter(|_| !r.assume_yes) {
		r.trace.push(format!("confirm: {msg}"));
		return Err(RunError::Cancelled);
	}
	walk(&target.body, &props, r)
}

fn walk(block: &Block, props: &Props, r: &mut Runner<'_>) -> Result<(), RunError> {
	for st in &block.statements {
		if let Err(e) = statement(st, props, r)
			&& !props.ignore_errors
		{
			return Err(e);
		}
	}
	Ok(())
}

fn statement(st: &Statement, props: &Props, r: &mut Runner<'_>) -> Result<(), RunError> {
	match st {
		Statement::Let { name, value, .. } => {
			let v = value_of(value, props, r)?;
			r.scope.bind(name, v);
			Ok(())
		}
		Statement::Assign { name, value, .. } => {
			let v = value_of(value, props, r)?;
			r.scope.bind(name, v);
			Ok(())
		}
		Statement::Call { expr, .. } => {
			eval_boundary(expr, &mut r.scope)?;
			Ok(())
		}
		Statement::If {
			cond,
			then,
			otherwise,
			span,
		} => {
			let taken = eval_boundary(cond, &mut r.scope)?
				.as_bool()
				.map_err(|e| EvalError::ty(span.line, e))?;
			match (taken, otherwise) {
				(true, _) => nested(then, props, r),
				(false, Some(b)) => nested(b, props, r),
				(false, None) => Ok(()),
			}
		}
		Statement::For { name, iter, body, span } => {
			let it = eval_boundary(iter, &mut r.scope)?;
			let items = match it {
				Value::List(v) => v,
				other => {
					return Err(RunError::ForNeedsList {
						actual: other.type_name(),
						line: span.line,
					});
				}
			};
			let prior = r.scope.vars.remove(name);
			let inner = props.extend(body, &mut r.scope, true)?;
			for item in items {
				r.scope.bind(name, item);
				if let Err(e) = walk(body, &inner, r)
					&& !inner.ignore_errors
				{
					r.scope.restore(name, prior);
					return Err(e);
				}
			}
			r.scope.restore(name, prior);
			Ok(())
		}
		Statement::Match {
			subject,
			cases,
			default,
			span,
		} => {
			let v = eval_boundary(subject, &mut r.scope)?.to_string();
			if let Some(c) = cases.iter().find(|c| c.label == v) {
				return nested(&c.body, props, r);
			}
			match default {
				Some(b) => nested(b, props, r),
				None => Err(RunError::NoCase {
					subject: v,
					cases: cases.iter().map(|c| c.label.clone()).collect::<Vec<_>>().join(", "),
					line: span.line,
				}),
			}
		}
		Statement::Run { target, args, .. } => {
			let t = interpolate_shell(target, &mut r.scope)?;
			let a: Vec<String> = args
				.iter()
				.map(|w| interpolate_shell(w, &mut r.scope))
				.collect::<Result<_, _>>()?;
			r.dispatch.run(&t, &a)
		}
		Statement::Exec { command, body, .. } => {
			let (cmd, text) = render(command.as_deref(), body, r)?;
			r.trace.push(text.clone());
			exec::spawn(Spawn {
				command: cmd.as_deref(),
				body: &text,
				cwd: &cwd(props, &r.anchor),
				env: &env_pairs(props),
				capture: false,
			})?;
			Ok(())
		}
	}
}

/// A nested block layers its own properties over the enclosing ones.
fn nested(block: &Block, props: &Props, r: &mut Runner<'_>) -> Result<(), RunError> {
	let inner = props.extend(block, &mut r.scope, true)?;
	walk(block, &inner, r)
}

/// `$`/`exec` in value position runs and yields stdout; anything else is a
/// plain expression.
fn value_of(e: &Expr, props: &Props, r: &mut Runner<'_>) -> Result<Value, RunError> {
	let Expr::Capture { command, body, .. } = e else {
		return Ok(eval_boundary(e, &mut r.scope)?);
	};
	let (cmd, text) = render(command.as_deref(), body, r)?;
	let out = exec::spawn(Spawn {
		command: cmd.as_deref(),
		body: &text,
		cwd: &cwd(props, &r.anchor),
		env: &env_pairs(props),
		capture: true,
	})?;
	Ok(Value::Str(out))
}

fn render(
	command: Option<&[InterpPart]>,
	body: &[Vec<InterpPart>],
	r: &mut Runner<'_>,
) -> Result<(Option<String>, String), RunError> {
	let cmd = match command {
		Some(c) => Some(interpolate_shell(c, &mut r.scope)?),
		None => None,
	};
	let mut lines = Vec::with_capacity(body.len());
	for l in body {
		lines.push(interpolate_shell(l, &mut r.scope)?);
	}
	Ok((cmd, lines.join("\n")))
}

fn cwd(props: &Props, anchor: &Path) -> PathBuf {
	match &props.workdir {
		Some(w) => anchor.join(w),
		None => anchor.to_path_buf(),
	}
}

fn env_pairs(props: &Props) -> Vec<(String, String)> {
	props.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}
