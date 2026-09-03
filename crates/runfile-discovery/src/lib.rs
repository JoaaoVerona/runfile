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
}

#[derive(Debug, thiserror::Error)]
pub enum DiscoverError {
	#[error("no runfiles/ directory found from {0}")]
	NotFound(PathBuf),
	#[error("target `{name}` is defined twice: {a} and {b}")]
	Duplicate { name: String, a: PathBuf, b: PathBuf },
}

/// Walk up for the nearest `runfiles/`, then down for `*/runfiles/`.
pub fn discover(from: &Path, home: Option<&Path>) -> Result<Catalog, DiscoverError> {
	let mut cat = Catalog::default();
	let local = find_upward(from);

	if let Some(dir) = &local {
		let anchor = dir.parent().unwrap_or(dir).to_path_buf();
		collect(dir, &anchor, "", Origin::Local, &mut cat)?;
		// Sibling projects: a depth-1 `runfiles/` is a namespace, no declaration.
		scan_subprojects(&anchor, 1, &mut cat)?;
	}

	if let Some(h) = home {
		let g = h.join(".runfiles");
		if g.is_dir() {
			let anchor = h.to_path_buf();
			collect(&g, &anchor, "", Origin::Global, &mut cat)?;
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

impl Catalog {
	/// Resolve by file name first; only scan aliases on a miss, so the common
	/// path costs one lookup.
	pub fn resolve(&self, name: &str) -> Option<&Target> {
		self.targets.get(name)
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
