//! `run :packages` -- what the cache holds of packages and downloads, how much
//! room it takes, and fetching and pruning it.
//!
//! A run fetches what it needs on its own, so none of this is needed to use a
//! package. It is for the questions a cache raises: what is in it and how big
//! it is (the summary, `list`, `downloads`), getting something now rather than
//! at the next run (`fetch`, which also asks the host about a tag or branch
//! whatever its age), and taking room back (`prune`, `remove`). Every line that
//! names something says what it weighs, and everything that removes says what
//! it freed.

use crate::help::{Row, Section};
use runfile_lang::reference::{self, Reference};
use runfile_remote::{Cache, CachedDownload, CachedTree, Network, human_age, human_size};
use runfile_runtime::term::sanitize;
use std::process::ExitCode;
use std::time::Duration;

/// What a prune with no age takes: anything fetched more than a week ago.
const DEFAULT_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

const INTRO: &str = "run :packages <command>   —   the cache of packages and downloads";

const SECTIONS: &[Section] = &[
	Section(
		"Commands",
		&[
			Row(
				"run :packages",
				"where the cache is, what it holds, and how much room it takes",
			),
			Row(
				"run :packages list",
				"every package copy, its versions, its size and its age (--json)",
			),
			Row(
				"run :packages downloads",
				"everything download() fetched, with its size and age (--json)",
			),
			Row(
				"run :packages fetch <package>...",
				"fetch now, asking the host what each tag or branch points at today",
			),
			Row(
				"run :packages prune [age]",
				"remove what was fetched more than 7 days ago, or than `age` (30d, 12h, 2w); --all for everything",
			),
			Row(
				"run :packages remove <package>...",
				"remove a repository's copies, or with @version the one that version is",
			),
		],
	),
	Section(
		"Options",
		&[
			Row(
				"    --dry-run",
				"for prune and remove: say what would go, and remove nothing",
			),
			Row("    --json", "for list and downloads: one JSON array, for tooling"),
		],
	),
];

pub(crate) fn usage() -> String {
	crate::help::render(INTRO, SECTIONS)
}

/// `run :fetch` became `run :packages fetch`; said apart from a typo, as
/// `:format` is, since it is a script written for the runner before.
pub const FETCH_IS_PACKAGES: &str = "`:fetch` is now `:packages fetch`, beside `list`, `downloads`, `prune` and `remove`; `run :packages --help` lists them";

pub(crate) fn dispatch(args: &[String]) -> Result<ExitCode, String> {
	if crate::help::wants_help(args) {
		print!("{}", usage());
		return Ok(ExitCode::SUCCESS);
	}
	let cache = Cache::open_default().ok_or_else(|| {
		format!(
			"there is no cache directory; set {} to one",
			runfile_remote::CACHE_DIR_ENV_VAR
		)
	})?;
	let Some(sub) = args.first() else {
		summary(&cache);
		return Ok(ExitCode::SUCCESS);
	};
	let rest = &args[1..];
	let flag = |n: &str| rest.iter().any(|a| a == n);
	let words: Vec<&str> = rest
		.iter()
		.filter(|a| !a.starts_with('-'))
		.map(String::as_str)
		.collect();
	let allowed: &[&str] = match sub.as_str() {
		"list" | "downloads" => &["--json"],
		"prune" => &["--all", "--dry-run"],
		"remove" => &["--dry-run"],
		_ => &[],
	};
	if let Some(bad) = rest
		.iter()
		.find(|a| a.starts_with('-') && !allowed.contains(&a.as_str()))
	{
		return Err(format!("unknown flag `{bad}` for `:packages {sub}`\n{}", usage()));
	}
	match sub.as_str() {
		"list" => no_words(sub, &words).map(|()| list(&cache, flag("--json"))),
		"downloads" => no_words(sub, &words).map(|()| downloads(&cache, flag("--json"))),
		"fetch" => fetch(&cache, &words),
		"prune" => prune(&cache, &words, flag("--all"), flag("--dry-run")),
		"remove" => remove(&cache, &words, flag("--dry-run")),
		other => Err(format!("unknown command `:packages {other}`\n{}", usage())),
	}?;
	Ok(ExitCode::SUCCESS)
}

fn no_words(sub: &str, words: &[&str]) -> Result<(), String> {
	match words.first() {
		Some(w) => Err(format!("`:packages {sub}` takes no `{w}`\n{}", usage())),
		None => Ok(()),
	}
}

