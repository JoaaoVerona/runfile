//! Running a named target, and the `run x` dispatch between them.
//!
//! Dispatch is in-process, so cycle detection is an ordinary stack rather than
//! an env-var protocol between re-execed binaries. Writing `$ run x` instead
//! is just a shell line and re-execs, which is the escape hatch when a target
//! genuinely wants a fresh process.

use crate::props::Props;
use crate::run::{Dispatch, RunError, Runner};
use runfile_discovery::Catalog;
use runfile_lang::Value;
use runfile_lang::eval::Scope;
use std::path::Path;
use std::sync::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum HostError {
	#[error("no target named `{name}`\n\ndid you mean one of: {near}")]
	Unknown { name: String, near: String },
	#[error("could not read {path}: {source}")]
	Read { path: String, source: std::io::Error },
	#[error("{path}: {source}")]
	Parse {
		path: String,
		source: runfile_lang::ParseError,
	},
	#[error("`{name}` is already running: {chain}")]
	Cycle { name: String, chain: String },
}

pub struct Host<'a> {
	pub catalog: &'a Catalog,
	pub assume_yes: bool,
	pub dry_run: bool,
	/// Asked for an input a target needs but was not given, under
	/// `--stdin-args`. A function pointer rather than a closure so it can cross
	/// a `.parallel` fan-out.
	pub ask: Option<fn(&str, &str) -> Option<String>>,
	/// Where `decrypt` gets its keys. Injected so the runtime never reaches
	/// into a credential store itself.
	pub keys: fn() -> Vec<String>,
	/// `Sync` so a `.parallel` fan-out can ask; a prompt during one is the
	/// caller's problem to serialise.
	pub prompt: Option<&'a (dyn Fn(&str) -> bool + Sync)>,
	/// Where non-fatal advice goes. The runtime never prints, so the CLI
	/// decides what a warning looks like and tests can capture it.
	pub warn: Option<&'a (dyn Fn(&str) + Sync)>,
	/// Every shell body that ran. Order is arrival order, which under
	/// `.parallel` is completion order rather than source order.
	pub trace: Mutex<Vec<String>>,
}

impl<'a> Host<'a> {
	pub fn new(catalog: &'a Catalog) -> Self {
		Self {
			catalog,
			assume_yes: false,
			dry_run: false,
			ask: None,
			keys: Vec::new,
			prompt: None,
			warn: None,
			trace: Mutex::new(Vec::new()),
		}
	}

	pub fn run(&self, name: &str, args: &[String]) -> Result<(), RunError> {
		// Only the top-level call banks its trace; nested ones hand theirs back
		// so the caller can splice them in where the call appeared.
		let trace = self.run_with_chain(name, args, &[])?;
		self.trace.lock().expect("trace lock").extend(trace);
		Ok(())
	}

	fn run_with_chain(&self, name: &str, args: &[String], chain: &[String]) -> Result<Vec<String>, RunError> {
		// A subproject is self-contained: `run compile` inside `web/runfiles/`
		// means that directory's `compile`, whatever the root calls it. Without
		// this a file would have to spell its own siblings' names differently
		// depending on where `run` was invoked from.
		let qualified = chain
			.last()
			.and_then(|caller| caller.rsplit_once(':'))
			.map(|(prefix, _)| format!("{prefix}:{name}"))
			.filter(|q| self.catalog.resolve(q).is_some());
		let name = qualified.as_deref().unwrap_or(name);

		let target = self.catalog.resolve(name).ok_or_else(|| {
			let near: Vec<&str> = self
				.catalog
				.targets
				.keys()
				.filter(|k| k.contains(name) || name.contains(k.as_str()))
				.map(String::as_str)
				.take(5)
				.collect();
			RunError::Host(Box::new(HostError::Unknown {
				name: name.to_string(),
				near: if near.is_empty() {
					"run :list".into()
				} else {
					near.join(", ")
				},
			}))
		})?;

		if chain.iter().any(|c| c == name) {
			let mut shown = chain.to_vec();
			shown.push(name.to_string());
			return Err(RunError::Host(Box::new(HostError::Cycle {
				name: name.to_string(),
				chain: shown.join(" -> "),
			})));
		}
		let mut next = chain.to_vec();
		next.push(name.to_string());
		self.run_inner(target, args, next)
	}

	/// Everything both `run_inner` and `header_props` need: a scope with the
	/// run context and arguments in place, plus `_shared.run` already folded in.
	///
	/// `advise` emits the unread-input warning. Only a real run wants it --
	/// `header_props` is called ahead of one, and warning twice would teach
	/// people to ignore it.
	fn prepare(
		&self,
		target: &runfile_discovery::Target,
		args: &[String],
		advise: bool,
	) -> Result<(runfile_lang::Target, Scope, Props), RunError> {
		let (ast, mut text) = parse_file(&target.path)?;
		let mut scope = Scope::new();
		populate_run_context(&mut scope, target, self.catalog);
		parse_args(&mut scope, args);
		scope.ask = self.ask;
		scope.base_dir = target.anchor.clone();
		scope.dry_run = self.dry_run;
		scope.private_keys = runfile_lang::Keys::new(self.keys);

		// `_shared.run` is the globals analog: its properties and bindings apply
		// to every target in the directory, so it is evaluated first into the
		// same scope.
		let shared_props = match self.catalog.shared_for(target) {
			Some(p) if p.is_file() => {
				let (shared, shared_text) = parse_file(&p)?;
				let props = Props::default().extend(&shared.body, &mut scope, false)?;
				crate::run::run_block_bindings(&shared.body, &mut scope)?;
				// A flag the shared file reads is read for every target.
				text.push_str(&shared_text);
				props
			}
			_ => Props::default(),
		};

		if advise && let Some(warn) = self.warn {
			for unread in unread_inputs(&scope, &text) {
				let (_, key) = unread.split_at(2);
				warn(&format!(
					"`{unread}` was passed to `{}`, which never reads `FLAG.{key}` or `ARG.{key}`; \
					 if it is meant for the command `{}` runs, put `--` before it",
					target.name, target.name,
				));
			}
		}
		Ok((ast, scope, shared_props))
	}

