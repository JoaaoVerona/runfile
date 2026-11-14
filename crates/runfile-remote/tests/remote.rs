//! The cache against real git, serving repositories from a folder.
//!
//! `https://example.test/` is pointed at that folder with git's own
//! `insteadOf`, handed to each git call (`Cache::rewriting`), so everything a
//! fetch does -- `ls-remote`, a shallow fetch by commit, a sparse checkout --
//! is the real thing, and only the transport is a file. Skipped where git is
//! not installed.

use runfile_lang::reference::{Reference, Remote, parse};
use runfile_remote::{Cache, Network, RECHECK, age_ref_for_test};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tempfile::TempDir;

fn has_git() -> bool {
	Command::new("git")
		.arg("--version")
		.output()
		.is_ok_and(|o| o.status.success())
}

/// A folder of repositories, and a cache that fetches from it.
struct Server {
	dir: TempDir,
	cache_dir: TempDir,
}

impl Server {
	fn new() -> Self {
		Self {
			dir: TempDir::new().unwrap(),
			cache_dir: TempDir::new().unwrap(),
		}
	}

	fn cache(&self) -> Cache {
		let to = format!("{}/", file_url(self.dir.path()));
		Cache::new(self.cache_dir.path().to_path_buf()).rewriting("https://example.test/", &to)
	}

	fn repo(&self, path: &str) -> PathBuf {
		let d = self.dir.path().join(path);
		std::fs::create_dir_all(&d).unwrap();
		git(&d, &["init", "-q", "-b", "main"]);
		// As GitHub, GitLab and Gitea do: a fetch may leave files out and ask
		// for them later, which is what fetching only `runfiles/` relies on.
		git(&d, &["config", "uploadpack.allowFilter", "true"]);
		d
	}
}

fn slashed(p: &Path) -> String {
	p.display().to_string().replace('\\', "/")
}

/// git as a person runs it, with none of this machine's configuration: no
/// signing key to ask a person for, no hooks.
fn git(dir: &Path, args: &[&str]) -> String {
	let empty = dir.join(".no-config");
	let o = Command::new("git")
		.args([
			"-c",
			"commit.gpgsign=false",
			"-c",
			"tag.gpgsign=false",
			"-c",
			"core.hooksPath=/nonexistent",
		])
		.args(args)
		.current_dir(dir)
		.env("GIT_CONFIG_GLOBAL", &empty)
		.env("GIT_CONFIG_NOSYSTEM", "1")
		.env("GIT_AUTHOR_NAME", "t")
		.env("GIT_AUTHOR_EMAIL", "t@example.test")
		.env("GIT_COMMITTER_NAME", "t")
		.env("GIT_COMMITTER_EMAIL", "t@example.test")
		.output()
		.unwrap();
	assert!(
		o.status.success(),
		"git {args:?}: {}",
		String::from_utf8_lossy(&o.stderr)
	);
	String::from_utf8(o.stdout).unwrap().trim().to_string()
}

fn write(dir: &Path, file: &str, body: &str) {
	let p = dir.join(file);
	std::fs::create_dir_all(p.parent().unwrap()).unwrap();
	std::fs::write(p, body).unwrap();
}

/// Commit everything, answering with the commit.
fn commit(dir: &Path, msg: &str) -> String {
	git(dir, &["add", "-A"]);
	git(dir, &["commit", "-q", "-m", msg]);
	git(dir, &["rev-parse", "HEAD"])
}

fn remote(word: &str) -> Remote {
	match parse(word).unwrap() {
		Reference::Remote(r) => r,
		p => panic!("{p:?}"),
	}
}

fn quiet(_: &str) {}

/// A repository with a package at its root, tagged `v1.0.0` (annotated, so
/// the tag is its own object and has to be followed to the commit).
fn tool(s: &Server) -> (PathBuf, String) {
	let d = s.repo("acme/tool.git");
	write(&d, "runfiles/_main.run", "# The tool\n\nprint(\"one\")\n");
	let c = commit(&d, "one");
	git(&d, &["tag", "-a", "v1.0.0", "-m", "v1"]);
	(d, c)
}