/// `n thing` or `n things`.
fn count(n: usize, one: &str, many: &str) -> String {
	format!("{n} {}", if n == 1 { one } else { many })
}

fn now() -> u64 {
	std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map_or(0, |d| d.as_secs())
}

fn repositories(trees: &[CachedTree]) -> usize {
	let mut names: Vec<&str> = trees.iter().map(|t| t.repository.as_str()).collect();
	names.dedup();
	names.len()
}

fn copies(trees: &[CachedTree]) -> String {
	format!(
		"{} of {}",
		count(trees.len(), "copy", "copies"),
		count(repositories(trees), "repository", "repositories")
	)
}

/// What a copy holds, as one phrase.
fn shape(t: &CachedTree) -> String {
	match (t.whole, t.folder.as_deref()) {
		(true, _) => "whole repository".to_string(),
		(false, Some("")) => "runfiles/".to_string(),
		(false, Some(f)) => format!("{f}/runfiles/"),
		(false, None) => "a folder's runfiles/".to_string(),
	}
}

/// The versions a copy is known by, or that it is known by none.
fn versions(t: &CachedTree) -> String {
	match t.names.is_empty() {
		true => "(a commit)".to_string(),
		false => t.names.join(", "),
	}
}

fn summary(cache: &Cache) {
	let trees = cache.trees();
	let downloads = cache.downloads();
	let leftovers = cache.leftovers();
	let tree_size: u64 = trees.iter().map(|t| t.size).sum();
	let download_size: u64 = downloads.iter().map(|d| d.size).sum();
	let leftover_size: u64 = leftovers.iter().map(|l| l.size).sum();
	let mut rows = vec![
		("packages", copies(&trees), tree_size),
		("downloads", count(downloads.len(), "file", "files"), download_size),
	];
	if !leftovers.is_empty() {
		rows.push((
			"unfinished",
			count(leftovers.len(), "interrupted fetch", "interrupted fetches"),
			leftover_size,
		));
	}
	println!("cache      {}", sanitize(&cache.root().display().to_string()));
	let width = rows.iter().map(|r| r.1.len()).max().unwrap_or(0);
	for (what, n, size) in &rows {
		println!("{what:<10} {n:<width$}  {:>10}", human_size(*size));
	}
	println!(
		"{:<10} {:<width$}  {:>10}",
		"total",
		"",
		human_size(tree_size + download_size + leftover_size)
	);
	let stale = cache.prune(Some(DEFAULT_AGE), true);
	println!();
	if stale.trees.is_empty() && stale.downloads.is_empty() && stale.leftovers.is_empty() {
		println!("Nothing was fetched more than 7 days ago.");
	} else {
		println!(
			"Fetched more than 7 days ago: {} ({}). `run :packages prune` removes them.",
			stale_parts(&stale).join(", "),
			human_size(stale.freed())
		);
	}
}

fn stale_parts(p: &runfile_remote::Pruned) -> Vec<String> {
	let mut parts = Vec::new();
	if !p.trees.is_empty() {
		parts.push(count(p.trees.len(), "package copy", "package copies"));
	}
	if !p.downloads.is_empty() {
		parts.push(count(p.downloads.len(), "download", "downloads"));
	}
	if !p.leftovers.is_empty() {
		parts.push(count(p.leftovers.len(), "unfinished fetch", "unfinished fetches"));
	}
	parts
}

fn list(cache: &Cache, json: bool) {
	let trees = cache.trees();
	if json {
		let items: Vec<serde_json::Value> = trees
			.iter()
			.map(|t| {
				serde_json::json!({
					"repository": t.repository,
					"commit": t.commit,
					"names": t.names,
					"whole": t.whole,
					"folder": t.folder,
					"size": t.size,
					"fetched": t.fetched,
					"path": t.path,
				})
			})
			.collect();
		println!("{}", serde_json::Value::Array(items));
		return;
	}
	let dir = cache.root().join("packages");
	if trees.is_empty() {
		println!("No packages in {}.", sanitize(&dir.display().to_string()));
		return;
	}
	let now = now();
	let names_w = trees.iter().map(|t| versions(t).chars().count()).max().unwrap_or(0);
	let shape_w = trees.iter().map(|t| shape(t).chars().count()).max().unwrap_or(0);
	let mut last = "";
	for t in &trees {
		if t.repository != last {
			println!("{}", sanitize(&t.repository));
			last = &t.repository;
		}
		println!(
			"  {}  {:<names_w$}  {:<shape_w$}  {:>10}  fetched {}",
			runfile_remote::short(&t.commit),
			sanitize(&versions(t)),
			sanitize(&shape(t)),
			human_size(t.size),
			human_age(t.fetched, now),
		);
	}
	println!();
	println!(
		"{}, {}, in {}",
		copies(&trees),
		human_size(trees.iter().map(|t| t.size).sum()),
		sanitize(&dir.display().to_string())
	);
}

