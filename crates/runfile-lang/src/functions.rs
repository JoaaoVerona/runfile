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
			// A number is already one; insisting on a string here would make
			// `number(length(x))` an error for no gain.
			match &v[0] {
				Value::Num(n) => Value::Num(*n),
				_ => Value::Num(parse_number(s(0)?).map_err(|e| ty(sp, e))?),
			}
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
				// Naming the type matters here: `one_of(ARGS, "patch")` compares a
				// one-element list against a string, and both print as `patch`, so
				// without it the message reads as a contradiction.
				let mismatched = options.iter().any(|o| o.type_name() != subject.type_name());
				let shown = if mismatched {
					format!("{subject} ({})", subject.type_name())
				} else {
					subject.to_string()
				};
				let opts: Vec<String> = options.iter().map(|o| o.to_string()).collect();
				return Err(EvalError::Other {
					msg: format!("`{shown}` is not one of: {}", opts.join(", ")),
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
		// Filesystem and regex functions live apart because they need the scope's
		// anchor and key pool; everything else above is pure.
		_ => match call_io(name, &v, sc, sp) {
			Some(r) => return r,
			None => {
				return Err(EvalError::UnknownFunction {
					name: name.into(),
					line: sp.line,
				});
			}
		},
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

// ------------------------------------------------------- filesystem and regex

use crate::value::Value as V;
use std::path::{Path, PathBuf};

/// Relative paths anchor to the target's directory, the same rule cwd and
/// `.env-file` follow, so one anchor explains all of them.
fn resolve(base: &Path, p: &str) -> PathBuf {
	let path = Path::new(p);
	if path.is_absolute() {
		path.to_path_buf()
	} else {
		base.join(path)
	}
}

/// Every function name the language knows.
///
/// Exported so tooling (completion, the language server) offers exactly what
/// exists, and cannot drift from what `call` accepts -- a test below walks this
/// list and rejects any name the dispatcher does not recognise.
pub const FUNCTIONS: &[&str] = &[
	"abs",
	"base64_decode",
	"base64_encode",
	"basename",
	"ceil",
	"concat",
	"contains",
	"decrypt",
	"dirname",
	"ends_with",
	"error",
	"extname",
	"file_exists",
	"first",
	"floor",
	"glob",
	"is_number",
	"join",
	"last",
	"length",
	"lines",
	"max",
	"number",
	"one_of",
	"read_file",
	"regex_capture",
	"regex_capture_all",
	"regex_matches",
	"regex_remove",
	"regex_replace",
	"remove_all",
	"remove_prefix",
	"remove_suffix",
	"replace_all",
	"round",
	"split",
	"starts_with",
	"stem",
	"to_lower",
	"to_upper",
	"trim",
	"trim_end",
	"trim_start",
	"write_file",
];

pub(crate) fn call_io(name: &str, v: &[Value], sc: &Scope, sp: Span) -> Option<Result<Value, EvalError>> {
	let s = |i: usize| -> Result<&str, EvalError> { v[i].as_str().map_err(|e| ty(sp, e)) };
	let other = |m: String| EvalError::Other { msg: m, line: sp.line };
	let n = v.len();

	Some(match name {
		"glob" if n == 1 => (|| {
			let pat = s(0)?;
			// `*` does not cross a directory boundary and `**` does, which is
			// what people expect from a shell glob -- globset defaults the
			// other way.
			let g = globset::GlobBuilder::new(pat)
				.literal_separator(true)
				.build()
				.map_err(|e| other(format!("bad glob `{pat}`: {e}")))?
				.compile_matcher();
			let mut hits = Vec::new();
			walk(&sc.base_dir, &sc.base_dir, &g, &mut hits);
			hits.sort();
			Ok(V::List(hits.into_iter().map(V::Str).collect()))
		})(),
		"read_file" if n == 1 => (|| {
			let p = resolve(&sc.base_dir, s(0)?);
			std::fs::read_to_string(&p)
				.map(V::Str)
				.map_err(|e| other(format!("could not read {}: {e}", p.display())))
		})(),
		// Writing functions are the one place a preview could change the world,
		// so they report the path they would have touched and do nothing.
		"write_file" if n == 2 && sc.dry_run => (|| Ok(Value::Str(format!("<would write {}>", s(0)?))))(),
		"decrypt" if n == 2 && sc.dry_run => (|| Ok(Value::Str(format!("<would decrypt to {}>", s(1)?))))(),
		"write_file" if n == 2 => (|| {
			let p = resolve(&sc.base_dir, s(0)?);
			if let Some(d) = p.parent() {
				let _ = std::fs::create_dir_all(d);
			}
			std::fs::write(&p, s(1)?)
				.map(|()| V::Str(String::new()))
				.map_err(|e| other(format!("could not write {}: {e}", p.display())))
		})(),
		"file_exists" if n == 1 => s(0).map(|p| V::Bool(resolve(&sc.base_dir, p).exists())),
		"base64_encode" if n == 1 => s(0).map(|x| {
			use base64::Engine;
			V::Str(base64::engine::general_purpose::STANDARD.encode(x))
		}),
		"base64_decode" if n == 1 => (|| {
			use base64::Engine;
			let raw = base64::engine::general_purpose::STANDARD
				.decode(s(0)?)
				.map_err(|e| other(format!("invalid base64: {e}")))?;
			String::from_utf8(raw)
				.map(V::Str)
				.map_err(|_| other("decoded bytes are not UTF-8".into()))
		})(),
		"regex_matches" if n == 2 => (|| {
			let re = compile(s(1)?, sp)?;
			Ok(V::Bool(re.is_match(s(0)?)))
		})(),
		"regex_replace" if n == 3 => (|| {
			let re = compile(s(1)?, sp)?;
			Ok(V::Str(re.replace_all(s(0)?, s(2)?).into_owned()))
		})(),
		"regex_remove" if n == 2 => (|| {
			let re = compile(s(1)?, sp)?;
			Ok(V::Str(re.replace_all(s(0)?, "").into_owned()))
		})(),
		"regex_capture" if n == 3 => (|| {
			let re = compile(s(1)?, sp)?;
			let idx = v[2].as_index().map_err(|e| ty(sp, e))?;
			Ok(V::Str(
				re.captures(s(0)?)
					.and_then(|c| c.get(idx))
					.map(|m| m.as_str().to_string())
					.unwrap_or_default(),
			))
		})(),
		// Returns a list, which is what `for image in regex_capture_all(...)`
		// iterates -- the old form joined with a separator and needed splitting.
		"regex_capture_all" if n == 3 => (|| {
			let re = compile(s(1)?, sp)?;
			let idx = v[2].as_index().map_err(|e| ty(sp, e))?;
			Ok(V::List(
				re.captures_iter(s(0)?)
					.map(|c| V::Str(c.get(idx).map(|m| m.as_str().to_string()).unwrap_or_default()))
					.collect(),
			))
		})(),
		"decrypt" if n == 2 => (|| {
			let src = resolve(&sc.base_dir, s(0)?);
			let dst = resolve(&sc.base_dir, s(1)?);
			decrypt_file(&src, &dst, sc.private_keys.get())
				.map(|()| V::Str(String::new()))
				.map_err(other)
		})(),
		_ => return None,
	})
}

fn compile(pattern: &str, sp: Span) -> Result<regex::Regex, EvalError> {
	regex::Regex::new(pattern).map_err(|e| EvalError::Other {
		msg: format!("bad regex `{pattern}`: {e}"),
		line: sp.line,
	})
}

fn walk(root: &Path, dir: &Path, g: &globset::GlobMatcher, out: &mut Vec<String>) {
	let Ok(rd) = std::fs::read_dir(dir) else { return };
	for e in rd.flatten() {
		let p = e.path();
		let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
		if p.is_dir() {
			if name == "node_modules" || name == ".git" || name == "target" {
				continue;
			}
			walk(root, &p, g, out);
			continue;
		}
		if let Ok(rel) = p.strip_prefix(root) {
			// Matching is on forward-slash relative paths, so a pattern reads
			// the same on every platform.
			let s = rel.to_string_lossy().replace('\\', "/");
			if g.is_match(&s) {
				out.push(s);
			}
		}
	}
}

/// Rewrite an encrypted env file as a plain one, preserving comments and
/// already-plain lines verbatim.
fn decrypt_file(src: &Path, dst: &Path, keys: &[String]) -> Result<(), String> {
	let text = std::fs::read_to_string(src).map_err(|e| format!("could not read {}: {e}", src.display()))?;
	let mut out = String::with_capacity(text.len());
	for line in text.lines() {
		match line.split_once('=') {
			Some((k, val)) if runfile_crypto::is_encrypted(val.trim()) => {
				let plain = keys
					.iter()
					.find_map(|key| runfile_crypto::decrypt(val.trim(), key).ok())
					.ok_or_else(|| format!("no key can decrypt {k}"))?;
				out.push_str(k);
				out.push('=');
				out.push_str(&plain);
			}
			_ if line.starts_with("RUNFILE_ENCRYPTION_PUBLIC_KEY=") => continue,
			_ => out.push_str(line),
		}
		out.push('\n');
	}
	std::fs::write(dst, out).map_err(|e| format!("could not write {}: {e}", dst.display()))
}
