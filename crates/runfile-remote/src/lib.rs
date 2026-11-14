//! Packages fetched from a repository, and the files `download()` asks for,
//! kept in the machine's cache.
//!
//! `run github.com/JoaaoVerona/j18n@v0.14.1 sync` names a repository at a
//! version. Git fetches it -- over HTTPS, which every host serves, so GitHub,
//! GitLab, Bitbucket, Gitea and Forgejo need no code of their own -- once per
//! commit, and the tree is kept without its `.git`:
//!
//! ```text
//! <cache>/packages/<host>/<owner>/<repo>/refs.json        what each tag and branch was, and when it was asked
//! <cache>/packages/<host>/<owner>/<repo>/<commit>-<hash>/ a package's runfiles/ at that commit, and nothing else
//! <cache>/packages/<host>/<owner>/<repo>/<commit>/        the whole repository at that commit
//! <cache>/downloads/<hash of the URL>/                    what download() fetched, unpacked
//! ```
//!
//! **Only a package's `runfiles/` is fetched**, unless its `_main.run` says
//! `.requires-whole-repository`: a package starts in the directory of whoever
//! ran it, so the rest of its repository is reachable only through
//! `RUN.parent` and `RUN.file`, and a package that uses neither needs nothing
//! else. Its entry is read in between, and the rest fetched when it asks. What
//! is kept is decided again from the entry each time, so a copy of only
//! `runfiles/` -- or a folder an older runner kept -- whose entry asks for more
//! has the rest fetched rather than run without it.
//!
//! **A commit names one tree forever**, so a package pinned to one is fetched
//! once and never asked about again. **A tag or a branch can move**, so the
//! host is asked what it points at again once [`RECHECK`] has passed since it
//! last was -- and when that question fails, offline or with the host down, the
//! commit it pointed at last time is used: a run that worked yesterday keeps
//! working.
//!
//! Everything lands in a temporary folder under the cache first and is renamed
//! into place, so a run interrupted half-way leaves nothing that looks
//! finished, and two runs fetching the same thing at once both end up with it.

mod git;
mod inventory;
mod unpack;
mod zip;

pub use inventory::{CachedDownload, CachedTree, Leftover, Pruned, human_age, human_size, parse_age};

use runfile_lang::reference::{ENTRY, Remote, Rev};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long a tag or a branch is taken at its word before the host is asked
/// again: a week.
pub const RECHECK: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Overrides where the cache is, used verbatim: a test's, or a CI job's that
/// keeps its cache between runs.
pub const CACHE_DIR_ENV_VAR: &str = "RUNFILE_CACHE_DIR";

/// Whether a lookup may reach the network, and how much it should trust what
/// it already has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
	/// Fetch what is missing, and ask about a tag or branch once its week is up.
	Allowed,
	/// Ask about a tag or branch now, however recently it was asked: `run
	/// :packages fetch`, which is what someone runs to pick a moved branch up today.
	Refresh,
	/// Use the cache, at any age, and fetch nothing: `--dry-run` and an editor.
	Offline,
}

/// Something that went wrong, as the sentence the runner prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteError(pub String);

impl std::fmt::Display for RemoteError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(&self.0)
	}
}

impl std::error::Error for RemoteError {}

fn fail<T>(msg: impl Into<String>) -> Result<T, RemoteError> {
	Err(RemoteError(msg.into()))
}

/// What a version turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
	pub commit: String,
	/// The tag it was found as, `v0.14.1`, which is what `RUN.version` says;
	/// empty for a branch or a commit.
	pub tag: String,
}

/// A package ready to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
	/// The folder holding its `runfiles/`, which is `RUN.parent`.
	pub root: PathBuf,
	pub commit: String,
	/// See [`Resolved::tag`].
	pub version: String,
}

/// The machine's cache.
#[derive(Debug, Clone)]
pub struct Cache {
	root: PathBuf,
	/// `-c` settings every git call is given: only ever a test's, pointing a
	/// host at a folder of repositories.
	git_config: Vec<String>,
}

impl Cache {
	pub fn new(root: PathBuf) -> Self {
		Self {
			root,
			git_config: Vec::new(),
		}
	}

