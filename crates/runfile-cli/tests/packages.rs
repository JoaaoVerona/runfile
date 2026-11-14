//! `run <package>`, driven as a subprocess: a repository fetched into the
//! cache, or a folder on disk, run from wherever someone stands.
//!
//! The repositories are served from a folder: the child is handed git's own
//! `insteadOf` for `https://example.test/` through `GIT_CONFIG_COUNT`, so each
//! fetch is the real one and only the transport is a file. Everything that
//! needs git or a POSIX shell skips cleanly where there is none.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

const CI_VARS: &[&str] = &[
	"CI",
	"GITHUB_ACTIONS",
	"GITLAB_CI",
	"CIRCLECI",
	"TRAVIS",
	"BUILDKITE",
	"JENKINS_URL",
	"TF_BUILD",
	"TEAMCITY_VERSION",
	"BITBUCKET_BUILD_NUMBER",
];

struct World {
	/// Where repositories are served from, as `example.test/<owner>/<repo>.git`.
	server: TempDir,
	home: TempDir,
	/// Where `run` is run: a project, or nothing at all.
	here: TempDir,
}

impl World {
	fn new() -> Self {
		Self {
			server: TempDir::new().unwrap(),
			home: TempDir::new().unwrap(),
			here: TempDir::new().unwrap(),
		}
	}

	fn cache(&self) -> PathBuf {
		self.home.path().join("cache")
	}

	fn command(&self, cwd: &Path, args: &[&str]) -> Command {
		let mut c = Command::new(env!("CARGO_BIN_EXE_run"));
		c.args(args)
			.current_dir(cwd)
			.env("HOME", self.home.path())
			.env("USERPROFILE", self.home.path())
			.env("RUNFILE_CONFIG_DIR", self.home.path())
			.env("RUNFILE_CACHE_DIR", self.cache())
			.env("XDG_CONFIG_HOME", self.home.path())
			.env("XDG_STATE_HOME", self.home.path())
			.env("XDG_CACHE_HOME", self.home.path())
			.env("APPDATA", self.home.path())
			.env("GIT_CONFIG_NOSYSTEM", "1")
			.env("GIT_CONFIG_COUNT", "1")
			.env(
				"GIT_CONFIG_KEY_0",
				format!("url.{}/.insteadOf", file_url(self.server.path())),
			)
			.env("GIT_CONFIG_VALUE_0", "https://example.test/")
			.env_remove("RUNFILE_SKIP_PREPARE")
			.env_remove("RUNFILE_PRIVATE_KEYS")
			.env_remove("RUNFILE_SAFE_DIRECTORIES")
			.env_remove("FORCE_COLOR")
			.env_remove("DBUS_SESSION_BUS_ADDRESS");
		for v in CI_VARS {
			c.env_remove(v);
		}
		c
	}

	fn run_in(&self, cwd: &Path, args: &[&str]) -> Output {
		self.command(cwd, args)
			.stdin(Stdio::null())
			.output()
			.expect("run binary")
	}

	fn run(&self, args: &[&str]) -> Output {
		self.run_in(self.here.path(), args)
	}

	/// A repository at `example.test/<path>`, with `files`, committed and
	/// tagged `v1.0.0`. Answers with the commit.
	fn repo(&self, path: &str, files: &[(&str, &str)]) -> String {
		let d = self.server.path().join(path);
		std::fs::create_dir_all(&d).unwrap();
		git(&d, &["init", "-q", "-b", "main"]);
		write_all(&d, files);
		git(&d, &["add", "-A"]);
		git(&d, &["commit", "-q", "-m", "one"]);
		git(&d, &["tag", "-a", "v1.0.0", "-m", "v1"]);
		git(&d, &["rev-parse", "HEAD"])
	}
}

fn write_all(dir: &Path, files: &[(&str, &str)]) {
	for (p, body) in files {
		let full = dir.join(p);
		std::fs::create_dir_all(full.parent().unwrap()).unwrap();
		std::fs::write(full, body).unwrap();
	}
}

fn slashed(p: &Path) -> String {
	p.display().to_string().replace('\\', "/")
}

/// `file:///tmp/x` and `file:///C:/x`, as git and curl read a local path.
fn file_url(p: &Path) -> String {
	let s = slashed(p);
	match s.starts_with('/') {
		true => format!("file://{s}"),
		false => format!("file:///{s}"),
	}
}

