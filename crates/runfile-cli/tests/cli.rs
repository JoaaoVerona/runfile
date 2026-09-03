//! CLI behaviour, driven as a subprocess.
//!
//! HOME/XDG/APPDATA point at an empty directory so a developer's real global
//! targets and prepare state cannot leak in and change what these assert.

use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use tempfile::TempDir;

struct Project {
	dir: TempDir,
	home: TempDir,
}

fn project(files: &[(&str, &str)]) -> Project {
	let dir = TempDir::new().unwrap();
	for (p, body) in files {
		let full = dir.path().join(p);
		std::fs::create_dir_all(full.parent().unwrap()).unwrap();
		std::fs::write(full, body).unwrap();
	}
	Project {
		dir,
		home: TempDir::new().unwrap(),
	}
}

impl Project {
	fn run(&self, args: &[&str]) -> Output {
		self.run_in(self.dir.path(), args)
	}

	/// Spawn without waiting — for watch mode, which never exits on its own.
	fn spawn(&self, args: &[&str]) -> Child {
		self.command(self.dir.path(), args)
			.stdout(Stdio::null())
			.stderr(Stdio::null())
			.spawn()
			.expect("spawn run")
	}

	fn run_in(&self, cwd: &Path, args: &[&str]) -> Output {
		self.command(cwd, args).output().expect("run binary")
	}

	fn command(&self, cwd: &Path, args: &[&str]) -> Command {
		let mut c = Command::new(env!("CARGO_BIN_EXE_run"));
		c.args(args)
			.current_dir(cwd)
			.env("HOME", self.home.path())
			.env("XDG_CONFIG_HOME", self.home.path())
			.env("XDG_STATE_HOME", self.home.path())
			.env("APPDATA", self.home.path())
			.env_remove("CI")
			.env_remove("GITHUB_ACTIONS");
		c
	}
}

/// Poll until `f` holds. Watch mode is inherently asynchronous — a fixed sleep
/// would be either flaky or slow, so every wait here is a bounded poll.
fn until(what: &str, mut f: impl FnMut() -> bool) {
	let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
	while std::time::Instant::now() < deadline {
		if f() {
			return;
		}
		std::thread::sleep(std::time::Duration::from_millis(25));
	}
	panic!("timed out waiting for {what}");
}

fn out(o: &Output) -> String {
	String::from_utf8_lossy(&o.stdout).into_owned()
}
fn err(o: &Output) -> String {
	String::from_utf8_lossy(&o.stderr).into_owned()
}

const MARK: &str = "runfiles/mark.run";
fn marker(path: &str) -> String {
	format!("# Writes a marker\n$ printf done > {path}\n")
}

// ------------------------------------------------------------------ invoking

#[test]
fn a_target_runs_and_exits_zero() {
	let p = project(&[(MARK, &marker("out.txt"))]);
	let o = p.run(&["mark"]);
	assert!(o.status.success(), "{}", err(&o));
	assert_eq!(std::fs::read_to_string(p.dir.path().join("out.txt")).unwrap(), "done");
}

#[test]
fn a_failing_target_exits_non_zero_and_says_why() {
	let p = project(&[("runfiles/boom.run", "$ exit 7\n")]);
	let o = p.run(&["boom"]);
	assert!(!o.status.success());
	assert!(err(&o).contains("status 7"), "{}", err(&o));
}

#[test]
fn runner_flags_before_the_target_are_consumed_and_after_it_are_passed_through() {
	// `run build --dry-run` must give the target its flag, not swallow it.
	let p = project(&[("runfiles/echoes.run", "$ printf '%s' {{ FLAG.dry-run }} > flag.txt\n")]);
	let o = p.run(&["echoes", "--dry-run"]);
	assert!(o.status.success(), "{}", err(&o));
	assert_eq!(
		std::fs::read_to_string(p.dir.path().join("flag.txt")).unwrap(),
		"true",
		"the flag reached the target"
	);
}

#[test]
fn arguments_reach_the_target_by_kind() {
	let p = project(&[(
		"runfiles/args.run",
		"$ printf '%s|%s|%s' {{ ARG.name }} {{ FLAG.loud }} {{ ARGS }} > got.txt\n",
	)]);
	let o = p.run(&["args", "--name=ada", "--loud", "first"]);
	assert!(o.status.success(), "{}", err(&o));
	assert_eq!(
		std::fs::read_to_string(p.dir.path().join("got.txt")).unwrap(),
		"ada|true|first"
	);
}

