//! `run :lint` -- every runfile in the one shape there is, and nothing in any of
//! them the runner would refuse.
//!
//! It was `run :format`, which could pass a file that failed the moment it ran.
//! Exit 0 is a promise about every file it looks at: each is formatted, and none
//! holds anything the runner refuses before running -- a syntax error, a
//! property that is unknown or out of place, a call to a function that does not
//! exist, a name nothing binds, a `run` of a target that is not there. That is
//! the list an editor underlines, because the same function makes both:
//! [`runfile_lsp::document::diagnostics`].
//!
//! No options for the shape itself, deliberately: a formatter with settings is
//! a formatter each project sets differently, and the point is that a `.run`
//! file looks the same wherever it is read.
//!
//! `_shared.run` is linted too. It is not a target, so nothing that walks the
//! catalog by name would reach it, and it is the file most likely to sit
//! unread and drift.

use crate::help::{Row, Section};
use runfile_discovery::{Catalog, Origin};
use runfile_lsp::analysis::Severity;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const INTRO: &str = "run :lint [path…]   —   format runfiles and check them for errors";

const SECTIONS: &[Section] = &[
	Section(
		"Arguments",
		&[Row(
			"path…",
			"files or directories to lint; default is every runfile in this project",
		)],
	),
	Section(
		"Flags",
		&[
			Row("--check", "write nothing, and fail on a file that needs formatting too"),
			Row("--stdout", "print the formatted files instead of writing them back"),
			Row(
				"--include-global",
				"include the machine-wide directory (always included from inside it)",
			),
		],
	),
	Section(
		"Exit status",
		&[
			Row(
				"0",
				"every file is formatted, and nothing in one would be refused or underlined",
			),
			Row("1", "something is not"),
		],
	),
];

/// Every flag `:lint` takes. Anything else is refused rather than ignored: a
/// mistyped `--check` that went unread would write every file.
pub const FLAGS: &[&str] = &["--check", "--stdout", "--include-global"];

/// What `run :format` became, said to whoever still types it -- a script or a
/// habit written for an older runner, not a typo to be matched against a list.
pub const FORMAT_IS_LINT: &str = "`:format` is now `:lint`, which formats every runfile and also reports anything the runner would refuse; `run :lint --check` writes nothing";

pub(crate) fn usage() -> String {
	crate::help::render(INTRO, SECTIONS)
}

/// The project `:lint` checks from `from`: every file on disk, whatever
/// directories a machine-wide one names.
///
/// A scope says where a target is offered, which is a question about running
/// it, and nothing here runs. Asked the other way, the scoped files of the
/// machine-wide directory were linted only from inside the directories they
/// name -- and a scoped file reached anyway was checked without its own
/// `_shared.run` or siblings. See [`runfile_discovery::discover_unscoped`].
pub fn catalog(from: &Path) -> Result<Catalog, runfile_discovery::DiscoverError> {
	runfile_discovery::discover_unscoped(from, crate::discovery_home().as_deref())
}

/// Every `.run` file the catalog knows about, `_shared.run` included.
///
/// The machine-wide directory is left out unless it is asked for, or unless
/// `from` is inside it: there it is the project, and leaving it out said
/// `0 files checked: no errors` about a directory full of runfiles.
pub fn project_files(cat: &Catalog, from: &Path, include_global: bool) -> Vec<PathBuf> {
	let global = include_global || runfile_discovery::is_machine_wide(from);
	// Judged by how discovery reached each file, a target and a `_shared.run`
	// alike. By path, `$HOME/runfiles` found as the nearest `runfiles/` had its
	// targets linted and its `_shared.run` files left out. In CI there is nothing
	// to leave out, since no machine-wide directory was read.
	let wanted = |origin: Origin| global || origin != Origin::Global;
	let mut out: Vec<PathBuf> = cat
		.targets
		.values()
		.filter(|t| wanted(t.origin))
		.map(|t| t.path.clone())
		.collect();
	// A `_shared.run` is not a target, so nothing that walks by name reaches it.
	out.extend(cat.shared.iter().filter(|(_, o)| wanted(**o)).map(|(p, _)| p.clone()));
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
			walk(&p, &mut out, 0);
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

/// Directories the walk never enters, matching discovery's own list.
const WALK_SKIP: &[&str] = &["node_modules", "target", "dist", "build", ".git", "vendor"];

/// A backstop against a pathologically deep tree; far below any real project.
const WALK_MAX_DEPTH: usize = 64;

fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
	if depth > WALK_MAX_DEPTH {
		return;
	}
	let Ok(rd) = std::fs::read_dir(dir) else { return };
	for e in rd.flatten() {
		let p = e.path();
		// The entry's own type, which (unlike `is_dir`/`is_file`) does not follow
		// a symlink: a symlinked directory is not descended -- so a loop cannot
		// spin and the walk cannot leave the named tree -- and a symlinked `.run`
		// is not collected, which would otherwise be rewritten through the link,
		// truncating a file outside the tree (audit SA-021). Mirrors discovery.
		let Ok(ft) = e.file_type() else { continue };
		if ft.is_symlink() {
			continue;
		}
		if ft.is_dir() {
			if p.file_name().is_some_and(|n| WALK_SKIP.iter().any(|s| n == *s)) {
				continue;
			}
			walk(&p, out, depth + 1);
		} else if ft.is_file() && p.extension().is_some_and(|x| x == "run") {
			out.push(p);
		}
	}
}

