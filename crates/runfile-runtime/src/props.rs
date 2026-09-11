//! Property resolution.
//!
//! Behavioural properties scope to the block they appear in and inherit into
//! nested blocks; descriptive ones are header-only and read once per target.
//! `IfStep`, `ForStep` and `MatchStep` already carried their own `ignoreErrors`
//! in the JSON model -- this makes that uniform.

use runfile_lang::Value;
use runfile_lang::ast::{Block, Property};
use runfile_lang::eval::{EvalError, Scope, eval_boundary};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub struct Props {
	pub shell: Option<String>,
	pub parallel: bool,
	pub ignore_errors: bool,
	/// Announce each command on stderr before it runs. Off unless asked for:
	/// most targets are run for their output, and a runner talking over it is
	/// noise. `run --dry-run` prints the commands to stdout instead and never
	/// announces.
	pub logging: bool,
	pub workdir: Option<String>,
	pub env: BTreeMap<String, String>,
	/// Both append rather than replace, so a block adds to what it inherited
	/// and a `_shared.run` entry is never lost by a target naming its own.
	pub env_files: Vec<String>,
	pub add_paths: Vec<String>,
	// header-only
	pub watch: Vec<String>,
	/// Whether this target came from the machine-wide directory. Not a
	/// property: it is a fact about where the file was found, and the one
	/// thing that makes `.only-in-directories` mean anything.
	pub machine_wide: bool,
}

/// Every property name, whether it may appear inside a block, and whether it
/// is a flag.
///
/// Exported so tooling offers exactly what exists, and so the three axes are
/// described once: `apply` reads this list rather than carrying its own copy,
/// and the language server reads the same one, which is what stops an editor
/// accepting a line the runner refuses. A test walks it and rejects any name
/// `extend` would call unknown.
/// One property name, as editor tooling sees it. Named apart from the AST's
/// `Property`, which is an occurrence of one in a file.
pub struct KnownProperty {
	pub name: &'static str,
	/// Whether it may appear inside an `if` / `for` / `match` block, rather
	/// than only at the top of a file.
	pub block_scoped: bool,
	/// Whether it is a flag: written bare for `= true`, and otherwise taking a
	/// bool. A constant that is not one is refused where it is written, and a
	/// value worked out during the run has to resolve to a bool or to one of
	/// the words a shell and an environment variable spell one with.
	pub flag: bool,
	/// Whether it has to be written above the block's first statement.
	///
	/// These describe the shape of the whole block rather than the environment
	/// of the commands under them, so "from here down" is not a reading they
	/// have: a fan-out collects every branch before any of them runs, and
	/// `.watch` and `.detach` are answered once, around the run. Everything
	/// header-only is also this; `parallel` is the one that is not.
	pub declaration_only: bool,
	pub doc: &'static str,
	/// What it looks like in a file. A property is a line someone writes
	/// rather than a value they compute, so the useful thing to show is the
	/// line.
	pub example: &'static str,
}

pub const PROPERTIES: &[KnownProperty] = &[
	KnownProperty {
		name: "add-path",
		block_scoped: true,
		flag: false,
		declaration_only: false,
		doc: "Prepend a directory to `PATH`, relative to the runfiles parent.",
		example: ".add-path = \"node_modules/.bin\"",
	},
	KnownProperty {
		name: "env",
		block_scoped: true,
		flag: false,
		declaration_only: false,
		doc: "Set an environment variable, addressed by sub-key: `.env.NAME = \"value\"`.",
		example: ".env.PORT = ARG.port ? \"3000\"\n.env.DATABASE_URL = \"postgres://localhost/app\"",
	},
	KnownProperty {
		name: "env-file",
		block_scoped: true,
		flag: false,
		declaration_only: false,
		doc: "Load a `.env` file. Encrypted values are decrypted in memory.",
		example: ".env-file = \".env.{{ one_of(ARG.env, \\\"dev\\\", \\\"prod\\\") }}\"",
	},
	KnownProperty {
		name: "ignore-errors",
		block_scoped: true,
		flag: true,
		declaration_only: false,
		doc: "Keep going when a command fails.",
		example: ".ignore-errors\n\n# Creating a volume that exists is an error worth ignoring.\n$ docker volume create app-data",
	},
	KnownProperty {
		name: "logging",
		block_scoped: false,
		flag: true,
		declaration_only: true,
		doc: "Announce each command on stderr before it runs.",
		example: ".logging = true\n\n# Each command announces itself on stderr as it runs.",
	},
	KnownProperty {
		name: "only-in-directories",
		block_scoped: false,
		flag: false,
		declaration_only: true,
		doc: "For the machine-wide directory: offer this target only inside these directories.",
		example: ".only-in-directories = [\"~/work/acme\", \"~/work/zed\"]",
	},
	KnownProperty {
		name: "parallel",
		block_scoped: true,
		flag: true,
		declaration_only: true,
		doc: "Run this block's commands at once, each branch labelled in the output.",
		example: "for compose in glob(\"**/docker-compose.yml\")\n\t.parallel\n\n\t$ docker compose -f {{ compose }} pull\nend",
	},
	KnownProperty {
		name: "shell",
		block_scoped: false,
		flag: false,
		declaration_only: true,
		doc: "Which POSIX shell `$` lines use: sh, bash, dash, ash, zsh, ksh, busybox or brush.",
		example: ".shell = \"sh\"\n\n# For any other interpreter, name it on the line: `exec pwsh`.",
	},
	KnownProperty {
		name: "watch",
		block_scoped: false,
		flag: false,
		declaration_only: true,
		doc: "Re-run whenever a matching file changes. A `!` prefix excludes.",
		example: ".watch = \"src/**/*.rs\"\n.watch = \"!src/generated/**\"",
	},
	KnownProperty {
		name: "workdir",
		block_scoped: true,
		flag: false,
		declaration_only: false,
		doc: "Where commands run, relative to the runfiles parent.",
		example: ".workdir = \"web\"",
	},
];

