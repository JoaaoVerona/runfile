//! The state every line runs in, and the statements that change it.
//!
//! Four things a line inherits from the lines above it: the **environment** a
//! command is handed and `ENV.X` reads, which of its values a `.env` file
//! supplied, the **working directory** a command runs in and a relative path
//! resolves against, and whether a failure **stops** the target. They live in
//! the [`Scope`], which is what `ENV.X` and the file functions already read, so
//! there is one copy of each and nothing to keep in step.
//!
//! They are changed by `ENV.NAME = …` and by the four [`EFFECTS`], each of
//! which is a line of its own -- the parser refuses one anywhere a value is
//! asked for. A change lasts **past the block it is written in**, the way a
//! `let` does: `if RUN.os == "mac"` around an `add_path()` is how a target puts
//! a directory on PATH for the rest of its lines. A `do` block is the one that
//! scopes: it takes the [`State`] as it opens and puts it back as it closes.
//! Each attempt of a `retry` starts from the state the `retry` began with, and
//! a parallel branch changes a copy of it.

use crate::eval::{EvalError, Scope};
use crate::span::Span;
use crate::value::{TypeError, Value};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

/// The functions that change the state the lines below them run in.
///
/// Each answers with nothing worth having, and the parser refuses one anywhere
/// but as a line of its own: inside a value it would be a change hidden in
/// something that reads like a question.
pub const EFFECTS: &[&str] = &["add_path", "ignore_errors", "import_env_file", "set_cwd"];

/// Whether `name` is one of [`EFFECTS`].
pub fn is_effect(name: &str) -> bool {
	EFFECTS.contains(&name)
}

/// What a `do` block puts back at its `end`, and a `retry` before each attempt.
///
/// Bindings are not part of it: they are flat everywhere, and putting a `let`
/// back at an `end` would make a `do` the one block that scoped names too.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
	pub env: HashMap<String, String>,
	/// The names whose value a `.env` file supplied, and nothing since has
	/// replaced: what a later file may still replace, and what a target `run`
	/// hands over as a default rather than as something exported.
	pub from_files: BTreeSet<String>,
	pub workdir: PathBuf,
	pub ignore_errors: bool,
}

impl Scope {
	/// The state the line being run inherits.
	pub fn state(&self) -> State {
		State {
			env: self.env.clone(),
			from_files: self.from_files.clone(),
			workdir: self.workdir.clone(),
			ignore_errors: self.ignore_errors,
		}
	}

	/// Put back a state [`Scope::state`] took.
	pub fn put_back(&mut self, s: State) {
		self.env = s.env;
		self.from_files = s.from_files;
		self.workdir = s.workdir;
		self.ignore_errors = s.ignore_errors;
	}

	/// The key `name` is held under in the environment.
	///
	/// Itself, unless the environment already holds it under another case on
	/// Windows, whose environment names ignore case -- so `ENV.Path = …` there
	/// replaces the `PATH` that is, rather than handing a command two of them.
	pub fn env_key(&self, name: &str) -> String {
		if self.env.contains_key(name) || !cfg!(windows) {
			return name.to_string();
		}
		self.env
			.keys()
			.find(|k| k.eq_ignore_ascii_case(name))
			.cloned()
			.unwrap_or_else(|| name.to_string())
	}

	/// Set `name` for every line below this one, as an assignment does: over
	/// whatever it held, a file's value or the caller's.
	pub fn set_env(&mut self, name: &str, value: String) {
		let key = self.env_key(name);
		self.from_files.remove(&key);
		self.env.insert(key, value);
	}

	/// Put `dirs` at the front of PATH, in the order given.
	///
	/// A directory PATH already holds is moved to the front rather than added a
	/// second time: a target another one `run`s is run with its caller's PATH,
	/// so a `_shared.run` both of them read would otherwise stack a copy per
	/// level. Moving finds exactly what the copy would have, since PATH is
	/// searched in order. A PATH none of them was on keeps its own text after
	/// them.
	pub fn prepend_path(&mut self, dirs: &[String]) {
		if dirs.is_empty() {
			return;
		}
		let key = self
			.env
			.keys()
			.find(|k| k.eq_ignore_ascii_case("PATH"))
			.cloned()
			.unwrap_or_else(|| "PATH".to_string());
		let sep = if cfg!(windows) { ";" } else { ":" };
		let held = self.env.get(&key).cloned().unwrap_or_default();
		let mut out: Vec<&str> = dirs.iter().map(String::as_str).collect();
		if !held.is_empty() {
			out.extend(held.split(sep).filter(|h| !dirs.iter().any(|d| d == h)));
		}
		let joined = out.join(sep);
		self.from_files.remove(&key);
		self.env.insert(key, joined);
	}
}

