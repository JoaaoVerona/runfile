//! Finding targets on disk.
//!
//! A project's targets live in `runfiles/` -- visible, because hiding them
//! hurts discoverability. The machine-wide one is one of `$HOME/.runfiles/`,
//! `$HOME/runfiles/` or `$HOME/Runfiles/`: a fixed set of names with no setting
//! to add to it, so that a person may spell it the way they like without the
//! runner having to be told. Exactly one of them may hold anything -- two
//! populated ones is an error rather than a merge, because which target wins
//! would be invisible.
//!
//! Nested directories contribute namespace segments, which is what replaced
//! `includes`: 27 of the corpus's 32 includes were depth-1 subdirectories
//! needing no declaration at all. A `:` can never appear in a file name --
//! NTFS forbids it -- so a namespace is always a real directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Directories never descended into when looking for `runfiles/`.
const SKIP: &[&str] = &["node_modules", "target", "dist", "build", ".git", "vendor"];
const MAX_DEPTH: usize = 3;

/// The file that holds a directory's shared properties and bindings -- the
/// `globals` analog. Not a target.
pub const SHARED: &str = "_shared.run";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Origin {
	/// The `runfiles/` at or above the working directory.
	Local,
	/// A `*/runfiles/` found in a subdirectory.
	Included,
	/// `$HOME/.runfiles/`.
	Global,
}

#[derive(Debug, Clone)]
pub struct Target {
	pub name: String,
	pub path: PathBuf,
	/// The parent of the `runfiles/` directory: the one anchor used for the
	/// working directory, `.env-file`, `.add-path` and `{{ RUN.parent }}`.
	pub anchor: PathBuf,
	pub origin: Origin,
}

/// Whether a target is kept out of listings: its **file name starts with `_`**.
///
/// This replaced a `.hide` property, which said the same thing in a second
/// place and let the two disagree. A helper other targets call is already
/// spelled with a leading underscore by convention -- fifteen of the sixteen
/// targets that set `.hide` were already named that way, and every
/// `_`-prefixed target set it. One spelling, no property.
///
/// The namespace is not part of the question: `backup:_aws` is hidden because
/// the file is `_aws.run`, not because of where it sits.
pub fn is_hidden(name: &str) -> bool {
	name.rsplit(':').next().is_some_and(|file| file.starts_with('_'))
}

#[derive(Debug, Default)]
pub struct Catalog {
	pub targets: BTreeMap<String, Target>,
	/// Directories whose `_shared.run` applies, keyed by the namespace prefix
	/// its targets carry.
	pub shared: BTreeMap<String, PathBuf>,
	/// The parent of the nearest `runfiles/`: where a project-level file such
	/// as `.zed/tasks.json` belongs.
	pub root: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum DiscoverError {
	#[error("no runfiles/ directory found from {0}")]
	NotFound(PathBuf),
	#[error("target `{name}` is defined twice: {a} and {b}")]
	Duplicate { name: String, a: PathBuf, b: PathBuf },
	#[error(
		"global runfiles live in two places at once: {a} and {b}\n\
		 keep one of them and remove or empty the other"
	)]
	AmbiguousGlobal { a: PathBuf, b: PathBuf },
	#[error(
		"{path}: line {line}: `.{SCOPE}` has to be a literal string, or a list of them\n\
		 it is read before any target is chosen, so there is nothing to interpolate from"
	)]
	UnreadableScope { path: PathBuf, line: usize },
}

/// The property that scopes the machine-wide directory.
pub const SCOPE: &str = "only-in-directories";

/// Where a scope is being judged from.
///
/// `home` is what a relative entry anchors to -- every level anchors to the
/// same place, so `work/acme` means one directory however deep the file naming
/// it sits. `cwd` is the directory that has to be covered.
struct Reach<'a> {
	home: &'a Path,
	cwd: &'a Path,
}

