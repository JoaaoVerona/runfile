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
		Expr::Ident(name, _) => sc.vars.get(name).cloned().ok_or(EvalError::Unbound {
			name: name.clone(),
			line,
		}),
		Expr::Source { kind, key, .. } => source(*kind, key.as_deref(), sc, line),
		Expr::Unary { op, rhs, .. } => {
			let v = eval(rhs, sc)?;
			match op {
				UnaryOp::Not => Ok(Value::Bool(!v.as_bool().map_err(|e| EvalError::ty(line, e))?)),
				UnaryOp::Neg => Ok(Value::Num(-v.as_num().map_err(|e| EvalError::ty(line, e))?)),
			}
		}
		Expr::Binary { op, lhs, rhs, .. } => binary(*op, lhs, rhs, sc, line),
		Expr::Chain { lhs, rhs, .. } => match eval(lhs, sc) {
			Ok(v) => Ok(v),
			Err(_) => eval(rhs, sc),
		},
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
			sc.args
				.get(k)
				.cloned()
				.map(Value::Str)
				.ok_or_else(|| EvalError::MissingArg {
					name: k.to_string(),
					line,
				})
		}
		SourceKind::Env => {
			let k = key.unwrap_or_default();
			sc.env
				.get(k)
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
