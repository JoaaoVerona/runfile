//! Running a named target, and the `run x` dispatch between them.
//!
//! Dispatch is in-process, so cycle detection is an ordinary stack rather than
//! an env-var protocol between re-execed binaries. Writing `$ run x` instead
//! is just a shell line and re-execs, which is the escape hatch when a target
//! genuinely wants a fresh process.

use crate::props::Props;
use crate::run::{Dispatch, RunError, Runner};
use runfile_discovery::Catalog;
use runfile_lang::args::Word;
use runfile_lang::eval::Scope;
use runfile_lang::reference::{self, ENTRY, Reference};
use runfile_lang::{Arg, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// What separates a package from the name of one of its targets in a call
/// chain. No folder and no target name can hold it.
const IN_PACKAGE: char = '\u{0}';

/// A package a run has reached: a repository fetched into the cache, or a
/// folder on disk, with a `runfiles/_main.run`.
///
/// It is a catalog of its own. Its `run x` means its own `x` -- the way a
/// subproject's means its sibling -- and never the caller's, which it cannot
/// know about.
pub struct Package {
	/// The reference as it reads once parsed: `host/repo@v1.0.0`, or the path
	/// as written. What a message names it by.
	pub shown: String,
	/// The folder holding its `runfiles/`: `RUN.parent`, and what tells it
	/// apart from every other package in the run.
	pub root: PathBuf,
	pub catalog: Catalog,
	/// The tag it was fetched at, which is `RUN.version`; empty otherwise.
	pub version: String,
}

impl Package {
	/// `runfiles/_main.run`, which is what running the package runs.
	pub fn entry(&self) -> Option<&runfile_discovery::Target> {
		self.catalog.resolve(ENTRY)
	}

	fn key(&self) -> String {
		self.root.display().to_string()
	}
}

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
	/// What [`Host::on_header`] said no with.
	#[error("{0}")]
	Header(String),
	/// A package that cannot be run: a reference that cannot be fetched, a
	/// fetch that failed, or a folder with no `runfiles/_main.run`.
	#[error("{0}")]
	Package(String),
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

/// Cleans a [`Host`]'s temp files on drop, so an unwinding panic cannot leave
/// a decrypted one behind. See [`Host::temp_guard`].
pub struct TempGuard<'a>(&'a Host<'a>);

impl Drop for TempGuard<'_> {
	fn drop(&mut self) {
		self.0.cleanup_temps();
	}
}