/// How a download was kept.
fn kept_as(d: &CachedDownload) -> String {
	match &d.file {
		Some(name) => format!("kept as {name}"),
		None => "unpacked".to_string(),
	}
}

fn downloads(cache: &Cache, json: bool) {
	let all = cache.downloads();
	if json {
		let items: Vec<serde_json::Value> = all
			.iter()
			.map(|d| {
				serde_json::json!({
					"url": d.url,
					"sha256": d.sha256,
					"file": d.file,
					"size": d.size,
					"fetched": d.fetched,
					"path": d.path,
				})
			})
			.collect();
		println!("{}", serde_json::Value::Array(items));
		return;
	}
	let dir = cache.root().join("downloads");
	if all.is_empty() {
		println!("No downloads in {}.", sanitize(&dir.display().to_string()));
		return;
	}
	let now = now();
	let kept_w = all.iter().map(|d| kept_as(d).chars().count()).max().unwrap_or(0);
	for d in &all {
		println!("{}", sanitize(&d.url));
		println!(
			"  {:<kept_w$}  {:>10}  fetched {}  sha256 {}",
			sanitize(&kept_as(d)),
			human_size(d.size),
			human_age(d.fetched, now),
			&d.sha256[..d.sha256.len().min(12)],
		);
	}
	println!();
	println!(
		"{}, {}, in {}",
		count(all.len(), "download", "downloads"),
		human_size(all.iter().map(|d| d.size).sum()),
		sanitize(&dir.display().to_string())
	);
}

fn fetch(cache: &Cache, words: &[&str]) -> Result<(), String> {
	if words.is_empty() {
		return Err(format!("`:packages fetch` needs a package to fetch\n{}", usage()));
	}
	let say = |m: &str| eprintln!("{} {m}", runfile_runtime::exec::tag());
	for word in words {
		let r = match reference::parse(word) {
			Ok(Reference::Remote(r)) => r,
			Ok(Reference::Path(_)) => {
				return Err(format!("`{word}` is a folder on this machine, which needs no fetching"));
			}
			Err(e) if reference::is_reference(word) => return Err(e.0),
			Err(_) => {
				return Err(format!(
					"`{word}` is not a package: one is named by its host and path, as `github.com/owner/repo@v1.0.0`"
				));
			}
		};
		let pkg = cache.package(&r, Network::Refresh, &say).map_err(|e| e.0)?;
		let what = match pkg.version.as_str() {
			"" => runfile_remote::short(&pkg.commit).to_string(),
			tag => format!("{} (tag {tag})", runfile_remote::short(&pkg.commit)),
		};
		// Which copy now answers for it, and what it weighs.
		let copy = cache
			.trees()
			.into_iter()
			.find(|t| t.repository == r.repository() && t.commit == pkg.commit && pkg.root.starts_with(&t.path));
		match copy {
			Some(t) => println!("{}: {what}, {}, {}", r.shown(), shape(&t), human_size(t.size)),
			None => println!("{}: {what}", r.shown()),
		}
	}
	Ok(())
}

