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
	#[error("alias `{alias}` is claimed by both `{a}` and `{b}`")]
	DuplicateAlias { alias: String, a: String, b: String },
	#[error(
		"global runfiles live in two places at once: {a} and {b}\n\
		 keep one of them and remove or empty the other"
	)]
	AmbiguousGlobal { a: PathBuf, b: PathBuf },
}

/// Read the declaration-region values of a property from a `_shared.run`.
/// Only literal strings are read: this runs before any target is chosen, so
/// there are no arguments to substitute.
fn shared_strings(path: &Path, name: &str) -> Vec<String> {
	let Ok(src) = std::fs::read_to_string(path) else {
		return Vec::new();
	};
	let Ok(ast) = runfile_lang::parse(&src) else {
		return Vec::new();
	};
	ast.body
		.properties
		.iter()
		.filter(|p| p.path.first().is_some_and(|h| h == name))
		.filter_map(|p| match &p.value {
			Some(runfile_lang::Expr::Str(parts, _)) => Some(parts),
			_ => None,
		})
		.filter_map(|parts| match parts.first() {
			Some(runfile_lang::InterpPart::Literal(t)) => Some(t.clone()),
			_ => None,
		})
		.collect()
}

/// Whether a directory-scoped source applies where we are standing.
///
/// Entries anchor to the source's own directory, absolute paths pass through,
/// both sides canonicalize with a raw-path fallback, and the comparison is on
/// path components so a name that merely shares a prefix does not match.
/// `~` expands, which the old field silently did not.
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

	if let Some(dir) = &local {
		let anchor = dir.parent().unwrap_or(dir).to_path_buf();
		cat.root = anchor.clone();
		collect(dir, &anchor, "", Origin::Local, &mut cat)?;
		// Sibling projects: a depth-1 `runfiles/` is a namespace, no declaration.
		scan_subprojects(&anchor, 1, &mut cat)?;
	}

	if let Some(h) = home {
		// `$HOME/runfiles` is a legal spelling, so it can also be the *local*
		// directory when the run started at or below the home directory.
		// Collecting it twice would report every target as a duplicate.
		if let Some(g) = global_dir(h)?.filter(|g| local.as_ref() != Some(g)) {
			// A machine-wide directory can scope itself: registered everywhere,
			// active only inside the directories it names.
			let scope = shared_strings(&g.join(SHARED), "only-in-directories");
			if covers_cwd(h, &scope, from) {
				collect(&g, h, "", Origin::Global, &mut cat)?;
			}
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
			collect(&candidate, &p, name, Origin::Included, cat)?;
		}
		scan_subprojects(&p, depth + 1, cat)?;
	}
	Ok(())
}

/// Every `.run` under `dir` becomes a target named by its path, with `/`
/// mapped to `:`.
fn collect(dir: &Path, anchor: &Path, prefix: &str, origin: Origin, cat: &mut Catalog) -> Result<(), DiscoverError> {
	walk_runs(dir, dir, anchor, prefix, origin, cat)
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
	cat: &mut Catalog,
) -> Result<(), DiscoverError> {
	// Every directory in the tree can carry settings, not just the top one:
	// `runfiles/api/_shared.run` applies to `api:*`. Registering only the root
	// meant a nested one was read by nothing at all.
	cat.shared.insert(dir_prefix(root, dir, prefix), dir.join(SHARED));

	let Ok(rd) = std::fs::read_dir(dir) else { return Ok(()) };
	let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
	entries.sort();
	for p in entries {
		if p.is_dir() {
			walk_runs(root, &p, anchor, prefix, origin, cat)?;
			continue;
		}
		if p.extension().is_none_or(|x| x != "run") {
			continue;
		}
		if p.file_name().is_some_and(|n| n == SHARED) {
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

/// The names a target answers to besides its own, qualified by its namespace.
///
/// An alias declared in `web/runfiles/setup.run` as `deps` answers to
/// `web:deps`, not `deps`: a subproject must not be able to claim a bare name
/// in the root, and the qualified spelling is the one every listing shows.
pub fn aliases_of(t: &Target) -> Vec<String> {
	let prefix = match t.name.rsplit_once(':') {
		Some((p, _)) => format!("{p}:"),
		None => String::new(),
	};
	shared_strings(&t.path, "alias")
		.into_iter()
		.map(|a| format!("{prefix}{a}"))
		.collect()
}

impl Catalog {
	/// Resolve by file name first; only scan aliases on a miss, so the common
	/// path costs one lookup and nothing is read from disk.
	pub fn resolve(&self, name: &str) -> Option<&Target> {
		self.targets.get(name).or_else(|| self.by_alias(name).ok().flatten())
	}

	/// Scan every target's declaration region for `.alias = "<name>"`. Only
	/// reached on a miss, which is why aliases cost nothing in the common case.
	pub fn by_alias(&self, name: &str) -> Result<Option<&Target>, DiscoverError> {
		let mut found: Option<&Target> = None;
		for t in self.targets.values() {
			if !aliases_of(t).iter().any(|a| a == name) {
				continue;
			}
			if let Some(prev) = found {
				return Err(DiscoverError::DuplicateAlias {
					alias: name.to_string(),
					a: prev.name.clone(),
					b: t.name.clone(),
				});
			}
			found = Some(t);
		}
		Ok(found)
	}

	/// The `_shared.run` that applies to a target, if any.
	/// Every `_shared.run` that applies to a target, outermost first, so a
	/// nested one layers over the directory above rather than replacing it.
	///
	/// The walk stops at the target's own `runfiles/` tree: a subproject is
	/// self-contained, so the root's settings are not its to inherit.
	pub fn shared_chain(&self, target: &Target) -> Vec<PathBuf> {
		let segments: Vec<&str> = target.name.split(':').collect();
		let namespace = &segments[..segments.len().saturating_sub(1)];
		// A subproject's own root is its first segment; everything else starts
		// at the top of the local tree.
		let from = usize::from(target.origin == Origin::Included);
		(from..=namespace.len())
			.filter_map(|end| self.shared.get(&namespace[..end].join(":")))
			.filter(|p| p.is_file())
			.cloned()
			.collect()
	}
}

#[cfg(test)]
mod tests;