/// What [`Host::on_header`] is: handed a target's properties, and able to say no.
pub type OnHeader<'a> = dyn Fn(&Props) -> Result<(), String> + Sync + 'a;

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
	/// into a credential store itself, and asked at most once in a run however
	/// many of its targets decrypt -- see `key_pool`.
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
	/// Handed the properties of the target a run was asked for, once its
	/// `_shared.run` chain has run and its header is worked out, and before its
	/// first line: what the watch loop learns `.watch` from.
	///
	/// From the run itself rather than from a probe ahead of it, because a
	/// `.watch` may read what the chain set, and the chain is lines that run --
	/// a probe would have had to run them twice. An error it answers with ends
	/// the run, as `HostError::Header`.
	pub on_header: Option<&'a OnHeader<'a>>,
	/// Where packages are fetched to and `download()` keeps what it fetches.
	/// `None` leaves both unavailable, saying so.
	pub cache: Option<runfile_remote::Cache>,
	/// The key pool of the run in progress, once something in it has needed one.
	pool: Mutex<Option<runfile_lang::Keys>>,
	/// Every package the host has reached, by the reference that reached it --
	/// so a package named twice in a run is asked about once.
	packages: Mutex<HashMap<String, Arc<Package>>>,
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
			on_header: None,
			cache: None,
			pool: Mutex::new(None),
			packages: Mutex::new(HashMap::new()),
		}
	}

	/// The package a reference names, fetched into the cache if it is not
	/// there, and read.
	///
	/// A path is relative to where the line naming it was (`env`'s working
	/// directory), or to the process's from the command line. A remote one is
	/// fetched only when it has to be, and never under `--dry-run`, whose
	/// preview reads what the cache already holds.
	pub fn package(&self, word: &str, env: Option<&crate::env::Inherited>) -> Result<Arc<Package>, RunError> {
		let refused = |m: String| RunError::Host(Box::new(HostError::Package(m)));
		let reference = reference::parse(word).map_err(|e| refused(e.0))?;
		let (memo, shown) = match &reference {
			Reference::Path(p) => {
				let base = env
					.and_then(|e| e.workdir.clone())
					.or_else(|| std::env::current_dir().ok())
					.unwrap_or_default();
				let dir = base.join(p);
				let dir = std::fs::canonicalize(&dir).map(plain).map_err(|_| {
					let shown: PathBuf = dir
						.components()
						.filter(|c| *c != std::path::Component::CurDir)
						.collect();
					refused(format!(
						"`{word}` is not a package: {} does not exist",
						crate::term::sanitize(&shown.display().to_string())
					))
				})?;
				(dir.display().to_string(), word.to_string())
			}
			Reference::Remote(r) => (r.shown(), r.shown()),
		};
		if let Some(p) = self.packages.lock().expect("packages lock").get(&memo) {
			return Ok(p.clone());
		}
		let (root, version) = match reference {
			Reference::Path(_) => (PathBuf::from(&memo), String::new()),
			Reference::Remote(r) => {
				let Some(cache) = &self.cache else {
					return Err(refused(format!(
						"there is no cache directory to fetch `{shown}` into; set {} to one",
						runfile_remote::CACHE_DIR_ENV_VAR
					)));
				};
				let network = if self.dry_run {
					runfile_remote::Network::Offline
				} else {
					runfile_remote::Network::Allowed
				};
				let fetched = cache.package(&r, network, &say).map_err(|e| refused(e.0))?;
				(fetched.root, fetched.version)
			}
		};
		let entry = root.join("runfiles").join(format!("{ENTRY}.run"));
		if !std::fs::metadata(&entry).is_ok_and(|m| m.is_file()) {
			return Err(refused(format!(
				"`{shown}` is not a package: there is no `runfiles/{ENTRY}.run` in {}",
				crate::term::sanitize(&root.display().to_string())
			)));
		}
		// Discovered from its own folder and nowhere else: no machine-wide
		// directory joins it, since its targets are its author's and only its own.
		let catalog = runfile_discovery::discover(&root, None).map_err(|e| refused(format!("`{shown}`: {e}")))?;
		let pkg = Arc::new(Package {
			shown,
			root,
			catalog,
			version,
		});
		Ok(self
			.packages
			.lock()
			.expect("packages lock")
			.entry(memo)
			.or_insert(pkg)
			.clone())
	}

	/// The package a call-chain entry belongs to, when it belongs to one.
	fn package_of(&self, entry: &str) -> Option<Arc<Package>> {
		let (key, _) = entry.split_once(IN_PACKAGE)?;
		self.packages
			.lock()
			.expect("packages lock")
			.values()
			.find(|p| p.key() == key)
			.cloned()
	}

	/// A call-chain entry as a person reads it: a package's entry is the
	/// package, and another of its targets is the package and the target.
	fn shown(&self, entry: &str) -> String {
		match (entry.split_once(IN_PACKAGE), self.package_of(entry)) {
			(Some((_, ENTRY)), Some(p)) => p.shown.clone(),
			(Some((_, name)), Some(p)) => format!("{} {name}", p.shown),
			_ => entry.to_string(),
		}
	}

	/// What `download()` fetches through, when there is a cache to keep what
	/// it fetches.
	fn downloader(&self) -> Option<runfile_lang::eval::Downloader> {
		let cache = self.cache.clone()?;
		Some(Arc::new(move |url: &str, pin: Option<&str>| {
			cache
				.download(url, pin, &say)
				.map(|p| plain(p).display().to_string())
				.map_err(|e| e.0)
		}))
	}

	/// The key pool every target in the run in progress decrypts from.
	///
	/// One per run rather than one per target. `Keys` memoizes behind a cache its
	/// clones share, so every clone handed out here -- to each target a `run`
	/// dispatches, and through `fork` to every parallel branch -- asks the
	/// credential store once between them. A `Keys::new` for each target asked
	/// once per target: two targets reading one encrypted `.env` file, through
	/// the `_shared.run` above both, unlocked the keyring twice in one run.
	fn key_pool(&self) -> runfile_lang::Keys {
		self.pool
			.lock()
			.expect("key pool lock")
			.get_or_insert_with(|| runfile_lang::Keys::new(self.keys))
			.clone()
	}

	/// A guard whose `Drop` runs [`Host::cleanup_temps`], so a **panic** cannot
	/// leave a decrypted temp file behind.
	///
	/// `cleanup_temps` is called explicitly on the normal path, but nothing
	/// catches an unwind -- the release profile does not `panic = "abort"` -- so
	/// a panic in the walker (a built-in given a value it could not hold, once)
	/// skipped it. Holding one of these across `host.run` closes that: cleanup
	/// is idempotent (`take` empties the list), so the explicit call and the
	/// drop do not conflict.
	pub fn temp_guard(&self) -> TempGuard<'_> {
		TempGuard(self)
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
		// Only the top-level call banks its trace; nested ones write into their
		// caller's, where the call appeared. Nothing ran it, so it is run with
		// the process's own environment.
		let mut trace = Vec::new();
		let out = self.run_with_chain(name, &literal(args), None, &[], None, &mut trace);
		// The run is over however it ended, and its key pool with it, so the next
		// run on this host -- a watch iteration -- asks again. A pool that outlived
		// its run would keep what it failed to load, from a keyring that was
		// locked or before a key was added, until the session restarted: the next
		// save is meant to be the retry.
		*self.pool.lock().expect("key pool lock") = None;
		// However it ended, too: a preview that stopped part-way still says what
		// it would have run up to there.
		self.trace.lock().expect("trace lock").extend(trace);
		out
	}

	fn run_with_chain(
		&self,
		name: &str,
		args: &[Word],
		env: Option<crate::env::Inherited>,
		chain: &[String],
		label: Option<&str>,
		trace: &mut Vec<String>,
	) -> Result<(), RunError> {
		// `run github.com/owner/repo@v1 sync`, or `run ../tools/j18n sync`: a
		// package, whose `_main.run` is what runs.
		if reference::is_reference(name) {
			let pkg = self.package(name, env.as_ref())?;
			let target = pkg.entry().expect("a package is only kept once its entry is there");
			let entry = format!("{}{IN_PACKAGE}{}", pkg.key(), target.name);
			let next = self.called(&entry, chain)?;
			return ended(
				&pkg.shown,
				self.run_inner(Some(&pkg), target, args, env, next, label, trace),
			);
		}
		// Inside a package, a name is the package's own.
		let pkg = chain.last().and_then(|c| self.package_of(c));
		let catalog = pkg.as_ref().map_or(self.catalog, |p| &p.catalog);
		let caller = chain
			.last()
			.map(|c| c.split_once(IN_PACKAGE).map_or(c.as_str(), |(_, n)| n));

		// A subproject is self-contained: `run compile` inside `web/runfiles/`
		// means that directory's `compile`, whatever the root calls it. Without
		// this a file would have to spell its own siblings' names differently
		// depending on where `run` was invoked from.
		let qualified = caller
			.and_then(|caller| caller.rsplit_once(':'))
			.map(|(prefix, _)| format!("{prefix}:{name}"))
			.filter(|q| catalog.resolve(q).is_some());
		let name = qualified.as_deref().unwrap_or(name);

		let target = catalog.resolve(name).ok_or_else(|| {
			let near: Vec<&str> = catalog
				.targets
				.keys()
				.filter(|k| k.contains(name) || name.contains(k.as_str()))
				.map(String::as_str)
				.take(5)
				.collect();
			// Strip control characters: `near` are catalog target names (file
			// names from the repo) and `name` is echoed back, both to the
			// terminal (audit SA-027).
			RunError::Host(Box::new(HostError::Unknown {
				name: crate::term::sanitize(name).into_owned(),
				near: if near.is_empty() {
					"run :list".into()
				} else {
					near.iter()
						.map(|k| crate::term::sanitize(k).into_owned())
						.collect::<Vec<_>>()
						.join(", ")
				},
			}))
		})?;

		let entry = match &pkg {
			Some(p) => format!("{}{IN_PACKAGE}{}", p.key(), target.name),
			None => name.to_string(),
		};
		let next = self.called(&entry, chain)?;
		let shown = self.shown(&entry);
		ended(
			&shown,
			self.run_inner(pkg.as_deref(), target, args, env, next, label, trace),
		)
	}

	/// The chain once `entry` is called from the end of it, refused when that
	/// would be calling it again.
	fn called(&self, entry: &str, chain: &[String]) -> Result<Vec<String>, RunError> {
		if chain.iter().any(|c| c == entry) {
			let mut shown: Vec<String> = chain.iter().map(|c| self.shown(c)).collect();
			shown.push(self.shown(entry));
			return Err(RunError::Host(Box::new(HostError::Cycle {
				name: self.shown(entry),
				chain: shown.join(" -> "),
			})));
		}
		let mut next = chain.to_vec();
		next.push(entry.to_string());
		Ok(next)
	}

	/// Everything `run_inner` needs before a line runs: the target's tree and
	/// its chain's, a scope with the run context and arguments in place, and
	/// the properties the target starts from.
	///
	/// `env` is what another target ran this one with, and `None` for a target
	/// run from the command line, which starts from the process's environment.
	fn prepare(
		&self,
		pkg: Option<&Package>,
		target: &runfile_discovery::Target,
		args: &[Word],
		env: Option<crate::env::Inherited>,
	) -> Result<Prepared, RunError> {
		let catalog = pkg.map_or(self.catalog, |p| &p.catalog);
		let (ast, shared) = self.load(catalog, target)?;

		// The whole `_shared.run` chain is walked for what it reads *before* the
		// command line is classified: a name only a shared file reads is still a
		// name this target takes a value for.
		let reads = runfile_lang::inputs::of_chain(&ast, &shared);

		let mut scope = Scope::new();
		populate_run_context(&mut scope, target, catalog);
		if let Some(p) = pkg {
			scope.run.insert("version".into(), Value::Str(p.version.clone()));
		}
		// A target another one ran reads what it was handed from its first line,
		// a `_shared.run`'s included.
		if let Some(i) = &env {
			i.install(&mut scope);
		}
		let unknown = parse_args(&mut scope, args, &reads).map_err(|e| RunError::MissingArgValue {
			key: e.key,
			next: e.next,
			target: target.name.clone(),
		})?;
		scope.ask = self.ask;
		scope.confirm = self.confirm;
		scope.assume_yes = self.assume_yes;
		scope.rooted_at(target.anchor.clone());
		// A package works on whatever called it, so it starts where the line
		// that ran it was, and finds its own files under `RUN.parent`.
		if pkg.is_some() {
			scope.workdir = env
				.as_ref()
				.and_then(|e| e.workdir.clone())
				.or_else(|| std::env::current_dir().ok())
				.unwrap_or_else(|| target.anchor.clone());
		}
		// A `--dry-run` writes nothing, confines its reads to the project, and
		// reads no `.env` file and asks for no key (SA-008).
		scope.dry_run = self.dry_run;
		scope.preview = self.dry_run;
		scope.temps = self.temps.clone();
		scope.private_keys = self.key_pool();
		scope.downloader = self.downloader();

		// An input the target does not read is a mistake, and used to be a
		// warning only because the check was textual guesswork. Walked from the
		// tree it is exact, so it says no -- a mistyped `--forse` that merely
		// warns is a flag that did not take effect, discovered later. A target
		// reading `ARGS` is the exception and is handled in `parse_args`: it can
		// read the word, so it gets it.
		if let Some((name, key)) = unknown {
			return Err(RunError::UnknownInput {
				name,
				key,
				target: target.name.clone(),
				reads: Box::new(reads),
			});
		}

		let props = Props {
			// Where the file *lives*, carried in so the one property that
			// depends on it can say no. It survives every `extend`, which
			// clones, so a `_shared.run` in the machine-wide directory may set
			// it too.
			//
			// Asked of the path rather than of `Origin`. The two once parted
			// company: `$HOME/runfiles` found by the upward walk was collected
			// as `Local`, so standing in `$HOME` made every machine-wide
			// target refuse its own scope. They agree now, but the path is
			// what the language server has to ask with, and one question
			// with two answers is how an editor and the runner come to
			// disagree.
			machine_wide: runfile_discovery::is_machine_wide(&target.path),
			..Props::default()
		};
		Ok((ast, shared, scope, props))
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
	fn load(&self, catalog: &Catalog, target: &runfile_discovery::Target) -> Result<Loaded, RunError> {
		let (ast, src) = parse_file(&target.path)?;
		let chain = catalog.shared_chain(target);
		let (shared, texts): (Vec<runfile_lang::Target>, Vec<String>) = chain
			.iter()
			.map(|p| parse_file(p))
			.collect::<Result<Vec<_>, _>>()?
			.into_iter()
			.unzip();
		for (i, (path, file)) in chain.iter().zip(&shared).enumerate() {
			refuse_unresolved(path, runfile_lang::resolve::of_shared(file, &shared[..i]))?;
			refuse_findings(path, findings(&texts[i], file, &shared[..i]))?;
		}
		refuse_unresolved(&target.path, runfile_lang::resolve::of_chain(&ast, &shared))?;
		refuse_findings(&target.path, findings(&src, &ast, &shared))?;
		Ok((ast, shared))
	}

	/// Whether a target would be let run at all: it parses, and so does its
	/// chain, every name they use resolves, and their shell holds nothing the
	/// shell checker finds.
	///
	/// `run --stdin-args` asks this before it asks anything, so nobody answers
	/// questions about a file that is refused the moment they finish.
	pub fn check(&self, target: &runfile_discovery::Target) -> Result<(), RunError> {
		self.load(self.catalog, target).map(|_| ())
	}

	/// [`Host::check`], for the entry of a package.
	pub fn check_package(&self, pkg: &Package) -> Result<(), RunError> {
		match pkg.entry() {
			Some(t) => self.load(&pkg.catalog, t).map(|_| ()),
			None => Ok(()),
		}
	}

	#[allow(clippy::too_many_arguments)]
	fn run_inner(
		&self,
		pkg: Option<&Package>,
		target: &runfile_discovery::Target,
		args: &[Word],
		env: Option<crate::env::Inherited>,
		chain: Vec<String>,
		label: Option<&str>,
		trace: &mut Vec<String>,
	) -> Result<(), RunError> {
		let top = chain.len() == 1;
		let (ast, shared, mut scope, props) = self.prepare(pkg, target, args, env)?;
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
			dispatch: &adapter,
			dry_run: self.dry_run,

			interrupted: self.interrupted,
			label: label.map(str::to_string),
			colour: None,
			trace: Vec::new(),
		};
		// Only the target the run was asked for says what to watch.
		let header = |p: &Props| match (top, self.on_header) {
			(true, Some(f)) => f(p).map_err(|m| RunError::Host(Box::new(HostError::Header(m)))),
			_ => Ok(()),
		};
		let out = crate::run::run_chain(&shared, &ast, props, &mut r, header);
		trace.append(&mut r.trace);
		out
	}
}

