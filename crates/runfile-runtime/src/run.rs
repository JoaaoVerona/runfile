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

	/// Whether this is an instruction to stop rather than a failure. A target
	/// may shrug off a command that failed; it does not get to shrug off
	/// `exit()`, or someone answering no to `confirm()`.
	pub fn is_stop(&self) -> bool {
		matches!(
			self,
			RunError::Eval(EvalError::Exit { .. } | EvalError::Cancelled { .. }) | RunError::Cancelled
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
	build_env(&props, r)?;

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
			&& (!props.ignore_errors || e.is_stop())
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
		Statement::For { name, iter, body, span } => {
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
			let prior = r.scope.vars.remove(name);
			let inner = props.extend(body, &mut r.scope, true)?;
			let out = with_block_env(props, &inner, r, |r| {
				for_body(name, items, body, &inner, prior.clone(), r)
			});
			r.scope.restore(name, prior);
			out
		}
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
		Statement::Exec { command, body, .. } => {
			let (cmd, text) = render(command.as_deref(), body, r)?;
			r.trace.push(text.clone());
			let env = merged_env(r, props);
			let dir = cwd(props, &r.anchor);
			exec::spawn(Spawn {
				command: command_for(cmd.as_deref(), props),
				body: &text,
				cwd: &dir,
				env: &env,
				capture: false,
				dry_run: r.dry_run,
				label: r.label.as_deref(),
				detach: props.detach,
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
	let workdir = cwd(props, &r.anchor);
	// The same deferred pool the `decrypt` function uses, so an encrypted
	// `.env-file` value resolves -- and an unencrypted one still never touches
	// the credential store.
	let keys = crate::env::Provider(r.scope.private_keys.clone());
	let built = crate::env::build(props, &r.anchor, &workdir, Some(&keys))?;
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
/// this tells. Rebuilt only then: reading and decrypting the files again for
/// every `if` would be work nothing asked for. Restoring afterwards is what
/// keeps a loop from carrying one iteration's files into the next, and is why
/// every block form has to come through here rather than calling `walk`.
fn with_block_env<T>(
	props: &Props,
	inner: &Props,
	r: &mut Runner<'_>,
	f: impl FnOnce(&mut Runner<'_>) -> Result<T, RunError>,
) -> Result<T, RunError> {
	if inner.env_files.len() == props.env_files.len() && inner.add_paths.len() == props.add_paths.len() {
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

/// The iterations of a `for`. The loop variable is restored by the caller,
/// which also owns the block's environment.
fn for_body(
	name: &str,
	items: Vec<Value>,
	body: &Block,
	inner: &Props,
	prior: Option<Value>,
	r: &mut Runner<'_>,
) -> Result<(), RunError> {
	// `.parallel` on a loop body means the *iterations* are the branches, not
	// just each body's own statements. Collecting across every iteration first
	// is what makes them one batch.
	if inner.parallel {
		let mut leaves = Vec::new();
		for item in items {
			r.scope.bind(name, item);
			collect(body, inner, r, &mut leaves)?;
		}
		r.scope.restore(name, prior);
		return run_leaves(leaves, inner, r);
	}
	for item in items {
		r.scope.bind(name, item);
		if let Err(e) = walk(body, inner, r)
			&& (!inner.ignore_errors || e.is_stop())
		{
			return Err(e);
		}
	}
	Ok(())
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
	let env = merged_env(r, props);
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
	let env = merged_env(r, props);
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
			Statement::Do { body, .. } => collect(body, props, r, out)?,
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
	let logging = props.logging;
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
						command: command_for(cmd.as_deref(), props),
						body,
						cwd: dir,
						env,
						capture: false,
						dry_run,
						label: Some(label),
						detach: *detach,
						announce: logging && !dry_run,
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
		Some(e) if !props.ignore_errors || e.is_stop() => Err(e),
		_ => Ok(()),
	}
}
