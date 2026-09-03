//! Finding targets on disk.
//!
//! A project's targets live in `runfiles/` -- visible, because hiding them
//! hurts discoverability. The machine-wide one is `$HOME/.runfiles/`, dotted
//! like every other `$HOME` config, at a fixed path with no setting to change
//! it.
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
		let g = h.join(".runfiles");
		// A machine-wide directory can scope itself: registered everywhere,
		// active only inside the directories it names.
		if g.is_dir() && covers_cwd(h, &shared_strings(&g.join(SHARED), "only-in-directories"), from) {
			collect(&g, h, "", Origin::Global, &mut cat)?;
		}
	}

	if cat.targets.is_empty() && local.is_none() {
		return Err(DiscoverError::NotFound(from.to_path_buf()));
	}
	Ok(cat)
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
	if !prefix.is_empty() || origin == Origin::Local || origin == Origin::Global {
		cat.shared.insert(prefix.to_string(), dir.join(SHARED));
	}
	walk_runs(dir, dir, anchor, prefix, origin, cat)
}

fn walk_runs(
	root: &Path,
	dir: &Path,
	anchor: &Path,
	prefix: &str,
	origin: Origin,
	cat: &mut Catalog,
) -> Result<(), DiscoverError> {
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
	pub fn shared_for(&self, target: &Target) -> Option<PathBuf> {
		let prefix = match target.name.rfind(':') {
			Some(_) if target.origin == Origin::Included => target.name.split(':').next().unwrap_or("").to_string(),
			_ => String::new(),
		};
		self.shared.get(&prefix).filter(|p| p.is_file()).cloned()
	}
}

#[cfg(test)]
mod tests;
