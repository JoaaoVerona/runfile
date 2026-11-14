//! What the cache holds, how much room it takes, and taking it away.
//!
//! Read from the layout itself, so nothing has to be kept in step with it: a
//! repository is a folder under `packages/<host>/` holding commit-named trees,
//! and a download is a folder under `downloads/` holding a `meta.json`. Beside
//! each tree is a small record of what it is and when it was fetched; a tree
//! an older runner kept has none, and its folder's own time stands in.

use crate::{Cache, RemoteError, now, remove_tree};
use runfile_lang::reference::{Reference, Remote, Rev, is_full_commit};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// One copy of a repository at one commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedTree {
	/// `host/owner/repo`.
	pub repository: String,
	pub commit: String,
	/// The whole repository, or only a package's `runfiles/`.
	pub whole: bool,
	/// For a copy of `runfiles/` only, the package's folder in the repository
	/// (`""` for its root); `None` when the copy does not say, being older than
	/// the record.
	pub folder: Option<String>,
	/// The tags and branches the cache last found pointing at this commit.
	pub names: Vec<String>,
	/// Seconds since the epoch.
	pub fetched: u64,
	pub size: u64,
	pub path: PathBuf,
}

/// One file `download()` fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedDownload {
	pub url: String,
	pub sha256: String,
	/// The file's name, when it was kept whole rather than unpacked.
	pub file: Option<String>,
	pub fetched: u64,
	pub size: u64,
	pub path: PathBuf,
}

/// What an interrupted fetch left under `tmp/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leftover {
	pub path: PathBuf,
	pub size: u64,
	pub fetched: u64,
}

/// What a prune took away, or would have, and what it left.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pruned {
	pub trees: Vec<CachedTree>,
	pub downloads: Vec<CachedDownload>,
	pub leftovers: Vec<Leftover>,
	pub kept_trees: usize,
	pub kept_downloads: usize,
	pub kept_size: u64,
}

impl Pruned {
	pub fn freed(&self) -> u64 {
		self.trees.iter().map(|t| t.size).sum::<u64>()
			+ self.downloads.iter().map(|d| d.size).sum::<u64>()
			+ self.leftovers.iter().map(|l| l.size).sum::<u64>()
	}
}

/// How old a leftover of a fetch has to be before nothing can still be
/// writing it: past the longest any git call is given.
const STALE_LEFTOVER: u64 = 60 * 60;

impl Cache {
	/// Every copy of every repository, by repository and then oldest first.
	pub fn trees(&self) -> Vec<CachedTree> {
		let packages = self.root().join("packages");
		let mut out = Vec::new();
		for host in children(&packages).into_iter().filter(|p| p.is_dir()) {
			let name = host.file_name().unwrap_or_default().to_string_lossy().replace('_', ":");
			repositories(&host, &name, &mut out);
		}
		out.sort_by(|a, b| (&a.repository, a.fetched, &a.commit).cmp(&(&b.repository, b.fetched, &b.commit)));
		out
	}

	/// Every download, oldest first.
	pub fn downloads(&self) -> Vec<CachedDownload> {
		let mut out: Vec<CachedDownload> = children(&self.root().join("downloads"))
			.into_iter()
			.filter_map(|d| download_at(&d))
			.collect();
		out.sort_by(|a, b| (a.fetched, &a.url).cmp(&(b.fetched, &b.url)));
		out
	}

	/// What interrupted fetches left behind.
	pub fn leftovers(&self) -> Vec<Leftover> {
		children(&self.root().join("tmp"))
			.into_iter()
			.map(|p| Leftover {
				size: size_of(&p),
				fetched: modified(&p),
				path: p,
			})
			.collect()
	}

