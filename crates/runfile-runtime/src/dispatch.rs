//! Running a named target, and the `run x` dispatch between them.
//!
//! Dispatch is in-process, so cycle detection is an ordinary stack rather than
//! an env-var protocol between re-execed binaries. Writing `$ run x` instead
//! is just a shell line and re-execs, which is the escape hatch when a target
//! genuinely wants a fresh process.

use crate::props::Props;
use crate::run::{Dispatch, RunError, Runner};
use runfile_discovery::Catalog;
use runfile_lang::eval::Scope;
use runfile_lang::{Arg, Value};
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
	/// A file names something nothing defines: a function the language does
	/// not have, or a binding no line before the read can have made. Refused
	/// before any of the target runs, and every one of them at once -- an
	/// editor underlines them all in one go, and a run that reports one per
	/// attempt is the slow way to learn the same list.
	#[error("{}", unresolved(path, problems))]
	Unresolved {
		path: String,
		problems: Vec<runfile_lang::Unresolved>,
	},
	/// A file does what its author cannot have meant, every time it runs: a
	/// value of a type the place it reaches refuses, a call given too many
	/// arguments, a comparison that is always false -- or shell that bash
	/// refuses to read, or quoting that breaks on the first value with a space
	/// in it. Refused before any of the target runs, every finding at once, the
	/// way a name nothing defines is.
	#[error("{}", findings_of(path, findings))]
	Findings {
		path: String,
		findings: Vec<runfile_lang::check::Finding>,
	},
}

/// One problem to a line, each naming its file. The first line is the error the
/// caller prints after its own prefix; every line after it carries that prefix
/// too, since everything the runner says while a target runs does.
fn unresolved(path: &str, problems: &[runfile_lang::Unresolved]) -> String {
	let next = format!("\n{} error: ", crate::exec::tag());
	problems
		.iter()
		.map(|u| format!("{path}: {u}"))
		.collect::<Vec<_>>()
		.join(&next)
}

/// One finding to a line, as [`unresolved`] reports a name.
fn findings_of(path: &str, findings: &[runfile_lang::check::Finding]) -> String {
	let next = format!("\n{} error: ", crate::exec::tag());
	findings
		.iter()
		.map(|f| format!("{path}: {f}"))
		.collect::<Vec<_>>()
		.join(&next)
}

pub struct Host<'a> {
	pub catalog: &'a Catalog,
	pub assume_yes: bool,
	pub dry_run: bool,
	/// Asked for an input a target needs but was not given, under
	/// `--stdin-args`. A function pointer rather than a closure so it can cross
	/// into a parallel branch -- which then drops it, since a branch shares the
	/// terminal with its siblings and asks it nothing.
	pub ask: Option<fn(&str, &str) -> Option<String>>,
	/// Where `decrypt` gets its keys. Injected so the runtime never reaches
	/// into a credential store itself.
	pub keys: fn() -> Vec<String>,
	/// Asked by `confirm(…)`. Never from inside a parallel branch: every branch
	/// shares one terminal, so `confirm()` refuses there rather than asking two
	/// questions at once.
	pub confirm: Option<fn(&str) -> bool>,
	/// Whether the run has been interrupted; see `Runner::interrupted`.
	pub interrupted: Option<&'a (dyn Fn() -> bool + Sync)>,
	/// Every shell body that ran, in arrival order. A parallel block's branches
	/// are spliced in by source order once they have all finished.
	pub trace: Mutex<Vec<String>>,
	/// What `temp_file` and `temp_dir` created during this run.
	pub temps: runfile_lang::TempFiles,
}

impl<'a> Host<'a> {
	pub fn new(catalog: &'a Catalog) -> Self {
		Self {
			catalog,
			assume_yes: false,
			dry_run: false,
			ask: None,
			keys: Vec::new,
			confirm: None,
			interrupted: None,
			trace: Mutex::new(Vec::new()),
			temps: runfile_lang::TempFiles::default(),
		}
	}

