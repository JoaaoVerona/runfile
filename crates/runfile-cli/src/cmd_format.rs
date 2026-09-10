//! `run :format` -- put every runfile into the one shape there is.
//!
//! No options for the shape itself, deliberately: a formatter with settings is
//! a formatter each project sets differently, and the point is that a `.run`
//! file looks the same wherever it is read.
//!
//! `_shared.run` is formatted too. It is not a target, so nothing that walks
//! the catalog by name would reach it, and it is the file most likely to sit
//! unread and drift.

use crate::help::{Row, Section};
use runfile_discovery::{Catalog, Origin};
use std::path::{Path, PathBuf};

const INTRO: &str = "run :format [path…]   —   format runfiles";

const SECTIONS: &[Section] = &[
	Section(
		"Arguments",
		&[Row(
			"path…",
			"files or directories to format; default is every runfile in this project",
		)],
	),
	Section(
		"Flags",
		&[
			Row("--check", "report what would change and exit 1; write nothing"),
			Row("--stdout", "print the result instead of writing it back"),
			Row("--include-global", "include the machine-wide directory"),
		],
	),
];

pub(crate) fn usage() -> String {
	crate::help::render(INTRO, SECTIONS)
}

/// Every `.run` file the catalog knows about, `_shared.run` included.
pub fn project_files(cat: &Catalog, include_global: bool) -> Vec<PathBuf> {
	// Through the gate, not `home_dir()` directly: in CI there is no
	// machine-wide directory to leave out, because none was read in.
	let global = crate::discovery_home().and_then(|h| runfile_discovery::global_dir(&h).ok().flatten());
	let is_global = |p: &Path| global.as_ref().is_some_and(|g| p.starts_with(g));

	let mut out: Vec<PathBuf> = cat
		.targets
		.values()
		.filter(|t| include_global || t.origin != Origin::Global)
		.map(|t| t.path.clone())
		.collect();
	// A `_shared.run` is registered for every directory whether or not one is
	// there, and it is not a target, so nothing that walks by name reaches it.
	for p in cat.shared.values().filter(|p| p.is_file()) {
		if include_global || !is_global(p) {
			out.push(p.clone());
		}
	}
	out.sort();
	out.dedup();
	out
}

/// Expand explicit arguments: a `.run` file, or a directory to walk.
pub fn from_paths(paths: &[String]) -> Result<Vec<PathBuf>, String> {
	let mut out = Vec::new();
	for p in paths {
		let p = PathBuf::from(p);
		if p.is_dir() {
			walk(&p, &mut out);
		} else if p.is_file() {
			out.push(p);
		} else {
			return Err(format!("no such file or directory: {}", p.display()));
		}
	}
	out.sort();
	out.dedup();
	Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
	let Ok(rd) = std::fs::read_dir(dir) else { return };
	for e in rd.flatten() {
		let p = e.path();
		if p.is_dir() {
			walk(&p, out);
		} else if p.extension().is_some_and(|x| x == "run") {
			out.push(p);
		}
	}
}

/// What one file needed.
enum Outcome {
	Unchanged,
	Changed,
	Failed(String),
}

pub fn format_files(files: &[PathBuf], check: bool, to_stdout: bool) -> Result<std::process::ExitCode, String> {
	let mut changed = 0usize;
	let mut failed = 0usize;
	for f in files {
		match one(f, check, to_stdout) {
			Outcome::Unchanged => {}
			Outcome::Changed => {
				changed += 1;
				if !to_stdout {
					println!("{}", f.display());
				}
			}
			Outcome::Failed(e) => {
				failed += 1;
				eprintln!("{}: {e}", f.display());
			}
		}
	}
	if failed > 0 {
		return Err(format!("{failed} file(s) could not be formatted"));
	}
	if check && changed > 0 {
		eprintln!("{changed} file(s) would be reformatted");
		return Ok(std::process::ExitCode::FAILURE);
	}
	if !check && !to_stdout {
		let n = files.len();
		println!("{changed} of {n} file{} reformatted", if n == 1 { "" } else { "s" });
	}
	Ok(std::process::ExitCode::SUCCESS)
}

fn one(path: &Path, check: bool, to_stdout: bool) -> Outcome {
	let src = match std::fs::read_to_string(path) {
		Ok(s) => s,
		Err(e) => return Outcome::Failed(e.to_string()),
	};
	let out = match runfile_lang::format(&src) {
		Ok(o) => o,
		Err(e) => return Outcome::Failed(e.to_string()),
	};
	if to_stdout {
		print!("{out}");
		return Outcome::Unchanged;
	}
	if out == src {
		return Outcome::Unchanged;
	}
	if check {
		return Outcome::Changed;
	}
	match std::fs::write(path, &out) {
		Ok(()) => Outcome::Changed,
		Err(e) => Outcome::Failed(e.to_string()),
	}
}
