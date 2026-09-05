//! Expression evaluation: scopes, sources and operators.
//!
//! Strictness is the point. Nothing coerces, so a string from `ARG.x` must go
//! through `number(…)` before arithmetic, `"a" + 1` is an error rather than a
//! silent join, and `"1" == 1` is false.

use crate::ast::*;
use crate::functions;
use crate::value::{TypeError, Value};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum EvalError {
	#[error("line {line}: {source}")]
	Type { line: usize, source: TypeError },
	#[error("line {line}: `{name}` is not defined")]
	Unbound { name: String, line: usize },
	#[error("line {line}: no argument `--{name}` was given")]
	MissingArg { name: String, line: usize },
	#[error(
		"line {line}: no argument `--{name}` was given; \
		 it was passed as a flag -- arguments take a value, as `--{name}=<value>`"
	)]
	MissingArgSawFlag { name: String, line: usize },
	#[error("line {line}: `{name}` is not set in the environment")]
	MissingEnv { name: String, line: usize },
	#[error("line {line}: unknown `RUN.{0}`", .name)]
	UnknownRun { name: String, line: usize },
	#[error("line {line}: unknown function `{name}`")]
	UnknownFunction { name: String, line: usize },
	#[error("line {line}: `{name}` takes {expected}, got {got}")]
	Arity {
		name: String,
		expected: String,
		got: usize,
		line: usize,
	},
	#[error("line {line}: {msg}")]
	Other { msg: String, line: usize },
	/// `try(…)` caught a failure. Carried rather than swallowed so a chain can
	/// still fall through: `try(x) ? "fallback"` must reach the fallback, while
	/// a bare `try(x)` resolves to an empty string at the boundary.
	#[error("line {line}: caught failure")]
	Caught { line: usize },
	/// `exit(code)`. Not a failure -- an instruction to stop with that status.
	/// It travels as an error because that is the only path out of an
	/// expression, and every catcher along the way lets it through: `try`, a
	/// `?` chain and `.ignore-errors` all re-raise it, the way an interrupt is
	/// not something a target gets to shrug off.
	#[error("exit {code}")]
	Exit { code: i32, line: usize },
}

impl EvalError {
	pub fn ty(line: usize, e: TypeError) -> Self {
		EvalError::Type { line, source: e }
	}
}

/// Everything an expression can read. Sources are fixed for a target; `vars`
/// changes as `let` bindings and loop variables come and go.
pub struct Scope {
	pub vars: HashMap<String, Value>,
	pub args: HashMap<String, String>,
	pub positional: Vec<String>,
	pub flags: Vec<String>,
	pub env: HashMap<String, String>,
	pub run: HashMap<String, Value>,
	/// Set while a `try(…)` is being evaluated so a failure can be caught.
	pub(crate) in_try: bool,
	/// Asked for an input the target needs but was not given, under
	/// `--stdin-args`. Only genuinely-missing values reach it: a chain that
	/// finds a default resolves without ever asking, because the chain catches
	/// the error before it surfaces here.
	pub ask: Option<fn(&str, &str) -> Option<String>>,
	/// The anchor: relative paths in `glob`, `read_file` and friends resolve
	/// against it, the same rule cwd and `.env-file` follow.
	pub base_dir: std::path::PathBuf,
	/// Private keys `decrypt` may try. Supplied by the host so the language
	/// crate never has to know about credential stores.
	pub private_keys: Keys,
	/// When set, functions that write must not. Reads still happen, since a
	/// preview that cannot read a file cannot say what would run.
	pub dry_run: bool,
	/// What `temp_file` and `temp_dir` created, for the caller to delete when
	/// the run ends.
	pub temps: TempFiles,
}

/// Paths created by `temp_file` / `temp_dir`, shared by every scope in a run.
///
/// A handle rather than a process-global: a run owns its temp files, and
/// `.parallel` hands the same handle to each branch. The host drains it on the
/// way out, whether the run succeeded, failed, or is a watch iteration about to
/// start another.
#[derive(Clone, Default)]
pub struct TempFiles(std::sync::Arc<std::sync::Mutex<Vec<std::path::PathBuf>>>);

