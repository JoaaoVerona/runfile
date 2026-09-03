//! The standard library, with typed signatures.
//!
//! Ported from the old `{{ }}` function registry. Three families changed shape:
//! the arithmetic ones became operators, `glob`/`split`/`lines`/`regex_capture_all`
//! now return lists rather than joined strings, and index arguments are numbers
//! rather than numeric strings.

use crate::ast::Expr;
use crate::eval::{EvalError, Scope, eval};
use crate::span::Span;
use crate::value::{TypeError, Value, parse_number};

fn ty(sp: Span, e: TypeError) -> EvalError {
	EvalError::ty(sp.line, e)
}

fn arity(name: &str, expected: &str, got: usize, sp: Span) -> EvalError {
	EvalError::Arity {
		name: name.into(),
		expected: expected.into(),
		got,
		line: sp.line,
	}
}

pub fn call(name: &str, args: &[Expr], sc: &mut Scope, sp: Span) -> Result<Value, EvalError> {
	// `try` is special: it must catch failures from its argument, so the
	// argument cannot be evaluated by the bulk pass below.
	if name == "try" {
		if args.len() != 1 {
			return Err(arity("try", "1 argument", args.len(), sp));
		}
		let was = std::mem::replace(&mut sc.in_try, true);
		let out = eval(&args[0], sc);
		sc.in_try = was;
		return out.map_err(|_| EvalError::Caught { line: sp.line });
	}

	let v: Vec<Value> = args.iter().map(|a| eval(a, sc)).collect::<Result<_, _>>()?;
	let n = v.len();
	let s = |i: usize| v[i].as_str().map_err(|e| ty(sp, e));
	let num = |i: usize| v[i].as_num().map_err(|e| ty(sp, e));
	let list = |i: usize| v[i].as_list().map_err(|e| ty(sp, e));

	macro_rules! want {
		($k:literal, $want:literal) => {
			if n != $k {
				return Err(arity(name, $want, n, sp));
			}
		};
	}

	Ok(match name {
		// ---- strings
		"to_upper" => {
			want!(1, "1 argument");
			Value::Str(s(0)?.to_uppercase())
		}
		"to_lower" => {
			want!(1, "1 argument");
			Value::Str(s(0)?.to_lowercase())
		}
		"trim" => {
			want!(1, "1 argument");
			Value::Str(s(0)?.trim().to_string())
		}
		"trim_start" => {
			want!(1, "1 argument");
			Value::Str(s(0)?.trim_start().to_string())
		}
		"trim_end" => {
			want!(1, "1 argument");
			Value::Str(s(0)?.trim_end().to_string())
		}
		"length" => {
			want!(1, "1 argument");
			match &v[0] {
				Value::List(l) => Value::Num(l.len() as f64),
				Value::Str(t) => Value::Num(t.chars().count() as f64),
				other => {
					return Err(ty(
						sp,
						TypeError::Expected {
							expected: "string or list",
							actual: other.type_name(),
						},
					));
				}
			}
		}
		"starts_with" => {
			want!(2, "2 arguments");
			Value::Bool(s(0)?.starts_with(s(1)?))
		}
		"ends_with" => {
			want!(2, "2 arguments");
			Value::Bool(s(0)?.ends_with(s(1)?))
		}
		"contains" => {
			want!(2, "2 arguments");
			match &v[0] {
				Value::List(l) => Value::Bool(l.contains(&v[1])),
				_ => Value::Bool(s(0)?.contains(s(1)?)),
			}
		}
		"replace_all" => {
			want!(3, "3 arguments");
			Value::Str(s(0)?.replace(s(1)?, s(2)?))
		}
		"remove_all" => {
			want!(2, "2 arguments");
			Value::Str(s(0)?.replace(s(1)?, ""))
		}
		"remove_suffix" => {
			want!(2, "2 arguments");
			let (a, b) = (s(0)?, s(1)?);
			Value::Str(a.strip_suffix(b).unwrap_or(a).to_string())
		}
		"remove_prefix" => {
			want!(2, "2 arguments");
			let (a, b) = (s(0)?, s(1)?);
			Value::Str(a.strip_prefix(b).unwrap_or(a).to_string())
		}
		"concat" => {
			if n == 0 {
				return Err(arity(name, "at least 1 argument", 0, sp));
			}
			Value::Str(v.iter().map(Value::to_string).collect())
		}
		"join" => {
			if n < 1 {
				return Err(arity(name, "a separator and a list", n, sp));
			}
			let sep = s(0)?;
			let items: Vec<String> = if n == 2 && matches!(v[1], Value::List(_)) {
				list(1)?.iter().map(Value::to_string).collect()
			} else {
				v[1..].iter().map(Value::to_string).collect()
			};
			Value::Str(items.join(sep))
		}
		// ---- lists
		"split" => {
			want!(2, "2 arguments");
			Value::List(s(0)?.split(s(1)?).map(|p| Value::Str(p.to_string())).collect())
		}
		"lines" => {
			want!(1, "1 argument");
			Value::List(
				s(0)?
					.lines()
					.filter(|l| !l.trim().is_empty())
					.map(|l| Value::Str(l.to_string()))
					.collect(),
			)
		}
		"first" => {
			want!(1, "1 argument");
			list(0)?.first().cloned().unwrap_or(Value::Str(String::new()))
		}
		"last" => {
			want!(1, "1 argument");
			list(0)?.last().cloned().unwrap_or(Value::Str(String::new()))
		}
		// ---- numbers
		"number" => {
			want!(1, "1 argument");
			Value::Num(parse_number(s(0)?).map_err(|e| ty(sp, e))?)
		}
		"is_number" => {
			want!(1, "1 argument");
			Value::Bool(matches!(&v[0], Value::Num(_)) || parse_number(s(0).unwrap_or("")).is_ok())
		}
		"abs" => {
			want!(1, "1 argument");
			Value::Num(num(0)?.abs())
		}
		"round" => {
			want!(1, "1 argument");
			Value::Num(num(0)?.round())
		}
		"floor" => {
			want!(1, "1 argument");
			Value::Num(num(0)?.floor())
		}
		"ceil" => {
			want!(1, "1 argument");
			Value::Num(num(0)?.ceil())
		}
		"min" | "max" => {
			if n == 0 {
				return Err(arity(name, "at least 1 argument", 0, sp));
			}
			let mut acc = num(0)?;
			for i in 1..n {
				let x = num(i)?;
				acc = if name == "min" { acc.min(x) } else { acc.max(x) };
			}
			Value::Num(acc)
		}
		// ---- paths
		"basename" => {
			want!(1, "1 argument");
			Value::Str(path_part(s(0)?, PathPart::Base))
		}
		"dirname" => {
			want!(1, "1 argument");
			Value::Str(path_part(s(0)?, PathPart::Dir))
		}
		"extname" => {
			want!(1, "1 argument");
			Value::Str(path_part(s(0)?, PathPart::Ext))
		}
		"stem" => {
			want!(1, "1 argument");
			Value::Str(path_part(s(0)?, PathPart::Stem))
		}
		// ---- validation
		"one_of" => {
			if n < 2 {
				return Err(arity(name, "a value and at least one option", n, sp));
			}
			let subject = &v[0];
			let options: Vec<&Value> = if n == 2 && matches!(v[1], Value::List(_)) {
				list(1)?.iter().collect()
			} else {
				v[1..].iter().collect()
			};
			if options.contains(&subject) {
				subject.clone()
			} else {
				let opts: Vec<String> = options.iter().map(|o| o.to_string()).collect();
				return Err(EvalError::Other {
					msg: format!("`{subject}` is not one of: {}", opts.join(", ")),
					line: sp.line,
				});
			}
		}
		"error" => {
			want!(1, "1 argument");
			return Err(EvalError::Other {
				msg: s(0)?.to_string(),
				line: sp.line,
			});
		}
		_ => {
			return Err(EvalError::UnknownFunction {
				name: name.into(),
				line: sp.line,
			});
		}
	})
}

enum PathPart {
	Base,
	Dir,
	Ext,
	Stem,
}

fn path_part(p: &str, which: PathPart) -> String {
	let path = std::path::Path::new(p);
	let pick = match which {
		PathPart::Base => path.file_name(),
		PathPart::Dir => {
			return path
				.parent()
				.map(|d| d.to_string_lossy().into_owned())
				.unwrap_or_default();
		}
		PathPart::Ext => path.extension(),
		PathPart::Stem => path.file_stem(),
	};
	pick.map(|x| x.to_string_lossy().into_owned()).unwrap_or_default()
}
