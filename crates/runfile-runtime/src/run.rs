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
	#[error(transparent)]
	Env(#[from] crate::env::EnvError),
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
	#[error("interrupted")]
	Interrupted,
	#[error("line {line}: `run` is not wired to a target resolver here")]
	NoResolver { line: usize },
	#[error(transparent)]
	Host(Box<dyn std::error::Error + Send + Sync>),
}

impl RunError {
	/// The status `exit(code)` asked for, if this is that rather than a
	/// failure. `.ignore-errors` consults it: a target may shrug off a command
	/// that failed, but not an instruction to stop.
	pub fn exit_code(&self) -> Option<i32> {
		match self {
			RunError::Eval(EvalError::Exit { code, .. }) => Some(*code),
			_ => None,
		}
	}
}

/// How a nested `run <target>` is dispatched. In-process, so cycle detection
/// and the step counter are ordinary data rather than an env-var protocol.
/// Writing `$ run <target>` instead re-execs the binary, which is just a shell
/// line and never reaches this.
/// `Sync` and `&self` because a `.parallel` block fans out across threads.
/// The call chain is passed rather than held: parallel branches have separate
/// paths, so a shared stack would make one branch look like a cycle to another.
pub trait Dispatch: Sync {
	/// Run `target`, returning its dry-run trace.
	///
	/// `label` prefixes everything the target prints, when it is one branch of a
	/// `.parallel` fan-out. It is inherited by whatever the target dispatches in
	/// turn, so a whole subtree reads as one branch.
	///
	/// The trace comes back rather than being written to shared state so the
	/// caller can splice it in where the call appeared. Writing it centrally
	/// printed every dependency before the line that invoked it, because a
	/// child finishes while its parent is still walking.
	fn run(
		&self,
		target: &str,
		args: &[String],
		chain: &[String],
		label: Option<&str>,
	) -> Result<Vec<String>, RunError>;
}

pub struct NoDispatch;
impl Dispatch for NoDispatch {
	fn run(&self, _t: &str, _a: &[String], _c: &[String], _l: Option<&str>) -> Result<Vec<String>, RunError> {
		Err(RunError::NoResolver { line: 0 })
	}
}

/// One unit of concurrent work, fully rendered so a thread needs no scope.
enum Leaf {
	Exec {
		cmd: Option<String>,
		body: String,
		env: Vec<(String, String)>,
		dir: PathBuf,
		/// What to prefix this branch's output with.
		label: String,
		detach: bool,
	},
	Run {
		target: String,
		args: Vec<String>,
	},
}

pub struct Runner<'a> {
	pub scope: Scope,
	/// Targets already on this call path, for cycle detection.
	pub chain: Vec<String>,
	/// Built once from the header properties, before the body is evaluated, so
	/// `{{ ENV.x }}` can see what `.env-file` brought in.
	pub env: Vec<(String, String)>,
	/// The anchor: the directory containing `runfiles/`. Everything relative
	/// resolves against it.
	pub anchor: PathBuf,
	pub dispatch: &'a dyn Dispatch,
	pub assume_yes: bool,
	/// Asked when a target declares `.confirm`. `None` means never prompt,
	/// which is what CI detection and `-y` reduce to.
	pub prompt: Option<&'a (dyn Fn(&str) -> bool + Sync)>,
	/// Whether the run has been interrupted. Injected rather than read from a
	/// global so the runtime reaches for no process state of its own, and so a
	/// test can interrupt one run without touching another.
	pub interrupted: Option<&'a (dyn Fn() -> bool + Sync)>,
	/// Prefix for this target's output; see `Dispatch::run`.
	pub label: Option<String>,
	/// Collected so a caller can show what ran without re-deriving it.
	pub dry_run: bool,
	pub trace: Vec<String>,
}

pub fn run_target(target: &Target, r: &mut Runner<'_>) -> Result<(), RunError> {
	run_target_with(target, Props::default(), r)
}

/// Evaluate only the `let` bindings of a block, used to fold `_shared.run`
/// values into a target's scope without running its statements.
pub fn run_block_bindings(block: &Block, sc: &mut Scope) -> Result<(), RunError> {
	for st in &block.statements {
		if let Statement::Let { name, value, .. } = st {
			let v = eval_boundary(value, sc)?;
			sc.bind(name, v);
		}
	}
	Ok(())
}

pub fn run_target_with(target: &Target, base: Props, r: &mut Runner<'_>) -> Result<(), RunError> {
	let props = base.extend(&target.body, &mut r.scope, false)?;

	// Env before the body: `.env-file` has to be readable by `{{ ENV.x }}`.
	let workdir = cwd(&props, &r.anchor);
	// The same deferred pool the `decrypt` function uses, so an encrypted
	// `.env-file` value resolves -- and an unencrypted one still never touches
	// the credential store.
	let keys = crate::env::Provider(r.scope.private_keys.clone());
	let built = crate::env::build(&props, &r.anchor, &workdir, Some(&keys))?;
	r.scope.env = built.clone();
	r.env = built.into_iter().collect();
	r.env.sort();

	// After the env exists, so the message can interpolate, and after argument
	// validation, so nobody confirms and is then asked for a missing argument.
	if let Some(msg) = props.confirm.clone() {
		let allowed = r.assume_yes || r.prompt.map(|p| p(&msg)).unwrap_or(false);
		if !allowed {
			r.trace.push(format!("confirm: {msg}"));
			return Err(RunError::Cancelled);
		}
	}
	walk(&target.body, &props, r)
}