impl TempFiles {
	pub fn track(&self, path: std::path::PathBuf) {
		if let Ok(mut g) = self.0.lock() {
			g.push(path);
		}
	}

	/// Hand back everything created so far and forget it.
	pub fn take(&self) -> Vec<std::path::PathBuf> {
		self.0.lock().map(|mut g| std::mem::take(&mut *g)).unwrap_or_default()
	}
}

impl std::fmt::Debug for TempFiles {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		let n = self.0.lock().map(|g| g.len()).unwrap_or(0);
		f.debug_struct("TempFiles").field("tracked", &n).finish()
	}
}

/// A deferred, memoized key pool.
///
/// Loading is deferred because the pool comes from an OS credential store: a
/// locked keyring blocks on an unlock prompt, so a target that decrypts nothing
/// must never ask for it. Memoized because a run that decrypts twice should
/// still prompt at most once.
#[derive(Clone)]
pub struct Keys {
	loader: fn() -> Vec<String>,
	cache: std::sync::Arc<std::sync::OnceLock<Vec<String>>>,
}

impl Keys {
	pub fn new(loader: fn() -> Vec<String>) -> Self {
		Self {
			loader,
			cache: std::sync::Arc::new(std::sync::OnceLock::new()),
		}
	}

	/// Load on first call, then hand back the same pool forever.
	pub fn get(&self) -> &[String] {
		self.cache.get_or_init(self.loader)
	}
}

impl Default for Keys {
	fn default() -> Self {
		Self::new(Vec::new)
	}
}

impl std::fmt::Debug for Keys {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		// Never print the pool itself.
		f.debug_struct("Keys")
			.field("loaded", &self.cache.get().is_some())
			.finish()
	}
}

impl Scope {
	pub fn new() -> Self {
		Self {
			vars: HashMap::new(),
			args: HashMap::new(),
			positional: Vec::new(),
			flags: Vec::new(),
			env: HashMap::new(),
			run: HashMap::new(),
			in_try: false,
			ask: None,
			base_dir: std::path::PathBuf::from("."),
			private_keys: Keys::default(),
			dry_run: false,
			temps: TempFiles::default(),
		}
	}

	/// Bind a name, returning the previous value so a block can restore it on
	/// exit. Loop variables and block-scoped `let`s both rely on this.
	pub fn bind(&mut self, name: &str, value: Value) -> Option<Value> {
		self.vars.insert(name.to_string(), value)
	}

	pub fn restore(&mut self, name: &str, prior: Option<Value>) {
		match prior {
			Some(v) => {
				self.vars.insert(name.to_string(), v);
			}
			None => {
				self.vars.remove(name);
			}
		}
	}
}

impl Default for Scope {
	fn default() -> Self {
		Self::new()
	}
}