#[test]
fn a_tag_is_followed_to_its_commit_and_fetched_once() {
	if !has_git() {
		return eprintln!("skipped: git is not installed");
	}
	let s = Server::new();
	let (_, c) = tool(&s);
	let cache = s.cache();
	let said = std::cell::RefCell::new(Vec::new());
	let r = remote("example.test/acme/tool@v1.0.0");
	let p = cache
		.package(&r, Network::Allowed, &|m| said.borrow_mut().push(m.to_string()))
		.unwrap();
	assert_eq!((p.commit.as_str(), p.version.as_str()), (c.as_str(), "v1.0.0"));
	assert!(p.root.join("runfiles/_main.run").is_file());
	// Without a `.git`: nothing in the cache looks like a repository someone
	// could commit to, or a git command run there could find.
	assert!(!p.root.join(".git").exists());
	assert_eq!(said.borrow().len(), 1, "{:?}", said.borrow());
	assert!(said.borrow()[0].starts_with("fetching example.test/acme/tool@v1.0.0"));
	// Kept: asked again, nothing is fetched.
	let again = cache
		.package(&r, Network::Allowed, &|m| panic!("fetched again: {m}"))
		.unwrap();
	assert_eq!(again, p);
}

#[test]
fn a_version_written_without_its_v_finds_the_tag_with_one() {
	if !has_git() {
		return eprintln!("skipped: git is not installed");
	}
	let s = Server::new();
	let (_, c) = tool(&s);
	let p = s
		.cache()
		.package(&remote("example.test/acme/tool@1.0.0"), Network::Allowed, &quiet)
		.unwrap();
	assert_eq!((p.commit.as_str(), p.version.as_str()), (c.as_str(), "v1.0.0"));
}

#[test]
fn a_branch_is_fetched_with_no_version_and_rechecked_after_a_week() {
	if !has_git() {
		return eprintln!("skipped: git is not installed");
	}
	let s = Server::new();
	let (d, first) = tool(&s);
	let cache = s.cache();
	let r = remote("example.test/acme/tool@main");
	let p = cache.package(&r, Network::Allowed, &quiet).unwrap();
	assert_eq!((p.commit.as_str(), p.version.as_str()), (first.as_str(), ""));

	write(&d, "runfiles/_main.run", "# The tool\n\nprint(\"two\")\n");
	let second = commit(&d, "two");
	// Asked less than a week ago: taken at its word.
	assert_eq!(cache.package(&r, Network::Allowed, &quiet).unwrap().commit, first);
	// A week on, the host is asked again, and the branch has moved.
	age_ref_for_test(&cache, &r, "main", RECHECK + Duration::from_secs(60));
	let p = cache.package(&r, Network::Allowed, &quiet).unwrap();
	assert_eq!(p.commit, second);
	assert!(
		std::fs::read_to_string(p.root.join("runfiles/_main.run"))
			.unwrap()
			.contains("two")
	);
	// `run :fetch` asks now, whatever the age.
	write(&d, "runfiles/_main.run", "# The tool\n\nprint(\"three\")\n");
	let third = commit(&d, "three");
	assert_eq!(cache.package(&r, Network::Refresh, &quiet).unwrap().commit, third);
}

#[test]
fn a_recheck_that_fails_keeps_using_what_it_found_last_time() {
	if !has_git() {
		return eprintln!("skipped: git is not installed");
	}
	let s = Server::new();
	let (d, c) = tool(&s);
	let cache = s.cache();
	let r = remote("example.test/acme/tool@v1.0.0");
	cache.package(&r, Network::Allowed, &quiet).unwrap();
	// The host is gone -- offline, or down -- and the week is up.
	std::fs::rename(&d, d.with_extension("gone")).unwrap();
	age_ref_for_test(&cache, &r, "v1.0.0", RECHECK * 2);
	let p = cache.package(&r, Network::Allowed, &quiet).unwrap();
	assert_eq!((p.commit.as_str(), p.version.as_str()), (c.as_str(), "v1.0.0"));
	// Asked for an answer from the host, it says why there is none.
	let e = cache.package(&r, Network::Refresh, &quiet).unwrap_err();
	assert!(
		e.0.contains("could not ask example.test/acme/tool about `v1.0.0`"),
		"{e}"
	);
}