	/// Take away every copy and download fetched more than `older_than` ago --
	/// everything, given `None` -- and whatever interrupted fetches left more
	/// than an hour ago. With `dry_run`, only say what that would be.
	///
	/// Age is the time since it was fetched, not since it was last run: a copy
	/// is cheap to fetch again, and the clock it keeps is one nothing has to
	/// write on every run.
	pub fn prune(&self, older_than: Option<Duration>, dry_run: bool) -> Pruned {
		let now = now();
		let old = |fetched: u64| older_than.is_none_or(|age| now.saturating_sub(fetched) >= age.as_secs());
		let mut out = Pruned::default();
		for t in self.trees() {
			if old(t.fetched) {
				out.trees.push(t);
			} else {
				out.kept_trees += 1;
				out.kept_size += t.size;
			}
		}
		for d in self.downloads() {
			if old(d.fetched) {
				out.downloads.push(d);
			} else {
				out.kept_downloads += 1;
				out.kept_size += d.size;
			}
		}
		out.leftovers = self
			.leftovers()
			.into_iter()
			.filter(|l| now.saturating_sub(l.fetched) >= STALE_LEFTOVER)
			.collect();
		if dry_run {
			return out;
		}
		for t in &out.trees {
			remove_tree(&t.path);
			let _ = std::fs::remove_file(t.path.with_extension("json"));
			if let Some(repo) = t.path.parent() {
				forget_if_empty(repo, &self.root().join("packages"));
			}
		}
		for d in &out.downloads {
			remove_tree(&d.path);
		}
		for l in &out.leftovers {
			remove_tree(&l.path);
		}
		out
	}

	/// Take away what the cache holds of a package's repository: every copy,
	/// or with a version, the copy that version was last found at.
	///
	/// `word` is a reference as `run` takes one, with or without its `@`.
	pub fn remove(&self, word: &str, dry_run: bool) -> Result<Vec<CachedTree>, RemoteError> {
		let (r, versioned) = match runfile_lang::reference::parse(word) {
			Ok(Reference::Remote(r)) => (r, true),
			Ok(Reference::Path(_)) => {
				return Err(RemoteError(format!(
					"`{word}` is a folder on this machine, which the cache holds nothing of"
				)));
			}
			// A reference without its version is what this takes most often:
			// the repository, whatever version of it is kept.
			Err(_) if !word.contains('@') => match runfile_lang::reference::parse(&format!("{word}@HEAD")) {
				Ok(Reference::Remote(r)) => (r, false),
				_ => return Err(not_a_repository(word)),
			},
			Err(e) => return Err(RemoteError(e.0)),
		};
		let repository = r.repository();
		let commit = match (&r.rev, versioned) {
			(_, false) => None,
			(Rev::Commit(c), true) => Some(c.clone()),
			(Rev::Name(n), true) => Some(
				refs_of(&self.repo_dir(&r))
					.into_iter()
					.find(|(name, _)| name == n)
					.map(|(_, c)| c)
					.ok_or_else(|| RemoteError(format!("the cache has no `{n}` of {repository}")))?,
			),
		};
		let gone: Vec<CachedTree> = self
			.trees()
			.into_iter()
			.filter(|t| t.repository == repository && commit.as_ref().is_none_or(|c| *c == t.commit))
			.collect();
		if gone.is_empty() {
			return Err(RemoteError(format!(
				"the cache holds nothing of {}",
				what(&r, versioned)
			)));
		}
		if !dry_run {
			for t in &gone {
				remove_tree(&t.path);
				let _ = std::fs::remove_file(t.path.with_extension("json"));
			}
			// With no version, what it knew of its tags and branches goes too.
			if !versioned {
				let _ = std::fs::remove_file(self.repo_dir(&r).join("refs.json"));
			}
			forget_if_empty(&self.repo_dir(&r), &self.root().join("packages"));
		}
		Ok(gone)
	}
}

fn what(r: &Remote, versioned: bool) -> String {
	match (versioned, &r.rev) {
		(true, Rev::Name(n) | Rev::Commit(n)) => format!("{}@{n}", r.repository()),
		_ => r.repository(),
	}
}

fn not_a_repository(word: &str) -> RemoteError {
	RemoteError(format!(
		"`{word}` is not a repository: one is named by its host and path, as `github.com/owner/repo`"
	))
}

/// Remember, beside a tree just fetched, what it is and when it came.
pub(crate) fn record_tree(tree: &Path, commit: &str, folder: Option<&str>) {
	let record = serde_json::json!({
		"commit": commit,
		"whole": folder.is_none(),
		"folder": folder,
		"fetched": now(),
	});
	let _ = std::fs::write(tree.with_extension("json"), record.to_string());
}