/// A notice the runner gives while it works -- a package being fetched, a file
/// downloaded -- on stderr, where nothing reading the run's output sees it.
fn say(msg: &str) {
	eprintln!("{} {msg}", crate::exec::tag());
}

/// A canonical path as everything else spells it: Windows' canonical form is a
/// verbatim `\\?\C:\…`, which most programs handed it as a working directory or
/// an argument do not read.
fn plain(p: PathBuf) -> PathBuf {
	#[cfg(windows)]
	{
		let s = p.display().to_string();
		if let Some(rest) = s.strip_prefix(r"\\?\")
			&& rest.as_bytes().get(1) == Some(&b':')
		{
			return PathBuf::from(rest);
		}
	}
	p
}

/// What a target's own `exit()` is to everything outside it.
///
/// `exit()` ends the target it is written in -- a helper's early `exit()` used
/// to end the whole run instead, so a caller's remaining lines were skipped and
/// the run reported whatever status the helper named, 0 included: a skipped
/// build that called itself a success. Its status is how the target went.
/// Status 0 is a target that went well, so the line that ran it carries on;
/// anything else fails that line, as `Exited`, which every caller up the chain
/// may forgive, retry or score. That is what `$ run <target>` makes of the same
/// `exit()`, and dispatching in-process is not meant to mean something else.
///
/// A status is a byte, as a process's is: `exit(-1)` is the 255 a shell would
/// report, and `exit(256)` is 0. Only the target's own `exit()` is converted --
/// an `Exited` from something it ran and let through is already the answer.
/// It is asked of `run_inner` whole, since the `_shared.run` chain runs on the
/// target's behalf.
fn ended(target: &str, out: Result<(), RunError>) -> Result<(), RunError> {
	match out {
		Err(RunError::Eval(runfile_lang::eval::EvalError::Exit { code, .. })) => match i32::from(code as u8) {
			0 => Ok(()),
			code => Err(RunError::Exited {
				target: target.to_string(),
				code,
			}),
		},
		out => out,
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
		env: crate::env::Inherited,
		chain: &[String],
		label: Option<&str>,
		trace: &mut Vec<String>,
	) -> Result<(), RunError> {
		self.host
			.run_with_chain(target, &literal(args), Some(env), chain, label, trace)
	}

	fn run_words(
		&self,
		target: &str,
		args: &[Word],
		env: crate::env::Inherited,
		chain: &[String],
		label: Option<&str>,
		trace: &mut Vec<String>,
	) -> Result<(), RunError> {
		self.host.run_with_chain(target, args, Some(env), chain, label, trace)
	}
}

/// A command line typed by a person, every word classified as written.
fn literal(args: &[String]) -> Vec<Word> {
	args.iter().map(Word::literal).collect()
}

/// A target's tree and its chain's, as [`Host::load`] hands them back.
type Loaded = (runfile_lang::Target, Vec<runfile_lang::Target>);

/// What [`Host::prepare`] hands back: the trees, the scope, and the properties
/// the chain starts from.
type Prepared = (runfile_lang::Target, Vec<runfile_lang::Target>, Scope, Props);

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
) -> Vec<runfile_lang::check::Finding> {
	let mut all = runfile_lang::check::check(src, file, Some(chain));
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
	let src = runfile_discovery::read_runfile(p).map_err(|e| {
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
	// The tag a package was fetched at, which `Host::prepare` sets for a
	// package's targets; every other target has none.
	sc.run.insert("version".into(), Value::Str(String::new()));
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
	// `process_env` uses `vars_os`, so a non-UTF-8 variable skips rather than
	// panicking and aborting the run before anything starts (audit SA-023).
	sc.env = runfile_env::process_env();
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
	args: &[Word],
	reads: &runfile_lang::Inputs,
) -> Result<Option<(String, String)>, runfile_lang::args::MissingValue> {
	let mut unknown = None;
	for a in runfile_lang::args::parse_words(args, reads)? {
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