#[test]
fn a_commit_is_fetched_once_and_never_asked_about_again() {
	if !has_git() {
		return eprintln!("skipped: git is not installed");
	}
	let s = Server::new();
	let (d, c) = tool(&s);
	// Not the tip: a commit is fetched by its id, wherever it is.
	write(&d, "runfiles/_main.run", "# later\n\nprint(\"later\")\n");
	commit(&d, "later");
	let cache = s.cache();
	let r = remote(&format!("example.test/acme/tool@{c}"));
	let p = cache.package(&r, Network::Allowed, &quiet).unwrap();
	assert_eq!((p.commit.as_str(), p.version.as_str()), (c.as_str(), ""));
	assert!(
		std::fs::read_to_string(p.root.join("runfiles/_main.run"))
			.unwrap()
			.contains("one")
	);
	std::fs::rename(&d, d.with_extension("gone")).unwrap();
	assert_eq!(cache.package(&r, Network::Allowed, &quiet).unwrap(), p);
	assert_eq!(cache.package(&r, Network::Offline, &quiet).unwrap(), p);
}

#[test]
fn a_package_in_a_folder_is_fetched_without_the_rest_of_the_repository() {
	if !has_git() {
		return eprintln!("skipped: git is not installed");
	}
	let s = Server::new();
	let d = s.repo("acme/mono.git");
	write(&d, "runfiles/_main.run", "# root\n\nprint(\"root\")\n");
	write(
		&d,
		"internal-packages/subdir/runfiles/_main.run",
		"# sub\n\nprint(\"sub\")\n",
	);
	write(&d, "big/blob.bin", "x");
	commit(&d, "one");
	git(&d, &["tag", "v2"]);
	let p = s
		.cache()
		.package(
			&remote("example.test/acme/mono/internal-packages/subdir@v2"),
			Network::Allowed,
			&quiet,
		)
		.unwrap();
	assert!(p.root.ends_with("internal-packages/subdir"), "{}", p.root.display());
	assert!(p.root.join("runfiles/_main.run").is_file());
	let checkout = p.root.parent().unwrap().parent().unwrap();
	assert!(!checkout.join("big").exists(), "the rest of the repository came too");
	assert!(!checkout.join("runfiles").exists(), "the root's package came too");
}

/// Every file in `dir`, `/`-separated and relative to it.
fn files_in(dir: &Path) -> Vec<String> {
	fn walk(root: &Path, d: &Path, out: &mut Vec<String>) {
		for e in std::fs::read_dir(d).unwrap().flatten() {
			let p = e.path();
			if p.is_dir() {
				walk(root, &p, out);
			} else {
				out.push(slashed(p.strip_prefix(root).unwrap()));
			}
		}
	}
	let mut out = Vec::new();
	walk(dir, dir, &mut out);
	out.sort();
	out
}

#[test]
fn a_package_that_asks_for_nothing_more_gets_only_its_runfiles() {
	if !has_git() {
		return eprintln!("skipped: git is not installed");
	}
	let s = Server::new();
	let d = s.repo("acme/tool.git");
	for (f, body) in [
		("runfiles/_main.run", "# The tool\n\nrun helper\n"),
		("runfiles/_shared.run", "# Shared\n\nlet x = 1\n"),
		("runfiles/helper.run", "# Helper\n\nprint(\"h\")\n"),
		("sub/runfiles/x.run", "# A subproject\n\nprint(\"x\")\n"),
		("cmd/main.go", "package main\n"),
		("README.md", "big\n"),
	] {
		write(&d, f, body);
	}
	commit(&d, "one");
	git(&d, &["tag", "v1"]);
	let p = s
		.cache()
		.package(&remote("example.test/acme/tool@v1"), Network::Allowed, &quiet)
		.unwrap();
	assert_eq!(
		files_in(&p.root),
		[
			"runfiles/_main.run",
			"runfiles/_shared.run",
			"runfiles/helper.run",
			"sub/runfiles/x.run"
		],
		"a package that reads nothing through RUN.parent can reach nothing else"
	);
}

