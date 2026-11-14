//! What the cache asks of git: what a tag or branch points at, and a tree at a
//! commit.
//!
//! git, rather than each host's archive and release API, because it is the one
//! protocol GitHub, GitLab, Bitbucket, Gitea and Forgejo all speak, the same
//! way: no host needs code of its own, a private repository is reached through
//! whatever credential helper the person already uses, and `ls-remote` answers
//! what a name points at in one round trip.

use crate::{RemoteError, Resolved, fail};
use runfile_lang::reference::{Remote, looks_abbreviated};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// What `name` points at on the host: an exact tag first (through to the
/// commit, for an annotated one), then `v`+`name` for a version written without
/// its `v`, then a branch.
pub(crate) fn ls_remote(r: &Remote, name: &str, wait: Duration, config: &[String]) -> Result<Resolved, RemoteError> {
	let tag = format!("refs/tags/{name}");
	let vtag = format!("refs/tags/v{name}");
	let branch = format!("refs/heads/{name}");
	let mut args = vec![
		"ls-remote".to_string(),
		r.clone_url(),
		tag.clone(),
		format!("{tag}^{{}}"),
		branch.clone(),
	];
	let with_v = name.starts_with(|c: char| c.is_ascii_digit());
	if with_v {
		args.push(vtag.clone());
		args.push(format!("{vtag}^{{}}"));
	}
	let out = git(config, &args, wait)
		.map_err(|e| RemoteError(format!("could not ask {} about `{name}`: {e}", r.repository())))?;
	let lines: Vec<(&str, &str)> = out
		.lines()
		.filter_map(|l| l.split_once('\t'))
		.map(|(sha, rf)| (sha.trim(), rf.trim()))
		.collect();
	let find = |rf: &str| lines.iter().find(|(_, x)| *x == rf).map(|(sha, _)| sha.to_string());
	let tag_commit = |t: &str| find(&format!("{t}^{{}}")).or_else(|| find(t));
	if let Some(c) = tag_commit(&tag) {
		return Ok(Resolved {
			commit: c,
			tag: name.to_string(),
		});
	}
	if with_v && let Some(c) = tag_commit(&vtag) {
		return Ok(Resolved {
			commit: c,
			tag: format!("v{name}"),
		});
	}
	if let Some(c) = find(&branch) {
		return Ok(Resolved {
			commit: c,
			tag: String::new(),
		});
	}
	let hint = if looks_abbreviated(name) {
		"; a commit is written in full, all 40 characters"
	} else {
		""
	};
	fail(format!("{} has no tag or branch named `{name}`{hint}", r.repository()))
}

/// How deep below a package's folder another `runfiles/` is still part of it:
/// discovery's own reach for subprojects.
const SUBPROJECT_DEPTH: usize = 3;

/// The package at `commit`, checked out under `tmp` with no `.git` in it, in
/// two steps: its `runfiles/` first -- its own and its subprojects', which is
/// everything a target can `run` or a `_shared.run` can add -- and then, when
/// `whole` says so of that tree, the rest of the repository. Answers with the
/// tree and whether it is the whole repository.
///
/// The first step fetches the commit's folders and the files of `runfiles/`
/// alone, so a package that needs nothing else costs what its runfiles weigh
/// however large the repository; the second fetches the files that are left.
/// A host that does not filter sends everything in the first, and the second
/// then has nothing left to fetch.
pub(crate) fn checkout(
	r: &Remote,
	commit: &str,
	tmp: &Path,
	config: &[String],
	whole: &dyn Fn(&Path) -> Result<bool, RemoteError>,
) -> Result<(PathBuf, bool), RemoteError> {
	let gd = tmp.join("git");
	let tree = tmp.join("tree");
	std::fs::create_dir_all(&tree).map_err(|e| RemoteError(format!("could not create {}: {e}", tree.display())))?;
	let at = |args: &[&str]| {
		let mut all = vec![format!("--git-dir={}", gd.display())];
		all.extend(args.iter().map(|a| a.to_string()));
		all
	};
	// The person's own hooks, from a global `core.hooksPath`, are no business
	// of a cache, and line endings stay as they were committed.
	let no_hooks = tmp.join("no-hooks");
	let in_tree = |args: &[&str]| {
		let mut all = vec![
			format!("--git-dir={}", gd.display()),
			format!("--work-tree={}", tree.display()),
			"-c".to_string(),
			format!("core.hooksPath={}", no_hooks.display()),
			"-c".to_string(),
			"core.autocrlf=false".to_string(),
			"-c".to_string(),
			"advice.detachedHead=false".to_string(),
		];
		all.extend(args.iter().map(|a| a.to_string()));
		all
	};
	let long = Duration::from_secs(600);
	let quick = Duration::from_secs(60);
	let bad = |e: String| RemoteError(format!("could not fetch {}: {e}", r.shown()));
	git(config, &at(&["init", "-q"]), quick).map_err(bad)?;
	git(config, &at(&["remote", "add", "origin", &r.clone_url()]), quick).map_err(bad)?;
	git(config, &at(&["config", "core.sparseCheckout", "true"]), quick).map_err(bad)?;
	let info = gd.join("info");
	std::fs::create_dir_all(&info).map_err(|e| bad(e.to_string()))?;
	let patterns = info.join("sparse-checkout");
	std::fs::write(&patterns, runfiles_patterns(&r.dir)).map_err(|e| bad(e.to_string()))?;
	// One commit. A host that will not hand over a commit by its id -- few do
	// not -- gets asked for everything instead.
	if git(
		config,
		&at(&[
			"fetch",
			"-q",
			"--no-tags",
			"--depth",
			"1",
			"--filter=blob:none",
			"origin",
			commit,
		]),
		long,
	)
	.is_err()
	{
		git(
			config,
			&at(&["fetch", "-q", "--tags", "origin", "+refs/heads/*:refs/remotes/origin/*"]),
			long,
		)
		.map_err(bad)?;
	}
	let object = format!("{commit}^{{commit}}");
	if git(config, &at(&["cat-file", "-e", &object]), quick).is_err() {
		return fail(format!("{} has no commit {commit}", r.repository()));
	}
	git(config, &in_tree(&["checkout", "-q", "--force", commit]), long).map_err(bad)?;
	if !whole(&tree)? {
		return Ok((tree, false));
	}
	// Everything, and the files the first step left out with it.
	std::fs::write(&patterns, "/*\n").map_err(|e| bad(e.to_string()))?;
	git(config, &in_tree(&["read-tree", "-mu", "HEAD"]), long).map_err(bad)?;
	Ok((tree, true))
}

