//! The statement walker.

use crate::exec::{self, ExecError, Spawn};
use crate::props::{PropError, Props};
use runfile_lang::Value;
use runfile_lang::ast::*;
use runfile_lang::eval::{EvalError, Scope, eval_boundary, interpolate_shell};
use std::borrow::Cow;
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
	/// A `--flag` or `--key=value` the target never reads, on a target that
	/// reads no positionals either -- so there is nowhere for it to go. Exact
	/// rather than a guess, because what a target reads is walked from its
	/// tree, so this can refuse rather than warn.
	#[error("{}", unknown_input(name, key, target, reads))]
	UnknownInput {
		name: String,
		key: String,
		target: String,
		reads: Box<runfile_lang::Inputs>,
	},
	/// `--key` named something read as `ARG.key`, with no value to put in it.
	#[error("{}", missing_arg_value(key, next.as_deref(), target))]
	MissingArgValue {
		key: String,
		next: Option<String>,
		target: String,
	},
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
	#[error("line {line}: a `retry` cannot be inside a `.parallel` block")]
	RetryInParallel { line: usize },
	/// A `while`, `until`, `loop`, `break` or `continue` inside a fan-out. A
	/// `for` is expanded there because its list is known before it starts; a
	/// conditional loop's passes are not, and neither is which one a `break`
	/// would leave once every branch is already collected.
	#[error("line {line}: a `{kind}` cannot be inside a `.parallel` block")]
	NotInParallel { kind: &'static str, line: usize },
	/// `break` -- travelling as an error because that is the only way back out
	/// of a walk. The message is a net: the parser refuses one outside a loop,
	/// so nothing should ever print this.
	#[error("line {line}: `break` outside a loop")]
	Break { line: usize },
	#[error("line {line}: `continue` outside a loop")]
	Continue { line: usize },
	#[error(transparent)]
	Host(Box<dyn std::error::Error + Send + Sync>),
}

/// What to say about an input a target does not read.
///
/// One line, because everything the runner says while a target runs carries
/// the `[runfile]` prefix and a continuation line would not. Written with
/// `push_str` rather than one long literal, so rustfmt cannot fold a line
/// continuation into the message itself -- which it did, twice, leaving tabs
/// in the middle of a sentence.
fn unknown_input(name: &str, key: &str, target: &str, reads: &runfile_lang::Inputs) -> String {
	let mut out = format!("`{name}` was passed to `{target}`, which never reads `FLAG.{key}` or `ARG.{key}`; ");
	// A target reading `ARGS` never reaches here -- it can read the word, so
	// the word is given to it as a positional rather than refused.
	if reads.args.is_empty() && reads.flags.is_empty() {
		out.push_str(&format!("`{target}` reads no arguments or flags at all"));
	} else {
		out.push_str(&format!("run `run {target} --help` to see what it does read"));
	}
	out
}