/// The directories a machine-wide file admits, or an error when it names them
/// in a form that cannot be read here.
///
/// This runs before any target is chosen, so there are no arguments to
/// substitute and only literals can be read. A value that is not one is
/// **refused** rather than passed over: read as no scope at all it fails
/// *open*, leaving the directory active everywhere -- the opposite of what was
/// asked for, and silent. That is what a list literal used to do.
///
/// A file that does not parse is left alone. It is broken whatever it says and
/// will say so when it runs, and a machine-wide directory holding one must not
/// stop every `run` on the machine.
fn scope_of(path: &Path) -> Result<Vec<String>, DiscoverError> {
	let Ok(src) = std::fs::read_to_string(path) else {
		return Ok(Vec::new());
	};
	// The property has to appear literally to be declared, so the text is an
	// exact gate on whether the file is worth parsing at all: a false positive
	// costs one parse, and a false negative cannot happen.
	if !src.contains(SCOPE) {
		return Ok(Vec::new());
	}
	let Ok(ast) = runfile_lang::parse(&src) else {
		return Ok(Vec::new());
	};
	let mut out = Vec::new();
	for p in ast
		.body
		.properties
		.iter()
		.filter(|p| p.path.first().is_some_and(|h| h == SCOPE))
	{
		let unreadable = || DiscoverError::UnreadableScope {
			path: path.to_path_buf(),
			line: p.span.line,
		};
		// One entry or several: repeated lines already append, and a list says
		// the same thing on one line the way every other list-valued property
		// accepts one.
		let items: Vec<&runfile_lang::Expr> = match &p.value {
			Some(runfile_lang::Expr::List(items, _)) => items.iter().collect(),
			Some(e) => vec![e],
			None => return Err(unreadable()),
		};
		for e in items {
			out.push(literal_string(e).ok_or_else(unreadable)?);
		}
	}
	Ok(out)
}

/// A string with nothing interpolated into it. The whole value has to be one
/// literal: `"{{ ENV.X }}/work"` names a directory this cannot know.
fn literal_string(e: &runfile_lang::Expr) -> Option<String> {
	match e {
		runfile_lang::Expr::Str(parts, _) => match parts.as_slice() {
			[] => Some(String::new()),
			[runfile_lang::InterpPart::Literal(t)] => Some(t.clone()),
			_ => None,
		},
		_ => None,
	}
}

/// Whether a directory-scoped source applies where we are standing.
///
/// Entries anchor to the home directory, absolute paths pass through, both
/// sides canonicalize with a raw-path fallback, and the comparison is on path
/// components so a name that merely shares a prefix does not match. `~`
/// expands, which the old field silently did not.
fn covers_cwd(anchor: &Path, dirs: &[String], cwd: &Path) -> bool {
	if dirs.is_empty() {
		return true;
	}
	let here = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
	dirs.iter().any(|d| {
		let expanded = match d.strip_prefix("~/") {
			Some(rest) => home_dir().map(|h| h.join(rest)).unwrap_or_else(|| PathBuf::from(d)),
			None => PathBuf::from(d),
		};
		let allowed = if expanded.is_absolute() {
			expanded
		} else {
			anchor.join(expanded)
		};
		let allowed = std::fs::canonicalize(&allowed).unwrap_or(allowed);
		here.starts_with(&allowed)
	})
}

/// Whether a file sits inside the machine-wide directory, judged from its path
/// alone.
///
/// The path is all an editor has to go on -- it holds one document, not a
/// catalog -- and it is enough, because the three names are fixed: a file is
/// machine-wide exactly when it is under one of them in this user's home. It
/// is what lets a language server refuse `.only-in-directories` in the same
/// places the runner does, rather than accepting what the runner will not.
pub fn is_machine_wide(path: &Path) -> bool {
	let Some(home) = home_dir() else { return false };
	let here = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
	GLOBAL_NAMES.iter().any(|n| {
		let g = home.join(n);
		let g = std::fs::canonicalize(&g).unwrap_or(g);
		here.starts_with(&g)
	})
}

/// The home directory: `HOME` when set, else `USERPROFILE`.
///
/// The environment first, on every platform: it is what Git Bash sets on
/// Windows and what a test fixture can redirect, whereas the platform's Known
/// Folder API answers the same thing however the process was started.
pub fn home_dir() -> Option<PathBuf> {
	std::env::var_os("HOME")
		.or_else(|| std::env::var_os("USERPROFILE"))
		.map(PathBuf::from)
}