	/// Delete everything `temp_file` and `temp_dir` made, and forget it.
	///
	/// Best effort, and called however the run ended: a target that fails
	/// half-way is exactly when a decoded credential must not be left behind.
	/// Idempotent, so watch mode can call it after every iteration.
	pub fn cleanup_temps(&self) {
		for p in self.temps.take() {
			let _ = if p.is_dir() {
				std::fs::remove_dir_all(&p)
			} else {
				std::fs::remove_file(&p)
			};
		}
	}

	pub fn run(&self, name: &str, args: &[String]) -> Result<(), RunError> {
		// Only the top-level call banks its trace; nested ones hand theirs back
		// so the caller can splice them in where the call appeared.
		let trace = self.run_with_chain(name, args, &[], None)?;
		self.trace.lock().expect("trace lock").extend(trace);
		Ok(())
	}

	fn run_with_chain(
		&self,
		name: &str,
		args: &[String],
		chain: &[String],
		label: Option<&str>,
	) -> Result<Vec<String>, RunError> {
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
		self.run_inner(target, args, next, label)
	}

	/// Everything both `run_inner` and `header_props` need: a scope with the
	/// run context and arguments in place, plus `_shared.run` already folded in.
	///
	/// `real` separates an actual run from a probe. `header_props` probes: it
	/// evaluates the declaration region only to read `.watch`, so it must
	/// neither refuse an unread input -- a probe rejecting the command line
	/// would report the failure before the run that owns it -- nor let a
	/// writing function write. Without the second half, `.env.X =
	/// temp_file(...)` created two files per run, one of them an orphan
	/// nothing referenced.
	fn prepare(
		&self,
		target: &runfile_discovery::Target,
		args: &[String],
		real: bool,
	) -> Result<(runfile_lang::Target, Scope, Props), RunError> {
		let (ast, shared) = self.load(target)?;

		// The whole `_shared.run` chain is walked for what it reads *before* the
		// command line is classified: a name only a shared file reads is still a
		// name this target takes a value for. It cannot simply be folded in where
		// it used to be, because evaluating those files reads `ARG.x` out of the
		// scope -- so the parse and the evaluation are two passes over one list.
		let reads = runfile_lang::inputs::of_chain(&ast, &shared);

		let mut scope = Scope::new();
		populate_run_context(&mut scope, target, self.catalog);
		let unknown = parse_args(&mut scope, args, &reads).map_err(|e| RunError::MissingArgValue {
			key: e.key,
			next: e.next,
			target: target.name.clone(),
		})?;
		scope.ask = self.ask;
		scope.confirm = self.confirm;
		scope.assume_yes = self.assume_yes;
		scope.base_dir = target.anchor.clone();
		scope.dry_run = self.dry_run || !real;
		scope.temps = self.temps.clone();
		scope.private_keys = runfile_lang::Keys::new(self.keys);

		// An input the target does not read is a mistake, and used to be a
		// warning only because the check was textual guesswork. Walked from the
		// tree it is exact, so it says no -- a mistyped `--forse` that merely
		// warns is a flag that did not take effect, discovered later. A target
		// reading `ARGS` is the exception and is handled in `parse_args`: it can
		// read the word, so it gets it.
		if real && let Some((name, key)) = unknown {
			return Err(RunError::UnknownInput {
				name,
				key,
				target: target.name.clone(),
				reads: Box::new(reads),
			});
		}

		// `_shared.run` is the globals analog: its properties and bindings apply
		// to every target in the directory, so they are evaluated first into the
		// same scope. Outermost first, so a nested directory's settings layer
		// over the one above it.
		let mut shared_props = Props {
			// Where the file *lives*, carried in so the one property that
			// depends on it can say no. It survives every `extend`, which
			// clones, so a `_shared.run` in the machine-wide directory may set
			// it too.
			//
			// Asked of the path rather than of `Origin`, which answers a
			// different question -- how discovery *reached* the file. The two
			// part company whenever the machine-wide directory is also the
			// nearest one: `$HOME/runfiles` found by the upward walk is
			// collected once, as `Local`, so standing in `$HOME` made every
			// machine-wide target refuse its own scope. It is also the
			// function the language server asks, and one question with two
			// answers is how an editor and the runner come to disagree.
			machine_wide: runfile_discovery::is_machine_wide(&target.path),
			..Props::default()
		};
		for s in &shared {
			shared_props = crate::run::fold_shared(&s.body, &shared_props, &mut scope)?;
		}
		Ok((ast, scope, shared_props))
	}

