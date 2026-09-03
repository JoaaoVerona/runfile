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
			.env_remove("GITHUB_ACTIONS")
			// A developer bypassing the gate in their own shell must not
			// silently disable the tests that check the gate.
			.env_remove("RUNFILE_SKIP_PREPARE")
			.env_remove("RUNFILE_PRIVATE_KEYS");
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

// ------------------------------------------------------------ init and names

#[test]
fn init_creates_a_target_that_immediately_runs() {
	// The starter file is only worth shipping if `run :init && run hello`
	// works, so the test does exactly that.
	let p = project(&[]);
	let o = p.run(&[":init"]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(out(&o).contains("run hello"), "{}", out(&o));

	let o = p.run(&["-y", "hello"]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(out(&o).contains("hello, world"), "{}", out(&o));

	let o = p.run(&["-y", "hello", "--name=you"]);
	assert!(out(&o).contains("hello, you"), "{}", out(&o));
}

#[test]
fn init_refuses_when_a_target_is_already_there() {
	let p = project(&[(MARK, &marker("out.txt"))]);
	p.run(&[":init"]);
	let o = p.run(&[":init"]);
	assert!(!o.status.success());
	assert!(err(&o).contains("already exists"), "{}", err(&o));
}

#[test]
fn list_names_prints_one_bare_name_per_line() {
	let p = project(&[
		("runfiles/a.run", "# A\n$ true\n"),
		("runfiles/b.run", "# B\n$ true\n"),
		("runfiles/c.run", ".hide = true\n$ true\n"),
	]);
	let o = p.run(&[":list", "--names"]);
	let text = out(&o);
	let mut names: Vec<&str> = text.lines().collect();
	names.sort_unstable();
	assert_eq!(names, ["a", "b"], "hidden targets stay out of completion too");
}

// ------------------------------------------------------------- completions

#[test]
fn completions_are_produced_for_each_supported_shell() {
	let p = project(&[]);
	for sh in ["bash", "zsh", "fish", "powershell"] {
		let o = p.run(&[":completions", sh]);
		assert!(o.status.success(), "{sh}: {}", err(&o));
		assert!(out(&o).contains("run :list --names"), "{sh} must ask for names");
	}
}

#[test]
fn an_unknown_shell_is_rejected() {
	let p = project(&[]);
	let o = p.run(&[":completions", "nushell"]);
	assert!(!o.status.success());
	assert!(err(&o).contains("bash, zsh, fish"), "{}", err(&o));
}

#[test]
fn completions_with_no_shell_prints_usage() {
	let p = project(&[]);
	let o = p.run(&[":completions"]);
	assert!(!o.status.success());
	assert!(err(&o).contains("usage:"), "{}", err(&o));
}

/// Source the generated bash script and ask it to complete, the way the shell
/// would. Without this the scripts are only ever eyeballed.
#[cfg(unix)]
fn complete_bash(p: &Project, line: &str) -> Vec<String> {
	let script = out(&p.run(&[":completions", "bash"]));
	let path = p.dir.path().join("comp.bash");
	std::fs::write(&path, &script).unwrap();
	// COMP_WORDS/COMP_CWORD are what bash-completion sets before calling the
	// function. Splitting on a space already yields an empty final word for a
	// line ending in one, which is exactly the "fresh word" case.
	let words: Vec<String> = line.split(' ').map(|w| format!("'{w}'")).collect();
	let prog = format!(
		"source {}\nCOMP_WORDS=({})\nCOMP_CWORD={}\n_run\nprintf '%s\\n' \"${{COMPREPLY[@]}}\"",
		path.display(),
		words.join(" "),
		words.len() - 1,
	);
	let o = Command::new("bash")
		.arg("-c")
		.arg(&prog)
		.current_dir(p.dir.path())
		.env(
			"PATH",
			format!("{}:{}", bin_dir().display(), std::env::var("PATH").unwrap()),
		)
		.env("HOME", p.home.path())
		.output()
		.expect("bash");
	String::from_utf8_lossy(&o.stdout)
		.lines()
		.filter(|l| !l.is_empty())
		.map(String::from)
		.collect()
}

#[cfg(unix)]
fn bin_dir() -> &'static Path {
	Path::new(env!("CARGO_BIN_EXE_run")).parent().unwrap()
}

#[cfg(unix)]
#[test]
fn the_bash_script_completes_target_names() {
	let p = project(&[("runfiles/deploy.run", "$ true\n"), ("runfiles/dev.run", "$ true\n")]);
	let mut got = complete_bash(&p, "run de");
	got.sort();
	assert_eq!(got, ["deploy", "dev"], "names come from the binary");
}

#[cfg(unix)]
#[test]
fn the_bash_script_completes_subcommands_after_a_colon() {
	let p = project(&[(MARK, &marker("o"))]);
	let got = complete_bash(&p, "run :l");
	assert_eq!(got, [":list"]);
}

#[cfg(unix)]
#[test]
fn the_bash_script_completes_flags_after_a_dash() {
	let p = project(&[(MARK, &marker("o"))]);
	let got = complete_bash(&p, "run --dry");
	assert_eq!(got, ["--dry-run"]);
}

#[cfg(unix)]
#[test]
fn the_bash_script_completes_env_subcommands() {
	let p = project(&[(MARK, &marker("o"))]);
	let mut got = complete_bash(&p, "run :env in");
	got.sort();
	assert_eq!(got, ["init", "inject"]);
}

#[cfg(unix)]
#[test]
fn the_bash_script_offers_names_after_a_leading_flag() {
	// A flag before the target must not make the completer think a target was
	// already chosen.
	let p = project(&[("runfiles/deploy.run", "$ true\n")]);
	let got = complete_bash(&p, "run --dry-run dep");
	assert_eq!(got, ["deploy"]);
}

#[test]
fn passing_an_argument_with_a_space_explains_itself() {
	// `--name you` is a flag plus a positional, because nothing declares which
	// names take values. The error has to say so, or it reads as a bug.
	let p = project(&[("runfiles/greet.run", "$ echo {{ ARG.name }}\n")]);
	let o = p.run(&["greet", "--name", "you"]);
	assert!(!o.status.success());
	assert!(err(&o).contains("--name=<value>"), "{}", err(&o));
}

#[test]
fn a_genuinely_absent_argument_says_only_that() {
	let p = project(&[("runfiles/greet.run", "$ echo {{ ARG.name }}\n")]);
	let o = p.run(&["greet"]);
	assert!(!o.status.success());
	assert!(err(&o).contains("no argument `--name`"), "{}", err(&o));
	assert!(!err(&o).contains("passed as a flag"), "no flag was passed: {}", err(&o));
}

#[test]
fn a_flag_used_as_a_flag_is_unaffected() {
	// A flag is a bool, branched on with `if` -- `?` is the default operator,
	// not a ternary.
	let p = project(&[(
		"runfiles/f.run",
		"if FLAG.force\n\t$ echo on\nelse\n\t$ echo off\nend\n",
	)]);
	assert!(out(&p.run(&["f", "--force"])).contains("on"));
	assert!(out(&p.run(&["f"])).contains("off"));
}

#[test]
fn list_json_describes_every_visible_target() {
	let p = project(&[
		("runfiles/build.run", "# Builds it\n$ true\n"),
		("runfiles/secret.run", ".hide = true\n$ true\n"),
	]);
	let o = p.run(&[":list", "--json"]);
	assert!(o.status.success(), "{}", err(&o));
	let text = out(&o);
	assert!(text.contains("\"formatVersion\": 1"), "{text}");
	assert!(text.contains("\"name\": \"build\""), "{text}");
	assert!(text.contains("\"description\": \"Builds it\""), "{text}");
	assert!(text.contains("\"origin\": \"local\""), "{text}");
	assert!(text.contains("build.run"), "the path lets tooling pin -f: {text}");
	assert!(!text.contains("secret"), "hidden targets stay out: {text}");
}

#[test]
fn list_json_escapes_text_that_would_break_the_document() {
	// Descriptions are free text from a comment block; a quote or backslash in
	// one must not produce unparseable JSON.
	let p = project(&[("runfiles/odd.run", "# He said \"hi\" \\ bye\n$ true\n")]);
	let o = p.run(&[":list", "--json"]);
	let text = out(&o);
	assert!(text.contains(r#"\"hi\""#), "{text}");
	assert!(text.contains(r"\\"), "{text}");
	// The real check: it round-trips through a parser.
	let v: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
	assert_eq!(v["targets"][0]["description"], "He said \"hi\" \\ bye");
}

#[test]
fn list_json_with_no_visible_targets_is_still_a_valid_document() {
	// An empty array, not a truncated document: tooling parses this on every
	// refresh and must not have to special-case "nothing to show".
	let p = project(&[("runfiles/h.run", ".hide = true\n$ true\n")]);
	let v: serde_json::Value = serde_json::from_str(&out(&p.run(&[":list", "--json"]))).expect("valid JSON");
	assert_eq!(v["targets"].as_array().unwrap().len(), 0);
}

#[test]
fn dry_run_prints_a_dependency_where_it_is_called() {
	// The child finishes while the parent is still walking, so a shared trace
	// buffer put every dependency first -- ahead of the line that invoked it.
	let p = project(&[
		("runfiles/main.run", "$ echo one\nrun dep\n$ echo three\n"),
		("runfiles/dep.run", "$ echo two\n"),
	]);
	let o = p.run(&["--dry-run", "main"]);
	assert!(o.status.success(), "{}", err(&o));
	let text = out(&o);
	let lines: Vec<&str> = text.lines().collect();
	assert_eq!(lines, ["echo one", "echo two", "echo three"]);
}

#[test]
fn dry_run_order_matches_execution_order() {
	// The preview is only worth having if it says what will actually happen.
	let files = &[
		("runfiles/main.run", "$ echo one\nrun dep\n$ echo three\n"),
		("runfiles/dep.run", "$ echo two\n"),
	];
	let p = project(files);
	let previewed: Vec<String> = out(&p.run(&["--dry-run", "main"]))
		.lines()
		.map(|l| l.to_string())
		.collect();
	let actual: Vec<String> = out(&p.run(&["main"])).lines().map(|l| format!("echo {l}")).collect();
	assert_eq!(previewed, actual);
}

#[test]
fn dry_run_expands_nested_dependencies_in_order() {
	let p = project(&[
		("runfiles/a.run", "$ echo a1\nrun b\n$ echo a2\n"),
		("runfiles/b.run", "$ echo b1\nrun c\n$ echo b2\n"),
		("runfiles/c.run", "$ echo c1\n"),
	]);
	let lines: Vec<String> = out(&p.run(&["--dry-run", "a"]))
		.lines()
		.map(|l| l.to_string())
		.collect();
	assert_eq!(lines, ["echo a1", "echo b1", "echo c1", "echo b2", "echo a2"]);
}

#[test]
fn dry_run_does_not_write_files() {
	// A preview that edits the working tree is worse than no preview: this
	// exact call bumped a real Cargo.toml before it was caught.
	let p = project(&[("runfiles/w.run", "write_file(\"out.txt\", \"changed\")\n$ true\n")]);
	let o = p.run(&["--dry-run", "w"]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(!p.dir.path().join("out.txt").exists(), "dry-run must not write");
}

#[test]
fn write_file_still_writes_when_actually_running() {
	let p = project(&[("runfiles/w.run", "write_file(\"out.txt\", \"changed\")\n$ true\n")]);
	assert!(p.run(&["w"]).status.success());
	assert_eq!(
		std::fs::read_to_string(p.dir.path().join("out.txt")).unwrap(),
		"changed"
	);
}

#[test]
fn dry_run_says_what_it_would_have_written() {
	let p = project(&[(
		"runfiles/w.run",
		"let r = write_file(\"out.txt\", \"x\")\n$ echo {{ r }}\n",
	)]);
	assert!(out(&p.run(&["--dry-run", "w"])).contains("would write out.txt"));
}

#[test]
fn dry_run_is_not_blocked_by_the_prepare_gate() {
	// A preview changes nothing, and reading what a target would do is a
	// reasonable thing to want before deciding to set the project up.
	let p = project(&[
		("runfiles/setup.run", "$ true\n"),
		("runfiles/build.run", "$ echo built\n"),
	]);
	assert!(!p.run(&["build"]).status.success(), "running is still gated");
	let o = p.run(&["--dry-run", "build"]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(out(&o).contains("echo built"), "{}", out(&o));
}

#[test]
fn a_subproject_target_calls_its_own_siblings() {
	// `run compile` inside web/runfiles/ means that directory's `compile`,
	// whatever the root calls it -- otherwise a subproject would have to spell
	// its siblings differently depending on where `run` was invoked.
	let p = project(&[
		("runfiles/root.run", "$ echo root\n"),
		("web/runfiles/compile.run", "$ echo web-compile\n"),
		("web/runfiles/build.run", "run compile\n$ echo web-build\n"),
	]);
	let o = p.run(&["web:build"]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(out(&o).contains("web-compile"), "{}", out(&o));

	// And the same file works when invoked from inside the subproject.
	let o = p.run_in(&p.dir.path().join("web"), &["build"]);
	assert!(out(&o).contains("web-compile"), "{}", err(&o));
}

#[test]
fn a_subproject_can_still_reach_a_root_target() {
	// Only siblings take precedence; a name with no sibling falls through.
	let p = project(&[
		("runfiles/shared.run", "$ echo from-root\n"),
		("web/runfiles/build.run", "run shared\n"),
	]);
	let o = p.run(&["web:build"]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(out(&o).contains("from-root"), "{}", out(&o));
}

#[test]
fn a_sibling_wins_over_a_root_target_with_the_same_name() {
	let p = project(&[
		("runfiles/build.run", "$ echo root-build\n"),
		("web/runfiles/build.run", "$ echo web-build\n"),
		("web/runfiles/all.run", "run build\n"),
	]);
	let o = p.run(&["web:all"]);
	assert!(out(&o).contains("web-build"), "{}", out(&o));
	assert!(!out(&o).contains("root-build"), "{}", out(&o));
}

#[test]
fn the_version_is_printed_by_every_spelling() {
	// `--version` and `-V` are what people type; `:version` is the form that
	// matches every other built-in.
	let p = project(&[(MARK, &marker("o"))]);
	for form in [":version", "--version", "-V"] {
		let o = p.run(&[form]);
		assert!(o.status.success(), "{form}: {}", err(&o));
		assert!(out(&o).starts_with("run "), "{form}: {}", out(&o));
		assert!(out(&o).contains(env!("CARGO_PKG_VERSION")), "{form}: {}", out(&o));
	}
}

// ------------------------------------------------------------ passthrough

#[test]
fn a_double_dash_forwards_the_rest_of_the_line_untouched() {
	let p = project(&[("runfiles/wrap.run", "$ echo {{ ARGS }}\n")]);
	let o = p.run(&["wrap", "--", "s3api", "--bucket", "x", "--dry-run"]);
	assert!(o.status.success(), "{}", err(&o));
	assert_eq!(out(&o).trim(), "s3api --bucket x --dry-run");
	assert!(err(&o).is_empty(), "nothing to warn about: {}", err(&o));
}

#[test]
fn a_forgotten_double_dash_is_warned_about_on_stderr() {
	// The command still runs -- the runner cannot know the flag was meant for
	// the wrapped tool -- but it no longer fails silently.
	let p = project(&[("runfiles/wrap.run", "$ echo {{ ARGS }}\n")]);
	let o = p.run(&["wrap", "s3api", "--bucket", "x"]);
	assert!(o.status.success(), "{}", err(&o));
	assert_eq!(out(&o).trim(), "s3api x");
	assert!(
		err(&o).starts_with("warning: `--bucket` was passed to `wrap`"),
		"{}",
		err(&o)
	);
	assert!(err(&o).contains("put `--` before it"), "{}", err(&o));
}

#[test]
fn a_flag_that_is_read_produces_no_warning() {
	let p = project(&[(
		"runfiles/f.run",
		"if FLAG.force\n\t$ echo on\nelse\n\t$ echo off\nend\n",
	)]);
	let o = p.run(&["f", "--force"]);
	assert!(err(&o).is_empty(), "{}", err(&o));
}
