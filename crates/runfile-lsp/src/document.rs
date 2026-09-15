//! What a file means where it sits: the targets it may call, the `_shared.run`
//! files above it, whether it is machine-wide -- and so every diagnostic the
//! runner's own rules have for it.
//!
//! Asked by the server for an open document, and by `run :lint` for every file
//! on disk. Were each to work the chain and the target names out its own way,
//! the editor and the command line would come to disagree about a file, which is
//! the one thing these diagnostics exist to prevent.

use crate::analysis::{self, Chain, Diagnostic};
use runfile_discovery::Catalog;
use std::path::{Path, PathBuf};

/// Every diagnostic the runner's rules have for `src`: the text of the file at
/// `path`, in the project `cat` describes.
///
/// `path` is `None` for a document that is not a file, which has nothing above
/// it. `cat` is `None` when discovery failed, so what is above the file is not
/// known: the names it reads are left alone, and what is certain is still
/// checked. `read` is another file's text -- the editor's copy, where it has one.
pub fn diagnostics(
	src: &str,
	path: Option<&Path>,
	cat: Option<&Catalog>,
	read: &dyn Fn(&Path) -> Option<String>,
) -> Vec<Diagnostic> {
	let Some(path) = path else {
		return analysis::diagnose(src, &[], false, Chain::Target(&[]));
	};
	let machine_wide = runfile_discovery::is_machine_wide(path);
	let Some(cat) = cat else {
		return analysis::diagnose(src, &[], machine_wide, Chain::Unknown);
	};
	// A shared file that does not parse -- mid-edit, usually -- binds nothing
	// anybody can read, which is not the same as binding nothing.
	let above: Option<Vec<runfile_lang::Target>> = shared_chain(cat, path)
		.iter()
		.map(|p| read(p).and_then(|text| runfile_lang::parse(&text).ok()))
		.collect();
	let is_shared = path.file_name().is_some_and(|n| n == runfile_discovery::SHARED);
	let chain = match &above {
		None => Chain::Unknown,
		Some(files) if is_shared => Chain::Shared(files),
		Some(files) => Chain::Target(files),
	};
	analysis::diagnose(src, &target_names(cat, path), machine_wide, chain)
}

/// Every `_shared.run` that applies to a document, outermost first.
///
/// A target's is the catalog's. A `_shared.run` is not a target, and inherits
/// from the ones in the directories above its own -- which is the chain of any
/// target at or below it, cut off at its own directory.
pub fn shared_chain(cat: &Catalog, here: &Path) -> Vec<PathBuf> {
	if let Some(t) = cat.targets.values().find(|t| same_file(&t.path, here)) {
		return cat.shared_chain(t);
	}
	let Some(dir) = here.parent() else {
		return Vec::new();
	};
	let Some(below) = cat.targets.values().find(|t| t.path.starts_with(dir)) else {
		return Vec::new();
	};
	cat.shared_chain(below)
		.into_iter()
		.filter(|p| !same_file(p, here) && p.parent().is_some_and(|d| dir.starts_with(d)))
		.collect()
}

/// Every target name a document may write, qualified and unqualified.
///
/// A file calls its siblings by their bare name: `run compile` inside
/// `web/runfiles/` resolves `web:compile` first, and only then a root
/// `compile`. Listing just the catalog's keys had the editor underline every
/// one of those as "no target named …" while the runner ran them happily --
/// the editor and the runner disagreeing about validity, which is the one
/// thing this analysis exists to prevent.
pub fn target_names(cat: &Catalog, doc: &Path) -> Vec<String> {
	let mut names: Vec<String> = cat.targets.keys().cloned().collect();
	if let Some(me) = cat.targets.values().find(|t| same_file(&t.path, doc))
		&& let Some((prefix, _)) = me.name.rsplit_once(':')
	{
		let prefix = format!("{prefix}:");
		let siblings: Vec<String> = cat
			.targets
			.keys()
			.filter_map(|k| k.strip_prefix(&prefix).map(str::to_string))
			.collect();
		names.extend(siblings);
	}
	names
}

/// Whether two paths name one file, however each was spelled.
fn same_file(a: &Path, b: &Path) -> bool {
	a == b || a.canonicalize().ok().is_some_and(|c| Some(c) == b.canonicalize().ok())
}

#[cfg(test)]
mod tests {
	use super::*;

	fn nothing(_: &Path) -> Option<String> {
		None
	}

	#[test]
	fn a_document_that_is_not_a_file_has_nothing_above_it() {
		let d = diagnostics("print(region)\n", None, None, &nothing);
		assert_eq!(d.len(), 1, "{d:?}");
		assert!(d[0].message.contains("`region` is not defined"), "{}", d[0].message);
	}

	#[test]
	fn a_file_whose_project_could_not_be_found_is_checked_only_for_what_is_certain() {
		let here = Path::new("/nowhere/runfiles/a.run");
		let d = diagnostics("print(region)\nnope()\n", Some(here), None, &nothing);
		assert_eq!(d.len(), 1, "{d:?}");
		assert!(d[0].message.contains("unknown function `nope`"), "{}", d[0].message);
	}
}