pub fn eval(e: &Expr, sc: &mut Scope) -> Result<Value, EvalError> {
	let line = e.span().line;
	match e {
		Expr::Number(n, _) => Ok(Value::Num(*n)),
		Expr::Bool(b, _) => Ok(Value::Bool(*b)),
		Expr::Str(parts, _) => Ok(Value::Str(interpolate_plain(parts, sc)?)),
		Expr::List(items, _) => items
			.iter()
			.map(|i| eval(i, sc))
			.collect::<Result<Vec<_>, _>>()
			.map(Value::List),
		Expr::Ident(name, _) => sc.vars.get(name).cloned().ok_or_else(|| {
			// A function named without parentheses is a call someone forgot to
			// finish, not an unknown binding. Saying so is the difference
			// between a one-word fix and a hunt. Checked here rather than at
			// parse time so a binding may still be named after a function --
			// a bound name is found above and never reaches this.
			if crate::functions::FUNCTIONS.iter().any(|f| f.name == name) {
				return EvalError::Other {
					msg: format!("`{name}` is a function; call it as `{name}()`"),
					line,
				};
			}
			EvalError::Unbound {
				name: name.clone(),
				line,
			}
		}),
		Expr::Source { kind, key, .. } => match source(*kind, key.as_deref(), sc, line) {
			// A missing input is offered to the prompter before it becomes an
			// error, but never inside a `try` or a chain that has a fallback.
			Err(e @ (EvalError::MissingArg { .. } | EvalError::MissingEnv { .. })) if !sc.in_try => {
				let (kind_name, name) = match &e {
					EvalError::MissingArg { name, .. } => ("argument --", name.clone()),
					EvalError::MissingEnv { name, .. } => ("environment ", name.clone()),
					_ => unreachable!(),
				};
				match sc.ask.and_then(|f| f(kind_name, &name)) {
					Some(v) => {
						let store = matches!(kind, SourceKind::Arg);
						if store {
							sc.args.insert(name, v.clone());
						} else {
							sc.env.insert(name, v.clone());
						}
						Ok(Value::Str(v))
					}
					None => Err(e),
				}
			}
			other => other,
		},
		Expr::Unary { op, rhs, .. } => {
			let v = eval(rhs, sc)?;
			match op {
				UnaryOp::Not => Ok(Value::Bool(!v.as_bool().map_err(|e| EvalError::ty(line, e))?)),
				UnaryOp::Neg => Ok(Value::Num(-v.as_num().map_err(|e| EvalError::ty(line, e))?)),
			}
		}
		Expr::Binary { op, lhs, rhs, .. } => binary(*op, lhs, rhs, sc, line),
		Expr::Chain { lhs, rhs, .. } => {
			// Suppress prompting while trying the left side: a chain that
			// reaches a literal default must resolve without asking anyone.
			let was = std::mem::replace(&mut sc.in_try, true);
			let left = eval(lhs, sc);
			sc.in_try = was;
			match left {
				Ok(v) => Ok(v),
				// `exit` is not a failure to fall back from.
				Err(e @ EvalError::Exit { .. }) => Err(e),
				Err(_) => eval(rhs, sc),
			}
		}
		Expr::Index { base, index, .. } => {
			let b = eval(base, sc)?;
			let i = eval(index, sc)?.as_index().map_err(|e| EvalError::ty(line, e))?;
			let items = b.as_list().map_err(|e| EvalError::ty(line, e))?;
			items.get(i).cloned().ok_or_else(|| {
				EvalError::ty(
					line,
					TypeError::IndexOutOfRange {
						index: i as i64,
						len: items.len(),
					},
				)
			})
		}
		Expr::Call { name, args, span } => functions::call(name, args, sc, *span),
		Expr::Capture { span, .. } => Err(EvalError::Other {
			msg: "`$`/`exec` capture needs a process host; not available in pure evaluation".into(),
			line: span.line,
		}),
	}
}

fn source(kind: SourceKind, key: Option<&str>, sc: &Scope, line: usize) -> Result<Value, EvalError> {
	match kind {
		SourceKind::Args => Ok(Value::List(sc.positional.iter().cloned().map(Value::Str).collect())),
		SourceKind::Arg => {
			let k = key.unwrap_or_default();
			sc.args.get(k).cloned().map(Value::Str).ok_or_else(|| {
				// `--x value` parses as a flag plus a positional, since nothing
				// declares which names take values. It cannot be guessed, but it
				// can be explained at the point it goes wrong.
				if sc.flags.iter().any(|f| f == k) {
					EvalError::MissingArgSawFlag {
						name: k.to_string(),
						line,
					}
				} else {
					EvalError::MissingArg {
						name: k.to_string(),
						line,
					}
				}
			})
		}
		SourceKind::Env => {
			let k = key.unwrap_or_default();
			// Exact first, then ignoring case. Windows environment variables
			// are case-insensitive and POSIX ones are not, so a file written
			// once has to work on both -- `{{ ENV.port }}` against a `PORT=`
			// line is ordinary, not a mistake. An exact hit still wins, so a
			// environment holding both keeps them distinct.
			sc.env
				.get(k)
				.or_else(|| sc.env.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v))
				.cloned()
				.map(Value::Str)
				.ok_or_else(|| EvalError::MissingEnv {
					name: k.to_string(),
					line,
				})
		}
		SourceKind::Flag => Ok(Value::Bool(sc.flags.iter().any(|f| f == key.unwrap_or_default()))),
		SourceKind::Run => {
			let k = key.unwrap_or_default();
			sc.run.get(k).cloned().ok_or_else(|| EvalError::UnknownRun {
				name: k.to_string(),
				line,
			})
		}
	}
}