/// `ENV.NAME = value`: what the value becomes, set for every line below.
///
/// An environment variable is text, so a number or a bool is written the way
/// `{{ … }}` writes it. A list has no one way to be written -- joined with a
/// space, a colon, a comma? -- so it is refused, with the call that says which.
pub fn assign_env(sc: &mut Scope, name: &str, value: Value, line: usize) -> Result<(), EvalError> {
	if let Value::List(_) = value {
		return Err(EvalError::Other {
			msg: format!(
				"`ENV.{name}` holds text, and this is a list -- `join(…, \" \")` makes one string of it, with the separator you mean"
			),
			line,
		});
	}
	sc.set_env(name, value.to_string());
	Ok(())
}

/// Carry out one of [`EFFECTS`], its arguments evaluated.
pub(crate) fn apply(name: &str, v: &[Value], sc: &mut Scope, sp: Span) -> Result<(), EvalError> {
	let want = |n: usize, expected: &str| -> Result<(), EvalError> {
		match v.len() == n {
			true => Ok(()),
			false => Err(EvalError::Arity {
				name: name.into(),
				expected: expected.into(),
				got: v.len(),
				line: sp.line,
			}),
		}
	};
	match name {
		// Relative to the one in effect, the way `cd` is.
		"set_cwd" => {
			want(1, "1 argument")?;
			let dir = v[0].as_str().map_err(|e| EvalError::ty(sp.line, e))?;
			sc.workdir = sc.workdir.join(dir);
		}
		"add_path" => {
			want(1, "1 argument")?;
			let dirs: Vec<String> = paths(&v[0], sp)?
				.into_iter()
				.map(|d| sc.workdir.join(d).to_string_lossy().into_owned())
				.collect();
			sc.prepend_path(&dirs);
		}
		"ignore_errors" => {
			want(0, "no arguments")?;
			sc.ignore_errors = true;
		}
		"import_env_file" => {
			want(1, "1 argument")?;
			for f in paths(&v[0], sp)? {
				import(sc, f, sp)?;
			}
		}
		_ => unreachable!("`{name}` is not one of EFFECTS"),
	}
	Ok(())
}

/// A path, or a list of them.
fn paths(v: &Value, sp: Span) -> Result<Vec<&str>, EvalError> {
	let expected = |actual| {
		EvalError::ty(
			sp.line,
			TypeError::Expected {
				expected: "a path or a list of paths",
				actual,
			},
		)
	};
	match v {
		Value::Str(s) => Ok(vec![s.as_str()]),
		Value::List(items) => items
			.iter()
			.map(|i| i.as_str().map_err(|_| expected(i.type_name())))
			.collect(),
		other => Err(expected(other.type_name())),
	}
}

/// Load one `.env` file into the environment, beneath what is already there.
///
/// A file is a default -- the dotenv convention, so a checked-in `.env` does not
/// clobber what someone exported -- which makes the rule one sentence: **a file
/// value fills a name nothing has set, or replaces another file's.** So a later
/// file layers over an earlier one, and neither the caller's environment nor an
/// `ENV.X = …` is ever replaced by one, whichever line comes first.
///
/// A file that is not there is passed over, since a project whose `.env` is
/// git-ignored still has to run. A preview reads none at all and asks for no
/// key (audit SA-008): previewing an untrusted repository must not unlock the
/// credential store to decrypt its files with the user's keys. Only a file's
/// own `encrypted:` values are decrypted (audit SA-004).
fn import(sc: &mut Scope, file: &str, sp: Span) -> Result<(), EvalError> {
	if sc.preview {
		return Ok(());
	}
	let path = sc.workdir.join(file);
	let keys = sc.private_keys.clone();
	let vars = runfile_env::read_env_file(&path, &sc.env, Some(&keys)).map_err(|e| EvalError::Other {
		msg: e.to_string(),
		line: sp.line,
	})?;
	let Some(vars) = vars else {
		return Ok(());
	};
	let mut vars: Vec<(String, String)> = vars.into_iter().collect();
	vars.sort();
	for (k, value) in vars {
		let key = sc.env_key(&k);
		if !sc.env.contains_key(&key) || sc.from_files.contains(&key) {
			sc.env.insert(key.clone(), value);
			sc.from_files.insert(key);
		}
	}
	Ok(())
}