	/// Fetch what is under `from` (`https://example.test/`) from `to` (a
	/// `file://` folder) instead, the way git's own `insteadOf` does -- handed
	/// to each git call, rather than set in the environment a whole test
	/// binary shares.
	#[doc(hidden)]
	pub fn rewriting(mut self, from: &str, to: &str) -> Self {
		self.git_config.push(format!("url.{to}.insteadOf={from}"));
		self
	}

	/// [`CACHE_DIR_ENV_VAR`], else `runfile` in the platform's cache directory:
	/// `~/.cache/runfile` on Linux, `~/Library/Caches/runfile` on macOS and
	/// `%LOCALAPPDATA%\runfile` on Windows.
	pub fn default_location() -> Option<PathBuf> {
		if let Some(dir) = std::env::var_os(CACHE_DIR_ENV_VAR)
			&& !dir.is_empty()
		{
			return Some(PathBuf::from(dir));
		}
		dirs::cache_dir().map(|d| d.join("runfile"))
	}

	pub fn open_default() -> Option<Self> {
		Self::default_location().map(Self::new)
	}

	pub fn root(&self) -> &Path {
		&self.root
	}

	/// Whether anything of this repository has been fetched on this machine:
	/// what a reference typed at the terminal asks consent for the first time.
	///
	/// A tree, not a lookup: asking what a tag points at -- which the question
	/// itself does, to say which commit it would run -- is not consent given.
	pub fn knows(&self, r: &Remote) -> bool {
		std::fs::read_dir(self.repo_dir(r))
			.map(|d| d.flatten().any(|e| e.file_type().is_ok_and(|t| t.is_dir())))
			.unwrap_or(false)
	}

	/// The commit a reference stands for now, by the rules in the module's
	/// description.
	pub fn resolve(&self, r: &Remote, network: Network) -> Result<Resolved, RemoteError> {
		let name = match &r.rev {
			Rev::Commit(c) => {
				return Ok(Resolved {
					commit: c.clone(),
					tag: String::new(),
				});
			}
			Rev::Name(n) => n,
		};
		let refs = self.repo_dir(r).join("refs.json");
		let known = read_refs(&refs).remove(name);
		if let Some(k) = &known {
			let fresh = now().saturating_sub(k.checked) < RECHECK.as_secs();
			if network == Network::Offline || (fresh && network == Network::Allowed) {
				return Ok(k.resolved.clone());
			}
		}
		if network == Network::Offline {
			return fail(not_fetched(r));
		}
		// A week-old answer is asked about briefly, since it has one to fall
		// back on; a first one is given the time a slow host needs.
		let wait = match &known {
			Some(_) if network == Network::Allowed => Duration::from_secs(30),
			_ => Duration::from_secs(120),
		};
		match git::ls_remote(r, name, wait, &self.git_config) {
			Ok(found) => {
				write_ref(&refs, name, &found);
				Ok(found)
			}
			// Kept using: the commit it pointed at the last time it was asked.
			Err(_) if network == Network::Allowed && known.is_some() => Ok(known.expect("known").resolved),
			Err(e) => Err(e),
		}
	}

	/// The package a reference names, fetched if it is not here yet.
	///
	/// `say` is told before anything is fetched, so a run that pauses for the
	/// network says why.
	pub fn package(&self, r: &Remote, network: Network, say: &dyn Fn(&str)) -> Result<Package, RemoteError> {
		let resolved = self.resolve(r, network)?;
		let whole = self.repo_dir(r).join(&resolved.commit);
		if whole.is_dir() {
			return package_at(r, &whole, resolved);
		}
		let thin = self.thin_dir(r, &resolved.commit);
		if thin.is_dir() && !requires_whole(r, &thin)? {
			return package_at(r, &thin, resolved);
		}
		if network == Network::Offline {
			return fail(not_fetched(r));
		}
		say(&format!("fetching {} ({})", r.shown(), short(&resolved.commit)));
		let tmp = self.scratch()?;
		let out = git::checkout(r, &resolved.commit, &tmp, &self.git_config, &|tree| {
			requires_whole(r, tree)
		})
		.and_then(|(tree, is_whole)| {
			let into = if is_whole { &whole } else { &thin };
			settle(&tree, into)?;
			inventory::record_tree(into, &resolved.commit, (!is_whole).then_some(r.dir.as_str()));
			Ok(into.clone())
		});
		remove_tree(&tmp);
		package_at(r, &out?, resolved)
	}