/// How a flag's value reads once the run has worked it out.
///
/// A bool is the answer; the four words are what the places a flag is usually
/// read from spell one with -- `ENV.CI`, an `ARG` typed on a command line, the
/// output of a `$` capture. Nothing else is accepted, because the alternative
/// is what this replaced: `matches!(v, Bool(true))` read every other value as
/// `false`, so `.ignore-errors = "yes"` was off and said nothing.
fn as_bool(v: &Value) -> Option<bool> {
	match v {
		Value::Bool(b) => Some(*b),
		Value::Str(s) => match s.trim().to_ascii_lowercase().as_str() {
			"true" | "1" => Some(true),
			"false" | "0" => Some(false),
			_ => None,
		},
		_ => None,
	}
}

#[derive(Debug, thiserror::Error)]
pub enum PropError {
	#[error("line {line}: unknown property `.{name}`")]
	Unknown { name: String, line: usize },
	#[error("line {line}: `.{name}` is header-only and cannot be set inside a block")]
	NotBlockScoped { name: String, line: usize },
	#[error("line {line}: `.{name}` needs a value")]
	NeedsValue { name: String, line: usize },
	#[error("line {line}: `.{name}` scopes the machine-wide directory; this target is part of the project")]
	NotMachineWide { name: String, line: usize },
	/// A constant that can never be a bool, read off the page. Separate from
	/// `FlagValue` because this one is a mistake in the text and is answerable
	/// before the run: the language server reports it from the same list.
	#[error("line {line}: `.{name}` is a flag and takes a bool, not {kind}")]
	NotABool { name: String, line: usize, kind: String },
	#[error("line {line}: `.{name}` is a flag and needs `true` or `false`; this resolved to `{got}`")]
	FlagValue { name: String, line: usize, got: String },
	#[error(
		"line {line}: `.{name}` describes the whole block, so it has to be written above the block's first statement"
	)]
	NotInDeclaration { name: String, line: usize },
	/// `.shell` names which of the eight POSIX shells `$` uses, and nothing
	/// else. Any other program reached the same spawn by a path that reads
	/// nothing like a shell -- no `-e`, and the script on stdin rather than
	/// after `-c` -- so `$ ssh host` under one was broken exactly as it was
	/// before scripts moved to `-c`, and nothing said so. `exec pwsh` has the
	/// identical mechanics and names the program on the line, so a reader is
	/// not misled about what `$` will do.
	#[error("line {line}: `.shell` names a POSIX shell, not `{got}` -- for another interpreter write `exec {got}`")]
	NotAShell { line: usize, got: String },
	#[error(transparent)]
	Eval(#[from] EvalError),
}

/// How a constant that is not a bool reads in a message, with the hint that
/// fits it. A string spelling a bool word is the near miss worth naming: the
/// quotes are the whole mistake.
pub fn describe_constant(c: &runfile_lang::Constant) -> String {
	match c {
		runfile_lang::Constant::Number => "a number".to_string(),
		runfile_lang::Constant::List => "a list".to_string(),
		runfile_lang::Constant::Str(s) => match s.trim().to_ascii_lowercase().as_str() {
			w @ ("true" | "false") => format!("a string -- write `{w}` without the quotes"),
			"1" => "a string -- write `true`".to_string(),
			"0" => "a string -- write `false`".to_string(),
			_ => "a string".to_string(),
		},
	}
}

