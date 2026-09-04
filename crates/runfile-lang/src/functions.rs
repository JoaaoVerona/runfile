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
			e @ EvalError::Exit { .. } => e,
			_ => EvalError::Caught { line: sp.line },
		});
	}

	let v: Vec<Value> = args.iter().map(|a| eval(a, sc)).collect::<Result<_, _>>()?;
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
}

pub const FUNCTIONS: &[Function] = &[
	Function {
		name: "abs",
		signature: "abs(n)",
		doc: "Magnitude, without the sign.",
	},
	Function {
		name: "base64_decode",
		signature: "base64_decode(s)",
		doc: "Decode standard base64 to text.",
	},
	Function {
		name: "base64_encode",
		signature: "base64_encode(s)",
		doc: "Encode text as standard base64.",
	},
	Function {
		name: "basename",
		signature: "basename(path)",
		doc: "The final component of a path.",
	},
	Function {
		name: "capitalize",
		signature: "capitalize(s)",
		doc: "Upper-case the first letter of every word.",
	},
	Function {
		name: "ceil",
		signature: "ceil(n)",
		doc: "Round up to a whole number.",
	},
	Function {
		name: "concat",
		signature: "concat(a, b, …)",
		doc: "Join values into one string. Strings do not add with `+`.",
	},
	Function {
		name: "contains",
		signature: "contains(s, needle)",
		doc: "Whether `needle` appears in `s`.",
	},
	Function {
		name: "decrypt",
		signature: "decrypt(source, dest)",
		doc: "Decrypt an encrypted `.env` file. Does nothing under `--dry-run`.",
	},
	Function {
		name: "dirname",
		signature: "dirname(path)",
		doc: "Everything before the final component.",
	},
	Function {
		name: "ends_with",
		signature: "ends_with(s, suffix)",
		doc: "Whether `s` ends with `suffix`.",
	},
	Function {
		name: "error",
		signature: "error(message)",
		doc: "Fail the target with this message.",
	},
	Function {
		name: "escape",
		signature: "escape(s)",
		doc: "Render control characters and quotes as backslash escapes. Not shell quoting: interpolation already does that.",
	},
	Function {
		name: "extname",
		signature: "extname(path)",
		doc: "The extension, including its dot.",
	},
	Function {
		name: "file_exists",
		signature: "file_exists(path)",
		doc: "Whether the path exists, relative to the runfiles parent.",
	},
	Function {
		name: "exit",
		signature: "exit(code?)",
		doc: "Stop the run with this exit status, or 0. May be written without parentheses.",
	},
	Function {
		name: "first",
		signature: "first(list)",
		doc: "The first item, or an empty string.",
	},
	Function {
		name: "floor",
		signature: "floor(n)",
		doc: "Round down to a whole number.",
	},
	Function {
		name: "glob",
		signature: "glob(pattern)",
		doc: "Matching paths as a list. `*` does not cross a directory separator.",
	},
	Function {
		name: "is_number",
		signature: "is_number(s)",
		doc: "Whether the string parses as a number.",
	},
	Function {
		name: "join",
		signature: "join(separator, list)",
		doc: "Join a list into one string.",
	},
	Function {
		name: "join_path",
		signature: "join_path(a, b, …)",
		doc: "Join path segments with this platform's separator.",
	},
	Function {
		name: "json_get",
		signature: "json_get(json, path)",
		doc: "Read a dotted path. A numeric segment indexes an array.",
	},
	Function {
		name: "json_set",
		signature: "json_set(json, path, value)",
		doc: "Set a dotted path and return the document. Containers are created as needed.",
	},
	Function {
		name: "last",
		signature: "last(list)",
		doc: "The final item, or an empty string.",
	},
	Function {
		name: "length",
		signature: "length(value)",
		doc: "Item count for a list, character count for a string.",
	},
	Function {
		name: "lines",
		signature: "lines(s)",
		doc: "Split into a list on line endings.",
	},
	Function {
		name: "max",
		signature: "max(a, b, …)",
		doc: "The largest number given.",
	},
	Function {
		name: "md5",
		signature: "md5(s)",
		doc: "Hex MD5. A fingerprint, not a secure hash.",
	},
	Function {
		name: "min",
		signature: "min(a, b, …)",
		doc: "The smallest number given.",
	},
	Function {
		name: "now",
		signature: "now([format])",
		doc: "The current UTC time. One of `unix`, `unix-ms`, `iso`, `iso-date`, `iso-time`, `year`, `month`, `day`, `hour`, `minute`, `second`.",
	},
	Function {
		name: "number",
		signature: "number(value)",
		doc: "Parse a string as a number. Required before arithmetic.",
	},
	Function {
		name: "one_of",
		signature: "one_of(value, a, b, …)",
		doc: "`value` if it is one of the options, else an error naming them.",
	},
	Function {
		name: "power",
		signature: "power(base, exponent)",
		doc: "`base` raised to `exponent`.",
	},
	Function {
		name: "read_file",
		signature: "read_file(path)",
		doc: "The file's contents, relative to the runfiles parent.",
	},
	Function {
		name: "regex_capture",
		signature: "regex_capture(s, pattern, group)",
		doc: "One capture group of the first match.",
	},
	Function {
		name: "regex_capture_all",
		signature: "regex_capture_all(s, pattern, group)",
		doc: "That group from every match, as a list.",
	},
	Function {
		name: "regex_matches",
		signature: "regex_matches(s, pattern)",
		doc: "Whether the pattern matches anywhere.",
	},
	Function {
		name: "regex_remove",
		signature: "regex_remove(s, pattern)",
		doc: "Delete every match.",
	},
	Function {
		name: "regex_replace",
		signature: "regex_replace(s, pattern, replacement)",
		doc: "Replace every match.",
	},
	Function {
		name: "remove_all",
		signature: "remove_all(s, needle)",
		doc: "Delete every occurrence.",
	},
	Function {
		name: "remove_prefix",
		signature: "remove_prefix(s, prefix)",
		doc: "Drop `prefix` if present.",
	},
	Function {
		name: "remove_suffix",
		signature: "remove_suffix(s, suffix)",
		doc: "Drop `suffix` if present.",
	},
	Function {
		name: "repeat",
		signature: "repeat(s, count)",
		doc: "`s` repeated `count` times.",
	},
	Function {
		name: "replace_all",
		signature: "replace_all(s, from, to)",
		doc: "Replace every occurrence.",
	},
	Function {
		name: "round",
		signature: "round(n)",
		doc: "Round to the nearest whole number.",
	},
	Function {
		name: "sha256",
		signature: "sha256(s)",
		doc: "Hex SHA-256.",
	},
	Function {
		name: "split",
		signature: "split(s, separator)",
		doc: "Split into a list.",
	},
	Function {
		name: "starts_with",
		signature: "starts_with(s, prefix)",
		doc: "Whether `s` starts with `prefix`.",
	},
	Function {
		name: "stem",
		signature: "stem(path)",
		doc: "The final component without its extension.",
	},
	Function {
		name: "substring",
		signature: "substring(s, start[, length])",
		doc: "A slice, counted in characters.",
	},
	Function {
		name: "temp_dir",
		signature: "temp_dir()",
		doc: "A fresh directory in the OS temp directory, removed when the run ends.",
	},
	Function {
		name: "temp_file",
		signature: "temp_file([content][, extension])",
		doc: "A fresh file in the OS temp directory, removed when the run ends however it ends.",
	},
	Function {
		name: "to_lower",
		signature: "to_lower(s)",
		doc: "Lower-case.",
	},
	Function {
		name: "to_upper",
		signature: "to_upper(s)",
		doc: "Upper-case.",
	},
	Function {
		name: "trim",
		signature: "trim(s)",
		doc: "Drop whitespace from both ends.",
	},
	Function {
		name: "trim_end",
		signature: "trim_end(s)",
		doc: "Drop trailing whitespace.",
	},
	Function {
		name: "trim_start",
		signature: "trim_start(s)",
		doc: "Drop leading whitespace.",
	},
	Function {
		name: "url_decode",
		signature: "url_decode(s)",
		doc: "Decode percent-encoding.",
	},
	Function {
		name: "url_encode",
		signature: "url_encode(s)",
		doc: "Percent-encode for a URL.",
	},
	Function {
		name: "uuid",
		signature: "uuid()",
		doc: "A fresh version 4 UUID.",
	},
	Function {
		name: "write_file",
		signature: "write_file(path, content)",
		doc: "Write a file. Does nothing under `--dry-run`.",
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
