//! The statement walker.

use crate::exec::{self, ExecError, Spawn};
use crate::props::{PropError, Props};
use crate::term;
use runfile_lang::Value;
use runfile_lang::args::Word;
use runfile_lang::ast::*;
use runfile_lang::eval::{EvalError, Scope, eval_boundary};

#[derive(Debug, thiserror::Error)]
pub enum RunError {
	#[error(transparent)]
	Eval(#[from] EvalError),
	/// Never an evaluation error: see the `From` below.
	#[error(transparent)]
	Prop(PropError),
	#[error(transparent)]
	Exec(#[from] ExecError),
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
	/// `break` -- travelling as an error because that is the only way back out
	/// of a walk. The message is a net: the parser refuses one outside a loop,
	/// so nothing should ever print this.
	#[error("line {line}: `break` outside a loop")]
	Break { line: usize },
	#[error("line {line}: `continue` outside a loop")]
	Continue { line: usize },
	/// A target ended with `exit(code)` and a status that is not 0 -- what that
	/// `exit()` becomes once it has left the target it was written in.
	///
	/// Inside that target it was an instruction nothing gets to shrug off. Past
	/// it, it is how the target went, and to the line that ran it a failure like
	/// any other: `.ignore-errors` forgives it and `retry` has another go, as
	/// they would at a `$ run` of the same target. So it is not `is_stop`. It
	/// keeps its status on the way up, which is what `code_of` scores and what
	/// the run ends with when nothing handles it.
	#[error("`{target}` exited with status {code}")]
	Exited { target: String, code: i32 },
	#[error(transparent)]
	Host(Box<dyn std::error::Error + Send + Sync>),
}

/// A property's value is an expression like any other, so what evaluating one
/// raised is an evaluation error like any other. Kept inside a `Prop`, an
/// `exit()` or a declined `confirm()` written in a value was neither a stop nor
/// a status: `.ignore-errors` forgave `.env.X = exit(3)` in a block where it
/// would not have forgiven `exit(3)` on the line below it, and the same `exit()`
/// in a header was reported as `error: exit 3`, status 1.
impl From<PropError> for RunError {
	fn from(e: PropError) -> Self {
		match e {
			PropError::Eval(e) => RunError::Eval(e),
			e => RunError::Prop(e),
		}
	}
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
	/// The status a target asked for with `exit(code)`, if this is that rather
	/// than a failure: an `exit()` still on its way out of the target that
	/// called it, or the status that target handed back (`Exited`). The CLI
	/// makes it the process's status and `code_of` makes it the score, rather
	/// than printing either as an error.
	pub fn exit_code(&self) -> Option<i32> {
		match self {
			RunError::Eval(EvalError::Exit { code, .. }) | RunError::Exited { code, .. } => Some(*code),
			_ => None,
		}
	}

	/// Whether this is something the run has to carry out rather than a
	/// command that went wrong. A target may shrug off a command that failed;
	/// it does not get to shrug off its own `exit()`, someone answering no to
	/// `confirm()`, or a `break` -- forgiving one of those would leave a
	/// statement that plainly did nothing, which is the worst way for it to be
	/// wrong. It is also what carries a `break` out through a `retry`, which
	/// would otherwise read it as a failed attempt and run the body again.
	///
	/// A target that ended with `exit(3)` is not a stop to the one that ran it
	/// (`Exited`): the caller did not say it, and to the line that ran the
	/// target, that status is a failure like any other.
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
	/// it went: Ctrl+C, or someone answering no to a `confirm()`.
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
/// `Sync` and `&self` because a parallel block runs its branches on threads.
/// The call chain is passed rather than held: parallel branches have separate
/// paths, so a shared stack would make one branch look like a cycle to another.
pub trait Dispatch: Sync {
	/// Run `target`, adding its dry-run trace to `trace`.
	///
	/// `env` is what the target is run with: the environment a command at the
	/// calling line would be given, as `env::handed_over` splits it. Handed
	/// over rather than left to the process, which is all a target had before,
	/// so a caller's `ENV.X = …`, `import_env_file()` and `add_path()` reach the
	/// target it runs the way they reach its own commands.
	///
	/// `label` prefixes everything the target prints, when it runs inside a
	/// parallel branch. It is inherited by whatever the target dispatches in
	/// turn, so a whole subtree reads as one branch.
	///
	/// `trace` is the caller's own rather than a shared one, so what the target
	/// ran lands where the call appeared. Writing it centrally printed every
	/// dependency before the line that invoked it, because a child finishes
	/// while its parent is still walking. It is added to however the target
	/// ended: one that stopped part-way -- at an `exit()`, or at a failure the
	/// caller then forgives or scores -- still ran what came before the stop,
	/// and a preview that dropped those lines showed the caller carrying on
	/// after a call that seemed to have done nothing.
	fn run(
		&self,
		target: &str,
		args: &[String],
		env: crate::env::Inherited,
		chain: &[String],
		label: Option<&str>,
		trace: &mut Vec<String>,
	) -> Result<(), RunError>;

	/// [`Dispatch::run`], with each argument saying whether it is a value a
	/// `run` statement interpolated, which the target then reads as a positional
	/// and never as a flag (audit 2026-11-06, SA-006). A dispatcher that does not
	/// classify arguments itself is handed their text.
	fn run_words(
		&self,
		target: &str,
		args: &[runfile_lang::args::Word],
		env: crate::env::Inherited,
		chain: &[String],
		label: Option<&str>,
		trace: &mut Vec<String>,
	) -> Result<(), RunError> {
		let text: Vec<String> = args.iter().map(|w| w.text.clone()).collect();
		self.run(target, &text, env, chain, label, trace)
	}
}

pub struct NoDispatch;
impl Dispatch for NoDispatch {
	fn run(
		&self,
		_t: &str,
		_a: &[String],
		_e: crate::env::Inherited,
		_c: &[String],
		_l: Option<&str>,
		_trace: &mut Vec<String>,
	) -> Result<(), RunError> {
		Err(RunError::NoResolver { line: 0 })
	}
}

pub struct Runner<'a> {
	pub scope: Scope,
	/// Targets already on this call path, for cycle detection.
	pub chain: Vec<String>,
	pub dispatch: &'a dyn Dispatch,
	/// Whether the run has been interrupted. Injected rather than read from a
	/// global so the runtime reaches for no process state of its own, and so a
	/// test can interrupt one run without touching another.
	pub interrupted: Option<&'a (dyn Fn() -> bool + Sync)>,
	/// Prefix for this target's output; see `Dispatch::run`.
	pub label: Option<String>,
	/// Which colour of the palette this branch's label was painted in, so the
	/// branches made *inside* it can start past it rather than repeating it.
	/// `None` outside a branch, and in a dispatched target: a colour says which
	/// of the branches made together this is, and a dispatch is not one of them.
	pub colour: Option<usize>,
	/// Collected so a caller can show what ran without re-deriving it.
	pub dry_run: bool,
	pub trace: Vec<String>,
}

pub fn run_target(target: &Target, r: &mut Runner<'_>) -> Result<(), RunError> {
	run_chain(&[], target, Props::default(), r, |_| Ok(()))
}

/// Run a target under the `_shared.run` chain above it, outermost first.
///
/// A shared file runs ahead of the target, line for line the way the target
/// does, and what it leaves behind -- its bindings, the environment, the
/// working directory, `ignore_errors()` -- is what the next file and then the
/// target start from. Its properties are layered over the ones above it, and a
/// file's are worked out once those have run, so a value reads what they set.
/// `header` is handed the target's own, before its first line runs: the watch
/// loop learns what to watch from it.
pub fn run_chain(
	shared: &[Target],
	target: &Target,
	base: Props,
	r: &mut Runner<'_>,
	header: impl FnOnce(&Props) -> Result<(), RunError>,
) -> Result<(), RunError> {
	// Only the target itself can be a package's entry.
	let entry = base.entry;
	let mut props = Props { entry: false, ..base };
	for s in shared {
		props = props.extend(&s.body, &mut r.scope)?;
		walk(&s.body, &props, r)?;
	}
	props.entry = entry;
	let props = props.extend(&target.body, &mut r.scope)?;
	header(&props)?;
	walk(&target.body, &props, r)
}

/// A block's statements, in order.
///
/// A failure stops the block unless `ignore_errors()` is in effect where it
/// happened. It is read once the statement is over, which is the state that
/// statement was run in: a `do` puts back what changed inside it before it
/// returns, so one that turned it on and then failed is judged by the block
/// around it.
fn walk(block: &Block, props: &Props, r: &mut Runner<'_>) -> Result<(), RunError> {
	for st in &block.statements {
		// Between statements, not inside one: a child already took the same
		// SIGINT and is gone, so there is nothing to wait for and nothing to
		// kill. `ignore_errors()` does not apply -- an interrupt is not a
		// failure the target gets to shrug off.
		if r.interrupted.is_some_and(|f| f()) {
			return Err(RunError::Interrupted);
		}
		if let Err(e) = statement(st, props, r)
			&& (!r.scope.ignore_errors || e.is_stop())
		{
			return Err(e);
		}
	}
	Ok(())
}

/// Run `f`, putting back the state it started in however it ends: what a `do`
/// block does at its `end`.
fn scoped<T>(r: &mut Runner<'_>, f: impl FnOnce(&mut Runner<'_>) -> Result<T, RunError>) -> Result<T, RunError> {
	let saved = r.scope.state();
	let out = f(r);
	r.scope.put_back(saved);
	out
}

fn statement(st: &Statement, props: &Props, r: &mut Runner<'_>) -> Result<(), RunError> {
	match st {
		Statement::Let { names, value, span } | Statement::Assign { names, value, span } => {
			let v = value_of(value, props, r)?;
			let parts = runfile_lang::eval::destructure(names, v, span.line)?;
			r.scope.bind_names(names, parts);
			Ok(())
		}
		Statement::SetEnv { name, value, span } => {
			let v = value_of(value, props, r)?;
			Ok(runfile_lang::state::assign_env(&mut r.scope, name, v, span.line)?)
		}
		Statement::Call { expr, .. } => {
			// Through value_of, so a bare `code_of($ cmd)` runs the command
			// rather than reaching the pure evaluator, which has no shell. An
			// effect -- `set_cwd()` and the rest -- changes the scope it is
			// handed, which is the state every line below runs in.
			value_of(expr, props, r)?;
			Ok(())
		}
		Statement::Do {
			body, parallel: true, ..
		} => parallel_do(body, props, r),
		Statement::Do { body, .. } => scoped(r, |r| walk(body, props, r)),
		Statement::If {
			cond,
			then,
			otherwise,
			span,
		} => {
			let taken = cond_of(cond, props, r, span.line)?;
			match (taken, otherwise) {
				(true, _) => walk(then, props, r),
				(false, Some(b)) => walk(b, props, r),
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
			// A delay is usually data, and `from_secs_f64` panics on a value it
			// cannot hold -- validated here, where `sleep` is, so neither aborts.
			let secs = match delay {
				Some(d) => {
					let s = eval_boundary(d, &mut r.scope)?
						.as_num()
						.map_err(|e| EvalError::ty(span.line, e))?;
					runfile_lang::functions::duration_secs(s)
						.map_err(|m| EvalError::Other {
							msg: format!("`retry … every` {m}"),
							line: span.line,
						})?
						.as_secs_f64()
				}
				None => 0.0,
			};
			let start = r.scope.state();
			match retry_attempts(body, props, n, secs, &start, r) {
				// The attempt that succeeded keeps what it changed. Failure was
				// visible inside it whatever the block around it said, and is
				// as forgiven as it was before once it is over.
				Ok(None) => {
					r.scope.ignore_errors = start.ignore_errors;
					Ok(())
				}
				// The `else` runs from where the `retry` began: it is what to do
				// when the block never worked, not part of it.
				Ok(Some(e)) => {
					r.scope.put_back(start);
					match otherwise {
						Some(b) => walk(b, props, r),
						None => Err(e),
					}
				}
				Err(e) => {
					r.scope.ignore_errors = start.ignore_errors;
					Err(e)
				}
			}
		}
		Statement::For {
			names,
			iter,
			body,
			parallel: true,
			span,
		} => parallel_for(names, iter, body, props, r, span.line),
		Statement::For {
			names,
			iter,
			body,
			span,
			..
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
			let out = for_body(names, items, body, props, span.line, r);
			restore_all(names, &priors, r);
			out
		}
		Statement::Loop { test, body, span } => loop_passes(test, body, props, span.line, r),
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
				return walk(&c.body, props, r);
			}
			match default {
				Some(b) => walk(b, props, r),
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
			let env = crate::env::handed_over(&r.scope);
			// The dependency's trace lands where the call appeared. A target
			// that ended with `exit(3)` fails this line, as `$ run` would have.
			r.dispatch
				.run_words(&t, &a, env, &r.chain, r.label.as_deref(), &mut r.trace)
		}
		Statement::Exec {
			command, body, detach, ..
		} => {
			let rendered = render(command.as_deref(), body, props, r)?;
			// The preview shows the resolved body -- its documented contract, and
			// what the argument-parsing tests read back. A `--dry-run` reads no
			// `.env` file and holds no key (SA-008), so a decrypted secret is
			// never in scope to be interpolated here; the auto-firing paths that
			// *do* run for real -- failure messages and `.logging` -- redact via
			// the display form instead (SA-012).
			r.trace.push(rendered.body.clone());
			let env = env_of(r);
			let dir = r.scope.workdir.clone();
			exec::spawn(Spawn {
				command: command_for(rendered.cmd.as_deref(), props),
				body: &rendered.body,
				show_command: command_for(rendered.show_cmd.as_deref(), props),
				display: &rendered.show_body,
				cwd: &dir,
				env: &env,
				capture: false,
				dry_run: r.dry_run,
				label: r.label.as_deref(),
				detach: *detach,
				announce: props.logging && !r.dry_run,
				direct: false,
			})?;
			Ok(())
		}
	}
}

/// The attempts of a `retry`. `Ok(None)` when one of them succeeded, and
/// `Ok(Some(e))` when every one failed -- which is the question an `else`
/// answers.
///
/// Every attempt starts from `start`, the state the `retry` began with: an
/// attempt is the block run *again*, and again from wherever the last one gave
/// up -- a `set_cwd("web")` already done, so the next one went to `web/web` --
/// is not what again means. Failure is visible inside the block whatever the
/// block around it said: a retry that could not see failure would run once.
fn retry_attempts(
	body: &Block,
	props: &Props,
	n: u64,
	secs: f64,
	start: &runfile_lang::state::State,
	r: &mut Runner<'_>,
) -> Result<Option<RunError>, RunError> {
	let mut last = None;
	for attempt in 0..n {
		if attempt > 0 {
			r.scope.put_back(start.clone());
		}
		r.scope.ignore_errors = false;
		if r.interrupted.is_some_and(|f| f()) {
			return Err(RunError::Interrupted);
		}
		match walk(body, props, r) {
			Ok(()) => return Ok(None),
			// This target's own `exit` is an instruction to stop, not a failure
			// to retry. A target it ran ending with one is a failure like any
			// other (`Exited`), and gets another go.
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

/// The iterations of a `for`. The loop variables are restored by the caller.
///
/// What a pass changes is what the next one starts from, as it is for any
/// block but a `do`: a body that wants each pass to start where the first did
/// is written as one.
fn for_body(
	names: &[String],
	items: Vec<Value>,
	body: &Block,
	props: &Props,
	line: usize,
	r: &mut Runner<'_>,
) -> Result<(), RunError> {
	for item in items {
		bind_item(names, item, line, r)?;
		match walk(body, props, r) {
			Ok(()) | Err(RunError::Continue { .. }) => {}
			Err(RunError::Break { .. }) => break,
			Err(e) => return Err(e),
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
fn loop_passes(test: &LoopTest, body: &Block, props: &Props, line: usize, r: &mut Runner<'_>) -> Result<(), RunError> {
	loop {
		// Checked here as well as in `walk`, which looks *between* statements:
		// a `loop` with an empty body has none, and would spin past every
		// chance to notice a Ctrl+C.
		if r.interrupted.is_some_and(|f| f()) {
			return Err(RunError::Interrupted);
		}
		let again = match test {
			LoopTest::Forever => true,
			LoopTest::While(c) => cond_of(c, props, r, line)?,
			LoopTest::Until(c) => !cond_of(c, props, r, line)?,
		};
		if !again {
			return Ok(());
		}
		match walk(body, props, r) {
			Ok(()) | Err(RunError::Continue { .. }) => {}
			Err(RunError::Break { .. }) => return Ok(()),
			Err(e) => return Err(e),
		}
		if r.dry_run {
			return Ok(());
		}
	}
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
	let rendered = render(command.as_deref(), body, props, r)?;
	let env = env_of(r);
	let dir = r.scope.workdir.clone();
	let out = exec::spawn(Spawn {
		command: command_for(rendered.cmd.as_deref(), props),
		body: &rendered.body,
		show_command: command_for(rendered.show_cmd.as_deref(), props),
		display: &rendered.show_body,
		cwd: &dir,
		env: &env,
		capture: true,
		dry_run: r.dry_run,
		label: r.label.as_deref(),
		// A capture's output is a value, not something to show, and it is
		// never detached: the whole point is waiting for what it prints.
		detach: false,
		announce: props.logging && !r.dry_run,
		direct: false,
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
	if let Expr::Program { command, .. } = e {
		return program_code(command, props, r);
	}
	let Expr::Capture { command, body, .. } = e else {
		// The parser used to guarantee this, back when `code_of` was the only
		// call a capture could sit inside. Now that any call may hold one,
		// answering 0 for `code_of("x")` would be a silent wrong number.
		return Err(RunError::Eval(runfile_lang::eval::EvalError::Other {
			msg: "`code_of` takes a `$` run, a `run` dispatch or an `exec` command, as `code_of($ mkdir out)`, `code_of(run build)` or `code_of(exec make)`"
				.into(),
			line: e.span().line,
		}));
	};
	let rendered = render(command.as_deref(), body, props, r)?;
	let env = env_of(r);
	let dir = r.scope.workdir.clone();
	Ok(exec::spawn_code(Spawn {
		command: command_for(rendered.cmd.as_deref(), props),
		body: &rendered.body,
		show_command: command_for(rendered.show_cmd.as_deref(), props),
		display: &rendered.show_body,
		cwd: &dir,
		env: &env,
		capture: false,
		dry_run: r.dry_run,
		label: r.label.as_deref(),
		detach: false,
		announce: props.logging && !r.dry_run,
		direct: false,
	})?)
}

/// `code_of(exec <command>)`: the program started directly, with the terminal,
/// and its exit status.
///
/// No shell reads the line, so it is written for the reader that does -- the
/// runner's own split, as an `exec` block's command is -- and nothing on
/// Windows needs a bash to start a native program. In a parallel branch it is
/// relayed and labelled like any other command, with no terminal to read.
fn program_code(command: &[InterpPart], props: &Props, r: &mut Runner<'_>) -> Result<i32, RunError> {
	let cmd = interpolate_command(command, &mut r.scope)?;
	let show = runfile_lang::eval::interpolate_display(command);
	// What a preview prints, as an `exec` block's resolved body is.
	r.trace.push(cmd.clone());
	let env = env_of(r);
	let dir = r.scope.workdir.clone();
	Ok(exec::spawn_code(Spawn {
		command: Some(&cmd),
		body: "",
		show_command: Some(&show),
		display: "",
		cwd: &dir,
		env: &env,
		capture: false,
		dry_run: r.dry_run,
		label: r.label.as_deref(),
		detach: false,
		announce: props.logging && !r.dry_run,
		direct: true,
	})?)
}

/// `code_of(run <target>)`: the status the dispatch would have exited with.
///
/// The number is the one `$ run <target>` yields, because dispatching
/// in-process is meant to stop re-execing the binary, not to mean something
/// else: a target that ends with `exit(3)` is scored 3 -- as is one that ran
/// such a target and let its status through -- and every other failure is the
/// 1 the CLI reports. The failure is printed the way that re-exec would have
/// printed it, since this is where it stops -- a status is an answer, so
/// nothing further up will say what went wrong.
///
/// A refusal is not a status and is passed on, which is also what a re-exec
/// does: Ctrl+C reaches the whole process group.
fn dispatch_code(target: &[InterpPart], args: &[Vec<InterpPart>], r: &mut Runner<'_>) -> Result<i32, RunError> {
	let t = runfile_lang::eval::interpolate_plain(target, &mut r.scope)?;
	let a = run_args(args, &mut r.scope)?;
	let env = crate::env::handed_over(&r.scope);
	match r
		.dispatch
		.run_words(&t, &a, env, &r.chain, r.label.as_deref(), &mut r.trace)
	{
		Ok(()) => Ok(0),
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
fn run_args(words: &[Vec<InterpPart>], sc: &mut Scope) -> Result<Vec<Word>, RunError> {
	let mut out = Vec::new();
	for w in words {
		if let [InterpPart::Expr(e)] = &w[..] {
			match eval_boundary(e, sc)? {
				// A list forwarded whole -- `{{ ARGS }}` -- is a command line
				// being handed on, and is read the way it was typed.
				Value::List(items) => out.extend(items.iter().map(|v| Word::literal(v.to_string()))),
				// One value is data: whatever it spells, the callee reads it as
				// a positional, never as a flag (audit 2026-11-06, SA-006).
				v => out.push(Word::data(v.to_string())),
			}
		} else {
			let text = runfile_lang::eval::interpolate_plain(w, sc)?;
			// A word that starts with a value is data for the same reason; one
			// the file starts, as `--name={{ x }}`, is classified by what the
			// file wrote.
			out.push(if matches!(w.first(), Some(InterpPart::Expr(_))) {
				Word::data(text)
			} else {
				Word::literal(text)
			});
		}
	}
	Ok(out)
}

/// An `exec` line with its values written in, each for the quotes the text
/// before it leaves open, so that `exec::split_command` reads every value back
/// as exactly itself.
fn interpolate_command(parts: &[InterpPart], sc: &mut Scope) -> Result<String, EvalError> {
	let (mut out, mut quote) = (String::new(), None);
	for p in parts {
		match p {
			InterpPart::Literal(t) => {
				quote = exec::quote_after(quote, t);
				out.push_str(t);
			}
			InterpPart::Expr(e) => out.push_str(&exec::command_text(&eval_boundary(e, sc)?, quote)),
		}
	}
	Ok(out)
}

/// Interpolate an `exec`'s command line and its body.
///
/// The command line is split into arguments by the runner, never by a shell,
/// so its values are written for that reader (`exec::command_text`): a value
/// with a space in it stays one argument, and one with a quote in it arrives
/// with the quote. **The body is quoted only when
/// the command is a shell.** Anybody else's body is their language, and shell
/// quoting is the wrong quoting for it -- `'it'\''s'` is not Python, a list
/// flattens to bare words, and a path comes through unquoted because nothing
/// in it needed quoting. Left alone, a value arrives as itself and the author
/// quotes it the way that language wants, which is the only thing that can be
/// right for every language.
///
/// A shell body's values are written for where each one sits, as the shell
/// checker reads the body the way bash would: one word outside quotes, and
/// escaped for the quotes around it inside `"…"`, a heredoc or backticks, so a
/// `$( )` in a value is text everywhere -- see [`runfile_lang::Quoting`]. In a
/// shell that reading does not follow, every value is the one quoted word it
/// always was.
/// The two forms of a command. `cmd`/`body` are what the shell runs, values
/// spliced in. `show_cmd`/`show_body` are what a human is shown -- every
/// `{{ … }}` kept as a source-form placeholder -- so an interpolated secret
/// does not reach a failure message, a `.logging` line or a `--dry-run` preview
/// (audit SA-012).
struct Rendered {
	cmd: Option<String>,
	body: String,
	show_cmd: Option<String>,
	show_body: String,
}

fn render(
	command: Option<&[InterpPart]>,
	body: &[Vec<InterpPart>],
	props: &Props,
	r: &mut Runner<'_>,
) -> Result<Rendered, RunError> {
	let cmd = match command {
		Some(c) => Some(interpolate_command(c, &mut r.scope)?),
		None => None,
	};
	// Whether the body is shell text sees through `env bash`, `sudo sh` and the
	// like (audit 2026-11-06, SA-001), which `exec::body_is_shell` -- the test for
	// what the runner itself starts with `-e` and `-c` -- does not.
	let lines = if runfile_shell::reads_as_shell(cmd.as_deref()) {
		let spots = runfile_shell::spots(command_for(cmd.as_deref(), props), body);
		runfile_lang::eval::interpolate_script(body, spots.as_deref(), &mut r.scope)?
	} else {
		let mut lines = Vec::with_capacity(body.len());
		for l in body {
			lines.push(runfile_lang::eval::interpolate_plain(l, &mut r.scope)?);
		}
		lines
	};
	// The display form evaluates nothing, so it reads no value and cannot fail.
	let show_cmd = command.map(runfile_lang::eval::interpolate_display);
	let show_body = body
		.iter()
		.map(|l| runfile_lang::eval::interpolate_display(l))
		.collect::<Vec<_>>()
		.join("\n");
	Ok(Rendered {
		cmd,
		body: lines.join("\n"),
		show_cmd,
		show_body,
	})
}

/// The environment a command is spawned with: exactly what `ENV.X` reads.
///
/// One map, the scope's, changed line by line. It used to be built twice -- a
/// copy here and another for `ENV.X` -- from properties layered in a fixed
/// order, and the two came to disagree about precedence more than once.
fn env_of(r: &Runner<'_>) -> Vec<(String, String)> {
	let mut env: Vec<(String, String)> = r.scope.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
	env.sort();
	env
}

// ------------------------------------------------------------------ parallel
//
// `parallel do` and `parallel for` are structured concurrency: every statement
// directly inside a `parallel do`, and every iteration of a `parallel for`, is
// a branch; the branches run at once, each on a thread of its own; and the
// block ends when the last one does. A branch is ordinary code, walked by the
// one walker, in the order it is written.
//
// It replaced the `.parallel` property, which was a second walker: it
// collected the commands out of a block ahead of time and ran only those at
// once. Every `let`, `print` and `sleep` ran first, in order, before anything
// was parallel at all; two adjacent `$` lines were folded into one process and
// so were one branch, run one after the other; and a `for` became one flat
// batch, so a service's `push` finished before its own `build`. It also had to
// refuse `retry` and every conditional loop, having no way to collect what
// they would run -- and it drifted from the real walker three times, over
// nested properties, preamble captures and leaf properties.

impl<'a> Runner<'a> {
	/// A runner for one branch, labelled `label`.
	///
	/// It has a copy of this one's scope, so what the branch binds reaches
	/// neither its siblings nor the code after the block -- and the parser
	/// refuses a branch assigning anything it did not bind itself, since that
	/// write would be lost. Branches that nest join their labels, so the output
	/// of `parallel do` inside `parallel for` says which iteration it is.
	///
	/// An empty label adds nothing. It is what a `parallel` block that is
	/// itself a branch gets: its own branches are what tell the output apart,
	/// so `web` says everything `parallel:9/web` would. It is a branch all the
	/// same, which is what keeps the terminal out of its reach.
	fn fork(&self, label: &str, nth: usize, width: usize) -> Runner<'a> {
		// Siblings take consecutive colours, so the branches whose output is
		// interleaved are exactly the ones guaranteed to differ; a nested set
		// starts past the colour of the branch it hangs off, so the two halves of
		// an `outer/inner` label are never the same colour either. Keyed on
		// stdout, which is the stream a pipeline reads -- `run dev > log` keeps
		// the escapes out of the file, and the cost is a terminal's stderr going
		// unpainted when stdout alone is redirected.
		let colour = self.colour.map_or(nth, |c| c + 1 + nth);
		let own = segment(label, colour, width);
		let label = match self.label.as_deref() {
			Some(outer) if !outer.is_empty() && !own.is_empty() => format!("{outer}/{own}"),
			Some(outer) if own.is_empty() => outer.to_string(),
			_ => own,
		};
		let mut scope = self.scope.clone();
		// Every branch shares one terminal, so nothing in one asks it anything:
		// `confirm()` refuses, and a missing input is an error, not a prompt.
		scope.branch = Some(label.clone());
		scope.ask = None;
		Runner {
			scope,
			chain: self.chain.clone(),
			dispatch: self.dispatch,
			interrupted: self.interrupted,
			label: Some(label),
			colour: Some(colour),
			dry_run: self.dry_run,
			trace: Vec::new(),
		}
	}
}

/// How wide a sibling set pads its labels, so the `|` gutters stand in a column
/// rather than wandering with the length of each branch's name.
///
/// The widest label in the set, which is known before any branch starts:
/// `parallel do` works its labels out from the statements and `parallel for`
/// from the list, both up front, so nothing has to be guessed at and no set is
/// padded wider than it needs. Capped, because one long name must not widen
/// every other line -- a 40-column path out of a `glob` beside three 4-column
/// ones would leave every one of them trailing thirty-odd spaces. Past the cap
/// a label sticks out on its own lines instead, which is the trade the deleted
/// `parallel_output` module made too.
fn pad_width(labels: &[String]) -> usize {
	const PAD_MAX: usize = 24;
	labels.iter().map(|l| term::width_of(l)).max().unwrap_or(0).min(PAD_MAX)
}

/// A branch's own segment of the label: painted, then padded out to `width`.
///
/// Padded *after* painting, so the colour wraps the name and the spaces are
/// plain -- and measured with `term::width_of` on the unpainted label, since an
/// escape code occupies no column and a wide character occupies two. A label
/// already at or past the width is left as it is.
fn segment(label: &str, colour: usize, width: usize) -> String {
	if label.is_empty() {
		// A `parallel` block that is itself a branch adds no segment, and
		// padding one into existence would give every line of it a column of
		// blank gutter in front of the branch names that do say something.
		return String::new();
	}
	// Strip control characters before painting, so a label built from a glob
	// path, captured output or an ARG/ENV value cannot carry an escape sequence
	// into the terminal; the runner's own colour is added after (audit SA-027).
	let label = term::sanitize(label);
	let mut s = exec::paint_branch(&label, colour, exec::paints(false));
	let w = term::width_of(&label);
	if w < width {
		s.push_str(&" ".repeat(width - w));
	}
	s
}

/// Run every branch at once and wait for the last of them.
///
/// Every branch runs to completion before a failure surfaces: stopping the
/// others would leave a half-started set of services behind, and one failing
/// says nothing about whether the rest should. A stop -- Ctrl+C, `exit()` -- is
/// reported ahead of any failure, so an interrupted run says it was
/// interrupted rather than naming whichever command the signal killed first;
/// among equals, the first in source order wins. Under `--dry-run` the
/// branches run in turn instead, so a preview reads the same way every time.
fn run_branches<'a, F>(branches: Vec<Runner<'a>>, run: F, r: &mut Runner<'a>) -> Result<(), RunError>
where
	F: Fn(usize, &mut Runner<'a>) -> Result<(), RunError> + Sync,
{
	let finish = |i: usize, mut b: Runner<'a>| {
		let res = run(i, &mut b);
		(b.trace, res)
	};
	let results: Vec<(Vec<String>, Result<(), RunError>)> = if r.dry_run {
		branches.into_iter().enumerate().map(|(i, b)| finish(i, b)).collect()
	} else {
		std::thread::scope(|s| {
			let finish = &finish;
			let handles: Vec<_> = branches
				.into_iter()
				.enumerate()
				.map(|(i, b)| s.spawn(move || finish(i, b)))
				.collect();
			handles
				.into_iter()
				.map(|h| h.join().expect("a parallel branch panicked"))
				.collect()
		})
	};
	let (mut stop, mut failure) = (None, None);
	for (trace, res) in results {
		// In source order, however the branches finished.
		r.trace.extend(trace);
		match res {
			Err(e) if e.is_stop() => {
				stop.get_or_insert(e);
			}
			Err(e) => {
				failure.get_or_insert(e);
			}
			Ok(()) => {}
		}
	}
	match stop.or(failure) {
		Some(e) => Err(e),
		None => Ok(()),
	}
}

/// `parallel do`: every statement directly inside it is a branch.
///
/// Each branch starts from a copy of the state the block opened in, and what
/// it changes is its own -- which is why the parser refuses a change that is a
/// whole branch: it would reach nothing. `ignore_errors()` in effect around the
/// block forgives each branch, the way it forgives each statement of a block
/// walked in order.
fn parallel_do(body: &Block, props: &Props, r: &mut Runner<'_>) -> Result<(), RunError> {
	let labels = branch_labels(&body.statements);
	let width = pad_width(&labels);
	let forks: Vec<Runner<'_>> = labels
		.iter()
		.enumerate()
		.map(|(nth, label)| r.fork(label, nth, width))
		.collect();
	let forgive = r.scope.ignore_errors;
	run_branches(
		forks,
		|i, b| {
			statement(&body.statements[i], props, b).or_else(|e| if forgive && !e.is_stop() { Ok(()) } else { Err(e) })
		},
		r,
	)
}

/// `parallel for`: every iteration is a branch, and its body runs in order
/// within it, from a copy of the state the loop opened in. The list is worked
/// out once, before any iteration starts, as a sequential `for`'s is.
fn parallel_for(
	names: &[String],
	iter: &Expr,
	body: &Block,
	props: &Props,
	r: &mut Runner<'_>,
	line: usize,
) -> Result<(), RunError> {
	let items = match value_of(iter, props, r)? {
		Value::List(v) => v,
		other => {
			return Err(RunError::ForNeedsList {
				actual: other.type_name(),
				line,
			});
		}
	};
	let mut forks = Vec::with_capacity(items.len());
	let labels: Vec<String> = items.iter().map(item_label).collect();
	let width = pad_width(&labels);
	for (nth, (item, label)) in items.into_iter().zip(labels).enumerate() {
		let mut fork = r.fork(&label, nth, width);
		bind_item(names, item, line, &mut fork)?;
		forks.push(fork);
	}
	run_branches(
		forks,
		|_, b| match walk(body, props, b) {
			// `continue` ends this iteration, and this iteration is the branch.
			Err(RunError::Continue { .. }) => Ok(()),
			other => other,
		},
		r,
	)
}

/// What each branch of a `parallel do` prefixes its output with: what it
/// runs, in as few words as tell it from its siblings -- `cargo test` and
/// `cargo clippy`, not `cargo` twice. Where no number of words does, a label
/// is its first word and its line, `print:3` beside `print:5`, which is also
/// what a block is: `if:12`, since nothing shorter tells two `if`s apart. A
/// nested `parallel` block is the one with no label: its own branches are
/// what get named.
fn branch_labels(statements: &[Statement]) -> Vec<String> {
	let words: Vec<Vec<String>> = statements.iter().map(label_words).collect();
	let mut labels: Vec<String> = words
		.iter()
		.enumerate()
		.map(|(i, own)| {
			let apart = |k: &usize| {
				words
					.iter()
					.enumerate()
					.all(|(j, other)| j == i || other.get(..*k) != own.get(..*k))
			};
			let k = (1..=own.len()).find(apart).unwrap_or(own.len().min(1));
			crate::term::fit(&own[..k].join(" "), 40).into_owned()
		})
		.collect();
	let shared: Vec<bool> = (0..labels.len())
		.map(|i| !labels[i].is_empty() && (0..labels.len()).any(|j| j != i && labels[j] == labels[i]))
		.collect();
	for ((label, st), shared) in labels.iter_mut().zip(statements).zip(shared) {
		if shared {
			*label = format!("{label}:{}", st.span().line);
		}
	}
	labels
}

/// The words a branch's label is cut from: a dispatch's target and arguments,
/// a command's words, a call's function. Read off the tree rather than
/// rendered -- rendering runs what is interpolated, and the branch renders it
/// again when it runs, so a `{{ temp_file(…) }}` would make two files -- which
/// is why they stop at the first interpolation.
fn label_words(st: &Statement) -> Vec<String> {
	let keyword = match st {
		Statement::Run { target, args, .. } => {
			return match whole(target) {
				Some(t) => std::iter::once(t).chain(args.iter().map_while(|a| whole(a))).collect(),
				None => vec!["run".to_string()],
			};
		}
		Statement::Exec { command, body, .. } => return command_words(command.as_deref(), body),
		Statement::Call {
			expr: Expr::Call { name, .. },
			..
		} => return vec![name.clone()],
		Statement::Do { parallel: true, .. } | Statement::For { parallel: true, .. } => return Vec::new(),
		Statement::Call { .. } => "call",
		Statement::If { .. } => "if",
		Statement::Do { .. } => "do",
		Statement::For { .. } => "for",
		Statement::Loop { test, .. } => match test {
			LoopTest::While(_) => "while",
			LoopTest::Until(_) => "until",
			LoopTest::Forever => "loop",
		},
		Statement::Match { .. } => "match",
		Statement::Retry { .. } => "retry",
		// The parser keeps these out of a parallel block's own statements.
		Statement::Let { .. }
		| Statement::Assign { .. }
		| Statement::SetEnv { .. }
		| Statement::Break { .. }
		| Statement::Continue { .. } => "branch",
	};
	vec![format!("{keyword}:{}", st.span().line)]
}

/// The text of `parts`, or `None` when an interpolation is part of it.
fn whole(parts: &[InterpPart]) -> Option<String> {
	parts
		.iter()
		.map(|p| match p {
			InterpPart::Literal(t) => Some(t.as_str()),
			InterpPart::Expr(_) => None,
		})
		.collect()
}

/// The words of `parts` up to its first interpolation, and whether it had
/// one. A word the interpolation runs into is only part of a word, so it is
/// left out.
fn literal_words(parts: &[InterpPart]) -> (Vec<String>, bool) {
	let mut text = String::new();
	let mut cut = false;
	for p in parts {
		match p {
			InterpPart::Literal(t) => text.push_str(t),
			InterpPart::Expr(_) => {
				cut = true;
				break;
			}
		}
	}
	let mut words: Vec<String> = text.split_whitespace().map(str::to_string).collect();
	if cut && !text.ends_with(char::is_whitespace) {
		words.pop();
	}
	(words, cut)
}

/// The words of the command an `exec` or `$` branch runs.
///
/// From the `exec` header when there is one, since `exec python3` names itself,
/// and otherwise the body's first command, over its continuation lines -- which
/// for a `$` line is the command being run. The default shell is never the
/// label: every `$` branch would be called `bash`.
fn command_words(command: Option<&[InterpPart]>, body: &[Vec<InterpPart>]) -> Vec<String> {
	const SHELLS: &[&str] = &["sh", "bash", "dash", "ash", "zsh", "ksh", "busybox", "brush"];
	if let Some((header, _)) = command.map(literal_words)
		&& header.first().is_some_and(|w| !SHELLS.contains(&w.as_str()))
	{
		return header;
	}
	let mut words = Vec::new();
	let skipped = |line: &&Vec<InterpPart>| {
		let (w, cut) = literal_words(line);
		(w.is_empty() && !cut) || w.first().is_some_and(|w| w.starts_with('#'))
	};
	// A continuation may arrive as its own line or folded into this one; the
	// backslash is not a word either way.
	for line in body.iter().skip_while(skipped) {
		let (w, cut) = literal_words(line);
		let continued = !cut && w.last().is_some_and(|w| w == "\\");
		words.extend(w.into_iter().filter(|w| w != "\\"));
		if !continued {
			break;
		}
	}
	if words.is_empty() {
		vec!["exec".to_string()]
	} else {
		words
	}
}

/// The labels `parallel do` gives the branches of `src`, which opens with one.
#[cfg(test)]
pub(crate) fn labels_for_test(src: &str) -> Vec<String> {
	match &runfile_lang::parse(src).expect("parses").body.statements[0] {
		Statement::Do { body, .. } => branch_labels(&body.statements),
		other => panic!("not a `parallel do`: {other:?}"),
	}
}

/// An iteration's label: its value, on one line and short enough to leave the
/// output room -- the path out of a `glob` is exactly what tells two apart,
/// where the first word of the command, `docker`, told none of them apart.
fn item_label(item: &Value) -> String {
	let one_line = item.to_string().split_whitespace().collect::<Vec<_>>().join(" ");
	crate::term::fit(&one_line, 40).into_owned()
}
