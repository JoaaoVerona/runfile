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
	/// `Sync` so a `.parallel` fan-out can ask; a prompt during one is the
	/// caller's problem to serialise.
	pub prompt: Option<&'a (dyn Fn(&str) -> bool + Sync)>,
	/// Every shell body that ran. Order is arrival order, which under
	/// `.parallel` is completion order rather than source order.
	pub trace: Mutex<Vec<String>>,
}

impl<'a> Host<'a> {
	pub fn new(catalog: &'a Catalog) -> Self {
		Self {
			catalog,
			assume_yes: false,
			prompt: None,
			trace: Mutex::new(Vec::new()),
		}
	}

	pub fn run(&self, name: &str, args: &[String]) -> Result<(), RunError> {
		self.run_with_chain(name, args, &[])
	}

	fn run_with_chain(&self, name: &str, args: &[String], chain: &[String]) -> Result<(), RunError> {
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

	fn run_inner(
		&self,
		target: &runfile_discovery::Target,
		args: &[String],
		chain: Vec<String>,
	) -> Result<(), RunError> {
		let ast = parse_file(&target.path)?;
		let mut scope = Scope::new();
		populate_run_context(&mut scope, target, self.catalog);
		parse_args(&mut scope, args);

		// `_shared.run` is the globals analog: its properties and bindings apply
		// to every target in the directory, so it is evaluated first into the
		// same scope.
		let shared_props = match self.catalog.shared_for(target) {
			Some(p) if p.is_file() => {
				let shared = parse_file(&p)?;
				let base = Props::default();
				let props = base.extend(&shared.body, &mut scope, false)?;
				crate::run::run_block_bindings(&shared.body, &mut scope)?;
				props
			}
			_ => Props::default(),
		};

		let adapter = HostDispatch { host: self };
		let mut r = Runner {
			scope,
			chain,
			env: Vec::new(),
			anchor: target.anchor.clone(),
			dispatch: &adapter,
			assume_yes: self.assume_yes,
			prompt: self.prompt,
			trace: Vec::new(),
		};
		let out = crate::run::run_target_with(&ast, shared_props, &mut r);
		self.trace.lock().expect("trace lock").extend(r.trace);
		out
	}
}

struct HostDispatch<'a, 'b> {
	host: &'a Host<'b>,
}

impl Dispatch for HostDispatch<'_, '_> {
	fn run(&self, target: &str, args: &[String], chain: &[String]) -> Result<(), RunError> {
		self.host.run_with_chain(target, args, chain)
	}
}

fn parse_file(p: &Path) -> Result<runfile_lang::Target, RunError> {
	let src = std::fs::read_to_string(p).map_err(|e| {
		RunError::Host(Box::new(HostError::Read {
			path: p.display().to_string(),
			source: e,
		}))
	})?;
	runfile_lang::parse(&src).map_err(|e| {
		RunError::Host(Box::new(HostError::Parse {
			path: p.display().to_string(),
			source: e,
		}))
	})
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

fn parse_args(sc: &mut Scope, args: &[String]) {
	for a in args {
		if let Some(rest) = a.strip_prefix("--") {
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