/// Everything about a property line that is answerable without running it.
///
/// One function, so the three static rules are asked in one order and the
/// language server can ask them the same way -- an editor accepting a line the
/// runner refuses is how the two come to disagree about what a file means.
pub fn check(p: &Property, nested: bool, trailing: bool) -> Result<&'static KnownProperty, PropError> {
	let head = p.path[0].as_str();
	let line = p.span.line;
	// Whether it exists comes first: the other questions are about a name this
	// list has, so asking any of them first told someone who typed
	// `.ignore-error` inside a `for` that it was header-only -- sending them
	// after a rule instead of a spelling mistake.
	let Some(known) = PROPERTIES.iter().find(|k| k.name == head) else {
		return Err(PropError::Unknown {
			name: p.path.join("."),
			line,
		});
	};
	// Where it may sit, from the outside in: what a block allows at all, then
	// whereabouts in one. Header-only is the sharper answer of the two, and
	// everything header-only is also declaration-only, so it is asked first.
	if nested && !known.block_scoped {
		return Err(PropError::NotBlockScoped {
			name: p.path.join("."),
			line,
		});
	}
	if trailing && known.declaration_only {
		return Err(PropError::NotInDeclaration {
			name: p.path.join("."),
			line,
		});
	}
	// A flag written with a constant that can never be a bool is answered here
	// rather than by evaluating it: `.parallel = 23` has one reading and it is
	// a mistake, so it is reported whether or not the line would have been
	// reached.
	if known.flag
		&& let Some(e) = &p.value
		&& let Some(c) = e.constant_non_bool()
	{
		return Err(PropError::NotABool {
			name: p.path.join("."),
			line,
			kind: describe_constant(&c),
		});
	}
	// The same shape for the one property whose *values* are a fixed set: a
	// constant is answered here, and anything the run works out is answered by
	// `apply`, against the same predicate.
	if head == "shell"
		&& let Some(e) = &p.value
		&& let Some(runfile_lang::Constant::Str(s)) = e.constant_non_bool()
		&& !crate::exec::body_is_shell(Some(&s))
	{
		return Err(PropError::NotAShell { line, got: s });
	}
	Ok(known)
}

impl Props {
	/// Layer a block's declaration region over the inherited set.
	///
	/// Only the region above the first statement: a property written below one
	/// is applied by the walker where it sits, so that it can read the bindings
	/// above it. The ones that could not mean that are refused here rather than
	/// there -- this runs before the block does, so the message arrives whether
	/// or not the line would have been reached.
	pub fn extend(&self, block: &Block, sc: &mut Scope, nested: bool) -> Result<Props, PropError> {
		let mut out = self.clone();
		// A nested block inherits behaviour but never a parent's one-shot header
		// state.
		if nested {
			out.watch.clear();
		}
		for p in block.trailing() {
			check(p, nested, true)?;
		}
		for p in block.declaration() {
			out.apply(p, sc, nested)?;
		}
		Ok(out)
	}

	pub(crate) fn apply(&mut self, p: &Property, sc: &mut Scope, nested: bool) -> Result<(), PropError> {
		let head = p.path[0].as_str();
		let line = p.span.line;
		check(p, nested, false)?;
		let value = |p: &Property, sc: &mut Scope| -> Result<Value, PropError> {
			match &p.value {
				Some(e) => Ok(eval_boundary(e, sc)?),
				None => Err(PropError::NeedsValue {
					name: p.path.join("."),
					line,
				}),
			}
		};
		let flag = |p: &Property, sc: &mut Scope| -> Result<bool, PropError> {
			match &p.value {
				None => Ok(true), // bare `.parallel` means `= true`
				Some(e) => {
					let v = eval_boundary(e, sc)?;
					as_bool(&v).ok_or_else(|| PropError::FlagValue {
						name: p.path.join("."),
						line,
						got: v.to_string(),
					})
				}
			}
		};
		match head {
			"shell" => {
				let v = value(p, sc)?.to_string();
				if !crate::exec::body_is_shell(Some(&v)) {
					return Err(PropError::NotAShell { line, got: v });
				}
				self.shell = Some(v);
			}
			"logging" => self.logging = flag(p, sc)?,
			"parallel" => self.parallel = flag(p, sc)?,
			"ignore-errors" => self.ignore_errors = flag(p, sc)?,
			"workdir" => self.workdir = Some(value(p, sc)?.to_string()),
			"env" => {
				if p.path.len() != 2 {
					return Err(PropError::Unknown {
						name: p.path.join("."),
						line,
					});
				}
				self.env.insert(p.path[1].clone(), value(p, sc)?.to_string());
			}
			// Repeated keys append; these are list-valued by nature.
			"env-file" => push_all(&mut self.env_files, value(p, sc)?),
			"add-path" => push_all(&mut self.add_paths, value(p, sc)?),
			"watch" => push_all(&mut self.watch, value(p, sc)?),
			// Read by discovery before anything runs, so there is nothing left
			// to do with it here -- but a project file setting it does nothing
			// at all, and saying so is the whole point: a scope that silently
			// did not apply is a target offered where it was meant to be
			// hidden, found out by someone else.
			"only-in-directories" => {
				if !self.machine_wide {
					return Err(PropError::NotMachineWide {
						name: p.path.join("."),
						line,
					});
				}
			}
			_ => {
				return Err(PropError::Unknown {
					name: p.path.join("."),
					line,
				});
			}
		}
		Ok(())
	}
}

fn push_all(dst: &mut Vec<String>, v: Value) {
	match v {
		Value::List(items) => dst.extend(items.iter().map(Value::to_string)),
		other => dst.push(other.to_string()),
	}
}