	/// A target's tree, and the trees of the `_shared.run` files above it,
	/// outermost first -- refused when any of them names something nothing
	/// defines, or holds shell that `runfile_shell` finds wrong.
	///
	/// Checked before a scope exists, so nothing a broken file does happens
	/// first: not a command above the line that is wrong, and not a shared
	/// file's `let` evaluated on its behalf. A shared file is checked ahead of
	/// the target, since whatever is wrong there is wrong for every target below
	/// it.
	fn load(&self, target: &runfile_discovery::Target) -> Result<Loaded, RunError> {
		let (ast, src) = parse_file(&target.path)?;
		let chain = self.catalog.shared_chain(target);
		let (shared, texts): (Vec<runfile_lang::Target>, Vec<String>) = chain
			.iter()
			.map(|p| parse_file(p))
			.collect::<Result<Vec<_>, _>>()?
			.into_iter()
			.unzip();
		for (i, (path, file)) in chain.iter().zip(&shared).enumerate() {
			refuse_unresolved(path, runfile_lang::resolve::of_shared(file, &shared[..i]))?;
			refuse_findings(path, findings(&texts[i], file, &shared[..i], true))?;
		}
		refuse_unresolved(&target.path, runfile_lang::resolve::of_chain(&ast, &shared))?;
		refuse_findings(&target.path, findings(&src, &ast, &shared, false))?;
		Ok((ast, shared))
	}