/// The sparse-checkout patterns for a package's `runfiles/`, and its
/// subprojects' below it.
fn runfiles_patterns(dir: &str) -> String {
	let base = if dir.is_empty() {
		String::new()
	} else {
		format!("/{dir}")
	};
	(0..=SUBPROJECT_DEPTH)
		.map(|depth| format!("{base}/{}runfiles/\n", "*/".repeat(depth)))
		.collect()
}

/// Run git, answering with its stdout, or with what it said went wrong.
///
/// It is given `wait` and then stopped, since a host that never answers would
/// otherwise hold the run for as long as the connection takes to give up. It
/// asks nobody for a password -- `GIT_TERMINAL_PROMPT=0` -- since a run may have
/// no terminal and a parallel branch must not read the one there is; a
/// credential helper still answers.
fn git(config: &[String], args: &[String], wait: Duration) -> Result<String, String> {
	let mut c = Command::new("git");
	for setting in config {
		c.arg("-c").arg(setting);
	}
	c.args(args)
		.env("GIT_TERMINAL_PROMPT", "0")
		.env("GIT_LFS_SKIP_SMUDGE", "1")
		.stdin(Stdio::null())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped());
	let mut child = c.spawn().map_err(|e| match e.kind() {
		std::io::ErrorKind::NotFound => {
			"git is not installed, or not on PATH; a package is fetched with it".to_string()
		}
		_ => format!("could not start git: {e}"),
	})?;
	let mut out = child.stdout.take().expect("piped");
	let mut err = child.stderr.take().expect("piped");
	let reader = std::thread::spawn(move || {
		let mut s = String::new();
		let _ = out.read_to_string(&mut s);
		s
	});
	let err_reader = std::thread::spawn(move || {
		let mut s = String::new();
		let _ = err.read_to_string(&mut s);
		s
	});
	let deadline = Instant::now() + wait;
	let status = loop {
		match child.try_wait() {
			Ok(Some(status)) => break status,
			Ok(None) if Instant::now() >= deadline => {
				let _ = child.kill();
				let _ = child.wait();
				return Err(format!("git gave no answer in {} seconds", wait.as_secs()));
			}
			Ok(None) => std::thread::sleep(Duration::from_millis(20)),
			Err(e) => return Err(format!("could not wait for git: {e}")),
		}
	};
	let stdout = reader.join().unwrap_or_default();
	let stderr = err_reader.join().unwrap_or_default();
	if status.success() {
		return Ok(stdout);
	}
	// git's own last word on it, which names the host or the status.
	let said = stderr
		.lines()
		.map(str::trim)
		.rfind(|l| !l.is_empty() && !l.starts_with("warning:") && !l.starts_with("hint:"))
		.unwrap_or("git failed")
		.trim_start_matches("fatal: ")
		.to_string();
	Err(said)
}
