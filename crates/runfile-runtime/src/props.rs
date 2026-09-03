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
	pub workdir: Option<String>,
	pub env: BTreeMap<String, String>,
	// header-only
	pub env_files: Vec<String>,
	pub add_paths: Vec<String>,
	pub confirm: Option<String>,
	pub watch: Vec<String>,
	pub hide: bool,
	/// Start the commands and do not wait; see `Spawn::detach`.
	pub detach: bool,
	pub aliases: Vec<String>,
	pub only_in_directories: Vec<String>,
}

/// Properties a nested block may set. Everything else is header-only, because
/// the runner has to know it before any statement runs.
const BLOCK_SCOPED: &[&str] = &["shell", "parallel", "ignore-errors", "workdir", "env"];

/// Every property name, and whether it may appear inside a block.
///
/// Exported so tooling offers exactly what exists. A test walks this list and
/// rejects any name `extend` would call unknown.
/// One property name, as editor tooling sees it. Named apart from the AST's
/// `Property`, which is an occurrence of one in a file.
pub struct KnownProperty {
	pub name: &'static str,
	/// Whether it may appear inside an `if` / `for` / `match` block, rather
	/// than only at the top of a file.
	pub block_scoped: bool,
	pub doc: &'static str,
}

pub const PROPERTIES: &[KnownProperty] = &[
	KnownProperty {
		name: "add-path",
		block_scoped: false,
		doc: "Prepend a directory to `PATH`, relative to the runfiles parent.",
	},
	KnownProperty {
		name: "alias",
		block_scoped: false,
		doc: "Another name this target answers to. Carries the target's namespace.",
	},
	KnownProperty {
		name: "detach",
		block_scoped: false,
		doc: "Start the commands and do not wait. For something meant to outlive the run.",
	},
	KnownProperty {
		name: "confirm",
		block_scoped: false,
		doc: "Ask before running. Skipped by `-y` and in CI.",
	},
	KnownProperty {
		name: "env",
		block_scoped: true,
		doc: "Set an environment variable, addressed by sub-key: `.env.NAME = \"value\"`.",
	},
	KnownProperty {
		name: "env-file",
		block_scoped: false,
		doc: "Load a `.env` file. Encrypted values are decrypted in memory.",
	},
	KnownProperty {
		name: "hide",
		block_scoped: false,
		doc: "Keep this target out of `run :list`. It still runs.",
	},
	KnownProperty {
		name: "ignore-errors",
		block_scoped: true,
		doc: "Keep going when a command fails.",
	},
	KnownProperty {
		name: "only-in-directories",
		block_scoped: false,
		doc: "For `~/.runfiles/`: offer these targets only inside these directories.",
	},
	KnownProperty {
		name: "parallel",
		block_scoped: true,
		doc: "Run this block's commands at once, each branch labelled in the output.",
	},
	KnownProperty {
		name: "shell",
		block_scoped: true,
		doc: "Which shell `$` lines use.",
	},
	KnownProperty {
		name: "watch",
		block_scoped: false,
		doc: "Re-run whenever a matching file changes. A `!` prefix excludes.",
	},
	KnownProperty {
		name: "workdir",
		block_scoped: true,
		doc: "Where commands run, relative to the runfiles parent.",
	},
];

#[derive(Debug, thiserror::Error)]
pub enum PropError {
	#[error("line {line}: unknown property `.{name}`")]
	Unknown { name: String, line: usize },
	#[error("line {line}: `.{name}` is header-only and cannot be set inside a block")]
	NotBlockScoped { name: String, line: usize },
	#[error("line {line}: `.{name}` needs a value")]
	NeedsValue { name: String, line: usize },
	#[error(transparent)]
	Eval(#[from] EvalError),
}

impl Props {
	/// Layer a block's own properties over the inherited set.
	pub fn extend(&self, block: &Block, sc: &mut Scope, nested: bool) -> Result<Props, PropError> {
		let mut out = self.clone();
		// A nested block inherits behaviour but never a parent's one-shot header
		// state -- a confirm must not fire again per loop iteration.
		if nested {
			out.confirm = None;
			out.watch.clear();
			out.aliases.clear();
			out.hide = false;
			out.detach = false;
		}
		for p in &block.properties {
			out.apply(p, sc, nested)?;
		}
		Ok(out)
	}

	fn apply(&mut self, p: &Property, sc: &mut Scope, nested: bool) -> Result<(), PropError> {
		let head = p.path[0].as_str();
		let line = p.span.line;
		if nested && !BLOCK_SCOPED.contains(&head) {
			return Err(PropError::NotBlockScoped {
				name: p.path.join("."),
				line,
			});
		}
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
				Some(e) => Ok(matches!(eval_boundary(e, sc)?, Value::Bool(true))),
			}
		};
		match head {
			"shell" => self.shell = Some(value(p, sc)?.to_string()),
			"parallel" => self.parallel = flag(p, sc)?,
			"ignore-errors" => self.ignore_errors = flag(p, sc)?,
			"workdir" => self.workdir = Some(value(p, sc)?.to_string()),
			"hide" => self.hide = flag(p, sc)?,
			"detach" => self.detach = flag(p, sc)?,
			"confirm" => self.confirm = Some(value(p, sc)?.to_string()),
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
			"alias" => push_all(&mut self.aliases, value(p, sc)?),
			"only-in-directories" => push_all(&mut self.only_in_directories, value(p, sc)?),
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
