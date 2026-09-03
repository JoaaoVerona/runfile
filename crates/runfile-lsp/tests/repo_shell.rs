//! Shellcheck, over every `.run` file in this repository.
//!
//! The spec asked for this as a CI gate. It lives here rather than in a
//! workflow step because the extraction is this crate's job: pulling the shell
//! out of a `.run` file, rendering interpolations as placeholders, and mapping
//! findings back are exactly what `shell.rs` does, so the gate exercises the
//! same path an editor does instead of a second approximation of it.
//!
//! Skipped when shellcheck is not installed, so a contributor without it does
//! not see a broken build.

use std::path::{Path, PathBuf};

use runfile_lsp::analysis::Severity;

fn repo_root() -> PathBuf {
	// crates/runfile-lsp -> the workspace root.
	Path::new(env!("CARGO_MANIFEST_DIR"))
		.parent()
		.and_then(Path::parent)
		.expect("a workspace root")
		.to_path_buf()
}

fn have_shellcheck() -> bool {
	std::process::Command::new("shellcheck")
		.arg("--version")
		.stdout(std::process::Stdio::null())
		.stderr(std::process::Stdio::null())
		.status()
		.is_ok_and(|s| s.success())
}

/// Every `.run` file under a `runfiles/` directory in the repository.
fn run_files(dir: &Path, out: &mut Vec<PathBuf>) {
	let Ok(entries) = std::fs::read_dir(dir) else { return };
	for e in entries.flatten() {
		let p = e.path();
		let name = e.file_name();
		let name = name.to_string_lossy();
		if p.is_dir() {
			if !matches!(name.as_ref(), "target" | "node_modules" | ".git" | "dist" | "build")
				&& !name.starts_with("target-")
			{
				run_files(&p, out);
			}
		} else if p.extension().is_some_and(|x| x == "run") {
			out.push(p);
		}
	}
}

#[test]
fn every_shell_line_in_the_repository_passes_shellcheck() {
	if !have_shellcheck() {
		eprintln!("skipped: shellcheck is not installed");
		return;
	}
	let root = repo_root();
	let mut files = Vec::new();
	run_files(&root, &mut files);
	files.sort();
	assert!(
		files.len() >= 20,
		"only {} .run files found; the sweep is not finding them",
		files.len()
	);

	let mut problems = Vec::new();
	for f in &files {
		let Ok(src) = std::fs::read_to_string(f) else { continue };
		for d in runfile_lsp::shell::diagnose(&src, "sh", "shellcheck") {
			// Style and information are opinions; an error or a warning is the
			// gate. Both would fail a person's own `shellcheck` run.
			if matches!(d.severity, Severity::Error | Severity::Warning) {
				let rel = f.strip_prefix(&root).unwrap_or(f);
				problems.push(format!("{}:{}: {}", rel.display(), d.range.start_line + 1, d.message));
			}
		}
	}
	assert!(
		problems.is_empty(),
		"shellcheck found {} problem(s) in this repository's own shell:\n  {}",
		problems.len(),
		problems.join("\n  ")
	);
}