fn binary(op: BinaryOp, lhs: &Expr, rhs: &Expr, sc: &mut Scope, line: usize) -> Result<Value, EvalError> {
	use BinaryOp::*;
	// Short-circuit before evaluating the right side.
	if matches!(op, And | Or) {
		let l = eval(lhs, sc)?.as_bool().map_err(|e| EvalError::ty(line, e))?;
		if (op == And && !l) || (op == Or && l) {
			return Ok(Value::Bool(l));
		}
		return Ok(Value::Bool(
			eval(rhs, sc)?.as_bool().map_err(|e| EvalError::ty(line, e))?,
		));
	}
	let (l, r) = (eval(lhs, sc)?, eval(rhs, sc)?);
	match op {
		// Equality never coerces: `"1" == 1` is false, not an error.
		Eq => return Ok(Value::Bool(l == r)),
		Ne => return Ok(Value::Bool(l != r)),
		_ => {}
	}
	let name = op_name(op);
	let (a, b) = match (&l, &r) {
		(Value::Num(a), Value::Num(b)) => (*a, *b),
		_ => {
			return Err(EvalError::ty(
				line,
				TypeError::BadOperands {
					op: name,
					lhs: l.type_name(),
					rhs: r.type_name(),
				},
			));
		}
	};
	Ok(match op {
		Add => Value::Num(a + b),
		Sub => Value::Num(a - b),
		Mul => Value::Num(a * b),
		Div | Rem if b == 0.0 => {
			return Err(EvalError::ty(line, TypeError::DivideByZero));
		}
		Div => Value::Num(a / b),
		Rem => Value::Num(a % b),
		Lt => Value::Bool(a < b),
		Le => Value::Bool(a <= b),
		Gt => Value::Bool(a > b),
		Ge => Value::Bool(a >= b),
		Eq | Ne | And | Or => unreachable!("handled above"),
	})
}

fn op_name(op: BinaryOp) -> &'static str {
	use BinaryOp::*;
	match op {
		Add => "+",
		Sub => "-",
		Mul => "*",
		Div => "/",
		Rem => "%",
		Lt => "<",
		Le => "<=",
		Gt => ">",
		Ge => ">=",
		Eq => "==",
		Ne => "!=",
		And => "&&",
		Or => "||",
	}
}

/// Evaluate at a boundary, where a caught `try(…)` failure becomes an empty
/// string rather than an error.
pub fn eval_boundary(e: &Expr, sc: &mut Scope) -> Result<Value, EvalError> {
	match eval(e, sc) {
		Err(EvalError::Caught { .. }) => Ok(Value::Str(String::new())),
		other => other,
	}
}

/// Join interpolation parts for a *string literal*: values render plainly,
/// because a string is data rather than a command line.
pub fn interpolate_plain(parts: &[InterpPart], sc: &mut Scope) -> Result<String, EvalError> {
	let mut out = String::new();
	for p in parts {
		match p {
			InterpPart::Literal(t) => out.push_str(t),
			InterpPart::Expr(e) => out.push_str(&eval_boundary(e, sc)?.to_string()),
		}
	}
	Ok(out)
}

/// Join interpolation parts for *shell text*: values are shell-quoted, and a
/// list becomes several arguments. This is why an interpolation must never be
/// wrapped in shell quotes by hand.
pub fn interpolate_shell(parts: &[InterpPart], sc: &mut Scope) -> Result<String, EvalError> {
	let mut out = String::new();
	for p in parts {
		match p {
			InterpPart::Literal(t) => out.push_str(t),
			InterpPart::Expr(e) => out.push_str(&eval_boundary(e, sc)?.to_shell()),
		}
	}
	Ok(out)
}