#[test]
fn a_package_that_asks_for_its_whole_repository_gets_all_of_it() {
	if !has_git() {
		return eprintln!("skipped: git is not installed");
	}
	// In a folder of its repository, and building what is outside it: the
	// whole repository is what it says, not only its own folder.
	let s = Server::new();
	let d = s.repo("acme/mono.git");
	for (f, body) in [
		(
			"tools/build/runfiles/_main.run",
			"# Builds the repository\n\n.requires-whole-repository = true\n\nprint(read_file(join_path(RUN.parent, \"..\", \"..\", \"go.mod\")))\n",
		),
		("go.mod", "module example.test/acme/mono\n"),
		("cmd/tool/main.go", "package main\n"),
	] {
		write(&d, f, body);
	}
	let c = commit(&d, "one");
	git(&d, &["tag", "v1"]);
	let cache = s.cache();
	let p = cache
		.package(
			&remote("example.test/acme/mono/tools/build@v1"),
			Network::Allowed,
			&quiet,
		)
		.unwrap();
	let repo = p.root.parent().unwrap().parent().unwrap();
	assert_eq!(
		repo.file_name().unwrap().to_str().unwrap(),
		c,
		"kept as the whole repository at its commit"
	);
	assert_eq!(
		files_in(repo),
		["cmd/tool/main.go", "go.mod", "tools/build/runfiles/_main.run"]
	);
	// Another package of the same repository at the same commit is found in it.
	let p2 = cache
		.package(
			&remote(&format!("example.test/acme/mono/tools/build@{c}")),
			Network::Offline,
			&quiet,
		)
		.unwrap();
	assert_eq!(p2.root, p.root);
}

#[test]
fn a_copy_of_only_runfiles_whose_entry_asks_for_more_has_the_rest_fetched() {
	if !has_git() {
		return eprintln!("skipped: git is not installed");
	}
	// What a runfile that kept only `runfiles/` -- or an older one, which kept
	// a different shape -- leaves for a runner that reads the entry differently:
	// the entry is read again, and the rest fetched rather than run without.
	let s = Server::new();
	let d = s.repo("acme/tool.git");
	write(
		&d,
		"runfiles/_main.run",
		"# The tool\n\n.requires-whole-repository = true\n\nprint(\"x\")\n",
	);
	write(&d, "cmd/main.go", "package main\n");
	commit(&d, "one");
	git(&d, &["tag", "v1"]);
	let cache = s.cache();
	let r = remote("example.test/acme/tool@v1");
	let p = cache.package(&r, Network::Allowed, &quiet).unwrap();
	// Made by hand into a copy of only its runfiles.
	let commit_dir = p.root.clone();
	let thin = std::fs::read_dir(commit_dir.parent().unwrap())
		.unwrap()
		.flatten()
		.map(|e| e.path())
		.find(|e| e.is_dir())
		.unwrap();
	assert_eq!(thin, commit_dir, "only the whole repository was kept");
	let renamed = commit_dir.with_file_name(format!(
		"{}-e3b0c44298fc",
		commit_dir.file_name().unwrap().to_str().unwrap()
	));
	std::fs::rename(&commit_dir, &renamed).unwrap();
	std::fs::remove_dir_all(renamed.join("cmd")).unwrap();
	// A preview fetches nothing, and says so rather than run a partial tree.
	let e = cache.package(&r, Network::Offline, &quiet).unwrap_err();
	assert!(e.0.contains("has not been fetched"), "{e}");
	let p = cache.package(&r, Network::Allowed, &quiet).unwrap();
	assert_eq!(p.root, commit_dir);
	assert!(p.root.join("cmd/main.go").is_file());
}

#[test]
fn an_entry_that_works_out_whether_it_needs_more_is_refused() {
	if !has_git() {
		return eprintln!("skipped: git is not installed");
	}
	let s = Server::new();
	let d = s.repo("acme/tool.git");
	write(
		&d,
		"runfiles/_main.run",
		"# The tool\n\n.requires-whole-repository = ENV.WHOLE\n\nprint(\"x\")\n",
	);
	commit(&d, "one");
	git(&d, &["tag", "v1"]);
	let e = s
		.cache()
		.package(&remote("example.test/acme/tool@v1"), Network::Allowed, &quiet)
		.unwrap_err();
	assert!(
		e.0.contains("runfiles/_main.run: line 3") && e.0.contains("not worked out"),
		"{e}"
	);
}