fn git(dir: &Path, args: &[&str]) -> String {
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
		.env("GIT_CONFIG_GLOBAL", dir.join(".no-config"))
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

fn has_git() -> bool {
	Command::new("git")
		.arg("--version")
		.output()
		.is_ok_and(|o| o.status.success())
}

fn out(o: &Output) -> String {
	String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
	String::from_utf8_lossy(&o.stderr).into_owned()
}

/// A package that says where it is, what it was given and which version it is.
/// It marks where it ran with a file, since a relative path is written where
/// the line runs: the same answer on every platform, which a shell's `pwd`
/// -- `/c/Users/…` under Git Bash -- is not.
const TELLS: &str = "# Says where it is\n\nprint(\"args={{ ARGS }}\")\nprint(\"version={{ RUN.version }}\")\nprint(\"parent={{ RUN.parent }}\")\nwrite_file(\"ran-here\", \"\")\n";

/// Whether the package ran in `dir`, and only there.
fn ran_in(dir: &Path) -> bool {
	dir.join("ran-here").is_file()
}

#[test]
fn a_package_runs_where_it_was_run_from_with_its_tag_as_its_version() {
	if !has_git() {
		return eprintln!("skipped: git is not installed");
	}
	let w = World::new();
	let commit = w.repo("acme/tool.git", &[("runfiles/_main.run", TELLS)]);
	// No project here at all: a package is run from wherever someone stands.
	let o = w.run(&["-y", "example.test/acme/tool@v1.0.0", "sync", "--help", "-f", "x.json"]);
	assert!(o.status.success(), "{}", err(&o));
	// `--help` after a package is the package's own, as is everything else.
	assert!(out(&o).contains("args=sync --help -f x.json"), "{}", out(&o));
	assert!(out(&o).contains("version=v1.0.0"), "{}", out(&o));
	assert!(
		out(&o).contains(&format!(
			"parent={}",
			w.cache()
				.join("packages/example.test/acme/tool")
				.join(&commit)
				.display()
		)),
		"{}",
		out(&o)
	);
	assert!(ran_in(w.here.path()), "it did not run where it was run from");
	assert!(
		!ran_in(&w.cache().join("packages/example.test/acme/tool").join(&commit)),
		"it ran in the cache"
	);
	assert!(
		err(&o).contains(&format!("fetching example.test/acme/tool@v1.0.0 ({})", &commit[..7])),
		"{}",
		err(&o)
	);
	// Kept: the second run fetches nothing.
	let o = w.run(&["example.test/acme/tool@v1.0.0"]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(!err(&o).contains("fetching"), "{}", err(&o));
}

#[test]
fn a_package_typed_at_the_terminal_is_asked_about_once_per_repository() {
	if !has_git() {
		return eprintln!("skipped: git is not installed");
	}
	let w = World::new();
	w.repo("acme/tool.git", &[("runfiles/_main.run", TELLS)]);
	// Nobody to say yes: nothing is fetched, and the message says how to.
	let o = w.run(&["example.test/acme/tool@v1.0.0"]);
	assert!(!o.status.success());
	assert!(
		err(&o).contains("not fetched: example.test/acme/tool runs only once you say yes, or with `-y`"),
		"{}",
		err(&o)
	);
	assert!(
		!w.cache()
			.join("packages/example.test/acme/tool")
			.read_dir()
			.is_ok_and(|mut d| d.any(|e| e.is_ok_and(|e| e.path().is_dir())))
	);
	// Answered on stdin under `--stdin-args`, which says someone answers there.
	let mut c = w.command(w.here.path(), &["--stdin-args", "example.test/acme/tool@v1.0.0"]);
	let child = c
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.unwrap();
	use std::io::Write;
	let mut child = child;
	let _ = child.stdin.take().unwrap().write_all(b"y\n\n");
	let o = child.wait_with_output().unwrap();
	assert!(o.status.success(), "{}", err(&o));
	assert!(
		err(&o).contains("has not been run on this machine; fetch example.test/acme/tool@v1.0.0"),
		"{}",
		err(&o)
	);
	// Known now: another version of it is not asked about.
	let o = w.run(&["example.test/acme/tool@main"]);
	assert!(o.status.success(), "{}", err(&o));
}

#[test]
fn ci_and_a_file_are_never_asked() {
	if !has_git() {
		return eprintln!("skipped: git is not installed");
	}
	let w = World::new();
	w.repo("acme/tool.git", &[("runfiles/_main.run", TELLS)]);
	let o = w
		.command(w.here.path(), &["example.test/acme/tool@v1.0.0"])
		.env("CI", "true")
		.output()
		.unwrap();
	assert!(o.status.success(), "{}", err(&o));

	let w = World::new();
	w.repo("acme/tool.git", &[("runfiles/_main.run", TELLS)]);
	write_all(
		w.here.path(),
		&[(
			"runfiles/locales.run",
			"# Translate\n\nrun example.test/acme/tool@v1.0.0 sync\n",
		)],
	);
	let o = w.run(&["locales"]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(out(&o).contains("args=sync"), "{}", out(&o));
}

#[test]
fn a_package_starts_in_the_directory_of_the_line_that_ran_it() {
	if !has_git() {
		return eprintln!("skipped: git is not installed");
	}
	// The monorepo shape: one folder pins the tool, and every subproject
	// reaches it by path -- from the root or from inside the subproject.
	let w = World::new();
	w.repo("acme/tool.git", &[("runfiles/_main.run", TELLS)]);
	write_all(
		w.here.path(),
		&[
			("runfiles/i18n.run", "# Every package\n\nrun web-docs:i18n\n"),
			(
				"tools/j18n/runfiles/_main.run",
				"# The tool, pinned once\n\nrun example.test/acme/tool@v1.0.0 {{ ARGS }}\n",
			),
			(
				"web-docs/runfiles/i18n.run",
				"# Translate\n\nrun ../tools/j18n sync -f x.json\n",
			),
		],
	);
	let docs = w.here.path().join("web-docs");
	for from in [w.here.path().to_path_buf(), w.here.path().join("web-docs")] {
		let _ = std::fs::remove_file(docs.join("ran-here"));
		// The root's `i18n` from the root, and `web-docs`'s own from inside it.
		let o = w.run_in(&from, &["i18n"]);
		assert!(o.status.success(), "{}", err(&o));
		assert!(out(&o).contains("args=sync -f x.json"), "{}", out(&o));
		assert!(ran_in(&docs), "run from {}", from.display());
		assert!(
			!ran_in(w.here.path()) && !ran_in(&w.here.path().join("tools/j18n")),
			"run from {}",
			from.display()
		);
	}
	// And a `set_cwd()` above the line moves where it starts, as it moves any
	// command.
	write_all(
		w.here.path(),
		&[
			(
				"web-docs/runfiles/elsewhere.run",
				"# Elsewhere\n\nset_cwd(\"src\")\nrun ../../tools/j18n\n",
			),
			("web-docs/src/.keep", ""),
		],
	);
	let o = w.run_in(&w.here.path().join("web-docs"), &["elsewhere"]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(ran_in(&docs.join("src")));
}

#[test]
fn a_package_s_own_names_are_its_own() {
	// A package's `run helper` is its helper, never the caller's.
	let w = World::new();
	write_all(
		w.here.path(),
		&[
			("runfiles/helper.run", "# The caller's\n\nprint(\"caller helper\")\n"),
			("runfiles/go.run", "# Go\n\nrun ./pkg\n"),
			("pkg/runfiles/_main.run", "# A package\n\nrun helper\nrun nope\n"),
			(
				"pkg/runfiles/helper.run",
				"# The package's\n\nprint(\"package helper\")\n",
			),
		],
	);
	let o = w.run(&["go"]);
	assert_eq!(out(&o), "package helper\n", "{}", err(&o));
	assert!(err(&o).contains("no target named `nope`"), "{}", err(&o));
	// Its version is empty: it was fetched at no tag, being on disk.
	write_all(
		w.here.path(),
		&[(
			"pkg/runfiles/_main.run",
			"# A package\n\nprint(\"[{{ RUN.version }}]\")\n",
		)],
	);
	assert_eq!(out(&w.run(&["./pkg"])), "[]\n");
}

#[test]
fn what_cannot_be_a_package_says_why_before_anything_runs() {
	let w = World::new();
	write_all(
		w.here.path(),
		&[
			(
				"runfiles/bad.run",
				"# Bad\n\nprint(\"ran\")\nrun example.test/acme/tool sync\n",
			),
			("empty/README", "nothing here"),
		],
	);
	let o = w.run(&["bad"]);
	assert!(!o.status.success());
	assert!(!out(&o).contains("ran"), "a line above the reference ran: {}", out(&o));
	assert!(
		err(&o).contains("names no version") && err(&o).contains("[package-reference]"),
		"{}",
		err(&o)
	);
	let o = w.run(&["./empty"]);
	assert!(
		err(&o).contains("`./empty` is not a package: there is no `runfiles/_main.run`"),
		"{}",
		err(&o)
	);
	let o = w.run(&["./missing"]);
	assert!(
		err(&o).contains("`./missing` is not a package") && err(&o).contains("does not exist"),
		"{}",
		err(&o)
	);
	let o = w.run(&["tools/j18n"]);
	assert!(err(&o).contains("is neither a package on disk"), "{}", err(&o));
	// `:lint` asks the same question and no other: a package is not a target
	// it can find in the catalog, and is not reported as a missing one.
	write_all(
		w.here.path(),
		&[(
			"runfiles/bad.run",
			"# Fine\n\nrun example.test/acme/tool@v1.0.0 sync\nrun ./pkg\n",
		)],
	);
	let o = w.run(&[":lint", "--check"]);
	assert!(o.status.success(), "{}{}", out(&o), err(&o));
}

#[test]
fn a_preview_fetches_nothing_and_fetch_fills_the_cache() {
	if !has_git() {
		return eprintln!("skipped: git is not installed");
	}
	let w = World::new();
	let commit = w.repo(
		"acme/tool.git",
		&[("runfiles/_main.run", "# Tool\n\n$ echo from-the-tool\n")],
	);
	let o = w.run(&["--dry-run", "example.test/acme/tool@v1.0.0"]);
	assert!(!o.status.success());
	assert!(
		err(&o).contains("a preview fetches nothing; `run :fetch example.test/acme/tool@v1.0.0` fetches it"),
		"{}",
		err(&o)
	);

	let o = w.run(&[":fetch", "example.test/acme/tool@v1.0.0", "example.test/acme/tool@main"]);
	assert!(o.status.success(), "{}", err(&o));
	assert_eq!(
		out(&o),
		format!(
			"example.test/acme/tool@v1.0.0: {} (tag v1.0.0)\nexample.test/acme/tool@main: {}\n",
			&commit[..7],
			&commit[..7]
		)
	);
	let o = w.run(&["--dry-run", "example.test/acme/tool@v1.0.0"]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(out(&o).contains("echo from-the-tool"), "{}", out(&o));
	assert!(
		!out(&o).lines().any(|l| l == "from-the-tool"),
		"a preview ran the command: {}",
		out(&o)
	);

	assert!(err(&w.run(&[":fetch", "./x"])).contains("needs no fetching"));
	assert!(out(&w.run(&[":fetch"])).contains("run :fetch <package>..."));
}

#[cfg(unix)]
#[test]
fn a_package_hands_itself_over_to_a_program_and_ends_the_way_it_did() {
	if !has_git() {
		return eprintln!("skipped: git is not installed");
	}
	// The whole of what j18n's `_main.run` does: download its build for this
	// machine and become it -- the program's stdin, its output and its status,
	// with nothing of the runner's in between.
	let w = World::new();
	let assets = TempDir::new().unwrap();
	let name = format!("tool-{}-{}", os(), arch());
	let bin = assets.path().join(&name);
	std::fs::create_dir_all(&bin).unwrap();
	std::fs::write(
		bin.join("tool"),
		"#!/bin/sh\nread line\necho \"tool $* read $line in $(pwd)\"\nexit 7\n",
	)
	.unwrap();
	let tarball = assets.path().join(format!("{name}.tar.gz"));
	std::fs::set_permissions(bin.join("tool"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
	let made = Command::new("tar")
		.args([
			"-czf",
			&tarball.display().to_string(),
			"-C",
			&assets.path().display().to_string(),
			&name,
		])
		.status()
		.unwrap();
	assert!(made.success());
	let entry = format!(
		"# The tool\n\nlet name = \"tool-{{{{ RUN.os }}}}-{{{{ RUN.arch }}}}\"\nlet dir = download(\"{}/{{{{ name }}}}.tar.gz\")\nlet status = code_of(exec {{{{ join_path(dir, name, \"tool\") }}}} {{{{ ARGS }}}})\n\nexit(status)\n",
		file_url(assets.path())
	);
	w.repo("acme/tool.git", &[("runfiles/_main.run", &entry)]);
	let mut child = w
		.command(
			w.here.path(),
			&["-y", "example.test/acme/tool@v1.0.0", "sync", "--help"],
		)
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.unwrap();
	use std::io::Write;
	child.stdin.take().unwrap().write_all(b"typed\n").unwrap();
	let o = child.wait_with_output().unwrap();
	assert_eq!(o.status.code(), Some(7), "{}", err(&o));
	assert_eq!(
		out(&o),
		format!(
			"tool sync --help read typed in {}\n",
			std::fs::canonicalize(w.here.path()).unwrap().display()
		)
	);
	// A status is the program's answer, not a failure of the runner's to report.
	assert!(!err(&o).contains("error"), "{}", err(&o));
	assert!(err(&o).contains("downloading file://"), "{}", err(&o));
}

#[cfg(unix)]
fn os() -> &'static str {
	if cfg!(target_os = "macos") { "mac" } else { "linux" }
}

#[cfg(unix)]
fn arch() -> &'static str {
	match std::env::consts::ARCH {
		"x86_64" => "x86-64",
		"aarch64" => "arm64",
		"riscv64" => "riscv64",
		_ => "unknown",
	}
}