	/// Resolve a target's declaration-region properties without running it.
	///
	/// Watch mode needs `.watch` before the first execution, and the patterns
	/// interpolate, so reading them off the source text would not do.
	pub fn header_props(&self, target: &runfile_discovery::Target, args: &[String]) -> Result<Props, RunError> {
		let (ast, mut scope, shared) = self.prepare(target, args, false)?;
		Ok(shared.extend(&ast.body, &mut scope, false)?)
	}

	fn run_inner(
		&self,
		target: &runfile_discovery::Target,
		args: &[String],
		chain: Vec<String>,
	) -> Result<Vec<String>, RunError> {
		let (ast, scope, shared_props) = self.prepare(target, args, true)?;

		let adapter = HostDispatch { host: self };
		let mut r = Runner {
			scope,
			chain,
			env: Vec::new(),
			anchor: target.anchor.clone(),
			dispatch: &adapter,
			dry_run: self.dry_run,
			assume_yes: self.assume_yes,
			prompt: self.prompt,
			trace: Vec::new(),
		};
		crate::run::run_target_with(&ast, shared_props, &mut r)?;
		Ok(r.trace)
	}
}

struct HostDispatch<'a, 'b> {
	host: &'a Host<'b>,
}

impl Dispatch for HostDispatch<'_, '_> {
	fn run(&self, target: &str, args: &[String], chain: &[String]) -> Result<Vec<String>, RunError> {
		self.host.run_with_chain(target, args, chain)
	}
}

/// Parse a target file, keeping its text: the unread-input check is textual.
fn parse_file(p: &Path) -> Result<(runfile_lang::Target, String), RunError> {
	let src = std::fs::read_to_string(p).map_err(|e| {
		RunError::Host(Box::new(HostError::Read {
			path: p.display().to_string(),
			source: e,
		}))
	})?;
	let ast = runfile_lang::parse(&src).map_err(|e| {
		RunError::Host(Box::new(HostError::Parse {
			path: p.display().to_string(),
			source: e,
		}))
	})?;
	Ok((ast, src))
}

/// `RUN.*`: the one place runtime context lives. `RUN.namespaces` is
/// list-valued, which is what `for ns in RUN.namespaces` iterates.
fn populate_run_context(sc: &mut Scope, t: &runfile_discovery::Target, cat: &Catalog) {
	sc.run.insert("os".into(), Value::Str(os_name().into()));
	sc.run.insert("arch".into(), Value::Str(arch_name().into()));
	sc.run.insert("file".into(), Value::Str(t.path.display().to_string()));
	sc.run
		.insert("parent".into(), Value::Str(t.anchor.display().to_string()));
	if let Ok(cwd) = std::env::current_dir() {
		sc.run.insert("cwd".into(), Value::Str(cwd.display().to_string()));
	}
	let mut ns: Vec<String> = cat
		.targets
		.values()
		.filter(|x| x.origin == runfile_discovery::Origin::Included)
		.filter_map(|x| x.name.split(':').next().map(str::to_string))
		.collect();
	ns.sort();
	ns.dedup();
	sc.run.insert(
		"namespaces".into(),
		Value::List(ns.into_iter().map(Value::Str).collect()),
	);
	sc.env = std::env::vars().collect();
}

/// `--key=value` is an argument, `--key` a flag, anything else a positional.
///
/// A bare `--` ends parsing: everything after it is a positional exactly as
/// typed, flags included. That is how a wrapper forwards a command line it
/// does not understand -- `run _aws -- s3api --bucket X` -- without the
/// runner claiming `--bucket` for itself.
fn parse_args(sc: &mut Scope, args: &[String]) {
	let mut passthrough = false;
	for a in args {
		if passthrough {
			sc.positional.push(a.clone());
		} else if a == "--" {
			passthrough = true;
		} else if let Some(rest) = a.strip_prefix("--") {
			match rest.split_once('=') {
				Some((k, v)) => {
					sc.args.insert(k.to_string(), v.to_string());
				}
				None => sc.flags.push(rest.to_string()),
			}
		} else {
			sc.positional.push(a.clone());
		}
	}
}

/// Flags and arguments the target was given but never reads.
///
/// A wrapper that forwards `{{ ARGS }}` drops any `--flag` handed to it
/// without a `--` before it, silently and with no error -- the command that
/// runs is simply wrong. This is the check that makes it not silent. Textual
/// rather than an AST walk, because `FLAG.x` and `ARG.x` are literal keys with
/// no dynamic form; a mention inside a comment suppresses the warning, which
/// is a harmless way for a heuristic to be wrong.
fn unread_inputs(sc: &Scope, text: &str) -> Vec<String> {
	let mut names: Vec<&String> = sc.flags.iter().chain(sc.args.keys()).collect();
	names.sort();
	names
		.into_iter()
		.filter(|k| !k.is_empty())
		.filter(|k| !text.contains(&format!("FLAG.{k}")) && !text.contains(&format!("ARG.{k}")))
		.map(|k| format!("--{k}"))
		.collect()
}

fn os_name() -> &'static str {
	match std::env::consts::OS {
		"macos" => "mac",
		"windows" => "windows",
		_ => "linux",
	}
}

fn arch_name() -> &'static str {
	match std::env::consts::ARCH {
		"x86_64" => "x86-64",
		"aarch64" => "arm64",
		"riscv64" => "riscv64",
		_ => "unknown",
	}
}