fn prune(cache: &Cache, words: &[&str], all: bool, dry_run: bool) -> Result<(), String> {
	let age = match (words, all) {
		([], true) => None,
		([], false) => Some(DEFAULT_AGE),
		([w], false) => Some(
			runfile_remote::parse_age(w)
				.ok_or_else(|| format!("`{w}` is not an age: write a number and a unit, as `30d`, `12h` or `2w`"))?,
		),
		([_], true) => return Err("`:packages prune` takes an age or `--all`, not both".to_string()),
		_ => return Err(format!("`:packages prune` takes one age\n{}", usage())),
	};
	let p = cache.prune(age, dry_run);
	let verb = if dry_run { "would remove" } else { "removed" };
	let now = now();
	let mut lines: Vec<(String, String, u64, Option<u64>)> = Vec::new();
	for t in &p.trees {
		lines.push((
			format!(
				"{} {} ({})",
				t.repository,
				runfile_remote::short(&t.commit),
				versions(t)
			),
			shape(t),
			t.size,
			Some(t.fetched),
		));
	}
	for d in &p.downloads {
		lines.push((d.url.clone(), kept_as(d), d.size, Some(d.fetched)));
	}
	for l in &p.leftovers {
		lines.push(("an interrupted fetch".to_string(), String::new(), l.size, None));
	}
	let what_w = lines.iter().map(|l| l.0.chars().count()).max().unwrap_or(0);
	let shape_w = lines.iter().map(|l| l.1.chars().count()).max().unwrap_or(0);
	for (what, shape, size, fetched) in &lines {
		let when = fetched
			.map(|f| format!("  fetched {}", human_age(f, now)))
			.unwrap_or_default();
		println!(
			"{verb}  {:<what_w$}  {:<shape_w$}  {:>10}{when}",
			sanitize(what),
			sanitize(shape),
			human_size(*size)
		);
	}
	let since = match age {
		None => String::new(),
		Some(a) => format!(", fetched in the last {}", age_words(a)),
	};
	let mut kept = Vec::new();
	if p.kept_trees > 0 {
		kept.push(count(p.kept_trees, "package copy", "package copies"));
	}
	if p.kept_downloads > 0 {
		kept.push(count(p.kept_downloads, "download", "downloads"));
	}
	let kept_line = match kept.is_empty() {
		true => "Kept nothing.".to_string(),
		false => format!("Kept {} ({}){since}.", kept.join(" and "), human_size(p.kept_size)),
	};
	if lines.is_empty() {
		let older = match age {
			None => "Nothing to remove.".to_string(),
			Some(a) => format!("Nothing was fetched more than {} ago.", age_words(a)),
		};
		println!("{older} {kept_line}");
		return Ok(());
	}
	let freed = if dry_run { "would free" } else { "freed" };
	println!();
	println!(
		"{freed} {}: {}. {kept_line}",
		human_size(p.freed()),
		stale_parts(&p).join(", ")
	);
	Ok(())
}

/// An age as the words a sentence says it with: `7 days`, `12 hours`.
fn age_words(d: Duration) -> String {
	let s = d.as_secs();
	let (n, one, many) = match s {
		_ if s.is_multiple_of(7 * 86_400) && s >= 7 * 86_400 && s != 7 * 86_400 => (s / (7 * 86_400), "week", "weeks"),
		_ if s.is_multiple_of(86_400) && s > 0 => (s / 86_400, "day", "days"),
		_ if s.is_multiple_of(3600) && s > 0 => (s / 3600, "hour", "hours"),
		_ if s.is_multiple_of(60) && s > 0 => (s / 60, "minute", "minutes"),
		_ => (s, "second", "seconds"),
	};
	count(n as usize, one, many)
}

fn remove(cache: &Cache, words: &[&str], dry_run: bool) -> Result<(), String> {
	if words.is_empty() {
		return Err(format!("`:packages remove` needs a package to remove\n{}", usage()));
	}
	let verb = if dry_run { "would remove" } else { "removed" };
	let mut freed = 0;
	for word in words {
		for t in cache.remove(word, dry_run).map_err(|e| e.0)? {
			println!(
				"{verb}  {} {} ({})  {}  {}",
				sanitize(&t.repository),
				runfile_remote::short(&t.commit),
				sanitize(&versions(&t)),
				sanitize(&shape(&t)),
				human_size(t.size)
			);
			freed += t.size;
		}
	}
	println!();
	println!("{} {}", if dry_run { "would free" } else { "freed" }, human_size(freed));
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn an_age_reads_as_the_words_for_it() {
		assert_eq!(age_words(DEFAULT_AGE), "7 days");
		assert_eq!(age_words(Duration::from_secs(14 * 86_400)), "2 weeks");
		assert_eq!(age_words(Duration::from_secs(30 * 86_400)), "30 days");
		assert_eq!(age_words(Duration::from_secs(12 * 3600)), "12 hours");
		assert_eq!(age_words(Duration::from_secs(90)), "90 seconds");
		assert_eq!(age_words(Duration::from_secs(60)), "1 minute");
	}
}
