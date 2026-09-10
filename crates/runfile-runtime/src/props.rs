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
	/// Start the commands and do not wait; see `Spawn::detach`.
	pub detach: bool,
	pub aliases: Vec<String>,
	/// Whether this target came from the machine-wide directory. Not a
	/// property: it is a fact about where the file was found, and the one
	/// thing that makes `.only-in-directories` mean anything.
	pub machine_wide: bool,
}

/// Properties a nested block may set. Everything else is header-only, because
/// the runner has to know it before any statement runs.
const BLOCK_SCOPED: &[&str] = &[
	"shell",
	"parallel",
	"ignore-errors",
	"logging",
	"workdir",
	"env",
	"env-file",
	"add-path",
];

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
	/// What it looks like in a file. A property is a line someone writes
	/// rather than a value they compute, so the useful thing to show is the
	/// line.
	pub example: &'static str,
}

pub const PROPERTIES: &[KnownProperty] = &[
	KnownProperty {
		name: "add-path",
		block_scoped: true,
		doc: "Prepend a directory to `PATH`, relative to the runfiles parent.",
		example: ".add-path = \"node_modules/.bin\"",
	},
	KnownProperty {
		name: "alias",
		block_scoped: false,
		doc: "Another name this target answers to. Carries the target's namespace.",
		example: ".alias = \"build:release\"\n\n# `run build:release` now reaches this file.",
	},
	KnownProperty {
		name: "detach",
		block_scoped: false,
		doc: "Start the commands and do not wait. For something meant to outlive the run.",
		example: ".detach = true\n\n$ cargo run --bin server",
	},
	KnownProperty {
		name: "env",
		block_scoped: true,
		doc: "Set an environment variable, addressed by sub-key: `.env.NAME = \"value\"`.",
		example: ".env.PORT = ARG.port ? \"3000\"\n.env.DATABASE_URL = \"postgres://localhost/app\"",
	},
	KnownProperty {
		name: "env-file",
		block_scoped: true,
		doc: "Load a `.env` file. Encrypted values are decrypted in memory.",
		example: ".env-file = \".env.{{ one_of(ARG.env, \\\"dev\\\", \\\"prod\\\") }}\"",
	},
	KnownProperty {
		name: "ignore-errors",
		block_scoped: true,
		doc: "Keep going when a command fails.",
		example: ".ignore-errors\n\n# Creating a volume that exists is an error worth ignoring.\n$ docker volume create app-data",
	},
	KnownProperty {
		name: "logging",
		block_scoped: true,
		doc: "Announce each command on stderr before it runs.",
		example: ".logging = true\n\n# Each command announces itself on stderr as it runs.",
	},
	KnownProperty {
		name: "only-in-directories",
		block_scoped: false,
		doc: "For the machine-wide directory: offer this target only inside these directories.",
		example: ".only-in-directories = [\"~/work/acme\", \"~/work/zed\"]",
	},
	KnownProperty {
		name: "parallel",
		block_scoped: true,
		doc: "Run this block's commands at once, each branch labelled in the output.",
		example: "for compose in glob(\"**/docker-compose.yml\")\n\t.parallel\n\n\t$ docker compose -f {{ compose }} pull\nend",
	},
	KnownProperty {
		name: "shell",
		block_scoped: true,
		doc: "Which shell `$` lines use.",
		example: ".shell = \"sh\"",
	},
	KnownProperty {
		name: "watch",
		block_scoped: false,
		doc: "Re-run whenever a matching file changes. A `!` prefix excludes.",
		example: ".watch = \"src/**/*.rs\"\n.watch = \"!src/generated/**\"",
	},
	KnownProperty {
		name: "workdir",
		block_scoped: true,
		doc: "Where commands run, relative to the runfiles parent.",
		example: ".workdir = \"web\"",
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
	#[error("line {line}: `.{name}` scopes the machine-wide directory; this target is part of the project")]
	NotMachineWide { name: String, line: usize },
	#[error(transparent)]
	Eval(#[from] EvalError),
}

impl Props {
	/// Layer a block's own properties over the inherited set.
	pub fn extend(&self, block: &Block, sc: &mut Scope, nested: bool) -> Result<Props, PropError> {
		let mut out = self.clone();
		// A nested block inherits behaviour but never a parent's one-shot header
		// state.
		if nested {
			out.watch.clear();
			out.aliases.clear();
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
		// Whether it exists comes first. `BLOCK_SCOPED` is a subset of the
		// known names, so asking about scope first told someone who typed
		// `.ignore-error` inside a `for` that it was header-only -- sending
		// them after a rule instead of a spelling mistake.
		if !PROPERTIES.iter().any(|k| k.name == head) {
			return Err(PropError::Unknown {
				name: p.path.join("."),
				line,
			});
		}
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
			"logging" => self.logging = flag(p, sc)?,
			"parallel" => self.parallel = flag(p, sc)?,
			"ignore-errors" => self.ignore_errors = flag(p, sc)?,
			"workdir" => self.workdir = Some(value(p, sc)?.to_string()),
			"detach" => self.detach = flag(p, sc)?,
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