	/// The package as the cache has it, at whatever age, without asking
	/// anything of anyone: what an editor shows about a reference.
	pub fn cached(&self, r: &Remote) -> Option<Package> {
		self.package(r, Network::Offline, &|_| {}).ok()
	}

	/// Fetch `url` once, verified against `pin` when one is given, and answer
	/// with the folder an archive was unpacked into or the file itself.
	pub fn download(&self, url: &str, pin: Option<&str>, say: &dyn Fn(&str)) -> Result<PathBuf, RemoteError> {
		unpack::download(self, url, pin, say)
	}

	fn repo_dir(&self, r: &Remote) -> PathBuf {
		let mut d = self.root.join("packages").join(r.host.replace(':', "_"));
		for seg in r.repo.split('/') {
			d.push(seg);
		}
		d
	}

	/// Where a package's `runfiles/` alone is kept: one per folder of the
	/// repository a package lives in, its root included.
	fn thin_dir(&self, r: &Remote, commit: &str) -> PathBuf {
		let h = hex::encode(Sha256::digest(r.dir.as_bytes()));
		self.repo_dir(r).join(format!("{commit}-{}", &h[..12]))
	}

	/// A fresh, empty folder under the cache for something not finished yet.
	/// Inside the cache, so the rename that finishes it never crosses devices.
	fn scratch(&self) -> Result<PathBuf, RemoteError> {
		static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
		let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
		let nanos = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.map_or(0, |d| d.subsec_nanos());
		let dir = self
			.root
			.join("tmp")
			.join(format!("{}-{n}-{nanos}", std::process::id()));
		std::fs::create_dir_all(&dir).map_err(|e| RemoteError(format!("could not create {}: {e}", dir.display())))?;
		Ok(dir)
	}
}

/// Whether the entry of the package in `tree` asks for its whole repository.
///
/// A missing or unreadable entry answers no, and is then reported as what it
/// is -- not a package, or a file that does not parse -- by whatever reads it
/// next, in the words it always uses.
fn requires_whole(r: &Remote, tree: &Path) -> Result<bool, RemoteError> {
	let mut entry = tree.to_path_buf();
	for seg in r.dir.split('/').filter(|s| !s.is_empty()) {
		entry.push(seg);
	}
	entry.push("runfiles");
	entry.push(format!("{ENTRY}.run"));
	// What the runner reads a runfile as, and no more: a file bigger than any
	// it would read is refused there, not read here.
	let small = std::fs::metadata(&entry).is_ok_and(|m| m.is_file() && m.len() <= 1 << 20);
	let Some(target) = small
		.then(|| std::fs::read_to_string(&entry).ok())
		.flatten()
		.and_then(|src| runfile_lang::parse(&src).ok())
	else {
		return Ok(false);
	};
	runfile_lang::reference::requires_whole_repository(&target)
		.map_err(|e| RemoteError(format!("`{}`: runfiles/{ENTRY}.run: {e}", r.shown())))
}

/// Move a finished `from` to `to`, unless another run got there first, which
/// is as good.
pub(crate) fn settle(from: &Path, to: &Path) -> Result<(), RemoteError> {
	if let Some(parent) = to.parent() {
		std::fs::create_dir_all(parent)
			.map_err(|e| RemoteError(format!("could not create {}: {e}", parent.display())))?;
	}
	match std::fs::rename(from, to) {
		Ok(()) => Ok(()),
		Err(_) if to.exists() => Ok(()),
		Err(e) => fail(format!("could not move into {}: {e}", to.display())),
	}
}

/// Remove a folder that may hold read-only files -- git writes its packs that
/// way, and Windows will not delete one until it is writable. Best effort: a
/// leftover under `tmp/` is harmless.
pub(crate) fn remove_tree(dir: &Path) {
	// A file -- what an interrupted download leaves -- is just removed.
	if std::fs::symlink_metadata(dir).is_ok_and(|m| !m.is_dir()) {
		let _ = std::fs::remove_file(dir);
		return;
	}
	if std::fs::remove_dir_all(dir).is_ok() || !dir.exists() {
		return;
	}
	fn writable(p: &Path) {
		let Ok(meta) = std::fs::symlink_metadata(p) else { return };
		if meta.is_dir()
			&& let Ok(rd) = std::fs::read_dir(p)
		{
			for e in rd.flatten() {
				writable(&e.path());
			}
		}
		let mut perms = meta.permissions();
		#[allow(clippy::permissions_set_readonly_false)]
		perms.set_readonly(false);
		let _ = std::fs::set_permissions(p, perms);
	}
	writable(dir);
	let _ = std::fs::remove_dir_all(dir);
}