/// Lint `files`, and answer with the status that says whether they passed.
///
/// `project` is the catalog they were listed from. Without one -- paths named on
/// the command line -- each file's project is discovered from where it sits,
/// which is how an editor finds it.
pub fn lint(files: &[PathBuf], project: Option<&Catalog>, check: bool, to_stdout: bool) -> ExitCode {
	let report = Report::new(to_stdout);
	let mut tally = Tally::default();
	let mut found: HashMap<PathBuf, Option<Catalog>> = HashMap::new();
	for file in files {
		// Formatted first, so what is checked -- and every position reported -- is
		// the file as it now stands on disk.
		let Some(text) = shape(file, check, to_stdout, &report, &mut tally) else {
			continue;
		};
		let cat = match project {
			Some(cat) => Some(cat),
			None => found
				.entry(directory_of(file))
				.or_insert_with_key(|dir| catalog(dir).ok())
				.as_ref(),
		};
		let read = |p: &Path| runfile_discovery::read_runfile(p).ok();
		for d in runfile_lsp::document::diagnostics(&text, Some(file), cat, &read) {
			report.problem(
				file,
				Some((d.range.start_line, d.range.start_col)),
				d.severity,
				&d.message,
			);
			if d.severity == Severity::Error {
				tally.error(file);
			}
		}
	}
	report.summary(files.len(), &tally);
	if tally.errors > 0 || (check && tally.unformatted > 0) {
		ExitCode::FAILURE
	} else {
		ExitCode::SUCCESS
	}
}

/// Put one file into shape, as far as asked: write it, say that it needs it, or
/// print it. Hands back the text the file holds afterwards, or `None` when it
/// could not be read at all.
fn shape(file: &Path, check: bool, to_stdout: bool, report: &Report, tally: &mut Tally) -> Option<String> {
	let src = match runfile_discovery::read_runfile(file) {
		Ok(s) => s,
		Err(e) => {
			report.problem(file, None, Severity::Error, &e.to_string());
			tally.error(file);
			return None;
		}
	};
	let out = match runfile_lang::format(&src) {
		Ok(out) => out,
		// One that does not parse is reported with the rest of its problems, and
		// left alone: reindenting a file whose blocks do not close is guesswork.
		Err(_) if runfile_lang::parse(&src).is_err() => return Some(src),
		// One that parses and still will not lay out is the formatter refusing to
		// change what the file means.
		Err(e) => {
			report.problem(file, None, Severity::Error, &format!("could not be laid out: {e}"));
			tally.error(file);
			return Some(src);
		}
	};
	if to_stdout {
		print!("{out}");
		return Some(src);
	}
	if out == src {
		return Some(src);
	}
	if check {
		report.note(file, "needs formatting");
		tally.unformatted += 1;
		return Some(src);
	}
	// Never rewrite a symlink: `fs::write` would follow it and truncate the file
	// it points at, outside the tree being linted (audit SA-021). Report it and
	// leave both the link and its target untouched; lint the target directly.
	if file.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()) {
		report.problem(
			file,
			None,
			Severity::Error,
			"is a symlink; not rewritten (lint its target directly)",
		);
		tally.error(file);
		return Some(src);
	}
	// Write atomically -- a temp file beside the target, then rename over it --
	// so an interrupt mid-write cannot leave the file truncated (audit SA-021).
	let mut tmp = file.as_os_str().to_owned();
	tmp.push(".lint-tmp");
	let tmp = PathBuf::from(tmp);
	let written = std::fs::write(&tmp, &out).and_then(|()| std::fs::rename(&tmp, file));
	match written {
		Ok(()) => {
			report.note(file, "formatted");
			tally.formatted += 1;
			Some(out)
		}
		Err(e) => {
			let _ = std::fs::remove_file(&tmp);
			report.problem(file, None, Severity::Error, &e.to_string());
			tally.error(file);
			Some(src)
		}
	}
}