#[test]
fn what_cannot_be_run_says_why() {
	if !has_git() {
		return eprintln!("skipped: git is not installed");
	}
	let s = Server::new();
	tool(&s);
	let cache = s.cache();
	let e = cache
		.package(&remote("example.test/acme/tool@v9"), Network::Allowed, &quiet)
		.unwrap_err();
	assert!(e.0.contains("has no tag or branch named `v9`"), "{e}");
	let e = cache
		.package(&remote("example.test/acme/tool@e8e95d8"), Network::Allowed, &quiet)
		.unwrap_err();
	assert!(e.0.contains("all 40 characters"), "{e}");
	let e = cache
		.package(&remote("example.test/acme/tool/nope@v1.0.0"), Network::Allowed, &quiet)
		.unwrap_err();
	assert!(
		e.0.contains("is not a package: there is no `runfiles/_main.run` in `nope`"),
		"{e}"
	);
	let e = cache
		.package(&remote("example.test/acme/missing@v1"), Network::Allowed, &quiet)
		.unwrap_err();
	assert!(e.0.contains("could not ask example.test/acme/missing"), "{e}");
	// A preview fetches nothing, and says how to.
	let e = s
		.cache()
		.package(&remote("example.test/acme/other@v1"), Network::Offline, &quiet)
		.unwrap_err();
	assert!(e.0.contains("run :fetch example.test/acme/other@v1"), "{e}");
}

#[test]
fn a_repository_once_fetched_is_known() {
	if !has_git() {
		return eprintln!("skipped: git is not installed");
	}
	let s = Server::new();
	tool(&s);
	let cache = s.cache();
	let r = remote("example.test/acme/tool@v1.0.0");
	assert!(!cache.knows(&r));
	assert!(cache.cached(&r).is_none());
	cache.package(&r, Network::Allowed, &quiet).unwrap();
	assert!(cache.knows(&remote("example.test/acme/tool@main")));
	assert!(cache.cached(&r).is_some());
}

// ---------------------------------------------------------------- download

fn file_url(p: &Path) -> String {
	let s = slashed(p);
	if s.starts_with('/') {
		format!("file://{s}")
	} else {
		format!("file:///{s}")
	}
}

fn sha256(bytes: &[u8]) -> String {
	use sha2::Digest;
	hex::encode(sha2::Sha256::digest(bytes))
}

/// A `.tar` holding `name` with `body`, executable.
fn tar_of(name: &str, body: &[u8]) -> Vec<u8> {
	let mut b = tar::Builder::new(Vec::new());
	let mut h = tar::Header::new_ustar();
	h.set_size(body.len() as u64);
	h.set_mode(0o755);
	h.set_cksum();
	b.append_data(&mut h, name, body).unwrap();
	b.into_inner().unwrap()
}

fn gz(bytes: &[u8]) -> Vec<u8> {
	use std::io::Write;
	let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
	e.write_all(bytes).unwrap();
	e.finish().unwrap()
}

fn xz(bytes: &[u8]) -> Vec<u8> {
	let mut out = Vec::new();
	lzma_rs::xz_compress(&mut std::io::BufReader::new(bytes), &mut out).unwrap();
	out
}

fn has_curl() -> bool {
	Command::new("curl")
		.arg("--version")
		.output()
		.is_ok_and(|o| o.status.success())
}