/// Every repository under `dir`, whose path on its host so far is `name`.
///
/// A repository is a folder holding commit-named trees; anything else is a
/// level of the path -- an owner, a GitLab subgroup -- to look inside.
fn repositories(dir: &Path, name: &str, out: &mut Vec<CachedTree>) {
	let entries = children(dir);
	let refs = refs_of(dir);
	for p in entries.iter().filter(|p| p.is_dir()) {
		let file = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
		let Some((commit, thin)) = tree_name(&file) else {
			repositories(p, &format!("{name}/{file}"), out);
			continue;
		};
		let record: Option<serde_json::Value> = std::fs::read_to_string(p.with_extension("json"))
			.ok()
			.and_then(|t| serde_json::from_str(&t).ok());
		let field = |k: &str| record.as_ref().and_then(|r| r.get(k).cloned());
		let mut names: Vec<String> = refs
			.iter()
			.filter(|(_, c)| *c == commit)
			.map(|(n, _)| n.clone())
			.collect();
		names.sort();
		out.push(CachedTree {
			repository: name.to_string(),
			commit: commit.to_string(),
			whole: !thin,
			folder: if thin {
				field("folder")
					.and_then(|f| f.as_str().map(str::to_string))
					// A copy kept before the record existed names its folder only
					// by a hash, which says at least whether it is the root.
					.or_else(|| file.ends_with(&root_hash()).then(String::new))
			} else {
				None
			},
			names,
			fetched: field("fetched").and_then(|f| f.as_u64()).unwrap_or_else(|| modified(p)),
			size: size_of(p),
			path: p.clone(),
		});
	}
}

/// The hash a package at a repository's root is kept under.
fn root_hash() -> String {
	use sha2::Digest;
	hex::encode(sha2::Sha256::digest(b""))[..12].to_string()
}

/// `<commit>` (a whole repository) or `<commit>-<hash>` (a package's
/// runfiles), as the commit and whether it is the second.
fn tree_name(name: &str) -> Option<(&str, bool)> {
	match name.split_once('-') {
		None if is_full_commit(name) => Some((name, false)),
		Some((c, h)) if is_full_commit(c) && h.len() == 12 && h.bytes().all(|b| b.is_ascii_hexdigit()) => {
			Some((c, true))
		}
		_ => None,
	}
}

/// What `refs.json` in a repository's folder says each name points at.
fn refs_of(dir: &Path) -> Vec<(String, String)> {
	let Some(serde_json::Value::Object(map)) = std::fs::read_to_string(dir.join("refs.json"))
		.ok()
		.and_then(|t| serde_json::from_str(&t).ok())
	else {
		return Vec::new();
	};
	map.into_iter()
		.filter_map(|(name, v)| Some((name, v.get("commit")?.as_str()?.to_string())))
		.collect()
}

fn download_at(dir: &Path) -> Option<CachedDownload> {
	let meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("meta.json")).ok()?).ok()?;
	let path = meta.get("path")?.as_str()?;
	Some(CachedDownload {
		url: meta.get("url")?.as_str()?.to_string(),
		sha256: meta
			.get("sha256")
			.and_then(|s| s.as_str())
			.unwrap_or_default()
			.to_string(),
		file: path.strip_prefix("content/").map(str::to_string),
		fetched: meta
			.get("fetched")
			.and_then(serde_json::Value::as_u64)
			.unwrap_or_else(|| modified(dir)),
		size: size_of(dir),
		path: dir.to_path_buf(),
	})
}

/// Take away a repository's folder once nothing of it is left, and the
/// owner and host folders above it that it leaves empty, up to `stop`.
fn forget_if_empty(repo: &Path, stop: &Path) {
	let holds_tree = children(repo).iter().any(|p| {
		p.is_dir()
			&& p.file_name()
				.and_then(|n| n.to_str())
				.is_some_and(|n| tree_name(n).is_some())
	});
	if holds_tree {
		return;
	}
	remove_tree(repo);
	let mut dir = repo.parent();
	while let Some(d) = dir {
		if d == stop || !d.starts_with(stop) || std::fs::remove_dir(d).is_err() {
			break;
		}
		dir = d.parent();
	}
}