fn walk(block: &Block, props: &Props, r: &mut Runner<'_>) -> Result<(), RunError> {
	if props.parallel {
		return walk_parallel(block, props, r);
	}
	for st in &block.statements {
		// Between statements, not inside one: a child already took the same
		// SIGINT and is gone, so there is nothing to wait for and nothing to
		// kill. `.ignore-errors` does not apply -- an interrupt is not a
		// failure the target gets to shrug off.
		if r.interrupted.is_some_and(|f| f()) {
			return Err(RunError::Interrupted);
		}
		if let Err(e) = statement(st, props, r)
			&& (!props.ignore_errors || e.exit_code().is_some())
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
			// `.parallel` on a loop body means the *iterations* are the
			// branches, not just each body's own statements. Collecting across
			// every iteration first is what makes them one batch.
			if inner.parallel {
				let mut leaves = Vec::new();
				for item in items {
					r.scope.bind(name, item);
					collect(body, &inner, r, &mut leaves)?;
				}
				r.scope.restore(name, prior);
				return run_leaves(leaves, &inner, r);
			}
			for item in items {
				r.scope.bind(name, item);
				if let Err(e) = walk(body, &inner, r)
					&& (!inner.ignore_errors || e.exit_code().is_some())
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
			let t = runfile_lang::eval::interpolate_plain(target, &mut r.scope)?;
			let a = run_args(args, &mut r.scope)?;
			// Splice the dependency's trace in where the call appeared.
			let child = r.dispatch.run(&t, &a, &r.chain, r.label.as_deref())?;
			r.trace.extend(child);
			Ok(())
		}
		Statement::Exec { command, body, .. } => {
			let (cmd, text) = render(command.as_deref(), body, r)?;
			r.trace.push(text.clone());
			let env = merged_env(r, props);
			let dir = cwd(props, &r.anchor);
			exec::spawn(Spawn {
				command: cmd.as_deref(),
				body: &text,
				cwd: &dir,
				env: &env,
				capture: false,
				dry_run: r.dry_run,
				label: r.label.as_deref(),
				detach: props.detach,
				announce: !r.dry_run,
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
	let env = merged_env(r, props);
	let dir = cwd(props, &r.anchor);
	let out = exec::spawn(Spawn {
		command: cmd.as_deref(),
		body: &text,
		cwd: &dir,
		env: &env,
		capture: true,
		dry_run: r.dry_run,
		label: r.label.as_deref(),
		// A capture's output is a value, not something to show, and it is
		// never detached: the whole point is waiting for what it prints.
		detach: false,
		announce: !r.dry_run,
	})?;
	Ok(Value::Str(out))
}

/// Arguments for a `run` statement.
///
/// They are values handed to an in-process target, not text handed to a shell,
/// so nothing is quoted -- quoting here reached the callee as literal quotes,
/// which its own shell lines then quoted again. A word that is one
/// interpolation of a list becomes one argument per item, the way it would on
/// a shell line; each expression is evaluated exactly once.
fn run_args(words: &[Vec<InterpPart>], sc: &mut Scope) -> Result<Vec<String>, RunError> {
	let mut out = Vec::new();
	for w in words {
		if let [InterpPart::Expr(e)] = &w[..] {
			match eval_boundary(e, sc)? {
				Value::List(items) => out.extend(items.iter().map(Value::to_string)),
				v => out.push(v.to_string()),
			}
		} else {
			out.push(runfile_lang::eval::interpolate_plain(w, sc)?);
		}
	}
	Ok(out)
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

/// The target's env, with any block-scoped `.env` layered on top.
fn merged_env(r: &Runner<'_>, props: &Props) -> Vec<(String, String)> {
	let mut out = r.env.clone();
	for (k, v) in &props.env {
		match out.iter_mut().find(|(ek, _)| ek == k) {
			Some(slot) => slot.1 = v.clone(),
			None => out.push((k.clone(), v.clone())),
		}
	}
	out
}

// ------------------------------------------------------------------ parallel

/// A parallel block evaluates its bindings in source order, then fans out the
/// executable leaves. Rendering happens first and sequentially, so a thread
/// carries a finished command rather than a borrow of the scope.
fn walk_parallel(block: &Block, props: &Props, r: &mut Runner<'_>) -> Result<(), RunError> {
	let mut leaves = Vec::new();
	collect(block, props, r, &mut leaves)?;
	run_leaves(leaves, props, r)
}

fn collect(block: &Block, props: &Props, r: &mut Runner<'_>, out: &mut Vec<Leaf>) -> Result<(), RunError> {
	for st in &block.statements {
		match st {
			Statement::Let { name, value, .. } | Statement::Assign { name, value, .. } => {
				let v = value_of(value, props, r)?;
				r.scope.bind(name, v);
			}
			Statement::Call { expr, .. } => {
				eval_boundary(expr, &mut r.scope)?;
			}
			Statement::Exec { command, body, .. } => {
				let (cmd, text) = render(command.as_deref(), body, r)?;
				out.push(Leaf::Exec {
					label: exec_label(cmd.as_deref(), &text),
					detach: props.detach,
					cmd,
					body: text,
					env: merged_env(r, props),
					dir: cwd(props, &r.anchor),
				});
			}
			Statement::Run { target, args, .. } => {
				let t = runfile_lang::eval::interpolate_plain(target, &mut r.scope)?;
				let a = run_args(args, &mut r.scope)?;
				out.push(Leaf::Run { target: t, args: a });
			}
			// Control flow is expanded here so its leaves join the same batch.
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
					(true, _) => collect(then, props, r, out)?,
					(false, Some(b)) => collect(b, props, r, out)?,
					(false, None) => {}
				}
			}
			Statement::For { name, iter, body, span } => {
				let items = match eval_boundary(iter, &mut r.scope)? {
					Value::List(v) => v,
					other => {
						return Err(RunError::ForNeedsList {
							actual: other.type_name(),
							line: span.line,
						});
					}
				};
				let prior = r.scope.vars.remove(name);
				for item in items {
					r.scope.bind(name, item);
					collect(body, props, r, out)?;
				}
				r.scope.restore(name, prior);
			}
			Statement::Match {
				subject,
				cases,
				default,
				span,
			} => {
				let v = eval_boundary(subject, &mut r.scope)?.to_string();
				match cases.iter().find(|c| c.label == v) {
					Some(c) => collect(&c.body, props, r, out)?,
					None => match default {
						Some(b) => collect(b, props, r, out)?,
						None => {
							return Err(RunError::NoCase {
								subject: v,
								cases: cases.iter().map(|c| c.label.clone()).collect::<Vec<_>>().join(", "),
								line: span.line,
							});
						}
					},
				}
			}
		}
	}
	Ok(())
}

/// What to call a branch in the output.
///
/// From the `exec` header when there is one, since `exec python3` names itself,
/// and otherwise the first word of the body -- which for a `$` line is the
/// command being run. The default shell is never the label: every `$` branch
/// would be called `bash`.
#[cfg(test)]
pub(crate) fn exec_label_for_test(command: Option<&str>, body: &str) -> String {
	exec_label(command, body)
}

fn exec_label(command: Option<&str>, body: &str) -> String {
	let from_header = command.and_then(|c| c.split_whitespace().next());
	let from_body = || {
		body.lines()
			.map(str::trim)
			.find(|l| !l.is_empty() && !l.starts_with('#'))
			.and_then(|l| l.split_whitespace().next())
	};
	// A shell is never the label: every `$` branch would be called `bash`.
	const SHELLS: &[&str] = &["sh", "bash", "dash", "ash", "zsh", "ksh", "busybox", "brush"];
	from_header
		.filter(|c| !SHELLS.contains(c))
		.or_else(from_body)
		.unwrap_or("exec")
		.to_string()
}

fn run_leaves(leaves: Vec<Leaf>, props: &Props, r: &mut Runner<'_>) -> Result<(), RunError> {
	let dispatch = r.dispatch;
	let chain = r.chain.clone();
	let dry_run = r.dry_run;
	let results: Vec<Result<Option<String>, RunError>> = std::thread::scope(|s| {
		let handles: Vec<_> = leaves
			.iter()
			.map(|leaf| {
				let chain = &chain;
				s.spawn(move || match leaf {
					Leaf::Exec {
						cmd,
						body,
						env,
						dir,
						label,
						detach,
					} => exec::spawn(Spawn {
						command: cmd.as_deref(),
						body,
						cwd: dir,
						env,
						capture: false,
						dry_run,
						label: Some(label),
						detach: *detach,
						announce: !dry_run,
					})
					.map(|_| Some(body.clone()))
					.map_err(RunError::from),
					// Branches finish in whatever order they finish, so a
					// dispatched target's trace joins the parent's as one block
					// rather than being interleaved line by line.
					// A dispatched target labels its own leaves, so nothing is
					// added here; its trace joins the parent's as one block.
					Leaf::Run { target, args } => dispatch
						.run(target, args, chain, Some(target))
						.map(|t| Some(t.join("\n"))),
				})
			})
			.collect();
		handles
			.into_iter()
			.map(|h| h.join().expect("branch panicked"))
			.collect()
	});

	let mut first_error = None;
	for res in results {
		match res {
			Ok(Some(body)) => r.trace.push(body),
			Ok(None) => {}
			Err(e) => {
				if first_error.is_none() {
					first_error = Some(e);
				}
			}
		}
	}
	// Every branch runs to completion before a failure surfaces -- stopping the
	// others would leave a half-started fan-out behind.
	match first_error {
		Some(e) if !props.ignore_errors || e.exit_code().is_some() => Err(e),
		_ => Ok(()),
	}
}
