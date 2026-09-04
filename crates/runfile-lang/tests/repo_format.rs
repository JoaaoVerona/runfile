//! The formatter, against this repository's own files.
//!
//! Two gates. The first is `cargo fmt --check` for `.run` files: every runfile
//! this project ships is already in the shape `run :format` produces, so the
//! corpus cannot drift away from its own formatter. The second is wider and
//! weaker -- it covers the test fixtures too, which are deliberately odd -- and
//! asks only that formatting them changes nothing about what they mean and
//! settles after one pass.

use std::path::{Path, PathBuf};

use runfile_lang::{fingerprint, format, parse};

fn repo_root() -> PathBuf {
	// crates/runfile-lang -> the workspace root.
	Path::new(env!("CARGO_MANIFEST_DIR"))
		.parent()
		.and_then(Path::parent)
		.expect("a workspace root")
		.to_path_buf()
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
	let Ok(entries) = std::fs::read_dir(dir) else { return };
	for e in entries.flatten() {
		let p = e.path();
		let name = e.file_name();
		let name = name.to_string_lossy();
		if p.is_dir() {
			if matches!(name.as_ref(), "target" | "node_modules" | ".git") {
				continue;
			}
			collect(&p, out);
		} else if p.extension().is_some_and(|x| x == "run") {
			out.push(p);
		}
	}
}

fn every_run_file() -> Vec<PathBuf> {
	let mut out = Vec::new();
	collect(&repo_root(), &mut out);
	assert!(out.len() > 20, "the sweep found almost nothing: {}", out.len());
	out.sort();
	out
}

#[test]
fn every_runfile_in_this_repository_is_already_formatted() {
	let mut unformatted = Vec::new();
	for p in every_run_file() {
		// Only the project's own targets: the golden fixtures exist to pin
		// parser edge cases, and one of those may want odd source.
		if !p.components().any(|c| c.as_os_str() == "runfiles") {
			continue;
		}
		let src = std::fs::read_to_string(&p).unwrap();
		let out = format(&src).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
		if out != src {
			unformatted.push(p);
		}
	}
	assert!(
		unformatted.is_empty(),
		"run `run :format`:\n{}",
		unformatted
			.iter()
			.map(|p| format!("  {}", p.display()))
			.collect::<Vec<_>>()
			.join("\n")
	);
}

#[test]
fn formatting_any_file_here_preserves_its_meaning_and_settles() {
	for p in every_run_file() {
		let src = std::fs::read_to_string(&p).unwrap();
		let Ok(before) = parse(&src) else { continue };
		let once = format(&src).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
		let twice = format(&once).unwrap_or_else(|e| panic!("{} (second pass): {e}", p.display()));
		assert_eq!(once, twice, "{} is not stable under formatting", p.display());
		assert_eq!(
			fingerprint(&before),
			fingerprint(&parse(&once).unwrap()),
			"{} means something else after formatting",
			p.display()
		);
	}
}