#[test]
fn an_archive_is_unpacked_once_and_kept() {
	if !has_curl() {
		return eprintln!("skipped: curl is not installed");
	}
	let src = TempDir::new().unwrap();
	let cache_dir = TempDir::new().unwrap();
	let cache = Cache::new(cache_dir.path().to_path_buf());
	for (name, bytes) in [
		(
			"tool-linux-x86-64.tar.gz",
			gz(&tar_of("tool-linux-x86-64/tool", b"#!/bin/sh\necho hi\n")),
		),
		(
			"tool-linux-x86-64.tar.xz",
			xz(&tar_of("tool-linux-x86-64/tool", b"#!/bin/sh\necho hi\n")),
		),
		(
			"tool-linux-x86-64.tar",
			tar_of("tool-linux-x86-64/tool", b"#!/bin/sh\necho hi\n"),
		),
	] {
		let at = src.path().join(name);
		std::fs::write(&at, &bytes).unwrap();
		let url = file_url(&at);
		let dir = cache.download(&url, Some(&sha256(&bytes)), &quiet).unwrap();
		let exe = dir.join("tool-linux-x86-64/tool");
		assert_eq!(std::fs::read(&exe).unwrap(), b"#!/bin/sh\necho hi\n", "{name}");
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;
			assert_eq!(
				std::fs::metadata(&exe).unwrap().permissions().mode() & 0o111,
				0o111,
				"{name}"
			);
		}
		// Gone from where it came, and still there: a URL is fetched once.
		std::fs::remove_file(&at).unwrap();
		assert_eq!(
			cache.download(&url, None, &|m| panic!("fetched again: {m}")).unwrap(),
			dir
		);
	}
}

#[test]
fn a_lone_file_is_kept_whole_and_made_executable() {
	if !has_curl() {
		return eprintln!("skipped: curl is not installed");
	}
	let src = TempDir::new().unwrap();
	let cache = Cache::new(TempDir::new().unwrap().keep());
	std::fs::write(src.path().join("tool-mac-arm64"), b"\x7fELF binary").unwrap();
	std::fs::write(src.path().join("notes.txt.gz"), gz(b"just text")).unwrap();
	let file = cache
		.download(&file_url(&src.path().join("tool-mac-arm64")), None, &quiet)
		.unwrap();
	assert_eq!(file.file_name().unwrap(), "tool-mac-arm64");
	assert_eq!(std::fs::read(&file).unwrap(), b"\x7fELF binary");
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o755);
	}
	let text = cache
		.download(&file_url(&src.path().join("notes.txt.gz")), None, &quiet)
		.unwrap();
	assert_eq!(text.file_name().unwrap(), "notes.txt");
	assert_eq!(std::fs::read(&text).unwrap(), b"just text");
	let _ = std::fs::remove_dir_all(cache.root());
}

#[test]
fn a_download_that_is_not_what_was_pinned_is_refused_and_not_kept() {
	if !has_curl() {
		return eprintln!("skipped: curl is not installed");
	}
	let src = TempDir::new().unwrap();
	let cache_dir = TempDir::new().unwrap();
	let cache = Cache::new(cache_dir.path().to_path_buf());
	let at = src.path().join("tool");
	std::fs::write(&at, b"one").unwrap();
	let url = file_url(&at);
	let e = cache.download(&url, Some(&sha256(b"two")), &quiet).unwrap_err();
	assert!(e.0.contains("is not what was pinned"), "{e}");
	assert!(
		!cache_dir.path().join("downloads").exists()
			|| std::fs::read_dir(cache_dir.path().join("downloads"))
				.unwrap()
				.next()
				.is_none()
	);
	// Kept once, and fetched again when a pin says what is kept is not it.
	cache.download(&url, None, &quiet).unwrap();
	std::fs::write(&at, b"two").unwrap();
	let file = cache.download(&url, Some(&sha256(b"two")), &quiet).unwrap();
	assert_eq!(std::fs::read(file).unwrap(), b"two");
}

#[test]
fn only_https_and_file_addresses_are_downloaded() {
	let cache = Cache::new(TempDir::new().unwrap().keep());
	for url in ["http://example.test/x", "ftp://example.test/x", "example.test/x"] {
		let e = cache.download(url, None, &quiet).unwrap_err();
		assert!(e.0.contains("`https://` and `file://`"), "{url}: {e}");
	}
	let e = cache
		.download("https://example.test/x", Some("abc"), &quiet)
		.unwrap_err();
	assert!(e.0.contains("64 hex digits"), "{e}");
	let _ = std::fs::remove_dir_all(cache.root());
}
