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
		return out.map_err(|e| match e {
			e @ (EvalError::Exit { .. } | EvalError::Cancelled { .. }) => e,
			_ => EvalError::Caught { line: sp.line },
		});
	}

	let v: Vec<Value> = args.iter().map(|a| eval(a, sc)).collect::<Result<_, _>>()?;
	call_with(name, v, sc, sp)
}

/// The same call, with its arguments already evaluated.
///
/// Split out for the runtime: a `$` run as the last argument has to be spawned
/// by something that owns a process host, so the value arrives here rather
/// than the expression.
pub fn call_with(name: &str, v: Vec<Value>, sc: &mut Scope, sp: Span) -> Result<Value, EvalError> {
	let n = v.len();
	let s = |i: usize| v[i].as_str().map_err(|e| ty(sp, e));
	let num = |i: usize| v[i].as_num().map_err(|e| ty(sp, e));
	let list = |i: usize| v[i].as_list().map_err(|e| ty(sp, e));

	// `exit` ends the run rather than producing a value, so it leaves here as
	// an error instead of falling through to the table below.
	if name == "exit" {
		let code = match n {
			0 => 0,
			1 => num(0)? as i32,
			_ => return Err(arity("exit", "0 or 1 arguments", n, sp)),
		};
		return Err(EvalError::Exit { code, line: sp.line });
	}

	macro_rules! want {
		($k:literal, $want:literal) => {
			if n != $k {
				return Err(arity(name, $want, n, sp));
			}
		};
	}

	Ok(match name {
		// ---- strings
		"capitalize" => {
			// The first character of every whitespace-separated word, the rest
			// untouched: `"hello world"` becomes `"Hello World"`.
			want!(1, "1 argument");
			let mut out = String::with_capacity(s(0)?.len());
			let mut at_start = true;
			for c in s(0)?.chars() {
				if c.is_whitespace() {
					at_start = true;
					out.push(c);
				} else if at_start {
					out.extend(c.to_uppercase());
					at_start = false;
				} else {
					out.push(c);
				}
			}
			Value::Str(out)
		}
		"substring" => {
			// Indices count Unicode scalar values, not bytes, so a multi-byte
			// character cannot be cut in half.
			if n != 2 && n != 3 {
				return Err(arity(name, "2 or 3 arguments", n, sp));
			}
			let start = count(num(1)?, name, "start", sp)?;
			let it = s(0)?.chars().skip(start);
			Value::Str(match n {
				3 => it.take(count(num(2)?, name, "length", sp)?).collect(),
				_ => it.collect(),
			})
		}
		"escape" => {
			// A printable, single-line rendering: control characters and
			// double quotes become backslash escapes. Not shell quoting, which
			// interpolation does for you, and not a JSON encoder.
			want!(1, "1 argument");
			let mut out = String::with_capacity(s(0)?.len());
			for c in s(0)?.chars() {
				match c {
					'\\' => out.push_str("\\\\"),
					'\n' => out.push_str("\\n"),
					'\r' => out.push_str("\\r"),
					'\t' => out.push_str("\\t"),
					'\0' => out.push_str("\\0"),
					'"' => out.push_str("\\\""),
					c if (c as u32) < 0x20 => out.push_str(&format!("\\x{:02x}", c as u32)),
					c => out.push(c),
				}
			}
			Value::Str(out)
		}
		"repeat" => {
			want!(2, "2 arguments");
			let times = count(num(1)?, name, "count", sp)?;
			// A bound, because `repeat(x, 1e9)` is a typo rather than a plan.
			const MAX: usize = 8 * 1024 * 1024;
			if s(0)?.len().checked_mul(times).is_none_or(|b| b > MAX) {
				return Err(EvalError::Other {
					msg: format!("`repeat` would build more than {MAX} bytes"),
					line: sp.line,
				});
			}
			Value::Str(s(0)?.repeat(times))
		}
		"url_encode" => {
			want!(1, "1 argument");
			let mut out = String::new();
			for &b in s(0)?.as_bytes() {
				match b {
					b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
					_ => out.push_str(&format!("%{b:02X}")),
				}
			}
			Value::Str(out)
		}
		"url_decode" => {
			want!(1, "1 argument");
			Value::Str(url_decode(s(0)?).ok_or_else(|| EvalError::Other {
				msg: format!("`{}` is not valid percent-encoding", s(0).unwrap_or_default()),
				line: sp.line,
			})?)
		}
		"sha256" => {
			want!(1, "1 argument");
			use sha2::{Digest, Sha256};
			Value::Str(hex::encode(Sha256::digest(s(0)?.as_bytes())))
		}
		"md5" => {
			// Not secure, and not offered as though it were: it is here for
			// tools that fingerprint content with it.
			want!(1, "1 argument");
			use md5::{Digest, Md5};
			Value::Str(hex::encode(Md5::digest(s(0)?.as_bytes())))
		}
		"uuid" => {
			want!(0, "no arguments");
			Value::Str(uuid_v4())
		}
		"now" => {
			// Read-only, so a preview shows the real time rather than a
			// placeholder: `--dry-run` is about not changing anything.
			if n > 1 {
				return Err(arity(name, "0 or 1 arguments", n, sp));
			}
			let format = if n == 1 { s(0)? } else { "iso" };
			Value::Str(now_formatted(format).ok_or_else(|| EvalError::Other {
				msg: format!(
					"unknown time format `{format}`; expected one of: unix, unix-ms, iso, \
					 iso-date, iso-time, year, month, day, hour, minute, second"
				),
				line: sp.line,
			})?)
		}
		"json_get" => {
			want!(2, "2 arguments");
			let doc: serde_json::Value = serde_json::from_str(s(0)?).map_err(|e| EvalError::Other {
				msg: format!("`json_get`: {e}"),
				line: sp.line,
			})?;
			let found = json_path(&doc, s(1)?).ok_or_else(|| EvalError::Other {
				msg: format!("`json_get`: no value at `{}`", s(1).unwrap_or_default()),
				line: sp.line,
			})?;
			json_to_value(found)
		}
		"json_set" => {
			want!(3, "3 arguments");
			let mut doc: serde_json::Value = serde_json::from_str(s(0)?).map_err(|e| EvalError::Other {
				msg: format!("`json_set`: {e}"),
				line: sp.line,
			})?;
			// A value that parses as JSON goes in as that; anything else is a
			// string, so `json_set(d, "name", "bob")` does what it looks like.
			let fresh = serde_json::from_str(s(2)?)
				.unwrap_or_else(|_| serde_json::Value::String(s(2).unwrap_or_default().to_string()));
			json_set_at(&mut doc, s(1)?, fresh).map_err(|m| EvalError::Other {
				msg: format!("`json_set`: {m}"),
				line: sp.line,
			})?;
			Value::Str(doc.to_string())
		}
		"power" => {
			want!(2, "2 arguments");
			let r = num(0)?.powf(num(1)?);
			if !r.is_finite() {
				return Err(EvalError::Other {
					msg: "`power` result is not a finite number".into(),
					line: sp.line,
				});
			}
			Value::Num(r)
		}
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
		// A list was read-only: it could be received from `split`, `lines`,
		// `glob` or a literal and then only looked at. Everything below
		// answers with a **new** list, so nothing here mutates what it was
		// given and a binding never changes behind another name.
		"append" => {
			if n < 2 {
				return Err(arity(name, "a list and at least one value", n, sp));
			}
			let mut out = list(0)?.to_vec();
			out.extend(v[1..].iter().cloned());
			Value::List(out)
		}
		"prepend" => {
			if n < 2 {
				return Err(arity(name, "a list and at least one value", n, sp));
			}
			let mut out: Vec<Value> = v[1..].to_vec();
			out.extend(list(0)?.iter().cloned());
			Value::List(out)
		}
		"concat_lists" => {
			if n < 1 {
				return Err(arity(name, "at least one list", n, sp));
			}
			let mut out = Vec::new();
			for i in 0..n {
				out.extend(list(i)?.iter().cloned());
			}
			Value::List(out)
		}
		// Compares as the language compares: numbers by value, everything
		// else by its text. A mixed list sorts numbers before the rest rather
		// than refusing, since refusing would make `sort(ARGS)` a type puzzle.
		"sort" => {
			want!(1, "1 argument");
			let mut out = list(0)?.to_vec();
			out.sort_by(compare);
			Value::List(out)
		}
		"reverse" => {
			want!(1, "1 argument");
			let mut out = list(0)?.to_vec();
			out.reverse();
			Value::List(out)
		}
		"unique" => {
			want!(1, "1 argument");
			// Order is the order first seen, which is the only one that does
			// not surprise: `unique(sort(x))` is how you ask for sorted.
			let mut out: Vec<Value> = Vec::new();
			for item in list(0)? {
				if !out.contains(item) {
					out.push(item.clone());
				}
			}
			Value::List(out)
		}
		// Counted in items, and clamped rather than refused: a slice past the
		// end of a list is an empty list, the way it is in every language that
		// has one.
		"slice" => {
			if !(2..=3).contains(&n) {
				return Err(arity(name, "a list, a start and an optional length", n, sp));
			}
			let items = list(0)?;
			let start = (num(1)?.max(0.0) as usize).min(items.len());
			let end = match n {
				3 => (start + num(2)?.max(0.0) as usize).min(items.len()),
				_ => items.len(),
			};
			Value::List(items[start..end].to_vec())
		}
		// One level, which is the one anybody means: a list of rows becomes
		// the fields, and a deeper structure stays visible.
		"flatten" => {
			want!(1, "1 argument");
			let mut out = Vec::new();
			for item in list(0)? {
				match item {
					Value::List(inner) => out.extend(inner.iter().cloned()),
					other => out.push(other.clone()),
				}
			}
			Value::List(out)
		}
		"index_of" => {
			want!(2, "a list and a value");
			let at = list(0)?.iter().position(|x| *x == v[1]);
			Value::Num(at.map_or(-1.0, |i| i as f64))
		}
		"without" => {
			if n < 2 {
				return Err(arity(name, "a list and at least one value", n, sp));
			}
			let drop = &v[1..];
			Value::List(list(0)?.iter().filter(|x| !drop.contains(x)).cloned().collect())
		}
		// Pairs, so two lists read together become the rows a `for` walks.
		// Stops at the shorter, rather than inventing a value for the gap.
		"zip" => {
			want!(2, "two lists");
			let (a, b) = (list(0)?.to_vec(), list(1)?.to_vec());
			Value::List(a.into_iter().zip(b).map(|(x, y)| Value::List(vec![x, y])).collect())
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
/// A count argument: a whole number, not negative.
fn count(x: f64, name: &str, arg: &str, sp: Span) -> Result<usize, EvalError> {
	if x < 0.0 || x.fract() != 0.0 || x > usize::MAX as f64 {
		return Err(EvalError::Other {
			msg: format!("`{name}` needs a whole, non-negative `{arg}`, got {x}"),
			line: sp.line,
		});
	}
	Ok(x as usize)
}

fn url_decode(s: &str) -> Option<String> {
	let b = s.as_bytes();
	let mut out = Vec::with_capacity(b.len());
	let mut i = 0;
	while i < b.len() {
		if b[i] == b'%' {
			let hi = (*b.get(i + 1)? as char).to_digit(16)?;
			let lo = (*b.get(i + 2)? as char).to_digit(16)?;
			out.push((hi * 16 + lo) as u8);
			i += 3;
		} else {
			out.push(b[i]);
			i += 1;
		}
	}
	String::from_utf8(out).ok()
}

/// A random-looking version 4 UUID without a dependency: SplitMix64 seeded from
/// the clock, the process id and a counter, so two calls never collide and two
/// processes starting together do not either.
fn uuid_v4() -> String {
	static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
	let nanos = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map(|d| d.as_nanos() as u64)
		.unwrap_or(0);
	let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
	let mut state = nanos ^ (std::process::id() as u64).wrapping_shl(32) ^ count.wrapping_mul(0x9E37_79B9_7F4A_7C15);
	let mut next = || {
		state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
		let mut z = state;
		z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
		z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
		z ^ (z >> 31)
	};
	let mut b = [0u8; 16];
	b[..8].copy_from_slice(&next().to_le_bytes());
	b[8..].copy_from_slice(&next().to_le_bytes());
	b[6] = (b[6] & 0x0f) | 0x40; // version 4
	b[8] = (b[8] & 0x3f) | 0x80; // variant 1
	let h = hex::encode(b);
	format!(
		"{}-{}-{}-{}-{}",
		&h[0..8],
		&h[8..12],
		&h[12..16],
		&h[16..20],
		&h[20..32]
	)
}

/// The current UTC time in a named format, or `None` for a name that is not one.
fn now_formatted(format: &str) -> Option<String> {
	let dur = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.ok()?;
	match format {
		"unix" | "unix-timestamp" => return Some(dur.as_secs().to_string()),
		"unix-ms" | "unix-millis" => return Some(dur.as_millis().to_string()),
		_ => {}
	}
	let (y, mo, d, h, mi, s) = civil_parts(dur.as_secs() as i64);
	Some(match format {
		"iso" | "iso-8601" | "rfc3339" => format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z"),
		"iso-date" | "date" => format!("{y:04}-{mo:02}-{d:02}"),
		"iso-time" | "time" => format!("{h:02}:{mi:02}:{s:02}"),
		"year" => format!("{y:04}"),
		"month" => format!("{mo:02}"),
		"day" => format!("{d:02}"),
		"hour" => format!("{h:02}"),
		"minute" => format!("{mi:02}"),
		"second" => format!("{s:02}"),
		_ => return None,
	})
}

/// The calendar conversion, for the test that pins it against known dates.
#[cfg(test)]
pub(crate) fn civil_parts_for_test(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
	civil_parts(secs)
}

/// Split a Unix timestamp into UTC calendar parts, by Howard Hinnant's
/// `civil_from_days`, so no date library is needed for nine format strings.
fn civil_parts(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
	let (days, tod) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
	let z = days + 719_468;
	let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
	let doe = z - era * 146_097;
	let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
	let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
	let mp = (5 * doy + 2) / 153;
	let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
	let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
	let year = yoe + era * 400 + i64::from(month <= 2);
	(
		year,
		month,
		day,
		(tod / 3600) as u32,
		((tod % 3600) / 60) as u32,
		(tod % 60) as u32,
	)
}

/// Walk a dotted path. A numeric segment indexes an array, so `users.0.name`
/// reads the first user's name. An empty path is the document itself.
fn json_path<'a>(doc: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
	let mut cur = doc;
	if path.is_empty() {
		return Some(cur);
	}
	for seg in path.split('.') {
		cur = match cur {
			serde_json::Value::Array(a) => a.get(seg.parse::<usize>().ok()?)?,
			other => other.get(seg)?,
		};
	}
	Some(cur)
}

fn json_set_at(doc: &mut serde_json::Value, path: &str, fresh: serde_json::Value) -> Result<(), String> {
	if path.is_empty() {
		*doc = fresh;
		return Ok(());
	}
	let segments: Vec<&str> = path.split('.').collect();
	if segments.iter().any(|s| s.is_empty()) {
		return Err(format!("`{path}` has an empty segment"));
	}
	let mut cur = doc;
	for (i, seg) in segments.iter().enumerate() {
		let last = i + 1 == segments.len();
		// Containers are created on demand, and the next segment decides which
		// kind: a number wants an array, a name wants an object.
		if cur.is_null() {
			*cur = match seg.parse::<usize>() {
				Ok(_) => serde_json::Value::Array(Vec::new()),
				Err(_) => serde_json::Value::Object(serde_json::Map::new()),
			};
		}
		cur = match cur {
			serde_json::Value::Array(a) => {
				let idx: usize = seg.parse().map_err(|_| format!("`{seg}` is not an array index"))?;
				if idx >= a.len() {
					a.resize(idx + 1, serde_json::Value::Null);
				}
				&mut a[idx]
			}
			serde_json::Value::Object(o) => o.entry(seg.to_string()).or_insert(serde_json::Value::Null),
			other => return Err(format!("cannot descend into {other} at `{seg}`")),
		};
		if last {
			*cur = fresh;
			return Ok(());
		}
	}
	Ok(())
}

/// A JSON value as a runfile value. Objects and arrays come back as their
/// compact JSON text, since the language has no map type and a nested array
/// would lose its shape as a list of strings.
fn json_to_value(v: &serde_json::Value) -> Value {
	match v {
		serde_json::Value::Null => Value::Str(String::new()),
		serde_json::Value::Bool(b) => Value::Bool(*b),
		serde_json::Value::Number(n) => n.as_f64().map(Value::Num).unwrap_or_else(|| Value::Str(n.to_string())),
		serde_json::Value::String(s) => Value::Str(s.clone()),
		other => Value::Str(other.to_string()),
	}
}

/// A path in the OS temp directory that nothing else holds.
///
/// The process id keeps two concurrent runs apart, the counter keeps two calls
/// in one run apart, and the clock keeps a reused process id apart from an
/// earlier run's leftovers. Creation is still exclusive, so a collision fails
/// loudly rather than clobbering.
fn unique_temp_path(ext: Option<&str>) -> PathBuf {
	static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
	let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
	let nanos = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map(|d| d.subsec_nanos())
		.unwrap_or(0);
	let mut name = format!("runfile-{}-{n}-{nanos}", std::process::id());
	if let Some(e) = ext {
		name.push('.');
		name.push_str(e);
	}
	std::env::temp_dir().join(name)
}

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
/// One entry in the standard library, as editor tooling sees it.
pub struct Function {
	pub name: &'static str,
	/// How it is called, for a completion detail and a hover heading.
	pub signature: &'static str,
	/// One sentence. Long enough to answer "what does this do", short enough
	/// to sit in a popup.
	pub doc: &'static str,
	/// A worked line or two: the call, and what it answers with. A signature
	/// says the shape of a call and a sentence says its purpose; neither
	/// answers "what do I type here", which is the question someone hovering
	/// a name is usually asking.
	pub example: &'static str,
}

pub const FUNCTIONS: &[Function] = &[
	Function {
		name: "abs",
		signature: "abs(n)",
		doc: "Magnitude, without the sign.",
		example: "let d = abs(-4)                              # 4",
	},
	Function {
		name: "base64_decode",
		signature: "base64_decode(s)",
		doc: "Decode standard base64 to text.",
		example: "let json = base64_decode(ENV.CREDS_BASE64)",
	},
	Function {
		name: "base64_encode",
		signature: "base64_encode(s)",
		doc: "Encode text as standard base64.",
		example: "let b = base64_encode(\"hi\")                  # \"aGk=\"",
	},
	Function {
		name: "basename",
		signature: "basename(path)",
		doc: "The final component of a path.",
		example: "let f = basename(\"/srv/app/main.rs\")         # \"main.rs\"",
	},
	Function {
		name: "capitalize",
		signature: "capitalize(s)",
		doc: "Upper-case the first letter of every word.",
		example: "let t = capitalize(\"hello wide world\")       # \"Hello Wide World\"",
	},
	Function {
		name: "ceil",
		signature: "ceil(n)",
		doc: "Round up to a whole number.",
		example: "let n = ceil(2.1)                            # 3",
	},
	Function {
		name: "confirm",
		signature: "confirm(question)",
		doc: "Ask before going on. Answering no stops the run, the way `exit()` does — nothing catches it. Skipped by `-y` and in CI, and never asked under `--dry-run`. Call it anywhere, including inside an `if`, so the question can depend on what is about to happen.",
		example: "if env == \"production\"\n\tconfirm(\"Deploy to production?\")\nend\n\n$ terraform apply -auto-approve",
	},
	Function {
		name: "concat",
		signature: "concat(a, b, …)",
		doc: "Join values into one string. Strings do not add with `+`.",
		example: "let tag = concat(\"v\", 1, \".\", 2)             # \"v1.2\"",
	},
	Function {
		name: "contains",
		signature: "contains(s, needle)",
		doc: "Whether `needle` appears in `s`.",
		example: "if !contains(ENV.PATH, \"/usr/local/bin\")\n\terror(\"/usr/local/bin is not on PATH\")\nend",
	},
	Function {
		name: "decrypt",
		signature: "decrypt(source, dest)",
		doc: "Decrypt an encrypted `.env` file. Does nothing under `--dry-run`.",
		example: "decrypt(\".env.production\", \".env\")",
	},
	Function {
		name: "dirname",
		signature: "dirname(path)",
		doc: "Everything before the final component.",
		example: "let d = dirname(\"/srv/app/main.rs\")          # \"/srv/app\"",
	},
	Function {
		name: "ends_with",
		signature: "ends_with(s, suffix)",
		doc: "Whether `s` ends with `suffix`.",
		example: "for f in glob(\"src/*.ts\")\n\tif ends_with(f, \".test.ts\")\n\t\t$ npx vitest run {{ f }}\n\tend\nend",
	},
	Function {
		name: "error",
		signature: "error(message)",
		doc: "Fail the target with this message.",
		example: "error(\"--token is required; mint one in Settings > Actions\")",
	},
	Function {
		name: "escape",
		signature: "escape(s)",
		doc: "Render control characters and quotes as backslash escapes. Not shell quoting: interpolation already does that.",
		example: "let shown = escape(\"a\\tb\")                   # \"a\\\\tb\"",
	},
	Function {
		name: "extname",
		signature: "extname(path)",
		doc: "The extension, including its dot.",
		example: "let e = extname(\"archive.tar.gz\")            # \".gz\"",
	},
	Function {
		name: "directory_exists",
		signature: "directory_exists(path)",
		doc: "Whether the path is a directory, relative to the runfiles parent.",
		example: "if directory_exists(\".git\") || file_exists(\".git\")\n\t# a worktree keeps a *file* there\nend",
	},
	Function {
		name: "file_exists",
		signature: "file_exists(path)",
		doc: "Whether the path is a file, relative to the runfiles parent.",
		example: "if !file_exists(\"Cargo.lock\")\n\t$ cargo generate-lockfile\nend",
	},
	Function {
		name: "is_executable",
		signature: "is_executable(path)",
		doc: "Whether the path is a file this user may execute.",
		example: "if !is_executable(\"scripts/deploy.sh\")\n\terror(\"scripts/deploy.sh is not executable\")\nend",
	},
	Function {
		name: "exit",
		signature: "exit(code?)",
		doc: "Stop the run with this exit status, or 0.",
		example: "if length(ARGS) == 0\n\tprint(\"nothing to do\")\n\texit()\nend",
	},
	Function {
		name: "first",
		signature: "first(list)",
		doc: "The first item, or an empty string.",
		example: "let part = first(ARGS)                       # \"major\"",
	},
	Function {
		name: "floor",
		signature: "floor(n)",
		doc: "Round down to a whole number.",
		example: "let n = floor(2.9)                           # 2",
	},
	Function {
		name: "glob",
		signature: "glob(pattern)",
		doc: "Matching paths as a list. `*` does not cross a directory separator.",
		example: "for compose in glob(\"**/docker-compose.yml\")\n\t$ docker compose -f {{ compose }} config -q\nend",
	},
	Function {
		name: "is_number",
		signature: "is_number(s)",
		doc: "Whether the string parses as a number.",
		example: "if !is_number(ARG.port)\n\terror(\"--port must be a number\")\nend",
	},
	Function {
		name: "join",
		signature: "join(separator, list)",
		doc: "Join a list into one string.",
		example: "let image = join(\";\", \"system-images\", sdk, arch)",
	},
	Function {
		name: "join_path",
		signature: "join_path(a, b, …)",
		doc: "Join path segments with this platform's separator.",
		example: "let cfg = join_path(ENV.HOME, \".config\", \"app.toml\")",
	},
	Function {
		name: "json_get",
		signature: "json_get(json, path)",
		doc: "Read a dotted path. A numeric segment indexes an array.",
		example: "let name = json_get(read_file(\"package.json\"), \"name\")",
	},
	Function {
		name: "json_set",
		signature: "json_set(json, path, value)",
		doc: "Set a dotted path and return the document. Containers are created as needed.",
		example: "let doc = json_set(doc, \"scripts.build\", \"vite build\")",
	},
	Function {
		name: "append",
		signature: "append(list, value, …)",
		doc: "A new list with the values added at the end.",
		example: "let files = append(sources, \"build.rs\")",
	},
	Function {
		name: "concat_lists",
		signature: "concat_lists(list, …)",
		doc: "One new list with every list's items, in order.",
		example: "let all = concat_lists(glob(\"src/**/*.rs\"), glob(\"tests/**/*.rs\"))",
	},
	Function {
		name: "flatten",
		signature: "flatten(list)",
		doc: "One level of nesting removed: a list of rows becomes the fields.",
		example: "let fields = flatten([[1, 2], [3]])          # [1, 2, 3]",
	},
	Function {
		name: "index_of",
		signature: "index_of(list, value)",
		doc: "Where the value first appears, or `-1`.",
		example: "if index_of(RUN.namespaces, \"web\") != -1\n\trun web:build\nend",
	},
	Function {
		name: "prepend",
		signature: "prepend(list, value, …)",
		doc: "A new list with the values added at the front.",
		example: "let argv = prepend(ARGS, \"--locked\")",
	},
	Function {
		name: "reverse",
		signature: "reverse(list)",
		doc: "A new list, back to front.",
		example: "let newest = first(reverse(sort(tags)))",
	},
	Function {
		name: "slice",
		signature: "slice(list, start[, length])",
		doc: "A new list, counted in items. Past the end is empty rather than an error.",
		example: "let rest = slice(ARGS, 1)                    # everything after the first",
	},
	Function {
		name: "sort",
		signature: "sort(list)",
		doc: "A new list in order: numbers by value, everything else by its text.",
		example: "for f in sort(glob(\"migrations/*.sql\"))\n\t$ psql -f {{ f }}\nend",
	},
	Function {
		name: "unique",
		signature: "unique(list)",
		doc: "A new list with repeats dropped, keeping the order first seen.",
		example: "let dirs = unique(map_dirs)",
	},
	Function {
		name: "without",
		signature: "without(list, value, …)",
		doc: "A new list with those values removed.",
		example: "for ns in without(RUN.namespaces, \"docs\")\n\trun {{ ns }}:test\nend",
	},
	Function {
		name: "zip",
		signature: "zip(a, b)",
		doc: "Pairs from two lists, stopping at the shorter — rows a `for` can walk.",
		example: "for pair in zip(names, uids)\n\t$ chown {{ pair[1] }} /v/{{ pair[0] }}\nend",
	},
	Function {
		name: "last",
		signature: "last(list)",
		doc: "The final item, or an empty string.",
		example: "let newest = last(lines(tags))",
	},
	Function {
		name: "length",
		signature: "length(value)",
		doc: "Item count for a list, character count for a string.",
		example: "if length(files) > 0\n\t$ rustfmt {{ files }}\nend",
	},
	Function {
		name: "lines",
		signature: "lines(s)",
		doc: "Split into a list on line endings.",
		example: "let staged = $ git diff --cached --name-only\nlet files = lines(staged)",
	},
	Function {
		name: "max",
		signature: "max(a, b, …)",
		doc: "The largest number given.",
		example: "let n = max(1, 7, 3)                         # 7",
	},
	Function {
		name: "md5",
		signature: "md5(s)",
		doc: "Hex MD5. A fingerprint, not a secure hash.",
		example: "let key = md5(read_file(\"pnpm-lock.yaml\"))",
	},
	Function {
		name: "min",
		signature: "min(a, b, …)",
		doc: "The smallest number given.",
		example: "let n = min(1, 7, 3)                         # 1",
	},
	Function {
		name: "now",
		signature: "now([format])",
		doc: "The current UTC time. One of `unix`, `unix-ms`, `iso`, `iso-date`, `iso-time`, `year`, `month`, `day`, `hour`, `minute`, `second`.",
		example: "let today = now(\"iso-date\")                  # \"2026-09-08\"",
	},
	Function {
		name: "number",
		signature: "number(value)",
		doc: "Parse a string as a number. Required before arithmetic.",
		example: "let next = number(parts[0]) + 1",
	},
	Function {
		name: "one_of",
		signature: "one_of(value, a, b, …)",
		doc: "`value` if it is one of the options, else an error naming them.",
		example: "let part = one_of(first(ARGS), \"major\", \"minor\", \"patch\")",
	},
	Function {
		name: "power",
		signature: "power(base, exponent)",
		doc: "`base` raised to `exponent`.",
		example: "let n = power(2, 10)                         # 1024",
	},
	Function {
		name: "print",
		signature: "print(value, …)",
		doc: "Write values to stdout, separated by spaces, and end the line.",
		example: "print(\"Bumped\", cur, \"->\", next)",
	},
	Function {
		name: "printf",
		signature: "printf(format, …)",
		doc: "Write to stdout with no newline. `%s`, `%d`, `%f`, `%.Nf` and `%%`.",
		example: "printf(\"  %s -> %s (%d files)\\n\", src, dst, n)",
	},
	Function {
		name: "read_file",
		signature: "read_file(path)",
		doc: "The file's contents, relative to the runfiles parent.",
		example: "let cargo = read_file(\"Cargo.toml\")",
	},
	Function {
		name: "regex_capture",
		signature: "regex_capture(s, pattern, group)",
		doc: "One capture group of the first match.",
		example: "let cur = regex_capture(cargo, \"(?m)^version = \\\"([^\\\"]+)\\\"\", 1)",
	},
	Function {
		name: "regex_capture_all",
		signature: "regex_capture_all(s, pattern, group)",
		doc: "That group from every match, as a list.",
		example: "for image in regex_capture_all(compose, r\"image:\\s*(\\S+)\", 1)\n\t$ trivy image {{ image }}\nend",
	},
	Function {
		name: "regex_matches",
		signature: "regex_matches(s, pattern)",
		doc: "Whether the pattern matches anywhere.",
		example: "if !regex_matches(ARG.instance, r\"^https?://\\S+$\")\n\terror(\"--instance must be a URL\")\nend",
	},
	Function {
		name: "regex_remove",
		signature: "regex_remove(s, pattern)",
		doc: "Delete every match.",
		example: "let bare = regex_remove(out, r\"\\x1b\\[[0-9;]*m\")",
	},
	Function {
		name: "regex_replace",
		signature: "regex_replace(s, pattern, replacement)",
		doc: "Replace every match.",
		example: "let bumped = regex_replace(cargo, \"(?m)^version = .*\", line)",
	},
	Function {
		name: "remove_all",
		signature: "remove_all(s, needle)",
		doc: "Delete every occurrence.",
		example: "let plain = remove_all(source, \".production\")",
	},
	Function {
		name: "remove_prefix",
		signature: "remove_prefix(s, prefix)",
		doc: "Drop `prefix` if present.",
		example: "let rel = remove_prefix(path, \"web/\")",
	},
	Function {
		name: "remove_suffix",
		signature: "remove_suffix(s, suffix)",
		doc: "Drop `suffix` if present.",
		example: "let base = remove_suffix(f, \".production\")",
	},
	Function {
		name: "repeat",
		signature: "repeat(s, count)",
		doc: "`s` repeated `count` times.",
		example: "print(repeat(\"-\", 40))",
	},
	Function {
		name: "replace_all",
		signature: "replace_all(s, from, to)",
		doc: "Replace every occurrence.",
		example: "text = replace_all(text, \"__BIN__\", \"/usr/local/bin/app\")",
	},
	Function {
		name: "round",
		signature: "round(n)",
		doc: "Round to the nearest whole number.",
		example: "let n = round(2.5)                           # 3",
	},
	Function {
		name: "sha256",
		signature: "sha256(s)",
		doc: "Hex SHA-256.",
		example: "let digest = sha256(read_file(\"dist/app.tar.gz\"))",
	},
	Function {
		name: "split",
		signature: "split(s, separator)",
		doc: "Split into a list.",
		example: "let parts = split(cur, \".\")                  # [\"1\", \"2\", \"3\"]",
	},
	Function {
		name: "starts_with",
		signature: "starts_with(s, prefix)",
		doc: "Whether `s` starts with `prefix`.",
		example: "if starts_with(branch, \"release/\")\n\trun deploy --env=prod\nend",
	},
	Function {
		name: "stem",
		signature: "stem(path)",
		doc: "The final component without its extension.",
		example: "let name = stem(\"archive.tar.gz\")            # \"archive.tar\"",
	},
	Function {
		name: "substring",
		signature: "substring(s, start[, length])",
		doc: "A slice, counted in characters.",
		example: "let short = substring(sha, 0, 7)             # \"a1b2c3d\"",
	},
	Function {
		name: "temp_dir",
		signature: "temp_dir()",
		doc: "A fresh directory in the OS temp directory, removed when the run ends.",
		example: "let units = temp_dir()\nwrite_file(join_path(units, \"app.service\"), rendered)",
	},
	Function {
		name: "temp_file",
		signature: "temp_file([content][, extension])",
		doc: "A fresh file in the OS temp directory, removed when the run ends however it ends.",
		example: ".env.GOOGLE_APPLICATION_CREDENTIALS = temp_file(base64_decode(ENV.SA_JSON), \"json\")",
	},
	Function {
		name: "to_lower",
		signature: "to_lower(s)",
		doc: "Lower-case.",
		example: "let os = to_lower(RUN.os)",
	},
	Function {
		name: "to_upper",
		signature: "to_upper(s)",
		doc: "Upper-case.",
		example: "let t = to_upper(\"abc\")                      # \"ABC\"",
	},
	Function {
		name: "trim",
		signature: "trim(s)",
		doc: "Drop whitespace from both ends.",
		example: "let v = trim(read_file(\"VERSION\"))",
	},
	Function {
		name: "trim_end",
		signature: "trim_end(s)",
		doc: "Drop trailing whitespace.",
		example: "let line = trim_end(raw)",
	},
	Function {
		name: "trim_start",
		signature: "trim_start(s)",
		doc: "Drop leading whitespace.",
		example: "let body = trim_start(raw)",
	},
	Function {
		name: "url_decode",
		signature: "url_decode(s)",
		doc: "Decode percent-encoding.",
		example: "let q = url_decode(\"a%20b\")                  # \"a b\"",
	},
	Function {
		name: "url_encode",
		signature: "url_encode(s)",
		doc: "Percent-encode for a URL.",
		example: "let q = url_encode(\"a b\")                    # \"a%20b\"",
	},
	Function {
		name: "uuid",
		signature: "uuid()",
		doc: "A fresh version 4 UUID.",
		example: "let id = uuid()",
	},
	Function {
		name: "write_file",
		signature: "write_file(path, content)",
		doc: "Write a file. Does nothing under `--dry-run`.",
		example: "write_file(\"Cargo.toml\", regex_replace(cargo, pattern, line))",
	},
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
		// A question, and the one answer that means stop. Never asked under
		// `--dry-run`: a preview changes nothing, so there is nothing to
		// approve, and asking made previewing a guarded target impossible.
		"confirm" if n == 1 => (|| {
			let question = s(0)?;
			let allowed = sc.assume_yes || sc.dry_run || sc.confirm.is_some_and(|ask| ask(question));
			if allowed {
				Ok(V::Bool(true))
			} else {
				Err(EvalError::Cancelled { line: sp.line })
			}
		})(),
		// Guarded here rather than falling off the end of the table, where a
		// missed guard reads as "unknown function" and sends the reader after
		// a spelling mistake they did not make.
		"print" | "printf" if n == 0 => Err(arity(name, "at least 1 argument", n, sp)),
		// Output, not a change to anything, so these run under `--dry-run` too
		// -- the same reason `now` and `uuid` answer with real values there. A
		// preview that hides what a run would say is a worse preview.
		"print" if n >= 1 => {
			let joined = v.iter().map(V::to_string).collect::<Vec<_>>().join(" ");
			write_stdout(&format!("{joined}{NEWLINE}")).map_err(other)
		}
		"printf" if n >= 1 => (|| write_stdout(&render_format(s(0)?, &v[1..], sp)?).map_err(other))(),
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
		// Files only, and directories only: `exists()` answers neither question
		// on its own, and a check meant for one silently passing for the other
		// is the kind of thing that surfaces as a confusing error much later.
		"file_exists" if n == 1 => s(0).map(|p| V::Bool(resolve(&sc.base_dir, p).is_file())),
		"directory_exists" if n == 1 => s(0).map(|p| V::Bool(resolve(&sc.base_dir, p).is_dir())),
		"is_executable" if n == 1 => s(0).map(|p| V::Bool(is_executable(&resolve(&sc.base_dir, p)))),
		// Joins with this platform's separator, and lets an absolute later
		// segment replace what came before, the way `Path::join` does.
		"join_path" if n >= 1 => (|| {
			let mut out = PathBuf::from(s(0)?);
			for i in 1..n {
				out.push(s(i)?);
			}
			Ok(V::Str(out.to_string_lossy().into_owned()))
		})(),
		// `temp_file([content], [extension])` and `temp_dir()`: a fresh path in
		// the OS temp directory, deleted when the run ends. Writing, so a
		// preview must not create one.
		"temp_file" if n <= 2 => (|| {
			if sc.dry_run {
				return Ok(V::Str("<would create a temp file>".into()));
			}
			let ext = match n {
				2 => Some(s(1)?.trim_start_matches('.')).filter(|e| !e.is_empty()),
				_ => None,
			};
			let path = unique_temp_path(ext);
			// Exclusive creation: in a world-writable directory this refuses to
			// follow a planted symlink or to truncate an existing file.
			let mut file = std::fs::OpenOptions::new()
				.write(true)
				.create_new(true)
				.open(&path)
				.map_err(|e| other(format!("could not create a temp file: {e}")))?;
			if n >= 1 {
				use std::io::Write as _;
				file.write_all(s(0)?.as_bytes())
					.map_err(|e| other(format!("could not write the temp file: {e}")))?;
			}
			sc.temps.track(path.clone());
			Ok(V::Str(path.to_string_lossy().into_owned()))
		})(),
		"temp_dir" if n == 0 => {
			if sc.dry_run {
				Ok(V::Str("<would create a temp directory>".into()))
			} else {
				let path = unique_temp_path(None);
				match std::fs::create_dir(&path) {
					Ok(()) => {
						sc.temps.track(path.clone());
						Ok(V::Str(path.to_string_lossy().into_owned()))
					}
					Err(e) => Err(other(format!("could not create a temp directory: {e}"))),
				}
			}
		}
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

/// Whether a path is a file this user may execute.
///
/// The Unix answer is the executable bit; there is no equivalent question on
/// Windows, where executability is decided by the extension and by PATHEXT, so
/// being a file is the whole of it there.
fn is_executable(p: &std::path::Path) -> bool {
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
	}
	#[cfg(not(unix))]
	{
		p.is_file()
	}
}

/// What `print` ends a line with. A file written on Windows is expected to
/// have Windows line endings, and this is the one place the language emits a
/// line ending of its own.
#[cfg(windows)]
const NEWLINE: &str = "\r\n";
#[cfg(not(windows))]
const NEWLINE: &str = "\n";

/// One locked write, then a flush.
///
/// Locked because a target dispatched into a `.parallel` branch may be
/// printing at the same moment, and half a line from each is worse than
/// either. Flushed because the next thing to write is usually a child process
/// holding the same descriptor, and a buffered line would arrive after output
/// that came later.
fn write_stdout(text: &str) -> Result<Value, String> {
	use std::io::Write;
	let mut out = std::io::stdout().lock();
	out.write_all(text.as_bytes())
		.and_then(|()| out.flush())
		.map(|()| Value::Str(String::new()))
		.map_err(|e| format!("could not write to stdout: {e}"))
}

/// `printf`'s substitutions: `%s`, `%d`, `%f`, `%.Nf` and `%%`.
///
/// The count has to match: a `%s` with nothing to put in it, or a value with
/// no `%` to go to, is a typo every time, and printing something odd rather
/// than saying so is how a format string quietly rots.
pub(crate) fn render_format(fmt: &str, args: &[Value], sp: Span) -> Result<String, EvalError> {
	let bad = |m: String| EvalError::Other { msg: m, line: sp.line };
	let chars: Vec<char> = fmt.chars().collect();
	let mut out = String::new();
	let mut used = 0;
	let mut i = 0;
	while i < chars.len() {
		if chars[i] != '%' {
			out.push(chars[i]);
			i += 1;
			continue;
		}
		i += 1;
		if chars.get(i) == Some(&'%') {
			out.push('%');
			i += 1;
			continue;
		}
		let mut precision = None;
		if chars.get(i) == Some(&'.') {
			i += 1;
			let from = i;
			while chars.get(i).is_some_and(char::is_ascii_digit) {
				i += 1;
			}
			if i == from {
				return Err(bad("`%.` must be followed by a number of digits".into()));
			}
			precision = chars[from..i].iter().collect::<String>().parse::<usize>().ok();
		}
		let Some(&verb) = chars.get(i) else {
			return Err(bad("`%` at the end of the format, with nothing to substitute".into()));
		};
		i += 1;
		if precision.is_some() && verb != 'f' {
			return Err(bad(format!("a precision is only for `%f`, not `%{verb}`")));
		}
		let Some(arg) = args.get(used) else {
			return Err(bad(format!(
				"the format has more substitutions than the {} value(s) given",
				args.len()
			)));
		};
		used += 1;
		match verb {
			's' => out.push_str(&arg.to_string()),
			'd' => {
				let x = arg.as_num().map_err(|e| ty(sp, e))?;
				if x.fract() != 0.0 {
					return Err(bad(format!("`%d` wants a whole number, and {x} is not one")));
				}
				out.push_str(&crate::value::format_num(x));
			}
			'f' => {
				let x = arg.as_num().map_err(|e| ty(sp, e))?;
				out.push_str(&format!("{x:.*}", precision.unwrap_or(6)));
			}
			_ => return Err(bad(format!("`%{verb}` is not a substitution; use `%s`, `%d` or `%f`"))),
		}
	}
	if used < args.len() {
		return Err(bad(format!(
			"{} value(s) given, but the format substitutes {used}",
			args.len()
		)));
	}
	Ok(out)
}

/// How `sort` orders two values: numbers by value, everything else by its
/// text, and a number before anything that is not one.
fn compare(a: &Value, b: &Value) -> std::cmp::Ordering {
	match (a, b) {
		(Value::Num(x), Value::Num(y)) => x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal),
		(Value::Num(_), _) => std::cmp::Ordering::Less,
		(_, Value::Num(_)) => std::cmp::Ordering::Greater,
		_ => a.to_string().cmp(&b.to_string()),
	}
}