/// Where discovery starts for a file named on the command line.
fn directory_of(file: &Path) -> PathBuf {
	match file.parent() {
		Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
		_ => PathBuf::from("."),
	}
}

/// What a run of `:lint` found.
#[derive(Default)]
struct Tally {
	formatted: usize,
	unformatted: usize,
	errors: usize,
	/// Every file with an error, counted once each.
	failing: BTreeSet<PathBuf>,
}

impl Tally {
	fn error(&mut self, file: &Path) {
		self.errors += 1;
		self.failing.insert(file.to_path_buf());
	}
}

/// Where the report goes: stdout, unless `--stdout` has given stdout to the
/// files themselves.
struct Report {
	stderr: bool,
	paint: bool,
}

impl Report {
	fn new(to_stdout: bool) -> Self {
		Report {
			stderr: to_stdout,
			paint: runfile_runtime::exec::paints(to_stdout),
		}
	}

	fn say(&self, line: &str) {
		if self.stderr {
			eprintln!("{line}");
		} else {
			println!("{line}");
		}
	}

	fn note(&self, file: &Path, what: &str) {
		self.say(&format!("{}: {what}", shown(file)));
	}

	/// One problem, as `path:line:column: error: message` -- one-based, which
	/// is what a terminal turns into a link to the place.
	fn problem(&self, file: &Path, at: Option<(usize, usize)>, severity: Severity, message: &str) {
		let place = match at {
			Some((line, col)) => format!("{}:{}:{}", shown(file), line + 1, col + 1),
			None => shown(file),
		};
		// A shell finding says what to write instead on a line of its own, which
		// sits under the problem it answers.
		let (first, rest) = message.split_once('\n').unwrap_or((message, ""));
		self.say(&format!("{place}: {}: {first}", self.severity(severity)));
		for line in rest.lines() {
			self.say(&format!("  {line}"));
		}
	}

	fn severity(&self, s: Severity) -> String {
		let (word, code) = match s {
			Severity::Error => ("error", "1;31"),
			Severity::Warning => ("warning", "1;33"),
			Severity::Information => ("info", "1;36"),
			Severity::Hint => ("hint", "2"),
		};
		if self.paint {
			format!("\x1b[{code}m{word}\x1b[0m")
		} else {
			word.to_string()
		}
	}

	fn summary(&self, files: usize, t: &Tally) {
		let mut parts = Vec::new();
		if t.formatted > 0 {
			parts.push(format!("{} formatted", t.formatted));
		}
		if t.unformatted > 0 {
			let verb = if t.unformatted == 1 { "needs" } else { "need" };
			parts.push(format!("{} {verb} formatting", t.unformatted));
		}
		parts.push(match t.errors {
			0 => "no errors".to_string(),
			n => format!(
				"{n} {} in {} {}",
				plural(n, "error"),
				t.failing.len(),
				plural(t.failing.len(), "file")
			),
		});
		self.say(&format!(
			"{files} {} checked: {}",
			plural(files, "file"),
			parts.join(", ")
		));
	}
}

fn plural(n: usize, word: &str) -> String {
	if n == 1 { word.to_string() } else { format!("{word}s") }
}

/// A path as a person at this terminal would write it: from where they stand,
/// when it is below there.
fn shown(p: &Path) -> String {
	let cwd = std::env::current_dir().ok();
	cwd.as_deref()
		.and_then(|c| p.strip_prefix(c).ok())
		.unwrap_or(p)
		.display()
		.to_string()
}
