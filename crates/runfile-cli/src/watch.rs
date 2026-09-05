//! Watch mode: re-run a target when files it names change.
//!
//! Entered automatically when a target declares `.watch` — there is no flag,
//! because a target that wants watching says so once in its own file.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use globset::{Glob, GlobSet, GlobSetBuilder};
use notify::{EventKind, RecursiveMode, Watcher};

/// Filesystem events arrive in bursts — a single editor save writes, renames
/// and chmods — so a quiet period is required before re-running.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// Compiled include/exclude sets. A `!`-prefixed pattern excludes; exclusion
/// always wins, so `["src/**", "!src/**/*.tmp"]` reads the way it looks.
#[derive(Debug)]
pub struct Patterns {
	include: GlobSet,
	exclude: GlobSet,
}

impl Patterns {
	pub fn compile(patterns: &[String]) -> Result<Self, String> {
		let (mut inc, mut exc) = (GlobSetBuilder::new(), GlobSetBuilder::new());
		for p in patterns {
			let (set, pat) = match p.strip_prefix('!') {
				Some(rest) => (&mut exc, rest),
				None => (&mut inc, p.as_str()),
			};
			let glob = Glob::new(pat).map_err(|e| format!("bad watch pattern `{pat}`: {e}"))?;
			set.add(glob);
		}
		Ok(Self {
			include: inc.build().map_err(|e| e.to_string())?,
			exclude: exc.build().map_err(|e| e.to_string())?,
		})
	}

	/// Patterns are relative to the anchor, and matched with forward slashes on
	/// every platform so one Runfile works everywhere.
	pub fn matches(&self, path: &Path, anchor: &Path) -> bool {
		let rel = path.strip_prefix(anchor).unwrap_or(path);
		let rel = rel.to_string_lossy().replace('\\', "/");
		self.include.is_match(&rel) && !self.exclude.is_match(&rel)
	}
}

/// Block until a watched path changes, or return `None` if the watcher died.
///
/// Split from the loop so the debounce and match logic can be tested without
/// spawning a real target.
fn next_change(rx: &Receiver<PathBuf>, pats: &Patterns, anchor: &Path) -> Option<()> {
	loop {
		let first = rx.recv().ok()?;
		if !pats.matches(&first, anchor) {
			continue;
		}
		// Absorb the rest of the burst; each new event restarts the timer.
		let mut deadline = Instant::now() + DEBOUNCE;
		loop {
			match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
				Ok(p) if pats.matches(&p, anchor) => deadline = Instant::now() + DEBOUNCE,
				Ok(_) => {}
				Err(RecvTimeoutError::Timeout) => return Some(()),
				Err(RecvTimeoutError::Disconnected) => return None,
			}
		}
	}
}

/// Run `once` now, then again after every debounced change under `anchor`.
///
/// Never returns on its own: watch mode ends with Ctrl+C. A failing run is
/// reported and watching continues — the next save is the retry.
pub fn watch(anchor: &Path, patterns: &[String], mut once: impl FnMut()) -> Result<(), String> {
	let pats = Patterns::compile(patterns)?;
	let (tx, rx) = channel();
	let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
		let Ok(ev) = res else { return };
		if matches!(ev.kind, EventKind::Access(_)) {
			return;
		}
		for p in ev.paths {
			let _ = tx.send(p);
		}
	})
	.map_err(|e| format!("cannot watch: {e}"))?;
	watcher
		.watch(anchor, RecursiveMode::Recursive)
		.map_err(|e| format!("cannot watch {}: {e}", anchor.display()))?;

	once();
	eprintln!("{} watching for changes — Ctrl+C to stop", runfile_runtime::exec::tag());
	while next_change(&rx, &pats, anchor).is_some() {
		// The message says Ctrl+C stops it, so it has to.
		if runfile_runtime::interrupt::interrupted() {
			break;
		}
		once();
		eprintln!("{} watching for changes — Ctrl+C to stop", runfile_runtime::exec::tag());
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	fn p(list: &[&str]) -> Patterns {
		Patterns::compile(&list.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
	}

	#[test]
	fn patterns_match_relative_to_the_anchor() {
		let a = Path::new("/proj");
		assert!(p(&["src/**/*.rs"]).matches(Path::new("/proj/src/a/b.rs"), a));
		assert!(!p(&["src/**/*.rs"]).matches(Path::new("/proj/docs/a.rs"), a));
	}

	#[test]
	fn a_bang_prefix_excludes_and_beats_the_include() {
		let pats = p(&["src/**", "!src/**/*.tmp"]);
		let a = Path::new("/proj");
		assert!(pats.matches(Path::new("/proj/src/main.rs"), a));
		assert!(!pats.matches(Path::new("/proj/src/x.tmp"), a), "exclusion wins");
	}

	#[test]
	fn a_path_matching_nothing_included_is_ignored() {
		assert!(!p(&["!x"]).matches(Path::new("/proj/x"), Path::new("/proj")));
	}

	// Only meaningful where `Path` treats `\\` as a separator; on Unix the whole
	// string is one component and there is nothing to normalize.
	#[cfg(windows)]
	#[test]
	fn windows_separators_match_forward_slash_patterns() {
		let pats = p(&["src/**/*.rs"]);
		assert!(pats.matches(Path::new(r"C:\proj\src\a.rs"), Path::new(r"C:\proj")));
	}

	#[test]
	fn a_bad_pattern_is_reported_not_ignored() {
		let e = Patterns::compile(&["src/[".to_string()]).unwrap_err();
		assert!(e.contains("bad watch pattern"), "{e}");
	}

	#[test]
	fn changes_are_debounced_into_one_wakeup() {
		let (tx, rx) = channel();
		let a = PathBuf::from("/proj");
		for _ in 0..5 {
			tx.send(a.join("src/main.rs")).unwrap();
		}
		assert!(next_change(&rx, &p(&["src/**"]), &a).is_some());
		// The whole burst was consumed by one wakeup.
		drop(tx);
		assert!(next_change(&rx, &p(&["src/**"]), &a).is_none());
	}

	#[test]
	fn unmatched_events_never_wake_the_loop() {
		let (tx, rx) = channel();
		let a = PathBuf::from("/proj");
		tx.send(a.join("target/debug/x")).unwrap();
		drop(tx);
		assert!(next_change(&rx, &p(&["src/**"]), &a).is_none(), "noise is skipped");
	}
}