/// Walk up for the nearest `runfiles/`, then down for `*/runfiles/`.
pub fn discover(from: &Path, home: Option<&Path>) -> Result<Catalog, DiscoverError> {
	let mut cat = Catalog::default();
	let local = find_upward(from);
	let global = match home {
		Some(h) => global_dir(h)?,
		None => None,
	};
	// Scoping belongs to the machine-wide directory wherever it is reached
	// from, so this is worked out once and handed to whichever walk collects
	// it.
	let reach = home.map(|h| Reach { home: h, cwd: from });

	if let Some(dir) = &local {
		let anchor = dir.parent().unwrap_or(dir).to_path_buf();
		cat.root = anchor.clone();
		// `$HOME/runfiles` reached by the upward walk *is* the machine-wide
		// directory -- the names are what make one -- so its files still get
		// to say where they belong. Collected as `Local` because it is also
		// the nearest one, which is a different question and the one `Origin`
		// answers.
		let mine = global.as_ref() == Some(dir);
		collect(
			dir,
			&anchor,
			"",
			Origin::Local,
			reach.as_ref().filter(|_| mine),
			&mut cat,
		)?;
		// Sibling projects: a depth-1 `runfiles/` is a namespace, no declaration.
		scan_subprojects(&anchor, 1, &mut cat)?;
	}

	if let Some(h) = home {
		// `$HOME/runfiles` is a legal spelling, so it can also be the *local*
		// directory when the run started at or below the home directory.
		// Collecting it twice would report every target as a duplicate.
		if let Some(g) = global.filter(|g| local.as_ref() != Some(g)) {
			// A machine-wide target can scope itself: registered everywhere,
			// active only inside the directories it names. Judged per file as
			// the tree is walked, so a directory, a namespace inside it and a
			// single target each get to say where they belong.
			collect(&g, h, "", Origin::Global, reach.as_ref(), &mut cat)?;
		}
	}

	if cat.targets.is_empty() && local.is_none() {
		return Err(DiscoverError::NotFound(from.to_path_buf()));
	}
	Ok(cat)
}

/// The names the machine-wide directory may go by, in the order they are
/// reported. A dotted one is the default; the other two exist because a person
/// who wants to see the directory in their home should not have to argue.
pub const GLOBAL_NAMES: &[&str] = &[".runfiles", "runfiles", "Runfiles"];

/// The one machine-wide directory, or an error naming the two that clash.
///
/// A directory that exists but holds nothing does not count: an empty one is
/// indistinguishable from a leftover, and refusing to run because of a
/// forgotten `mkdir` would be absurd.
pub fn global_dir(home: &Path) -> Result<Option<PathBuf>, DiscoverError> {
	let mut found: Option<PathBuf> = None;
	for name in GLOBAL_NAMES {
		let dir = home.join(name);
		if !has_content(&dir) {
			continue;
		}
		match &found {
			// `runfiles` and `Runfiles` are one directory on a case-insensitive
			// filesystem, and it must not be reported as clashing with itself.
			Some(first) if same_dir(first, &dir) => {}
			Some(first) => {
				return Err(DiscoverError::AmbiguousGlobal {
					a: first.clone(),
					b: dir,
				});
			}
			None => found = Some(dir),
		}
	}
	Ok(found)
}

/// Whether the directory exists and holds at least one entry.
fn has_content(dir: &Path) -> bool {
	std::fs::read_dir(dir).is_ok_and(|mut e| e.next().is_some())
}

fn same_dir(a: &Path, b: &Path) -> bool {
	match (a.canonicalize(), b.canonicalize()) {
		(Ok(a), Ok(b)) => a == b,
		_ => false,
	}
}

fn find_upward(from: &Path) -> Option<PathBuf> {
	let mut dir = Some(from);
	while let Some(d) = dir {
		let c = d.join("runfiles");
		if c.is_dir() {
			return Some(c);
		}
		dir = d.parent();
	}
	None
}

fn scan_subprojects(root: &Path, depth: usize, cat: &mut Catalog) -> Result<(), DiscoverError> {
	if depth > MAX_DEPTH {
		return Ok(());
	}
	let Ok(rd) = std::fs::read_dir(root) else { return Ok(()) };
	for e in rd.flatten() {
		let p = e.path();
		if !p.is_dir() {
			continue;
		}
		let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
			continue;
		};
		if SKIP.contains(&name) || name.starts_with('.') || name == "runfiles" {
			continue;
		}
		let candidate = p.join("runfiles");
		if candidate.is_dir() {
			collect(&candidate, &p, name, Origin::Included, None, cat)?;
		}
		scan_subprojects(&p, depth + 1, cat)?;
	}
	Ok(())
}

/// Every `.run` under `dir` becomes a target named by its path, with `/`
/// mapped to `:`.
///
/// `reach` is `Some` only for the machine-wide directory, which is the one
/// tree whose files may scope themselves. A project's targets are visible to
/// anyone reading the repository, so hiding some of them by working directory
/// would recreate the very invisibility the machine-wide rule exists to fix --
/// and it is why a project file setting the property is refused outright
/// rather than quietly doing nothing.
fn collect(
	dir: &Path,
	anchor: &Path,
	prefix: &str,
	origin: Origin,
	reach: Option<&Reach<'_>>,
	cat: &mut Catalog,
) -> Result<(), DiscoverError> {
	walk_runs(dir, dir, anchor, prefix, origin, reach, cat)
}

