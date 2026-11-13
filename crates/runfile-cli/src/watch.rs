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

/// Block until a watched path changes, or return `None` if the watcher died
/// or `stop` says the session is over.
///
/// Split from the loop so the debounce and match logic can be tested without
/// spawning a real target. `stop` is asked an `interrupt::POLL` at a time while
/// nothing changes, and after every event: a signal handler can only set a
/// flag, so a wait that does not look at it is a wait Ctrl+C cannot end -- it
/// used to be a plain `recv`, and the session went on until a file changed.
fn next_change(rx: &Receiver<PathBuf>, pats: &Patterns, stop: impl Fn() -> bool) -> Option<()> {
	use runfile_runtime::interrupt::POLL;
	loop {
		let first = match rx.recv_timeout(POLL) {
			Ok(p) => p,
			Err(RecvTimeoutError::Timeout) => {
				if stop() {
					return None;
				}
				continue;
			}
			Err(RecvTimeoutError::Disconnected) => return None,
		};
		if stop() {
			return None;
		}
		if !pats.matches(&first) {
			continue;
		}
		// Absorb the rest of the burst; each new event restarts the timer. A
		// burst that never ends is still asked about `stop` at every event.
		let mut deadline = Instant::now() + DEBOUNCE;
		loop {
			match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
				Ok(_) if stop() => return None,
				Ok(p) if pats.matches(&p) => deadline = Instant::now() + DEBOUNCE,
				Ok(_) => {}
				Err(RecvTimeoutError::Timeout) => return Some(()),
				Err(RecvTimeoutError::Disconnected) => return None,
			}
		}
	}
}

/// A watcher on a target's patterns, and what it has heard so far.
///
/// Started by the run itself, the moment the target's header is worked out and
/// before its first line: a `.watch` may read what its `_shared.run` chain set,
/// and the chain is lines that run, so nothing short of the run knows the
/// patterns. Started then rather than after the run, so a change made while the
/// first run is still going is heard and runs it again.
pub struct Watching {
	pats: Patterns,
	rx: Receiver<PathBuf>,
	// Held for its `Drop`, which stops it.
	_watcher: notify::RecommendedWatcher,
}

impl Watching {
	pub fn start(patterns: &[WatchPattern]) -> Result<Self, String> {
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
		Ok(Watching {
			pats,
			rx,
			_watcher: watcher,
		})
	}

	/// Run `once` again after every debounced change a pattern matches, the
	/// first run having been and gone.
	///
	/// Never returns on its own: watch mode ends with Ctrl+C, or any other
	/// signal the runner catches -- while it waits, or during a run, which a
	/// signal stops part-way. A failing run is reported and watching continues
	/// — the next save is the retry.
	pub fn follow(self, mut once: impl FnMut()) {
		// The message says Ctrl+C stops it, so it has to.
		let stop = runfile_runtime::interrupt::interrupted;
		while !stop() {
			eprintln!("{} watching for changes — Ctrl+C to stop", runfile_runtime::exec::tag());
			if next_change(&self.rx, &self.pats, stop).is_none() {
				return;
			}
			once();
		}
	}
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
		assert!(next_change(&rx, &p(&["src/**"]), never).is_some());
		// The whole burst was consumed by one wakeup.
		drop(tx);
		assert!(next_change(&rx, &p(&["src/**"]), never).is_none());
	}

	#[test]
	fn unmatched_events_never_wake_the_loop() {
		let (tx, rx) = channel();
		let a = PathBuf::from("/proj");
		tx.send(a.join("target/debug/x")).unwrap();
		drop(tx);
		assert!(next_change(&rx, &p(&["src/**"]), never).is_none(), "noise is skipped");
	}

	#[test]
	fn a_stop_ends_the_wait_with_nothing_changing() {
		// The sender is kept: a watcher that is alive and quiet is the wait a
		// Ctrl+C has to end, and a plain `recv` on it never returned.
		let (tx, rx) = channel::<PathBuf>();
		let asked = std::sync::atomic::AtomicUsize::new(0);
		let stop = || asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 2;
		assert!(next_change(&rx, &p(&["src/**"]), stop).is_none());
		assert!(asked.into_inner() >= 3, "it looked, and kept looking until told");
		drop(tx);
	}

	#[test]
	fn a_stop_wins_over_a_change_that_arrives_with_it() {
		let (tx, rx) = channel();
		tx.send(PathBuf::from("/proj/src/main.rs")).unwrap();
		assert!(
			next_change(&rx, &p(&["src/**"]), || true).is_none(),
			"no run after a stop"
		);
		drop(tx);
	}

	fn never() -> bool {
		false
	}
}
