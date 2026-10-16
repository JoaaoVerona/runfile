//! Watch mode: re-run a target when files it names change.
//!
//! Entered automatically when a target declares `.watch` — there is no flag,
//! because a target that wants watching says so once in its own file.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use globset::{Glob, GlobSet, GlobSetBuilder};
use notify::{EventKind, RecursiveMode, Watcher};
use runfile_runtime::props::WatchPattern;

/// Filesystem events arrive in bursts — a single editor save writes, renames
/// and chmods — so a quiet period is required before re-running.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// Compiled include/exclude sets, one pair for each directory the patterns
/// were written under. A `!`-prefixed pattern excludes; exclusion always wins,
/// so `["src/**", "!src/**/*.tmp"]` reads the way it looks -- and an exclusion
/// written under one `.workdir` still beats an inclusion written under
/// another, since both are about the same file.
#[derive(Debug)]
pub struct Patterns {
	dirs: Vec<Under>,
}

/// The patterns written under one directory, which is what they are matched
/// beneath. Kept apart from the pattern rather than joined onto it, so a
/// directory whose name holds a `[` or a `*` is not read as a glob.
#[derive(Debug)]
struct Under {
	root: PathBuf,
	include: GlobSet,
	exclude: GlobSet,
	includes_any: bool,
}

impl Patterns {
	pub fn compile(patterns: &[WatchPattern]) -> Result<Self, String> {
		let mut groups: Vec<(PathBuf, GlobSetBuilder, GlobSetBuilder, bool)> = Vec::new();
		for w in patterns {
			let at = match groups.iter().position(|g| g.0 == w.dir) {
				Some(i) => i,
				None => {
					groups.push((w.dir.clone(), GlobSetBuilder::new(), GlobSetBuilder::new(), false));
					groups.len() - 1
				}
			};
			let group = &mut groups[at];
			let (set, pat) = match w.pattern.strip_prefix('!') {
				Some(rest) => (&mut group.2, rest),
				None => {
					group.3 = true;
					(&mut group.1, w.pattern.as_str())
				}
			};
			let glob = Glob::new(pat).map_err(|e| format!("bad watch pattern `{pat}`: {e}"))?;
			set.add(glob);
		}
		let dirs = groups
			.into_iter()
			.map(|(root, include, exclude, includes_any)| {
				Ok(Under {
					root,
					include: include.build().map_err(|e| e.to_string())?,
					exclude: exclude.build().map_err(|e| e.to_string())?,
					includes_any,
				})
			})
			.collect::<Result<_, String>>()?;
		Ok(Self { dirs })
	}

	/// Each pattern is relative to the directory it was written under, and
	/// matched with forward slashes on every platform so one Runfile works
	/// everywhere.
	pub fn matches(&self, path: &Path) -> bool {
		let rel = |root: &Path| {
			path.strip_prefix(root)
				.ok()
				.map(|r| r.to_string_lossy().replace('\\', "/"))
		};
		let included = self
			.dirs
			.iter()
			.any(|u| rel(&u.root).is_some_and(|r| u.include.is_match(&r)));
		included
			&& !self
				.dirs
				.iter()
				.any(|u| rel(&u.root).is_some_and(|r| u.exclude.is_match(&r)))
	}

	/// What to watch: every directory something is included beneath, less any
	/// inside another, since each is watched recursively and would otherwise
	/// report the same change twice.
	pub fn roots(&self) -> Vec<&Path> {
		let mut all: Vec<&Path> = self
			.dirs
			.iter()
			.filter(|u| u.includes_any)
			.map(|u| u.root.as_path())
			.collect();
		all.sort();
		let mut out: Vec<&Path> = Vec::new();
		for r in all {
			if !out.iter().any(|kept| r.starts_with(kept)) {
				out.push(r);
			}
		}
		out
	}
}

/// Block until a watched path changes, or return `None` if the watcher died.
///
/// Split from the loop so the debounce and match logic can be tested without
/// spawning a real target.
fn next_change(rx: &Receiver<PathBuf>, pats: &Patterns) -> Option<()> {
	loop {
		let first = rx.recv().ok()?;
		if !pats.matches(&first) {
			continue;
		}
		// Absorb the rest of the burst; each new event restarts the timer.
		let mut deadline = Instant::now() + DEBOUNCE;
		loop {
			match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
				Ok(p) if pats.matches(&p) => deadline = Instant::now() + DEBOUNCE,
				Ok(_) => {}
				Err(RecvTimeoutError::Timeout) => return Some(()),
				Err(RecvTimeoutError::Disconnected) => return None,
			}
		}
	}
}