fn children(dir: &Path) -> Vec<PathBuf> {
	let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
		.map(|rd| rd.flatten().map(|e| e.path()).collect())
		.unwrap_or_default();
	out.sort();
	out
}

/// The bytes a file or a folder holds, links counted as themselves.
pub(crate) fn size_of(p: &Path) -> u64 {
	let Ok(meta) = std::fs::symlink_metadata(p) else {
		return 0;
	};
	if !meta.is_dir() {
		return meta.len();
	}
	children(p).iter().map(|c| size_of(c)).sum()
}

fn modified(p: &Path) -> u64 {
	std::fs::metadata(p)
		.and_then(|m| m.modified())
		.ok()
		.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
		.map_or(0, |d| d.as_secs())
}

/// `30d`, `12h`, `2w`, `90m`, `45s`: how long ago, for a prune.
pub fn parse_age(text: &str) -> Option<Duration> {
	let t = text.trim();
	let unit = t.chars().last()?;
	let n: u64 = t[..t.len() - unit.len_utf8()].parse().ok()?;
	let secs = match unit {
		's' => 1,
		'm' => 60,
		'h' => 60 * 60,
		'd' => 24 * 60 * 60,
		'w' => 7 * 24 * 60 * 60,
		_ => return None,
	};
	n.checked_mul(secs).map(Duration::from_secs)
}

/// A size as a person reads one: `512 B`, `48.0 KiB`, `1.2 MiB`.
pub fn human_size(bytes: u64) -> String {
	const UNITS: [&str; 5] = ["KiB", "MiB", "GiB", "TiB", "PiB"];
	if bytes < 1024 {
		return format!("{bytes} B");
	}
	let mut v = bytes as f64 / 1024.0;
	let mut unit = 0;
	while v >= 1024.0 && unit + 1 < UNITS.len() {
		v /= 1024.0;
		unit += 1;
	}
	format!("{v:.1} {}", UNITS[unit])
}

/// How long ago `then` was, from `now`, both seconds since the epoch.
pub fn human_age(then: u64, now: u64) -> String {
	let secs = now.saturating_sub(then);
	let (n, unit) = match secs {
		0..60 => return "just now".to_string(),
		60..3600 => (secs / 60, "minute"),
		3600..86_400 => (secs / 3600, "hour"),
		_ => (secs / 86_400, "day"),
	};
	format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn an_age_is_a_number_and_a_unit() {
		assert_eq!(parse_age("30d"), Some(Duration::from_secs(30 * 86_400)));
		assert_eq!(parse_age("12h"), Some(Duration::from_secs(12 * 3600)));
		assert_eq!(parse_age("2w"), Some(Duration::from_secs(14 * 86_400)));
		assert_eq!(parse_age("0d"), Some(Duration::ZERO));
		for bad in ["", "d", "30", "3x", "-1d", "1.5d", "99999999999999999999d"] {
			assert_eq!(parse_age(bad), None, "{bad}");
		}
	}

	#[test]
	fn sizes_and_ages_read_the_way_a_person_says_them() {
		assert_eq!(human_size(0), "0 B");
		assert_eq!(human_size(1023), "1023 B");
		assert_eq!(human_size(1536), "1.5 KiB");
		assert_eq!(human_size(5 * 1024 * 1024), "5.0 MiB");
		assert_eq!(human_age(100, 100), "just now");
		assert_eq!(human_age(0, 60), "1 minute ago");
		assert_eq!(human_age(0, 7200), "2 hours ago");
		assert_eq!(human_age(0, 86_400 * 9), "9 days ago");
	}

	#[test]
	fn only_commit_named_folders_are_copies() {
		let c = "e8e95d87b054cd7f081f88531cf85a54057a5d85";
		assert_eq!(tree_name(c), Some((c, false)));
		assert_eq!(tree_name(&format!("{c}-e3b0c44298fc")), Some((c, true)));
		assert_eq!(tree_name("acme"), None);
		assert_eq!(tree_name(&format!("{c}-x")), None);
	}
}