	/// Whether a target would be let run at all: it parses, and so does its
	/// chain, every name they use resolves, and their shell holds nothing the
	/// shell checker finds.
	///
	/// `run --stdin-args` asks this before it asks anything, so nobody answers
	/// questions about a file that is refused the moment they finish.
	pub fn check(&self, target: &runfile_discovery::Target) -> Result<(), RunError> {
		self.load(target).map(|_| ())
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
		label: Option<&str>,
	) -> Result<Vec<String>, RunError> {
		let (ast, mut scope, shared_props) = self.prepare(target, args, true)?;
		// Dispatched from a parallel branch, it is part of that branch: what it
		// prints carries the branch's label, and it asks the terminal nothing.
		if let Some(l) = label {
			scope.branch = Some(l.to_string());
			scope.ask = None;
		}

		let adapter = HostDispatch { host: self };
		let mut r = Runner {
			scope,
			chain,
			env: Vec::new(),
			anchor: target.anchor.clone(),
			dispatch: &adapter,
			dry_run: self.dry_run,

			interrupted: self.interrupted,
			label: label.map(str::to_string),
			colour: None,
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
	fn run(
		&self,
		target: &str,
		args: &[String],
		chain: &[String],
		label: Option<&str>,
	) -> Result<Vec<String>, RunError> {
		self.host.run_with_chain(target, args, chain, label)
	}
}

/// A target's tree and its chain's, as [`Host::load`] hands them back.
type Loaded = (runfile_lang::Target, Vec<runfile_lang::Target>);

/// Refuse a file for the names in it that resolve to nothing, when there are any.
fn refuse_unresolved(path: &Path, problems: Vec<runfile_lang::Unresolved>) -> Result<(), RunError> {
	if problems.is_empty() {
		return Ok(());
	}
	Err(RunError::Host(Box::new(HostError::Unresolved {
		path: path.display().to_string(),
		problems,
	})))
}

/// Everything the language's checker and the shell checker find in one file,
/// in the order it is written.
fn findings(
	src: &str,
	file: &runfile_lang::Target,
	chain: &[runfile_lang::Target],
	shared: bool,
) -> Vec<runfile_lang::check::Finding> {
	let mut all = runfile_lang::check::check(src, file, Some(chain), shared);
	all.extend(runfile_shell::check(src, file, Some(chain)));
	all.sort_by_key(|f| (f.span.start, f.span.end));
	all
}

/// Refuse a file for what is wrong with it every time it runs, when anything is.
fn refuse_findings(path: &Path, findings: Vec<runfile_lang::check::Finding>) -> Result<(), RunError> {
	if findings.is_empty() {
		return Ok(());
	}
	Err(RunError::Host(Box::new(HostError::Findings {
		path: path.display().to_string(),
		findings,
	})))
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
pub(crate) fn populate_run_context(sc: &mut Scope, t: &runfile_discovery::Target, cat: &Catalog) {
	sc.run.insert("os".into(), Value::Str(os_name().into()));
	sc.run.insert("arch".into(), Value::Str(arch_name().into()));
	sc.run.insert("file".into(), Value::Str(t.path.display().to_string()));
	sc.run
		.insert("parent".into(), Value::Str(t.anchor.display().to_string()));
	// Whoever is running this, which a target otherwise has to shell out for:
	// `id -un` on Unix, and a different variable on each platform.
	if let Some(u) = std::env::var_os("USER")
		.or_else(|| std::env::var_os("USERNAME"))
		.or_else(|| std::env::var_os("LOGNAME"))
	{
		sc.run
			.insert("user".into(), Value::Str(u.to_string_lossy().into_owned()));
	}
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

/// Apply a command line to the scope, answering the first word the target
/// reads under no name.
///
/// The classification itself is `runfile_lang::args::parse`, which the
/// `--stdin-args` prompt reads from too, so the two cannot disagree about
/// whether `--target aarch64` supplied `ARG.target`.
///
/// An unknown `--key` reaches `ARGS` when the target reads `ARGS`: a wrapper
/// can read the word, and forwarding is what it was written to do -- which is
/// what makes `run build --target aarch64` reach `cargo` with no `--` in front
/// of it. When the target reads no positionals there is nowhere for the word
/// to go and nothing to forward it to, so it stays the error it has always
/// been, and a mistyped `--forse` is still caught.
///
/// The unknown is answered rather than raised here, because whether it is an
/// error depends on `real`: a probe evaluates the declaration region to read
/// `.watch` and must not refuse. It is the first in **command-line order**,
/// which is where a reader will look for it.
fn parse_args(
	sc: &mut Scope,
	args: &[String],
	reads: &runfile_lang::Inputs,
) -> Result<Option<(String, String)>, runfile_lang::args::MissingValue> {
	let mut unknown = None;
	for a in runfile_lang::args::parse(args, reads)? {
		match a {
			Arg::Arg { key, value } => {
				sc.args.insert(key, value);
			}
			Arg::Flag(key) => sc.flags.push(key),
			Arg::Positional(v) => sc.positional.push(v),
			Arg::Unknown { token, .. } if reads.positional => sc.positional.push(token),
			Arg::Unknown { token, key } => {
				unknown.get_or_insert((token, key));
			}
		}
	}
	Ok(unknown)
}

fn os_name() -> &'static str {
	os_named(std::env::consts::OS)
}

/// What `RUN.os` says on an OS Rust names `os`: one of
/// [`runfile_lang::eval::OS_NAMES`], which is what lets a check call a
/// comparison with anything else a mistake.
pub(crate) fn os_named(os: &str) -> &'static str {
	match os {
		"macos" => "mac",
		"windows" => "windows",
		_ => "linux",
	}
}

fn arch_name() -> &'static str {
	arch_named(std::env::consts::ARCH)
}

/// What `RUN.arch` says on a CPU Rust names `arch`: one of
/// [`runfile_lang::eval::ARCH_NAMES`].
pub(crate) fn arch_named(arch: &str) -> &'static str {
	match arch {
		"x86_64" => "x86-64",
		"aarch64" => "arm64",
		"riscv64" => "riscv64",
		_ => "unknown",
	}
}