/// The namespace a directory inside a `runfiles/` tree contributes to.
fn dir_prefix(root: &Path, dir: &Path, prefix: &str) -> String {
	let rel = dir.strip_prefix(root).unwrap_or(dir);
	let mut parts: Vec<String> = Vec::new();
	if !prefix.is_empty() {
		parts.push(prefix.to_string());
	}
	parts.extend(rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()));
	parts.join(":")
}

fn walk_runs(
	root: &Path,
	dir: &Path,
	anchor: &Path,
	prefix: &str,
	origin: Origin,
	reach: Option<&Reach<'_>>,
	cat: &mut Catalog,
) -> Result<(), DiscoverError> {
	// A directory's `_shared.run` scopes everything below it, so a subtree we
	// are standing outside of is pruned whole -- one file read rather than one
	// per target, which is the common case for a scoped machine-wide
	// directory.
	if let Some(r) = reach
		&& !covers_cwd(r.home, &scope_of(&dir.join(SHARED))?, r.cwd)
	{
		return Ok(());
	}

	// Every directory in the tree can carry settings, not just the top one:
	// `runfiles/api/_shared.run` applies to `api:*`. Registering only the root
	// meant a nested one was read by nothing at all.
	cat.shared.insert(dir_prefix(root, dir, prefix), dir.join(SHARED));

	let Ok(rd) = std::fs::read_dir(dir) else { return Ok(()) };
	let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
	entries.sort();
	for p in entries {
		if p.is_dir() {
			walk_runs(root, &p, anchor, prefix, origin, reach, cat)?;
			continue;
		}
		if p.extension().is_none_or(|x| x != "run") {
			continue;
		}
		if p.file_name().is_some_and(|n| n == SHARED) {
			continue;
		}
		// A target narrows further within the directory that admitted it. Every
		// level that names directories has to cover us, so a file can only ever
		// narrow -- naming a directory its `_shared.run` excludes does not let
		// it back out.
		if let Some(r) = reach
			&& !covers_cwd(r.home, &scope_of(&p)?, r.cwd)
		{
			continue;
		}
		let rel = p.strip_prefix(root).unwrap_or(&p).with_extension("");
		let mut name = rel
			.components()
			.map(|c| c.as_os_str().to_string_lossy())
			.collect::<Vec<_>>()
			.join(":");
		if !prefix.is_empty() {
			name = format!("{prefix}:{name}");
		}
		if let Some(prev) = cat.targets.get(&name) {
			return Err(DiscoverError::Duplicate {
				name,
				a: prev.path.clone(),
				b: p.clone(),
			});
		}
		cat.targets.insert(
			name.clone(),
			Target {
				name,
				path: p,
				anchor: anchor.to_path_buf(),
				origin,
			},
		);
	}
	Ok(())
}

impl Catalog {
	/// A target is reached by its file name, and only by its file name: one
	/// hash lookup, nothing read from disk.
	pub fn resolve(&self, name: &str) -> Option<&Target> {
		self.targets.get(name)
	}

	/// Every `_shared.run` that applies to a target, outermost first, so a
	/// nested one layers over the directory above rather than replacing it.
	///
	/// Walked from the target's own file rather than looked up by namespace.
	/// A namespace is not unique across trees: the machine-wide one has none,
	/// so its `_shared.run` was registered under the same empty key as a
	/// project's own -- and since the key is written whether or not the file
	/// exists, merely *having* a `~/.runfiles` silently disabled the root
	/// `_shared.run` of every project on the machine. A path cannot collide.
	///
	/// The walk stops at the anchor, so a subproject is self-contained: the
	/// settings above its own `runfiles/` are not its to inherit.
	pub fn shared_chain(&self, target: &Target) -> Vec<PathBuf> {
		let mut out = Vec::new();
		let mut dir = target.path.parent();
		while let Some(d) = dir.filter(|d| *d != target.anchor && d.starts_with(&target.anchor)) {
			let p = d.join(SHARED);
			if p.is_file() {
				out.push(p);
			}
			dir = d.parent();
		}
		out.reverse();
		out
	}
}

#[cfg(test)]
mod tests;