/// Run `once` now, then again after every debounced change a pattern matches.
///
/// Never returns on its own: watch mode ends with Ctrl+C. A failing run is
/// reported and watching continues — the next save is the retry.
pub fn watch(patterns: &[WatchPattern], mut once: impl FnMut()) -> Result<(), String> {
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
	for root in pats.roots() {
		watcher
			.watch(root, RecursiveMode::Recursive)
			.map_err(|e| format!("cannot watch {}: {e}", root.display()))?;
	}

	once();
	eprintln!("{} watching for changes — Ctrl+C to stop", runfile_runtime::exec::tag());
	while next_change(&rx, &pats).is_some() {
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

	/// Patterns written with the working directory at `dir`.
	fn under(dir: &str, list: &[&str]) -> Vec<WatchPattern> {
		list.iter()
			.map(|s| WatchPattern {
				dir: PathBuf::from(dir),
				pattern: s.to_string(),
			})
			.collect()
	}

	fn p(list: &[&str]) -> Patterns {
		Patterns::compile(&under("/proj", list)).unwrap()
	}

	#[test]
	fn patterns_match_relative_to_the_directory_they_were_written_under() {
		assert!(p(&["src/**/*.rs"]).matches(Path::new("/proj/src/a/b.rs")));
		assert!(!p(&["src/**/*.rs"]).matches(Path::new("/proj/docs/a.rs")));
		// Under `.workdir = "web"`, `src/**` is `web/src`, and nothing else.
		let web = Patterns::compile(&under("/proj/web", &["src/**"])).unwrap();
		assert!(web.matches(Path::new("/proj/web/src/app.ts")));
		assert!(!web.matches(Path::new("/proj/src/main.rs")));
	}

	#[test]
	fn a_bang_prefix_excludes_and_beats_the_include() {
		let pats = p(&["src/**", "!src/**/*.tmp"]);
		assert!(pats.matches(Path::new("/proj/src/main.rs")));
		assert!(!pats.matches(Path::new("/proj/src/x.tmp")), "exclusion wins");
	}

	#[test]
	fn an_exclusion_under_one_directory_beats_an_inclusion_under_another() {
		let mut list = under("/proj", &["**"]);
		list.extend(under("/proj/web", &["!node_modules/**"]));
		let pats = Patterns::compile(&list).unwrap();
		assert!(pats.matches(Path::new("/proj/web/src/a.ts")));
		assert!(!pats.matches(Path::new("/proj/web/node_modules/x/a.js")));
		assert!(pats.matches(Path::new("/proj/node_modules/x/a.js")), "only under web");
	}

	#[test]
	fn a_path_matching_nothing_included_is_ignored() {
		assert!(!p(&["!x"]).matches(Path::new("/proj/x")));
	}

	#[test]
	fn a_directory_name_holding_glob_characters_is_not_read_as_a_glob() {
		let pats = Patterns::compile(&under("/work/app [old]", &["*.rs"])).unwrap();
		assert!(pats.matches(Path::new("/work/app [old]/main.rs")));
		assert!(!pats.matches(Path::new("/work/app o/main.rs")));
	}

	#[test]
	fn every_directory_something_is_included_beneath_is_watched_once() {
		let mut list = under("/proj", &["src/**"]);
		list.extend(under("/proj/web", &["src/**"]));
		list.extend(under("/elsewhere", &["*.toml"]));
		list.extend(under("/only-excluded", &["!x"]));
		let pats = Patterns::compile(&list).unwrap();
		assert_eq!(
			pats.roots(),
			vec![Path::new("/elsewhere"), Path::new("/proj")],
			"`/proj/web` is inside `/proj`, and an exclusion alone watches nothing"
		);
	}

	// Only meaningful where `Path` treats `\\` as a separator; on Unix the whole
	// string is one component and there is nothing to normalize.
	#[cfg(windows)]
	#[test]
	fn windows_separators_match_forward_slash_patterns() {
		let pats = Patterns::compile(&under(r"C:\proj", &["src/**/*.rs"])).unwrap();
		assert!(pats.matches(Path::new(r"C:\proj\src\a.rs")));
	}

	#[test]
	fn a_bad_pattern_is_reported_not_ignored() {
		let e = Patterns::compile(&under("/proj", &["src/["])).unwrap_err();
		assert!(e.contains("bad watch pattern"), "{e}");
	}

	#[test]
	fn changes_are_debounced_into_one_wakeup() {
		let (tx, rx) = channel();
		let a = PathBuf::from("/proj");
		for _ in 0..5 {
			tx.send(a.join("src/main.rs")).unwrap();
		}
		assert!(next_change(&rx, &p(&["src/**"])).is_some());
		// The whole burst was consumed by one wakeup.
		drop(tx);
		assert!(next_change(&rx, &p(&["src/**"])).is_none());
	}

	#[test]
	fn unmatched_events_never_wake_the_loop() {
		let (tx, rx) = channel();
		let a = PathBuf::from("/proj");
		tx.send(a.join("target/debug/x")).unwrap();
		drop(tx);
		assert!(next_change(&rx, &p(&["src/**"])).is_none(), "noise is skipped");
	}
}