/// What to say about a `--key` that names an argument and carries no value.
///
/// The same shape as `unknown_input`: the word, the target, then the one thing
/// that helps. Which that is depends on whether something *was* there -- with
/// a word after it, saying "put the value after it" describes what the person
/// just did, so the message has to name the word it declined to take instead.
fn missing_arg_value(key: &str, next: Option<&str>, target: &str) -> String {
	let mut out = format!("`--{key}` was passed to `{target}` with no value; ");
	match next {
		Some(n) => out.push_str(&format!(
			"`{n}` looks like another flag, so write `--{key}={n}` if it is the value"
		)),
		None => out.push_str(&format!("write `--{key}=<value>`, or put the value after it")),
	}
	out
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

	/// Whether this is something the run has to carry out rather than a
	/// command that went wrong. A target may shrug off a command that failed;
	/// it does not get to shrug off `exit()`, someone answering no to
	/// `confirm()`, or a `break` -- forgiving one of those would leave a
	/// statement that plainly did nothing, which is the worst way for it to be
	/// wrong. It is also what carries a `break` out through a `retry`, which
	/// would otherwise read it as a failed attempt and run the body again.
	pub fn is_stop(&self) -> bool {
		matches!(
			self,
			RunError::Eval(EvalError::Exit { .. } | EvalError::Cancelled { .. })
				| RunError::Cancelled
				| RunError::Break { .. }
				| RunError::Continue { .. }
		)
	}

	/// Whether a *person* stopped the run, rather than a target reporting how
	/// it went: Ctrl+C, or someone answering no to a `.confirm`.
	///
	/// `code_of(run …)` scores every other outcome, `exit(3)` included -- that
	/// number is what it was asked for. Someone who declined has not asked the
	/// caller to carry on regardless and write down a 1.
	pub fn is_refusal(&self) -> bool {
		matches!(
			self,
			RunError::Eval(EvalError::Cancelled { .. }) | RunError::Cancelled | RunError::Interrupted
		)
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
///
/// Every property a branch runs under is resolved here, at collect time, and
/// carried rather than re-read from the batch's own properties: a nested block
/// inside a fan-out layers its own `.shell`, `.logging` and `.ignore-errors`
/// the same way it layers `.workdir`, and only the leaf knows which block it
/// came out of.
enum Leaf {
	Exec {
		/// Already through `command_for`, so a block's own `.shell` is the one
		/// that runs it.
		command: Option<String>,
		body: String,
		env: Vec<(String, String)>,
		dir: PathBuf,
		/// What to prefix this branch's output with.
		label: String,
		detach: bool,
		announce: bool,
		ignore_errors: bool,
	},
	Run {
		target: String,
		args: Vec<String>,
		ignore_errors: bool,
	},
}

impl Leaf {
	fn ignore_errors(&self) -> bool {
		match self {
			Leaf::Exec { ignore_errors, .. } | Leaf::Run { ignore_errors, .. } => *ignore_errors,
		}
	}
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

/// Fold a `_shared.run` into a scope and a set of properties, in source order.
///
/// A shared file's statements never run -- it holds settings, not actions --
/// but its `let`s are bound and its properties applied, interleaved the way
/// they are written, so a property below a binding can read it. That is the
/// rule a target follows too; a shared file is never walked, so it has to be
/// spelled here or a property below a `let` is never applied at all. Unlike a
/// walk, a property below the last binding still counts: in a shared file,
/// after the value it is computed from is exactly where a setting sits.
pub fn fold_shared(block: &Block, base: &Props, sc: &mut Scope) -> Result<Props, RunError> {
	let mut props = base.extend(block, sc, false)?;
	// Nothing here is walked, so nothing rebuilds the environment between one
	// line and the next -- `keep_current` does it, for the lines that read it,
	// the way it does inside a header. `extend` put back what it rebuilt, so
	// everything folded so far is still to be brought in.
	let mut stale = props.touches_env();
	let apply = |p: &Property, props: &mut Props, sc: &mut Scope, stale: &mut bool| -> Result<(), RunError> {
		crate::props::keep_current(props, p.value.as_ref(), sc, stale)?;
		props.apply(p, sc, false)?;
		*stale |= crate::props::changes_env(p);
		Ok(())
	};
	let mut trailing = block.trailing().iter().peekable();
	for st in &block.statements {
		while let Some(p) = trailing.next_if(|p| p.span.line < st.span().line) {
			apply(p, &mut props, sc, &mut stale)?;
		}
		if let Statement::Let { names, value, span } = st {
			crate::props::keep_current(&props, Some(value), sc, &mut stale)?;
			let v = eval_boundary(value, sc)?;
			sc.bind_names(names, runfile_lang::eval::destructure(names, v, span.line)?);
		}
	}
	for p in trailing {
		apply(p, &mut props, sc, &mut stale)?;
	}
	Ok(props)
}

pub fn run_target_with(target: &Target, base: Props, r: &mut Runner<'_>) -> Result<(), RunError> {
	let props = base.extend(&target.body, &mut r.scope, false)?;

	// Env before the body: `.env-file` has to be readable by `{{ ENV.x }}`.
	build_env(&props, r)?;

	walk(&target.body, &props, r)
}

fn walk(block: &Block, props: &Props, r: &mut Runner<'_>) -> Result<(), RunError> {
	if props.parallel {
		return walk_parallel(block, props, r);
	}
	// A block almost always writes its properties at the top, and those are
	// already in `props`. Only when one is written below a statement is there
	// anything to apply mid-walk -- and only then is there an environment to
	// put back, which is why the whole walk is wrapped rather than each
	// property: the block's own environment is what the block after it expects.
	if block.trailing().is_empty() {
		return walk_body(block, props, r);
	}
	let saved_env = r.env.clone();
	let saved_scope_env = r.scope.env.clone();
	let out = walk_body(block, props, r);
	r.env = saved_env;
	r.scope.env = saved_scope_env;
	out
}

fn walk_body(block: &Block, props: &Props, r: &mut Runner<'_>) -> Result<(), RunError> {
	// Borrowed until a trailing property makes it its own: a block without one
	// walks on exactly the properties it was handed.
	let mut props = Cow::Borrowed(props);
	let mut trailing = block.trailing().iter().peekable();
	for st in &block.statements {
		// Properties written above this line but below the one before it. They
		// read the scope as it stands here -- which is the whole point: the
		// binding they name was made by a statement already run.
		while let Some(p) = trailing.next_if(|p| p.span.line < st.span().line) {
			apply_trailing(p, &mut props, r)?;
		}
		// Between statements, not inside one: a child already took the same
		// SIGINT and is gone, so there is nothing to wait for and nothing to
		// kill. `.ignore-errors` does not apply -- an interrupt is not a
		// failure the target gets to shrug off.
		if r.interrupted.is_some_and(|f| f()) {
			return Err(RunError::Interrupted);
		}
		if let Err(e) = statement(st, &props, r)
			&& (!props.ignore_errors || e.is_stop())
		{
			return Err(e);
		}
	}
	// Below the last statement there is nothing left for it to affect, but it
	// is still applied where it sits: its value is evaluated, so a broken one
	// says so rather than being skipped, and a `_shared.run` -- which is folded
	// rather than walked -- treats one the same way.
	for p in trailing {
		apply_trailing(p, &mut props, r)?;
	}
	Ok(())
}

/// Apply a property written below a statement, and rebuild the environment when
/// it is one the environment is made of.
///
/// `nested` is `false` because it cannot matter: `Props::extend` has already
/// refused every declaration-only property here, and everything header-only is
/// declaration-only, so what reaches this may sit in a block by definition.
fn apply_trailing(p: &Property, props: &mut Cow<'_, Props>, r: &mut Runner<'_>) -> Result<(), RunError> {
	props.to_mut().apply(p, &mut r.scope, false)?;
	if matches!(p.path[0].as_str(), "env" | "env-file" | "add-path" | "workdir") {
		build_env(props, r)?;
	}
	Ok(())
}

fn statement(st: &Statement, props: &Props, r: &mut Runner<'_>) -> Result<(), RunError> {
	match st {
		Statement::Let { names, value, span } | Statement::Assign { names, value, span } => {
			let v = value_of(value, props, r)?;
			let parts = runfile_lang::eval::destructure(names, v, span.line)?;
			r.scope.bind_names(names, parts);
			Ok(())
		}
		Statement::Call { expr, .. } => {
			// Through value_of, so a bare `code_of($ cmd)` runs the command
			// rather than reaching the pure evaluator, which has no shell.
			value_of(expr, props, r)?;
			Ok(())
		}
		Statement::Do { body, .. } => nested(body, props, r),
		Statement::If {
			cond,
			then,
			otherwise,
			span,
		} => {
			let taken = cond_of(cond, props, r, span.line)?;
			match (taken, otherwise) {
				(true, _) => nested(then, props, r),
				(false, Some(b)) => nested(b, props, r),
				(false, None) => Ok(()),
			}
		}
		Statement::Retry {
			attempts,
			delay,
			body,
			otherwise,
			span,
		} => {
			let n = eval_boundary(attempts, &mut r.scope)?
				.as_num()
				.map_err(|e| EvalError::ty(span.line, e))?
				.max(1.0) as u64;
			let secs = match delay {
				Some(d) => eval_boundary(d, &mut r.scope)?
					.as_num()
					.map_err(|e| EvalError::ty(span.line, e))?
					.max(0.0),
				None => 0.0,
			};
			// The body has to see its own failures: `.ignore-errors` around it
			// would forgive them, and a retry that cannot tell would run once.
			let mut inner = props.extend(body, &mut r.scope, true)?;
			inner.ignore_errors = false;

			// The `else` runs outside the body's environment: it is what to do
			// when the block never worked, not part of it.
			let last = with_block_env(props, &inner, r, |r| retry_attempts(body, &inner, n, secs, r))?;

			match (last, otherwise) {
				(None, _) => Ok(()),
				(Some(_), Some(b)) => nested(b, props, r),
				(Some(e), None) => Err(e),
			}
		}
		Statement::For {
			names,
			iter,
			body,
			span,
		} => {
			// Through `value_of`, so `for f in lines($ git ls-files)` can run
			// its command -- the pure evaluator has no process to run it in.
			let it = value_of(iter, props, r)?;
			let items = match it {
				Value::List(v) => v,
				other => {
					return Err(RunError::ForNeedsList {
						actual: other.type_name(),
						line: span.line,
					});
				}
			};
			let priors: Vec<Option<Value>> = names.iter().map(|n| r.scope.vars.remove(n)).collect();
			let inner = props.extend(body, &mut r.scope, true)?;
			let out = with_block_env(props, &inner, r, |r| {
				for_body(names, items, body, &inner, &priors, span.line, r)
			});
			restore_all(names, &priors, r);
			out
		}
		Statement::Loop { test, body, span } => {
			let inner = props.extend(body, &mut r.scope, true)?;
			with_block_env(props, &inner, r, |r| loop_passes(test, body, &inner, span.line, r))
		}
		// Control flow, not a failure: it leaves as an error because that is
		// the only way back out of a walk, and every catcher on the way lets
		// it through -- see `is_stop`.
		Statement::Break { span } => Err(RunError::Break { line: span.line }),
		Statement::Continue { span } => Err(RunError::Continue { line: span.line }),
		Statement::Match {
			subject,
			cases,
			default,
			span,
		} => {
			let v = subject_of(subject, props, r)?;
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
		Statement::Exec {
			command, body, detach, ..
		} => {
			let (cmd, text) = render(command.as_deref(), body, r)?;
			r.trace.push(text.clone());
			let env = merged_env(r);
			let dir = cwd(props, &r.anchor);
			exec::spawn(Spawn {
				command: command_for(cmd.as_deref(), props),
				body: &text,
				cwd: &dir,
				env: &env,
				capture: false,
				dry_run: r.dry_run,
				label: r.label.as_deref(),
				detach: *detach,
				announce: props.logging && !r.dry_run,
			})?;
			Ok(())
		}
	}
}

/// A nested block layers its own properties over the enclosing ones.
/// Build the environment these properties describe, into the runner and into
/// the scope -- the second is what lets `{{ ENV.x }}` read what a `.env-file`
/// brought in.
fn build_env(props: &Props, r: &mut Runner<'_>) -> Result<(), RunError> {
	let built = crate::env::for_props(props, &r.anchor, &r.scope.private_keys)?;
	r.scope.env = built.clone();
	r.env = built.into_iter().collect();
	r.env.sort();
	Ok(())
}

/// Run `f` with the environment `inner` describes, putting back the one that
/// was there when it returns.
///
/// `.env-file` and `.add-path` are block-scoped, and both append -- so a block
/// that named one has a longer list than the block around it, which is how
/// this tells. A block's own `.env` counts too: it was left out, and a command
/// saw it only because `merged_env` laid it back on top, so `$X` inside the
/// block was the property's while `{{ ENV.X }}` was the enclosing one. Rebuilt
/// only when one of the three changed: reading and decrypting the files again
/// for every `if` would be work nothing asked for. Restoring afterwards is what
/// keeps a loop from carrying one iteration's files into the next, and is why
/// every block form has to come through here rather than calling `walk`.
fn with_block_env<T>(
	props: &Props,
	inner: &Props,
	r: &mut Runner<'_>,
	f: impl FnOnce(&mut Runner<'_>) -> Result<T, RunError>,
) -> Result<T, RunError> {
	if inner.env_files.len() == props.env_files.len()
		&& inner.add_paths.len() == props.add_paths.len()
		&& inner.env == props.env
	{
		return f(r);
	}
	let saved_env = std::mem::take(&mut r.env);
	let saved_scope_env = std::mem::take(&mut r.scope.env);
	let out = build_env(inner, r).and_then(|()| f(r));
	r.env = saved_env;
	r.scope.env = saved_scope_env;
	out
}

/// The attempts of a `retry`. `Ok(None)` when one of them succeeded, and
/// `Ok(Some(e))` when every one failed -- which is the question an `else`
/// answers.
fn retry_attempts(
	body: &Block,
	inner: &Props,
	n: u64,
	secs: f64,
	r: &mut Runner<'_>,
) -> Result<Option<RunError>, RunError> {
	let mut last = None;
	for attempt in 0..n {
		if r.interrupted.is_some_and(|f| f()) {
			return Err(RunError::Interrupted);
		}
		match walk(body, inner, r) {
			Ok(()) => {
				last = None;
				break;
			}
			// `exit` is an instruction to stop, not a failure to retry.
			Err(e) if e.is_stop() => return Err(e),
			Err(e) => {
				last = Some(e);
				if attempt + 1 < n && secs > 0.0 && !r.dry_run {
					std::thread::sleep(std::time::Duration::from_secs_f64(secs));
				}
			}
		}
	}
	Ok(last)
}

/// The iterations of a `for`. The loop variables are restored by the caller,
/// which also owns the block's environment.
fn for_body(
	names: &[String],
	items: Vec<Value>,
	body: &Block,
	inner: &Props,
	priors: &[Option<Value>],
	line: usize,
	r: &mut Runner<'_>,
) -> Result<(), RunError> {
	// `.parallel` on a loop body means the *iterations* are the branches, not
	// just each body's own statements. Collecting across every iteration first
	// is what makes them one batch.
	if inner.parallel {
		let mut leaves = Vec::new();
		for item in items {
			bind_item(names, item, line, r)?;
			collect(body, inner, r, &mut leaves)?;
		}
		restore_all(names, priors, r);
		return run_leaves(leaves, r);
	}
	for item in items {
		bind_item(names, item, line, r)?;
		match walk(body, inner, r) {
			Ok(()) => {}
			Err(RunError::Break { .. }) => break,
			Err(RunError::Continue { .. }) => {}
			Err(e) if !inner.ignore_errors || e.is_stop() => return Err(e),
			Err(_) => {}
		}
	}
	Ok(())
}

/// One iteration's value against the loop's names: the whole item for one
/// name, unpacked for several -- the same rule a `let` follows.
fn bind_item(names: &[String], item: Value, line: usize, r: &mut Runner<'_>) -> Result<(), RunError> {
	let parts = runfile_lang::eval::destructure(names, item, line)?;
	r.scope.bind_names(names, parts);
	Ok(())
}

fn restore_all(names: &[String], priors: &[Option<Value>], r: &mut Runner<'_>) {
	for (n, p) in names.iter().zip(priors) {
		r.scope.restore(n, p.clone());
	}
}

/// The passes of a `while`, `until` or `loop`.
///
/// Under `--dry-run` the body is walked **once** and the loop stops. A preview
/// performs none of the effects the condition is asking about, so the answer
/// it would get back never changes: `while` would run forever or not at all,
/// and neither is what happened. One pass is the honest thing a preview has to
/// say -- here is the body, once; how often is not knowable without running
/// it.
fn loop_passes(test: &LoopTest, body: &Block, inner: &Props, line: usize, r: &mut Runner<'_>) -> Result<(), RunError> {
	loop {
		// Checked here as well as in `walk`, which looks *between* statements:
		// a `loop` with an empty body has none, and would spin past every
		// chance to notice a Ctrl+C.
		if r.interrupted.is_some_and(|f| f()) {
			return Err(RunError::Interrupted);
		}
		let again = match test {
			LoopTest::Forever => true,
			LoopTest::While(c) => cond_of(c, inner, r, line)?,
			LoopTest::Until(c) => !cond_of(c, inner, r, line)?,
		};
		if !again {
			return Ok(());
		}
		match walk(body, inner, r) {
			Ok(()) => {}
			Err(RunError::Break { .. }) => return Ok(()),
			Err(RunError::Continue { .. }) => {}
			Err(e) if !inner.ignore_errors || e.is_stop() => return Err(e),
			Err(_) => {}
		}
		if r.dry_run {
			return Ok(());
		}
	}
}

fn nested(block: &Block, props: &Props, r: &mut Runner<'_>) -> Result<(), RunError> {
	let inner = props.extend(block, &mut r.scope, true)?;
	with_block_env(props, &inner, r, |r| walk(block, &inner, r))
}

/// `$`/`exec` in value position runs and yields stdout; anything else is a
/// plain expression.
fn value_of(e: &Expr, props: &Props, r: &mut Runner<'_>) -> Result<Value, RunError> {
	if let Expr::Call { name, args, .. } = e
		&& name == "code_of"
		&& args.len() == 1
	{
		return Ok(Value::Num(f64::from(exit_code(&args[0], props, r)?)));
	}
	// A call whose last argument is a `$` run -- `lines($ git ls-files)`. The
	// capture needs a process to run in, which the pure evaluator has none of,
	// so it is run here and the call is applied to the value.
	if let Expr::Call { name, args, span } = e
		&& matches!(args.last(), Some(Expr::Capture { .. }))
	{
		let mut values = Vec::with_capacity(args.len());
		for a in args {
			values.push(value_of(a, props, r)?);
		}
		return Ok(runfile_lang::functions::call_with(name, values, &mut r.scope, *span)?);
	}
	let Expr::Capture { command, body, .. } = e else {
		return Ok(eval_boundary(e, &mut r.scope)?);
	};
	let (cmd, text) = render(command.as_deref(), body, r)?;
	let env = merged_env(r);
	let dir = cwd(props, &r.anchor);
	let out = exec::spawn(Spawn {
		command: command_for(cmd.as_deref(), props),
		body: &text,
		cwd: &dir,
		env: &env,
		capture: true,
		dry_run: r.dry_run,
		label: r.label.as_deref(),
		// A capture's output is a value, not something to show, and it is
		// never detached: the whole point is waiting for what it prints.
		detach: false,
		announce: props.logging && !r.dry_run,
	})?;
	Ok(Value::Str(out))
}

/// What actually runs a `$` line: the command an `exec` named, else the
/// target's `.shell`, else the default.
///
/// The property was parsed, stored, documented and offered in completion, and
/// read by nothing -- so `.shell = "sh"` ran under bash and said nothing about
/// it. A script written for `sh` that happens to work in bash is the quiet
/// case; one that uses a bashism and passes locally is the other.
fn command_for<'a>(cmd: Option<&'a str>, props: &'a Props) -> Option<&'a str> {
	cmd.or(props.shell.as_deref())
}

/// Run a capture and report its exit status.
///
/// Not captured: `if $ grep -q x f` produces nothing either way, and hiding
/// what `$ mkdir out` says about why it failed would be the wrong default for
/// the one form whose whole purpose is to keep going afterwards.
fn exit_code(e: &Expr, props: &Props, r: &mut Runner<'_>) -> Result<i32, RunError> {
	if let Expr::Dispatch { target, args, .. } = e {
		return dispatch_code(target, args, r);
	}
	let Expr::Capture { command, body, .. } = e else {
		// The parser used to guarantee this, back when `code_of` was the only
		// call a capture could sit inside. Now that any call may hold one,
		// answering 0 for `code_of("x")` would be a silent wrong number.
		return Err(RunError::Eval(runfile_lang::eval::EvalError::Other {
			msg: "`code_of` takes a `$` run or a `run` dispatch, as `code_of($ mkdir out)` or `code_of(run build)`"
				.into(),
			line: e.span().line,
		}));
	};
	let (cmd, text) = render(command.as_deref(), body, r)?;
	let env = merged_env(r);
	let dir = cwd(props, &r.anchor);
	Ok(exec::spawn_code(Spawn {
		command: command_for(cmd.as_deref(), props),
		body: &text,
		cwd: &dir,
		env: &env,
		capture: false,
		dry_run: r.dry_run,
		label: r.label.as_deref(),
		detach: false,
		announce: props.logging && !r.dry_run,
	})?)
}

/// `code_of(run <target>)`: the status the dispatch would have exited with.
///
/// The number is the one `$ run <target>` yields, because dispatching
/// in-process is meant to stop re-execing the binary, not to mean something
/// else: a target that calls `exit(3)` is scored 3, and every other failure is
/// the 1 the CLI reports. The failure is printed the way that re-exec would
/// have printed it, since this is where it stops -- a status is an answer, so
/// nothing further up will say what went wrong.
///
/// A refusal is not a status and is passed on, which is also what a re-exec
/// does: Ctrl+C reaches the whole process group.
fn dispatch_code(target: &[InterpPart], args: &[Vec<InterpPart>], r: &mut Runner<'_>) -> Result<i32, RunError> {
	let t = runfile_lang::eval::interpolate_plain(target, &mut r.scope)?;
	let a = run_args(args, &mut r.scope)?;
	match r.dispatch.run(&t, &a, &r.chain, r.label.as_deref()) {
		Ok(child) => {
			r.trace.extend(child);
			Ok(0)
		}
		Err(e) if e.is_refusal() => Err(e),
		Err(e) => Ok(e.exit_code().unwrap_or_else(|| {
			eprintln!("{} error: {e}", exec::tag());
			1
		})),
	}
}

/// A condition, which may be a `$` run: `if $ cmd` is true when it succeeds.
fn cond_of(cond: &Expr, props: &Props, r: &mut Runner<'_>, line: usize) -> Result<bool, RunError> {
	if let Expr::Capture { .. } = cond {
		return Ok(exit_code(cond, props, r)? == 0);
	}
	Ok(eval_boundary(cond, &mut r.scope)?
		.as_bool()
		.map_err(|e| EvalError::ty(line, e))?)
}

/// A `match` subject, which may be a `$` run: the cases are then exit codes.
fn subject_of(subject: &Expr, props: &Props, r: &mut Runner<'_>) -> Result<String, RunError> {
	if let Expr::Capture { .. } = subject {
		return Ok(exit_code(subject, props, r)?.to_string());
	}
	Ok(eval_boundary(subject, &mut r.scope)?.to_string())
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

/// Interpolate an `exec`'s command line and its body.
///
/// The command line is always shell-quoted: it is split into arguments, so a
/// value with a space in it has to stay one. **The body is quoted only when
/// the command is a shell.** Anybody else's body is their language, and shell
/// quoting is the wrong quoting for it -- `'it'\''s'` is not Python, a list
/// flattens to bare words, and a path comes through unquoted because nothing
/// in it needed quoting. Left alone, a value arrives as itself and the author
/// quotes it the way that language wants, which is the only thing that can be
/// right for every language.
fn render(
	command: Option<&[InterpPart]>,
	body: &[Vec<InterpPart>],
	r: &mut Runner<'_>,
) -> Result<(Option<String>, String), RunError> {
	let cmd = match command {
		Some(c) => Some(interpolate_shell(c, &mut r.scope)?),
		None => None,
	};
	let shell = exec::body_is_shell(cmd.as_deref());
	let mut lines = Vec::with_capacity(body.len());
	for l in body {
		lines.push(if shell {
			interpolate_shell(l, &mut r.scope)?
		} else {
			runfile_lang::eval::interpolate_plain(l, &mut r.scope)?
		});
	}
	Ok((cmd, lines.join("\n")))
}

fn cwd(props: &Props, anchor: &Path) -> PathBuf {
	match &props.workdir {
		Some(w) => anchor.join(w),
		None => anchor.to_path_buf(),
	}
}

/// The environment a command is spawned with: exactly what `ENV.X` reads.
///
/// It used to lay the block's `.env` back on top of the built one, because a
/// block that set only `.env` was never rebuilt -- and that made it a second
/// description of precedence, disagreeing with the first: the property won
/// here and the caller's shell won in `build_env`, so one target gave two
/// answers for one name. Every block that changes `.env` is rebuilt now, and
/// `build_env` puts the property on top, so there is nothing left to overlay.
fn merged_env(r: &Runner<'_>) -> Vec<(String, String)> {
	r.env.clone()
}

// ------------------------------------------------------------------ parallel

/// A parallel block evaluates its bindings in source order, then fans out the
/// executable leaves. Rendering happens first and sequentially, so a thread
/// carries a finished command rather than a borrow of the scope.
fn walk_parallel(block: &Block, props: &Props, r: &mut Runner<'_>) -> Result<(), RunError> {
	let mut leaves = Vec::new();
	collect(block, props, r, &mut leaves)?;
	run_leaves(leaves, r)
}

/// A block nested inside a fan-out, layering its own properties over the
/// enclosing ones -- `nested` for the collecting walk.
///
/// A leaf is rendered where it is collected, so a `.workdir`, `.env` or
/// `.shell` not applied here is applied never: the parser accepted the
/// property and the branch then ran without it.
fn collect_nested(block: &Block, props: &Props, r: &mut Runner<'_>, out: &mut Vec<Leaf>) -> Result<(), RunError> {
	let inner = props.extend(block, &mut r.scope, true)?;
	with_block_env(props, &inner, r, |r| collect(block, &inner, r, out))
}

fn collect(block: &Block, props: &Props, r: &mut Runner<'_>, out: &mut Vec<Leaf>) -> Result<(), RunError> {
	// A fan-out's preamble runs in source order, so a property written inside
	// one is applied in source order too -- from there down, over the leaves
	// collected after it. The environment is not put back here: `walk_parallel`
	// is called from `walk`, which already wraps a block that has one.
	let mut props = Cow::Borrowed(props);
	let mut trailing = block.trailing().iter().peekable();
	for st in &block.statements {
		while let Some(p) = trailing.next_if(|p| p.span.line < st.span().line) {
			apply_trailing(p, &mut props, r)?;
		}
		let props = props.as_ref();
		match st {
			Statement::Let { names, value, span } | Statement::Assign { names, value, span } => {
				let v = value_of(value, props, r)?;
				let parts = runfile_lang::eval::destructure(names, v, span.line)?;
				r.scope.bind_names(names, parts);
			}
			Statement::Call { expr, .. } => {
				// Through `value_of`, so a bare `code_of($ cmd)` runs the
				// command rather than reaching the pure evaluator, which has
				// no shell -- the same reason the sequential walk does.
				value_of(expr, props, r)?;
			}
			Statement::Exec {
				command, body, detach, ..
			} => {
				let (cmd, text) = render(command.as_deref(), body, r)?;
				out.push(Leaf::Exec {
					// From the header as written, before `.shell` is folded in:
					// a `$` branch under `.shell = "python3"` is still named by
					// its own first word.
					label: exec_label(cmd.as_deref(), &text),
					command: command_for(cmd.as_deref(), props).map(str::to_string),
					detach: *detach,
					announce: props.logging,
					ignore_errors: props.ignore_errors,
					body: text,
					env: merged_env(r),
					dir: cwd(props, &r.anchor),
				});
			}
			Statement::Run { target, args, .. } => {
				let t = runfile_lang::eval::interpolate_plain(target, &mut r.scope)?;
				let a = run_args(args, &mut r.scope)?;
				out.push(Leaf::Run {
					target: t,
					args: a,
					ignore_errors: props.ignore_errors,
				});
			}
			// Control flow is expanded here so its leaves join the same batch.
			Statement::Do { body, .. } => collect_nested(body, props, r, out)?,
			Statement::If {
				cond,
				then,
				otherwise,
				span,
			} => {
				// A condition decides which branches join the batch, so it is
				// answered before the batch exists -- and `if $ cmd` is a
				// condition like any other, which needs a process to ask.
				let taken = cond_of(cond, props, r, span.line)?;
				match (taken, otherwise) {
					(true, _) => collect_nested(then, props, r, out)?,
					(false, Some(b)) => collect_nested(b, props, r, out)?,
					(false, None) => {}
				}
			}
			Statement::For {
				names,
				iter,
				body,
				span,
			} => {
				// Through `value_of` too: a fan-out's list is what says how
				// many branches there are, and `for f in lines($ git ls-files)`
				// cannot answer that from the pure evaluator.
				let items = match value_of(iter, props, r)? {
					Value::List(v) => v,
					other => {
						return Err(RunError::ForNeedsList {
							actual: other.type_name(),
							line: span.line,
						});
					}
				};
				let priors: Vec<Option<Value>> = names.iter().map(|n| r.scope.vars.remove(n)).collect();
				let inner = props.extend(body, &mut r.scope, true)?;
				let res = with_block_env(props, &inner, r, |r| {
					for item in items {
						bind_item(names, item, span.line, r)?;
						collect(body, &inner, r, out)?;
					}
					Ok(())
				});
				restore_all(names, &priors, r);
				res?;
			}
			// A fan-out collects every branch before any of them runs, and a
			// conditional loop has nothing to collect until its body has run
			// at least once. `break` has the same problem from the other end:
			// by the time one could be honoured, the batch is already built.
			Statement::Loop { test, span, .. } => {
				return Err(RunError::NotInParallel {
					kind: test.keyword(),
					line: span.line,
				});
			}
			Statement::Break { span } => {
				return Err(RunError::NotInParallel {
					kind: "break",
					line: span.line,
				});
			}
			Statement::Continue { span } => {
				return Err(RunError::NotInParallel {
					kind: "continue",
					line: span.line,
				});
			}
			// Retrying inside a fan-out would mean several bodies sleeping and
			// re-running against each other, with no useful reading of what
			// "attempts" counted. Refused rather than guessed at.
			Statement::Retry { span, .. } => return Err(RunError::RetryInParallel { line: span.line }),
			Statement::Match {
				subject,
				cases,
				default,
				span,
			} => {
				let v = subject_of(subject, props, r)?;
				match cases.iter().find(|c| c.label == v) {
					Some(c) => collect_nested(&c.body, props, r, out)?,
					None => match default {
						Some(b) => collect_nested(b, props, r, out)?,
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
	// As in `walk_body`: applied where it sits, even with nothing below it.
	for p in trailing {
		apply_trailing(p, &mut props, r)?;
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

fn run_leaves(leaves: Vec<Leaf>, r: &mut Runner<'_>) -> Result<(), RunError> {
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
						command,
						body,
						env,
						dir,
						label,
						detach,
						announce,
						..
					} => exec::spawn(Spawn {
						command: command.as_deref(),
						body,
						cwd: dir,
						env,
						capture: false,
						dry_run,
						label: Some(label),
						detach: *detach,
						announce: *announce && !dry_run,
					})
					.map(|_| Some(body.clone()))
					.map_err(RunError::from),
					// Branches finish in whatever order they finish, so a
					// dispatched target's trace joins the parent's as one block
					// rather than being interleaved line by line.
					// A dispatched target labels its own leaves, so nothing is
					// added here; its trace joins the parent's as one block.
					Leaf::Run { target, args, .. } => dispatch
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
	// Paired with its leaf, because `.ignore-errors` is block-scoped: a branch
	// out of a nested block that named it is forgiven while its siblings are
	// not.
	for (leaf, res) in leaves.iter().zip(results) {
		match res {
			Ok(Some(body)) => r.trace.push(body),
			Ok(None) => {}
			Err(e) if leaf.ignore_errors() && !e.is_stop() => {}
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
		Some(e) => Err(e),
		None => Ok(()),
	}
}