fn package_at(r: &Remote, tree: &Path, resolved: Resolved) -> Result<Package, RemoteError> {
	let mut root = tree.to_path_buf();
	for seg in r.dir.split('/').filter(|s| !s.is_empty()) {
		root.push(seg);
	}
	let entry = root.join("runfiles").join(format!("{ENTRY}.run"));
	if !entry.is_file() {
		let place = if r.dir.is_empty() {
			String::from("its root")
		} else {
			format!("`{}`", r.dir)
		};
		return fail(format!(
			"`{}` is not a package: there is no `runfiles/{ENTRY}.run` in {place} at {}",
			r.shown(),
			short(&resolved.commit)
		));
	}
	Ok(Package {
		root,
		commit: resolved.commit,
		version: resolved.tag,
	})
}

fn not_fetched(r: &Remote) -> String {
	format!(
		"`{}` has not been fetched on this machine, and a preview fetches nothing; `run :packages fetch {}` fetches it",
		r.shown(),
		r.shown()
	)
}

/// The first seven characters, as git abbreviates a commit.
pub fn short(commit: &str) -> &str {
	&commit[..commit.len().min(7)]
}

pub(crate) fn now() -> u64 {
	SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

struct Known {
	resolved: Resolved,
	checked: u64,
}

fn read_refs(path: &Path) -> std::collections::HashMap<String, Known> {
	let Ok(text) = std::fs::read_to_string(path) else {
		return Default::default();
	};
	let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(&text) else {
		return Default::default();
	};
	map.into_iter()
		.filter_map(|(name, v)| {
			let commit = v.get("commit")?.as_str()?;
			if !runfile_lang::reference::is_full_commit(commit) {
				return None;
			}
			Some((
				name,
				Known {
					resolved: Resolved {
						commit: commit.to_string(),
						tag: v.get("tag").and_then(|t| t.as_str()).unwrap_or_default().to_string(),
					},
					checked: v.get("checked").and_then(serde_json::Value::as_u64).unwrap_or(0),
				},
			))
		})
		.collect()
}

/// Record what `name` was found to be, now. Best effort: a cache that cannot
/// be written is asked again next time, which costs a round trip and nothing
/// else.
fn write_ref(path: &Path, name: &str, found: &Resolved) {
	let mut map = match std::fs::read_to_string(path)
		.ok()
		.and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
	{
		Some(serde_json::Value::Object(m)) => m,
		_ => serde_json::Map::new(),
	};
	map.insert(
		name.to_string(),
		serde_json::json!({ "commit": found.commit, "tag": found.tag, "checked": now() }),
	);
	if let Some(parent) = path.parent() {
		let _ = std::fs::create_dir_all(parent);
	}
	let text = serde_json::to_string_pretty(&serde_json::Value::Object(map)).unwrap_or_default();
	let _ = runfile_crypto::atomic::write(path, text.as_bytes(), runfile_crypto::atomic::Mode::Keep);
}

/// Pretend `name` was last asked about `age` ago: for tests of the re-check.
#[doc(hidden)]
pub fn age_ref_for_test(cache: &Cache, r: &Remote, name: &str, age: Duration) {
	let path = cache.repo_dir(r).join("refs.json");
	let Ok(text) = std::fs::read_to_string(&path) else {
		return;
	};
	let Ok(serde_json::Value::Object(mut map)) = serde_json::from_str::<serde_json::Value>(&text) else {
		return;
	};
	if let Some(serde_json::Value::Object(e)) = map.get_mut(name) {
		e.insert("checked".into(), serde_json::json!(now().saturating_sub(age.as_secs())));
	}
	let _ = std::fs::write(
		&path,
		serde_json::to_string(&serde_json::Value::Object(map)).unwrap_or_default(),
	);
}