#[test]
fn discovery_walks_upward_from_a_nested_directory() {
	let p = project(&[(MARK, &marker("out.txt"))]);
	let deep = p.dir.path().join("src/deep");
	std::fs::create_dir_all(&deep).unwrap();
	let o = p.run_in(&deep, &["mark"]);
	assert!(o.status.success(), "{}", err(&o));
}

// --------------------------------------------------------------------- :list

#[test]
fn list_shows_descriptions_and_groups_subprojects() {
	let p = project(&[
		("runfiles/build.run", "# Builds everything\n$ true\n"),
		("api/runfiles/build.run", "# Builds the api\n$ true\n"),
	]);
	let o = p.run(&[":list"]);
	let text = out(&o);
	assert!(text.contains("Builds everything"), "{text}");
	assert!(text.contains("subprojects:"), "{text}");
	assert!(text.contains("api:build"), "{text}");
}

#[test]
fn a_hidden_target_is_absent_from_list_but_still_runnable() {
	let p = project(&[("runfiles/helper.run", ".hide\n$ printf hi > out.txt\n")]);
	assert!(!out(&p.run(&[":list"])).contains("helper"));
	assert!(p.run(&["helper"]).status.success(), "hiding is not disabling");
}

#[test]
fn an_unknown_target_suggests_near_matches_and_fails() {
	let p = project(&[("runfiles/build.run", "$ true\n")]);
	let o = p.run(&["buil"]);
	assert!(!o.status.success());
	assert!(err(&o).contains("did you mean"), "{}", err(&o));
	assert!(err(&o).contains("build"), "{}", err(&o));
}

#[test]
fn no_arguments_prints_usage_and_succeeds() {
	let p = project(&[("runfiles/build.run", "$ true\n")]);
	let o = p.run(&[]);
	assert!(o.status.success());
	assert!(out(&o).contains("run <target>"), "{}", out(&o));
}

#[test]
fn an_unknown_colon_command_is_an_error_not_a_target_lookup() {
	let p = project(&[("runfiles/build.run", "$ true\n")]);
	let o = p.run(&[":nonsense"]);
	assert!(!o.status.success());
	assert!(err(&o).contains("unknown command"), "{}", err(&o));
}

// ------------------------------------------------------------------ dry-run

#[test]
fn dry_run_prints_resolved_commands_without_running_them() {
	let p = project(&[(
		"runfiles/touchy.run",
		"let name = \"out.txt\"\n$ printf done > {{ name }}\n",
	)]);
	let o = p.run(&["--dry-run", "touchy"]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(
		out(&o).contains("printf done > out.txt"),
		"fully interpolated: {}",
		out(&o)
	);
	assert!(!p.dir.path().join("out.txt").exists(), "nothing actually ran");
}

// ------------------------------------------------------------- prepare gate

#[test]
fn the_gate_blocks_until_setup_has_run_then_stops_blocking() {
	let p = project(&[
		("runfiles/setup.run", "$ true\n"),
		("runfiles/build.run", "$ printf done > out.txt\n"),
	]);
	let blocked = p.run(&["build"]);
	assert!(!blocked.status.success());
	assert!(err(&blocked).contains("never been run"), "{}", err(&blocked));

	assert!(p.run(&["setup"]).status.success());
	let after = p.run(&["build"]);
	assert!(after.status.success(), "{}", err(&after));
}

#[test]
fn editing_setup_re_triggers_the_gate() {
	// The fingerprint is over setup's own text, so changing what it does
	// invalidates it -- while runtime values never do.
	let p = project(&[("runfiles/setup.run", "$ true\n"), ("runfiles/build.run", "$ true\n")]);
	assert!(p.run(&["setup"]).status.success());
	assert!(p.run(&["build"]).status.success());

	std::fs::write(p.dir.path().join("runfiles/setup.run"), "$ true\n$ true\n").unwrap();
	let o = p.run(&["build"]);
	assert!(!o.status.success());
	assert!(err(&o).contains("has changed"), "{}", err(&o));
}

#[test]
fn the_gate_does_not_gate_itself() {
	let p = project(&[("runfiles/setup.run", "$ printf done > out.txt\n")]);
	assert!(
		p.run(&["setup"]).status.success(),
		"setup must be runnable when unprepared"
	);
}

#[test]
fn skip_prepare_bypasses_the_gate() {
	let p = project(&[("runfiles/setup.run", "$ true\n"), ("runfiles/build.run", "$ true\n")]);
	let o = Command::new(env!("CARGO_BIN_EXE_run"))
		.args(["build"])
		.current_dir(p.dir.path())
		.env("HOME", p.home.path())
		.env("XDG_CONFIG_HOME", p.home.path())
		.env("RUNFILE_SKIP_PREPARE", "1")
		.env_remove("CI")
		.output()
		.unwrap();
	assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

// ---------------------------------------------------------------- confirm

#[test]
fn confirm_cancels_when_stdin_is_not_a_terminal() {
	// A test harness has no terminal, so an unconsented target must not run.
	let p = project(&[("runfiles/risky.run", ".confirm = \"proceed?\"\n$ printf x > out.txt\n")]);
	let o = p.run(&["risky"]);
	assert!(!o.status.success());
	assert!(!p.dir.path().join("out.txt").exists());
}

#[test]
fn yes_skips_the_confirmation() {
	let p = project(&[("runfiles/risky.run", ".confirm = \"proceed?\"\n$ printf x > out.txt\n")]);
	assert!(p.run(&["-y", "risky"]).status.success());
	assert!(p.dir.path().join("out.txt").exists());
}

#[test]
fn ci_is_treated_as_consent() {
	let p = project(&[("runfiles/risky.run", ".confirm = \"proceed?\"\n$ true\n")]);
	let o = Command::new(env!("CARGO_BIN_EXE_run"))
		.args(["risky"])
		.current_dir(p.dir.path())
		.env("HOME", p.home.path())
		.env("XDG_CONFIG_HOME", p.home.path())
		.env("CI", "true")
		.output()
		.unwrap();
	assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

// ------------------------------------------------------------------ globals

#[test]
fn a_target_in_the_machine_wide_directory_is_reachable_anywhere() {
	let p = project(&[("runfiles/local.run", "$ true\n")]);
	let g = p.home.path().join(".runfiles");
	std::fs::create_dir_all(&g).unwrap();
	std::fs::write(g.join("deploy.run"), "# Ships it\n$ true\n").unwrap();

	let listed = out(&p.run(&[":list"]));
	assert!(listed.contains("global:"), "{listed}");
	assert!(listed.contains("deploy"), "{listed}");
	assert!(p.run(&["deploy"]).status.success());
}

// -------------------------------------------------------------------- watch

/// Counts runs by appending a byte per run, so the test can tell a re-run from
/// a slow first run.
const COUNTER: &str = "# Counts its own runs\n.watch = \"src/**\"\n$ printf x >> runs.txt\n";

#[test]
fn a_watch_target_reruns_when_a_watched_file_changes() {
	let p = project(&[("runfiles/tick.run", COUNTER), ("src/a.txt", "1")]);
	let runs = p.dir.path().join("runs.txt");
	let mut child = p.spawn(&["tick"]);

	until("the first run", || std::fs::read(&runs).is_ok_and(|b| b == b"x"));
	std::fs::write(p.dir.path().join("src/a.txt"), "2").unwrap();
	until("the re-run", || std::fs::read(&runs).is_ok_and(|b| b.len() >= 2));

	child.kill().unwrap();
	child.wait().unwrap();
}

#[test]
fn a_change_outside_the_watched_patterns_does_not_rerun() {
	let p = project(&[("runfiles/tick.run", COUNTER), ("src/a.txt", "1"), ("docs/b.txt", "1")]);
	let runs = p.dir.path().join("runs.txt");
	let mut child = p.spawn(&["tick"]);

	until("the first run", || std::fs::read(&runs).is_ok_and(|b| b == b"x"));
	std::fs::write(p.dir.path().join("docs/b.txt"), "2").unwrap();
	// Prove the watcher is alive and merely uninterested: an unwatched write
	// changes nothing, a watched one does.
	std::thread::sleep(std::time::Duration::from_millis(600));
	assert_eq!(std::fs::read(&runs).unwrap(), b"x", "docs/ is not watched");
	std::fs::write(p.dir.path().join("src/a.txt"), "2").unwrap();
	until("the re-run", || std::fs::read(&runs).is_ok_and(|b| b.len() >= 2));

	child.kill().unwrap();
	child.wait().unwrap();
}

#[test]
fn watch_keeps_going_after_a_failing_run() {
	let p = project(&[
		(
			"runfiles/tick.run",
			"# Fails\n.watch = \"src/**\"\n$ printf x >> runs.txt\n$ exit 1\n",
		),
		("src/a.txt", "1"),
	]);
	let runs = p.dir.path().join("runs.txt");
	let mut child = p.spawn(&["tick"]);

	until("the first run", || std::fs::read(&runs).is_ok_and(|b| b == b"x"));
	std::fs::write(p.dir.path().join("src/a.txt"), "2").unwrap();
	until("a re-run after failure", || {
		std::fs::read(&runs).is_ok_and(|b| b.len() >= 2)
	});
	assert!(
		child.try_wait().unwrap().is_none(),
		"a failing run must not end the session"
	);

	child.kill().unwrap();
	child.wait().unwrap();
}

#[test]
fn dry_run_previews_a_watch_target_instead_of_watching_it() {
	let p = project(&[("runfiles/tick.run", COUNTER), ("src/a.txt", "1")]);
	let o = p.run(&["--dry-run", "tick"]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(out(&o).contains("printf x >> runs.txt"), "{}", out(&o));
	assert!(!p.dir.path().join("runs.txt").exists(), "dry-run must not execute");
}

// -------------------------------------------------------------------- alias

#[test]
fn a_target_can_be_invoked_by_its_alias() {
	let p = project(&[("runfiles/build.run", ".alias = \"b\"\n$ printf done > out.txt\n")]);
	let o = p.run(&["b"]);
	assert!(o.status.success(), "{}", err(&o));
	assert_eq!(std::fs::read_to_string(p.dir.path().join("out.txt")).unwrap(), "done");
}

#[test]
fn an_alias_shadowed_by_a_real_file_name_loses() {
	// `x.run` declares the alias `build`, but a real `build.run` exists; the
	// file name must win, so aliases can never hijack a target.
	let p = project(&[
		("runfiles/build.run", "$ printf real > out.txt\n"),
		("runfiles/x.run", ".alias = \"build\"\n$ printf alias > out.txt\n"),
	]);
	let o = p.run(&["build"]);
	assert!(o.status.success(), "{}", err(&o));
	assert_eq!(std::fs::read_to_string(p.dir.path().join("out.txt")).unwrap(), "real");
}

// ------------------------------------------------------- workdir and add-path

#[test]
fn workdir_moves_the_target_without_moving_the_anchor() {
	let p = project(&[
		(
			"runfiles/where.run",
			".workdir = \"sub\"\n$ pwd > {{ RUN.parent }}/out.txt\n",
		),
		("sub/.keep", ""),
	]);
	let o = p.run(&["where"]);
	assert!(o.status.success(), "{}", err(&o));
	let got = std::fs::read_to_string(p.dir.path().join("out.txt")).unwrap();
	assert!(got.trim().ends_with("sub"), "ran in {got}");
}

#[test]
fn add_path_puts_a_relative_directory_on_path() {
	let p = project(&[
		("runfiles/tool.run", ".add-path = \"bin\"\n$ mytool > out.txt\n"),
		("bin/mytool", "#!/bin/sh\nprintf found\n"),
	]);
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		let exe = p.dir.path().join("bin/mytool");
		std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
	}
	let o = p.run(&["tool"]);
	assert!(o.status.success(), "{}", err(&o));
	assert_eq!(std::fs::read_to_string(p.dir.path().join("out.txt")).unwrap(), "found");
}

#[test]
fn add_path_anchors_to_the_runfiles_parent_not_the_workdir() {
	// The anchor rule: `bin/` is resolved against the project root even though
	// the target runs in `sub/`.
	let p = project(&[
		(
			"runfiles/tool.run",
			".add-path = \"bin\"\n.workdir = \"sub\"\n$ mytool > out.txt\n",
		),
		("bin/mytool", "#!/bin/sh\nprintf found\n"),
		("sub/.keep", ""),
	]);
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		let exe = p.dir.path().join("bin/mytool");
		std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
	}
	let o = p.run(&["tool"]);
	assert!(o.status.success(), "{}", err(&o));
	assert_eq!(
		std::fs::read_to_string(p.dir.path().join("sub/out.txt")).unwrap(),
		"found"
	);
}
