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
			// Windows reads the home and config directories through the Known
			// Folder API, which ignores HOME and APPDATA; these two are the
			// overrides the CLI honours on every platform.
			.env("USERPROFILE", self.home.path())
			.env("RUNFILE_CONFIG_DIR", self.home.path())
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

/// Everything on stderr that the runner did not say itself, so a test can
/// assert on warnings and a command's own output without the `[runfile]`
/// announcements in the way.
fn err_from_commands(o: &Output) -> String {
	err(o)
		.lines()
		.filter(|l| !l.starts_with("[runfile]"))
		.map(|l| format!("{l}\n"))
		.collect()
}

/// The commands a `--dry-run` printed, without its `#` header lines.
fn dry_commands(o: &Output) -> Vec<String> {
	out(o)
		.lines()
		.filter(|l| !l.starts_with('#'))
		.map(str::to_string)
		.collect()
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
		let o = p.run(&[":completions", "output", sh]);
		assert!(o.status.success(), "{sh}: {}", err(&o));
		assert!(
			out(&o).contains("run :complete"),
			"{sh} must ask the binary what follows what"
		);
	}
}

#[test]
fn an_unknown_shell_is_rejected() {
	let p = project(&[]);
	let o = p.run(&[":completions", "output", "nushell"]);
	assert!(!o.status.success());
	assert!(err(&o).contains("bash, zsh, fish"), "{}", err(&o));
}

#[test]
fn completions_with_no_command_prints_its_help() {
	let p = project(&[]);
	let o = p.run(&[":completions"]);
	assert!(o.status.success(), "{}", err(&o));
	for word in ["install", "uninstall", "output"] {
		assert!(out(&o).contains(word), "{}", out(&o));
	}
}

/// Source the generated bash script and ask it to complete, the way the shell
/// would. Without this the scripts are only ever eyeballed.
#[cfg(unix)]
fn complete_bash(p: &Project, line: &str) -> Vec<String> {
	let script = out(&p.run(&[":completions", "output", "bash"]));
	let path = p.dir.path().join("comp.bash");
	std::fs::write(&path, &script).unwrap();

	// Split the line the way readline does, using the script's own
	// COMP_WORDBREAKS -- not on spaces alone. `:` is a word break by default,
	// so a script that does not deal with it sees `run : env`, not `run :env`,
	// and every nested completion silently stops working. Splitting by hand
	// hides exactly that.
	let breaks = bash(
		p,
		&format!("source {}\nprintf '%s' \"$COMP_WORDBREAKS\"", path.display()),
	);
	let breaks: Vec<char> = breaks.chars().filter(|c| !c.is_whitespace()).collect();
	let mut words: Vec<String> = Vec::new();
	for tok in line.split(' ').filter(|t| !t.is_empty()) {
		let mut buf = String::new();
		for c in tok.chars() {
			if breaks.contains(&c) {
				if !buf.is_empty() {
					words.push(std::mem::take(&mut buf));
				}
				words.push(c.to_string());
			} else {
				buf.push(c);
			}
		}
		if !buf.is_empty() {
			words.push(buf);
		}
	}
	if line.ends_with(' ') || words.is_empty() {
		words.push(String::new());
	}
	let cword = words.len() - 1;
	let quoted: Vec<String> = words.iter().map(|w| format!("'{w}'")).collect();

	let prog = format!(
		"source {}\nCOMP_WORDS=({})\nCOMP_CWORD={}\n_run\nprintf '%s\\n' \"${{COMPREPLY[@]}}\"",
		path.display(),
		quoted.join(" "),
		cword,
	);
	bash(p, &prog)
		.lines()
		.filter(|l| !l.is_empty())
		.map(String::from)
		.collect()
}

#[cfg(unix)]
fn bash(p: &Project, prog: &str) -> String {
	let o = Command::new("bash")
		.arg("-c")
		.arg(prog)
		.current_dir(p.dir.path())
		.env(
			"PATH",
			format!("{}:{}", bin_dir().display(), std::env::var("PATH").unwrap()),
		)
		.env("HOME", p.home.path())
		.output()
		.expect("bash");
	String::from_utf8_lossy(&o.stdout).into_owned()
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

	// A bare Tab is the discovery case: every command and every target.
	let got = complete_bash(&p, "run ");
	for w in [":list", ":env", ":completions", "deploy", "dev"] {
		assert!(got.iter().any(|g| g == w), "`{w}` missing from {got:?}");
	}
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
fn the_bash_script_takes_the_colon_out_of_the_word_breaks() {
	// Readline splits on `:` by default, which would hand this function
	// `run : env` and make it insert a candidate's colon after the typed one.
	// Asserting the mechanism, because the symptom only shows under readline.
	let p = project(&[(MARK, &marker("o"))]);
	let script = out(&p.run(&[":completions", "output", "bash"]));
	let path = p.dir.path().join("c.bash");
	std::fs::write(&path, &script).unwrap();
	let got = bash(
		&p,
		&format!(
			"source {}\ncase \"$COMP_WORDBREAKS\" in *:*) echo split;; *) echo whole;; esac",
			path.display()
		),
	);
	assert_eq!(
		got.trim(),
		"whole",
		"a `:` in COMP_WORDBREAKS breaks every namespaced name"
	);
}

#[cfg(unix)]
#[test]
fn the_bash_script_completes_a_namespaced_target() {
	// `vscode:test` is one word to a person and two to readline. It must
	// survive both the split and the insertion.
	let p = project(&[("runfiles/vscode/test.run", "$ true\n")]);
	assert_eq!(complete_bash(&p, "run vscode:"), ["vscode:test"]);
	assert_eq!(complete_bash(&p, "run vscode:te"), ["vscode:test"]);
}

#[cfg(unix)]
#[test]
fn the_bash_script_completes_a_subcommand_of_a_subcommand() {
	// Three words deep. This used to fall through to file names, because each
	// script carried its own walk and none of them recursed.
	let p = project(&[(MARK, &marker("o"))]);
	let mut got = complete_bash(&p, "run :env secret-keys ");
	got.sort();
	assert_eq!(got, ["add", "get-private", "list", "remove"]);

	let got = complete_bash(&p, "run :env secret-keys get");
	assert_eq!(got, ["get-private"]);
}

#[cfg(unix)]
#[test]
fn the_bash_script_completes_the_shell_a_completion_command_takes() {
	let p = project(&[(MARK, &marker("o"))]);
	let got = complete_bash(&p, "run :completions install f");
	assert_eq!(got, ["fish"]);
	let got = complete_bash(&p, "run :completions output z");
	assert_eq!(got, ["zsh"]);
}

#[cfg(unix)]
#[test]
fn the_bash_script_completes_a_nested_commands_own_flags() {
	let p = project(&[(MARK, &marker("o"))]);
	let got = complete_bash(&p, "run :generate zed --incl");
	assert_eq!(got, ["--include-global"]);
	let got = complete_bash(&p, "run :env rotate --delete");
	assert_eq!(got, ["--delete-current-key"]);
}

#[cfg(unix)]
#[test]
fn the_bash_script_hands_paths_back_to_the_shell() {
	// `run :env get <Tab>` names a file, and only the shell completes those
	// properly. The marker asks it to, and must not be offered as a word.
	let p = project(&[
		(MARK, &marker("o")),
		(
			".env.local",
			"A=1
",
		),
	]);
	let got = complete_bash(&p, "run :env get .env");
	assert_eq!(got, [".env.local"], "the marker itself is never a candidate");

	let got = complete_bash(&p, "run --dir runfi");
	assert_eq!(got, ["runfiles"], "--dir takes a directory, not a file");
}

#[cfg(unix)]
#[test]
fn the_bash_script_leaves_a_targets_arguments_alone() {
	let p = project(&[(
		"runfiles/deploy.run",
		"$ true
",
	)]);
	assert!(
		complete_bash(&p, "run deploy --any").is_empty(),
		"a target's own flags are its business, and we cannot know them"
	);
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
	assert_eq!(dry_commands(&o), ["echo one", "echo two", "echo three"]);
}

#[test]
fn dry_run_order_matches_execution_order() {
	// The preview is only worth having if it says what will actually happen.
	let files = &[
		("runfiles/main.run", "$ echo one\nrun dep\n$ echo three\n"),
		("runfiles/dep.run", "$ echo two\n"),
	];
	let p = project(files);
	let previewed = dry_commands(&p.run(&["--dry-run", "main"]));
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
	assert_eq!(
		dry_commands(&p.run(&["--dry-run", "a"])),
		["echo a1", "echo b1", "echo c1", "echo b2", "echo a2"]
	);
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
	// Flags only: a `:version` command would be the odd one out, since nothing
	// else about the binary itself is a command.
	let p = project(&[(MARK, &marker("o"))]);
	for form in ["--version", "-v", "-V"] {
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
	assert!(err_from_commands(&o).is_empty(), "nothing to warn about: {}", err(&o));
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
	assert!(err_from_commands(&o).is_empty(), "{}", err(&o));
}

#[test]
fn a_crlf_file_runs_the_same_as_an_lf_one() {
	// Windows editors write CRLF. An indented exec block in such a file never
	// closed, and the carriage return reached the shell as part of each line.
	let body = "if true\r\n\texec sh\r\n\t\techo hi\r\n\tend\r\nend\r\n$ echo after\r\n";
	let p = project(&[("runfiles/t.run", body)]);
	let o = p.run(&["t"]);
	assert!(o.status.success(), "{}", err(&o));
	assert_eq!(out(&o), "hi\nafter\n");
}

// --------------------------------------------------------------- generate

const GEN: &[(&str, &str)] = &[
	("runfiles/build.run", "# Builds it\n$ true\n"),
	("runfiles/deploy.run", "# Ships it\n$ echo {{ ARG.env }}\n"),
	("runfiles/secret.run", ".hide = true\n$ true\n"),
	("web/runfiles/dev.run", "$ true\n"),
];

fn json_file(p: &Project, rel: &str) -> serde_json::Value {
	let text = std::fs::read_to_string(p.dir.path().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
	serde_json::from_str(&text).unwrap_or_else(|e| panic!("{rel} is not JSON: {e}\n{text}"))
}

#[test]
fn generate_zed_writes_one_task_per_visible_target() {
	let p = project(GEN);
	let o = p.run(&[":generate", "zed"]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(out(&o).contains(".zed/tasks.json: 3 added"), "{}", out(&o));
	let tasks = json_file(&p, ".zed/tasks.json");
	let labels: Vec<&str> = tasks
		.as_array()
		.unwrap()
		.iter()
		.map(|t| t["label"].as_str().unwrap())
		.collect();
	assert_eq!(
		labels,
		["run build", "run deploy", "run web:dev"],
		"hidden left out, subproject in"
	);
	assert_eq!(tasks[0]["args"], serde_json::json!(["--stdin-args", "build"]));
	assert_eq!(tasks[0]["cwd"], "$ZED_WORKTREE_ROOT");
	assert_eq!(
		tasks[1]["args"][2], "$ZED_CUSTOM_ARGS",
		"a target that reads arguments offers a prompt"
	);
	assert_eq!(tasks[1]["allow_concurrent_runs"], true);
}

#[test]
fn generate_merges_into_an_existing_file_and_keeps_its_indentation() {
	let mut files = GEN.to_vec();
	// A person's own task, a stale one of ours, and tabs.
	files.push((
		".zed/tasks.json",
		"[\n\t{\n\t\t\"label\": \"lint everything\",\n\t\t\"command\": \"make\"\n\t},\n\t{\n\t\t\"label\": \"run gone\",\n\t\t\"command\": \"run\",\n\t\t\"args\": [\"--stdin-args\", \"gone\"]\n\t}\n]\n",
	));
	let p = project(&files);
	let o = p.run(&[":generate", "zed"]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(out(&o).contains("3 added, 0 updated, 1 removed"), "{}", out(&o));
	let text = std::fs::read_to_string(p.dir.path().join(".zed/tasks.json")).unwrap();
	assert!(text.contains("\n\t{\n\t\t\"label\""), "tabs kept:\n{text}");
	let tasks: serde_json::Value = serde_json::from_str(&text).unwrap();
	let labels: Vec<&str> = tasks
		.as_array()
		.unwrap()
		.iter()
		.map(|t| t["label"].as_str().unwrap())
		.collect();
	assert_eq!(labels, ["lint everything", "run build", "run deploy", "run web:dev"]);
}

#[test]
fn generate_refuses_to_touch_a_file_it_cannot_parse() {
	let mut files = GEN.to_vec();
	files.push((".zed/tasks.json", "[ not json"));
	let p = project(&files);
	let o = p.run(&[":generate", "zed"]);
	assert!(!o.status.success());
	assert!(err(&o).contains("not valid JSON"), "{}", err(&o));
	assert_eq!(
		std::fs::read_to_string(p.dir.path().join(".zed/tasks.json")).unwrap(),
		"[ not json"
	);
}

#[test]
fn global_runfiles_may_use_any_of_the_three_names() {
	for name in [".runfiles", "runfiles", "Runfiles"] {
		let p = project(&[("runfiles/local.run", "$ true\n")]);
		let g = p.home.path().join(name);
		std::fs::create_dir_all(&g).unwrap();
		std::fs::write(g.join("mine.run"), "$ echo global\n").unwrap();
		let o = p.run(&["mine"]);
		assert!(o.status.success(), "{name}: {}", err(&o));
		assert!(out(&o).contains("global"), "{name} was not read");
	}
}

#[test]
fn two_populated_global_directories_stop_the_run_and_name_both() {
	let p = project(&[("runfiles/local.run", "$ true\n")]);
	for name in [".runfiles", "Runfiles"] {
		let g = p.home.path().join(name);
		std::fs::create_dir_all(&g).unwrap();
		std::fs::write(g.join("mine.run"), "$ true\n").unwrap();
	}
	let o = p.run(&["local"]);
	assert!(!o.status.success(), "a silent winner is the thing being avoided");
	let e = err(&o);
	assert!(e.contains(".runfiles"), "{e}");
	assert!(e.contains("Runfiles"), "{e}");
	assert!(e.contains("keep one of them"), "the error must say what to do: {e}");
}

fn code_of(o: &std::process::Output) -> i32 {
	o.status.code().expect("an exit status")
}

#[cfg(unix)]
#[test]
fn a_command_is_announced_when_it_runs_not_before() {
	// A block of `$` lines is one process, so the runner used to print every
	// command before any of them ran -- and a failure then landed at the end,
	// under nothing. The announcement goes inside the script instead.
	let p = project(&[("runfiles/e.run", "$ echo first\n$ echo second\n$ false\n$ echo never\n")]);
	let o = p.run(&["e"]);
	assert!(!o.status.success());

	// stdout and stderr are separate streams, so ordering is asserted within
	// stderr: the second announcement must come after the first, and the third
	// must be the last thing said.
	let e = err(&o);
	let at = |needle: &str| e.find(needle).unwrap_or_else(|| panic!("{needle:?} not in {e}"));
	assert!(at("echo first") < at("echo second"), "{e}");
	assert!(at("echo second") < at("false"), "{e}");
	assert!(
		!e.contains("echo never"),
		"a command that never ran is never announced: {e}"
	);
}

#[cfg(unix)]
#[test]
fn a_block_the_runner_cannot_take_apart_is_announced_whole() {
	// A `for` spread over `$` lines is one command to the shell. It still runs,
	// and every line of it is still shown.
	let p = project(&[("runfiles/e.run", "$ for f in a b; do\n$ echo $f\n$ done\n")]);
	let o = p.run(&["e"]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(out(&o).contains('a') && out(&o).contains('b'), "{}", out(&o));
	assert!(err(&o).contains("for f in a b"), "{}", err(&o));
}

#[test]
fn exit_sets_the_process_status_and_is_not_an_error() {
	for (body, want) in [
		("exit()\n", 0),
		("exit(0)\n", 0),
		("exit(3)\n", 3),
		// The shell truncates to a byte; -1 is the classic 255.
		("exit(-1)\n", 255),
	] {
		let p = project(&[("runfiles/e.run", body)]);
		let o = p.run(&["e"]);
		assert_eq!(code_of(&o), want, "{body:?}");
		assert!(
			err(&o).is_empty() || !err(&o).contains("error:"),
			"not a failure: {}",
			err(&o)
		);
	}
}

#[test]
fn a_function_without_parentheses_is_reported_as_one() {
	// Every call is written with parentheses. A bare `exit` used to be the one
	// exception; reporting it as an unknown binding would send a person
	// looking for a `let` that was never missing.
	let p = project(&[("runfiles/e.run", "exit\n")]);
	let o = p.run(&["e"]);
	assert!(!o.status.success());
	assert!(err(&o).contains("call it as `exit()`"), "{}", err(&o));
}

#[test]
fn exit_stops_the_statements_after_it() {
	let p = project(&[("runfiles/e.run", "$ echo before\nexit(2)\n$ echo after\n")]);
	let o = p.run(&["e"]);
	assert_eq!(code_of(&o), 2);
	assert!(out(&o).contains("before"), "{}", out(&o));
	assert!(!out(&o).contains("after"), "everything past it is skipped: {}", out(&o));
}

#[test]
fn ignore_errors_does_not_shrug_off_an_exit() {
	// A target may forgive a command that failed. Being told to stop is not
	// that -- same reason an interrupt is not ignorable.
	let p = project(&[(
		"runfiles/e.run",
		".ignore-errors = true\n$ false\nexit(5)\n$ echo after\n",
	)]);
	let o = p.run(&["e"]);
	assert_eq!(code_of(&o), 5);
	assert!(!out(&o).contains("after"), "{}", out(&o));
}

#[test]
fn an_exit_inside_a_called_target_ends_the_whole_run() {
	let p = project(&[
		("runfiles/parent.run", "run child\n$ echo after\n"),
		("runfiles/child.run", "exit(7)\n"),
	]);
	let o = p.run(&["parent"]);
	assert_eq!(code_of(&o), 7, "{}", err(&o));
	assert!(!out(&o).contains("after"), "{}", out(&o));
}

#[test]
fn exit_inside_a_branch_ends_the_run_there() {
	let p = project(&[("runfiles/e.run", "if FLAG.stop\n\texit(4)\nend\n$ echo went-on\n")]);
	assert_eq!(code_of(&p.run(&["e", "--stop"])), 4);
	let o = p.run(&["e"]);
	assert_eq!(code_of(&o), 0);
	assert!(out(&o).contains("went-on"), "{}", out(&o));
}

#[test]
fn format_rewrites_every_runfile_in_the_project() {
	let p = project(&[
		("runfiles/a.run", "if x==1\n$ echo a\nend\n"),
		("runfiles/sub/b.run", "let y=[1,2]\n"),
	]);
	let o = p.run(&[":format"]);
	assert!(o.status.success(), "{}", err(&o));
	assert_eq!(
		std::fs::read_to_string(p.dir.path().join("runfiles/a.run")).unwrap(),
		"if x == 1\n\t$ echo a\nend\n"
	);
	assert_eq!(
		std::fs::read_to_string(p.dir.path().join("runfiles/sub/b.run")).unwrap(),
		"let y = [1, 2]\n"
	);
}

#[test]
fn format_reaches_shared_run_which_is_not_a_target() {
	// Nothing that walks the catalog by name would find it, and it is the file
	// most likely to sit unread and drift.
	let p = project(&[
		("runfiles/a.run", "$ true\n"),
		("runfiles/_shared.run", ".shell   =  \"bash\"\n"),
	]);
	assert!(p.run(&[":format"]).status.success());
	assert_eq!(
		std::fs::read_to_string(p.dir.path().join("runfiles/_shared.run")).unwrap(),
		".shell = \"bash\"\n"
	);
}

#[test]
fn format_check_reports_and_fails_without_writing() {
	let p = project(&[("runfiles/a.run", "let y=1\n")]);
	let o = p.run(&[":format", "--check"]);
	assert!(!o.status.success(), "--check must fail when work is needed");
	assert!(out(&o).contains("a.run"), "it has to say which file: {}", out(&o));
	assert_eq!(
		std::fs::read_to_string(p.dir.path().join("runfiles/a.run")).unwrap(),
		"let y=1\n",
		"--check must not write"
	);
}

#[test]
fn format_stdout_prints_without_writing() {
	let p = project(&[("runfiles/a.run", "let y=1\n")]);
	let o = p.run(&[":format", "--stdout"]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(out(&o).contains("let y = 1"), "{}", out(&o));
	assert_eq!(
		std::fs::read_to_string(p.dir.path().join("runfiles/a.run")).unwrap(),
		"let y=1\n"
	);
}

#[test]
fn format_takes_explicit_paths_including_directories() {
	let p = project(&[("runfiles/a.run", "$ true\n")]);
	std::fs::create_dir_all(p.dir.path().join("elsewhere")).unwrap();
	std::fs::write(p.dir.path().join("elsewhere/x.run"), "let y=1\n").unwrap();
	let o = p.run(&[":format", "elsewhere"]);
	assert!(o.status.success(), "{}", err(&o));
	assert_eq!(
		std::fs::read_to_string(p.dir.path().join("elsewhere/x.run")).unwrap(),
		"let y = 1\n",
		"a directory argument is walked"
	);
}

#[test]
fn format_leaves_the_global_directory_alone_unless_asked() {
	let p = project(&[("runfiles/a.run", "$ true\n")]);
	let g = p.home.path().join(".runfiles");
	std::fs::create_dir_all(&g).unwrap();
	std::fs::write(g.join("mine.run"), "let y=1\n").unwrap();

	assert!(p.run(&[":format"]).status.success());
	assert_eq!(
		std::fs::read_to_string(g.join("mine.run")).unwrap(),
		"let y=1\n",
		"a task file is committed; the machine-wide directory is one person's"
	);

	assert!(p.run(&[":format", "--include-global"]).status.success());
	assert_eq!(std::fs::read_to_string(g.join("mine.run")).unwrap(), "let y = 1\n");
}

#[test]
fn format_refuses_a_file_that_does_not_parse_and_leaves_it_whole() {
	let p = project(&[("runfiles/a.run", "$ true\n")]);
	std::fs::write(p.dir.path().join("runfiles/broken.run"), "if x\n$ echo a\n").unwrap();
	let o = p.run(&[":format"]);
	assert!(!o.status.success(), "a file it could not read must not pass silently");
	assert_eq!(
		std::fs::read_to_string(p.dir.path().join("runfiles/broken.run")).unwrap(),
		"if x\n$ echo a\n",
		"reindenting a file whose blocks do not close is guesswork"
	);
}

#[test]
fn generate_leaves_global_targets_out_unless_asked() {
	let p = project(GEN);
	std::fs::create_dir_all(p.home.path().join(".runfiles")).unwrap();
	std::fs::write(p.home.path().join(".runfiles/mine.run"), "$ true\n").unwrap();
	p.run(&[":generate", "zed"]);
	assert!(
		!json_file(&p, ".zed/tasks.json").to_string().contains("run mine"),
		"a task file is committed; ~/.runfiles is one person's"
	);
	p.run(&[":generate", "zed", "--include-global"]);
	assert!(json_file(&p, ".zed/tasks.json").to_string().contains("run mine"));
}

#[test]
fn generate_stdout_prints_instead_of_writing() {
	let p = project(GEN);
	let o = p.run(&[":generate", "zed", "--stdout"]);
	assert!(o.status.success(), "{}", err(&o));
	let v: serde_json::Value = serde_json::from_str(&out(&o)).expect("JSON on stdout");
	assert_eq!(v.as_array().unwrap().len(), 3);
	assert!(!p.dir.path().join(".zed").exists());
}

#[test]
fn generate_vscode_declares_the_argument_prompt_it_uses() {
	let p = project(GEN);
	let o = p.run(&[":generate", "vscode"]);
	assert!(o.status.success(), "{}", err(&o));
	let file = json_file(&p, ".vscode/tasks.json");
	assert_eq!(file["version"], "2.0.0");
	let deploy = &file["tasks"][1];
	assert_eq!(deploy["type"], "shell");
	assert_eq!(deploy["detail"], "Ships it");
	assert_eq!(deploy["args"][2], "${input:args}");
	assert_eq!(file["inputs"][0]["id"], "args", "the prompt it references is declared");
	// Running again neither duplicates the input nor counts anything as changed.
	let o = p.run(&[":generate", "vscode"]);
	assert!(out(&o).contains("0 added, 3 updated, 0 removed"), "{}", out(&o));
	assert_eq!(
		json_file(&p, ".vscode/tasks.json")["inputs"].as_array().unwrap().len(),
		1
	);
}

#[test]
fn generate_jetbrains_writes_one_configuration_per_target_and_respects_foreign_files() {
	let mut files = GEN.to_vec();
	files.push((
		".idea/runConfigurations/Runfile_build.run.xml",
		"<component>someone else's</component>\n",
	));
	files.push((".idea/runConfigurations/Runfile_gone.run.xml", "<component name=\"ProjectRunConfigurationManager\">\n  <configuration default=\"false\" name=\"Gone\" type=\"ShConfigurationType\">\n    <option name=\"SCRIPT_TEXT\" value=\"run --stdin-args gone\" />\n    <option name=\"SCRIPT_WORKING_DIRECTORY\" value=\"$PROJECT_DIR$\" />\n  </configuration>\n</component>\n"));
	let p = project(&files);
	let o = p.run(&[":generate", "jetbrains"]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(out(&o).contains("2 added, 0 updated, 1 removed"), "{}", out(&o));
	assert!(
		out(&o).contains("skipped .idea/runConfigurations/Runfile_build.run.xml"),
		"{}",
		out(&o)
	);
	let dir = p.dir.path().join(".idea/runConfigurations");
	assert_eq!(
		std::fs::read_to_string(dir.join("Runfile_build.run.xml")).unwrap(),
		"<component>someone else's</component>\n",
		"not ours, not touched"
	);
	assert!(
		!dir.join("Runfile_gone.run.xml").exists(),
		"stale configuration of ours removed"
	);
	let deploy = std::fs::read_to_string(dir.join("Runfile_web_dev.run.xml")).unwrap();
	assert!(deploy.contains(r#"name="Web Dev""#), "{deploy}");
	assert!(deploy.contains(r#"value="run --stdin-args web:dev""#), "{deploy}");
}

#[test]
fn generate_needs_an_editor_it_knows() {
	let p = project(GEN);
	// No editor at all is a request for help, not an error.
	let o = p.run(&[":generate"]);
	assert!(
		o.status.success() && out(&o).contains("run :generate zed"),
		"{}",
		out(&o)
	);
	let o = p.run(&[":generate", "emacs"]);
	assert!(!o.status.success());
	assert!(err(&o).contains("unknown editor `emacs`"), "{}", err(&o));
	assert!(err(&p.run(&[":generate", "zed", "--bogus"])).contains("unknown option"));
}

#[cfg(unix)]
#[test]
fn the_bash_script_completes_generate_editors() {
	let p = project(&[(MARK, &marker("o"))]);
	let mut got = complete_bash(&p, "run :generate ");
	got.sort();
	assert_eq!(got, ["jetbrains", "vscode", "zed"]);
}

#[test]
fn a_nested_shared_file_layers_over_the_one_above_it() {
	// Both apply, and the nested one wins where they disagree.
	let p = project(&[
		(
			"runfiles/_shared.run",
			".env.SHARED = \"root\"\n.env.ONLY_ROOT = \"yes\"\n",
		),
		("runfiles/api/_shared.run", ".env.SHARED = \"api\"\n"),
		("runfiles/api/show.run", "$ echo {{ ENV.SHARED }} {{ ENV.ONLY_ROOT }}\n"),
	]);
	let o = p.run(&["api:show"]);
	assert!(o.status.success(), "{}", err(&o));
	assert_eq!(out(&o).trim(), "api yes");
}

#[test]
fn a_binding_in_a_nested_shared_file_is_visible_to_its_targets() {
	let p = project(&[
		("runfiles/api/_shared.run", "let region = \"eu\"\n"),
		("runfiles/api/show.run", "$ echo {{ region }}\n"),
	]);
	let o = p.run(&["api:show"]);
	assert!(o.status.success(), "{}", err(&o));
	assert_eq!(out(&o).trim(), "eu");
}

#[test]
fn a_temp_file_is_gone_once_the_binary_exits() {
	// End to end: the registry is only useful if the process that owns it
	// actually drains it, on both the succeeding and the failing path.
	let p = project(&[
		(
			"runfiles/ok.run",
			"let f = temp_file(\"secret\", \"json\")\n$ echo {{ f }}\n",
		),
		(
			"runfiles/bad.run",
			"let f = temp_file(\"secret\")\n$ echo {{ f }}\n$ false\n",
		),
	]);
	for (target, should_succeed) in [("ok", true), ("bad", false)] {
		let o = p.run(&[target]);
		assert_eq!(o.status.success(), should_succeed, "{target}: {}", err(&o));
		let path = out(&o).trim().to_string();
		assert!(!path.is_empty(), "{target} printed no path");
		assert!(!Path::new(&path).exists(), "{target} left {path} behind");
	}
}

#[test]
fn dry_run_reports_the_temp_file_it_would_have_made() {
	let p = project(&[("runfiles/t.run", "let f = temp_file(\"x\")\n$ echo {{ f }}\n")]);
	let o = p.run(&["--dry-run", "t"]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(out(&o).contains("would create a temp file"), "{}", out(&o));
}

// ------------------------------------------------------------- interrupt
//
// A real SIGINT, to the child's own process group, which is what a terminal
// does on Ctrl+C. The group matters: without it the signal would reach the
// test runner too.

#[cfg(unix)]
fn spawn_in_own_group(p: &Project, args: &[&str]) -> Child {
	use std::os::unix::process::CommandExt;
	let mut c = p.command(p.dir.path(), args);
	c.stdout(Stdio::piped()).stderr(Stdio::null());
	c.process_group(0);
	c.spawn().expect("spawn run")
}

#[cfg(unix)]
fn interrupt_group(child: &Child) {
	// Negative pid means the group, exactly as `kill %1` and Ctrl+C do.
	unsafe { libc::kill(-(child.id() as i32), libc::SIGINT) };
}

#[cfg(unix)]
#[test]
fn ctrl_c_stops_the_run_and_exits_130() {
	let p = project(&[(
		"runfiles/slow.run",
		"$ echo started > started.txt\n$ sleep 30\nlet x = \"1\"\n$ echo after > after.txt\n",
	)]);
	let mut child = spawn_in_own_group(&p, &["slow"]);
	until("the run to start", || p.dir.path().join("started.txt").exists());
	interrupt_group(&child);

	let status = until_exit(&mut child);
	assert_eq!(status.code(), Some(130), "a shell reports 130 for SIGINT");
	assert!(
		!p.dir.path().join("after.txt").exists(),
		"it stopped rather than carrying on"
	);
}

#[cfg(unix)]
#[test]
fn a_run_that_is_not_interrupted_still_exits_normally() {
	// The control: the handler is installed for every run, so it has to be
	// invisible when no signal arrives.
	let p = project(&[("runfiles/quick.run", "$ echo done > done.txt\n")]);
	let mut child = spawn_in_own_group(&p, &["quick"]);
	let status = until_exit(&mut child);
	assert_eq!(status.code(), Some(0), "{status:?}");
	assert!(p.dir.path().join("done.txt").exists());
}

#[cfg(unix)]
#[test]
fn an_interrupt_removes_the_temp_files_the_run_made() {
	// The reason it is caught rather than left to the OS.
	let p = project(&[(
		"runfiles/slow.run",
		"let f = temp_file(\"secret\")\n$ echo {{ f }} > path.txt\n$ sleep 30\n",
	)]);
	let mut child = spawn_in_own_group(&p, &["slow"]);
	until("the temp file", || p.dir.path().join("path.txt").exists());
	let path = std::fs::read_to_string(p.dir.path().join("path.txt"))
		.unwrap()
		.trim()
		.to_string();
	assert!(Path::new(&path).exists(), "made: {path}");

	interrupt_group(&child);
	assert_eq!(until_exit(&mut child).code(), Some(130));
	assert!(!Path::new(&path).exists(), "{path} survived the interrupt");
}

#[cfg(unix)]
fn until_exit(child: &mut Child) -> std::process::ExitStatus {
	let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
	while std::time::Instant::now() < deadline {
		if let Some(s) = child.try_wait().expect("try_wait") {
			return s;
		}
		std::thread::sleep(std::time::Duration::from_millis(20));
	}
	let _ = child.kill();
	panic!("the run never exited");
}

#[test]
fn editing_a_comment_in_setup_does_not_re_trigger_the_gate() {
	let p = project(&[
		("runfiles/setup.run", "# Sets up\n$ true\n"),
		("runfiles/build.run", "$ echo built\n"),
	]);
	assert!(p.run(&["setup"]).status.success());
	assert!(p.run(&["build"]).status.success(), "gate satisfied");

	std::fs::write(
		p.dir.path().join("runfiles/setup.run"),
		"# Sets up.\n#\n# At more length.\n$ true\n",
	)
	.unwrap();
	let o = p.run(&["build"]);
	assert!(
		o.status.success(),
		"a comment is not a change to what setup does: {}",
		err(&o)
	);

	std::fs::write(
		p.dir.path().join("runfiles/setup.run"),
		"# Sets up\n$ true\n$ echo more\n",
	)
	.unwrap();
	assert!(!p.run(&["build"]).status.success(), "but a new command is");
}

#[test]
fn dry_run_says_which_shell_a_dollar_line_uses() {
	let p = project(&[(MARK, &marker("o"))]);
	let o = p.run(&["--dry-run", "mark"]);
	assert!(o.status.success(), "{}", err(&o));
	let first = out(&o).lines().next().unwrap_or_default().to_string();
	assert!(first.starts_with("# $ runs "), "{}", out(&o));
	assert!(first.contains("sh"), "names a shell: {first}");
}

#[test]
fn parallel_branches_prefix_every_line_they_print() {
	// Several children write at once, so each line says which branch it came
	// from. Sorted, because the interleaving is the point: order is not fixed.
	let p = project(&[(
		"runfiles/t.run",
		".parallel = true\n\n$ printf 'a1\\na2\\n'\nlet x = \"1\"\n$ printf 'b1\\n'\n",
	)]);
	let o = p.run(&["t"]);
	assert!(o.status.success(), "{}", err(&o));
	let text = out(&o);
	let mut lines: Vec<&str> = text.lines().collect();
	lines.sort_unstable();
	assert_eq!(lines, ["printf | a1", "printf | a2", "printf | b1"], "{text}");
}

#[test]
fn a_sequential_run_prints_no_labels() {
	// The prefix is for telling concurrent branches apart; one at a time needs
	// none, and adding one would break every pipeline reading `run`'s output.
	let p = project(&[("runfiles/t.run", "$ printf 'plain\\n'\n")]);
	let o = p.run(&["t"]);
	assert_eq!(out(&o), "plain\n", "{}", err(&o));
}

#[test]
fn a_parallel_branch_labels_its_stderr_too() {
	let p = project(&[(
		"runfiles/t.run",
		".parallel = true\n\n$ printf 'oops\\n' >&2\nlet x = \"1\"\n$ printf 'fine\\n'\n",
	)]);
	let o = p.run(&["t"]);
	assert!(o.status.success(), "{}", err(&o));
	assert_eq!(err_from_commands(&o).trim(), "printf | oops", "{}", err(&o));
	assert_eq!(out(&o).trim(), "printf | fine");
}

#[test]
fn a_dispatched_branch_is_labelled_with_its_target_name() {
	let p = project(&[
		("runfiles/all.run", ".parallel = true\n\nrun one\nrun two\n"),
		("runfiles/one.run", "$ printf 'from-one\\n'\n"),
		("runfiles/two.run", "$ printf 'from-two\\n'\n"),
	]);
	let o = p.run(&["all"]);
	assert!(o.status.success(), "{}", err(&o));
	let text = out(&o);
	let mut lines: Vec<&str> = text.lines().collect();
	lines.sort_unstable();
	// The target name, not the command it happens to run: that is what tells
	// you which branch of the fan-out you are reading.
	assert_eq!(lines, ["one | from-one", "two | from-two"], "{text}");
}

#[test]
fn a_branch_label_reaches_the_whole_subtree() {
	// A dependency of a branch is still that branch's output, so it carries the
	// same name rather than its own.
	let p = project(&[
		("runfiles/all.run", ".parallel = true\n\nrun one\n"),
		("runfiles/one.run", "run deep\n$ printf 'mine\\n'\n"),
		("runfiles/deep.run", "$ printf 'nested\\n'\n"),
	]);
	let o = p.run(&["all"]);
	assert!(o.status.success(), "{}", err(&o));
	let text = out(&o);
	let mut lines: Vec<&str> = text.lines().collect();
	lines.sort_unstable();
	assert_eq!(lines, ["one | mine", "one | nested"], "{text}");
}

#[test]
fn the_listing_shows_the_names_a_target_also_answers_to() {
	// Without this an alias is undiscoverable: the listing reports the file
	// name, and nothing tells you the other name works.
	let p = project(&[
		("runfiles/build.run", "# Builds it\n.alias = \"b\"\n$ true\n"),
		("runfiles/plain.run", ".alias = \"p\"\n$ true\n"),
	]);
	let o = p.run(&[":list"]);
	assert!(o.status.success(), "{}", err(&o));
	let text = out(&o);
	assert!(text.contains("Builds it  (also `b`)"), "{text}");
	assert!(
		text.contains("also `p`"),
		"a target with no description still shows it: {text}"
	);
}

#[test]
fn list_json_carries_the_aliases_too() {
	let p = project(&[("runfiles/build.run", "# Builds it\n.alias = \"b\"\n$ true\n")]);
	let v: serde_json::Value = serde_json::from_str(&out(&p.run(&[":list", "--json"]))).expect("JSON");
	assert_eq!(v["targets"][0]["aliases"][0], "b");
}

#[test]
fn a_target_with_no_alias_lists_exactly_as_before() {
	let p = project(&[("runfiles/build.run", "# Builds it\n$ true\n")]);
	let text = out(&p.run(&[":list"]));
	assert!(text.contains("build  Builds it"), "{text}");
	assert!(!text.contains("also"), "{text}");
}

// ---------------------------------------------- logging, detach, parallel for

#[test]
fn the_runner_announces_each_command_on_stderr() {
	// Native, with no property to turn it on: the old `logging: true` field was
	// cut because of this. On stderr, so a pipeline reading stdout is unaffected.
	let p = project(&[("runfiles/t.run", "$ echo hello\n")]);
	let o = p.run(&["t"]);
	assert!(o.status.success(), "{}", err(&o));
	assert_eq!(out(&o), "hello\n", "stdout stays exactly the command's own output");
	assert!(err(&o).contains("[runfile] echo hello"), "{}", err(&o));
}

#[test]
fn an_exec_block_is_announced_whole() {
	// It is one process, so showing half of it would be a lie.
	let p = project(&[("runfiles/t.run", "exec sh\n\techo one\n\techo two\nend\n")]);
	let o = p.run(&["t"]);
	assert!(err(&o).contains("[runfile] echo one"), "{}", err(&o));
	assert!(err(&o).contains("[runfile] echo two"), "{}", err(&o));
}

#[test]
fn a_preview_announces_nothing_twice() {
	// `--dry-run` already prints the commands to stdout; announcing them again
	// on stderr would double every line.
	let p = project(&[("runfiles/t.run", "$ echo hello\n")]);
	let o = p.run(&["--dry-run", "t"]);
	assert!(!err(&o).contains("[runfile]"), "{}", err(&o));
	assert!(out(&o).contains("echo hello"));
}

#[test]
fn a_detached_target_does_not_wait_for_what_it_starts() {
	// Fire and forget: the run returns while the command is still going.
	let p = project(&[("runfiles/t.run", ".detach = true\n\n$ sleep 30; echo late > late.txt\n")]);
	let started = std::time::Instant::now();
	let o = p.run(&["t"]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(started.elapsed().as_secs() < 10, "it waited: {:?}", started.elapsed());
	assert!(
		!p.dir.path().join("late.txt").exists(),
		"and the command is still running"
	);
}

#[test]
fn a_target_without_detach_still_waits() {
	let p = project(&[("runfiles/t.run", "$ echo done > done.txt\n")]);
	assert!(p.run(&["t"]).status.success());
	assert!(
		p.dir.path().join("done.txt").exists(),
		"the file is there by the time run returns"
	);
}

#[test]
fn parallel_on_a_for_block_runs_the_iterations_at_once() {
	// The spec's own example shape. `.parallel` inside the loop means the
	// iterations are the branches, not just each body's statements.
	let p = project(&[(
		"runfiles/t.run",
		"for n in [\"1\", \"2\", \"3\"]\n\t.parallel\n\t$ sleep 1; echo {{ n }} >> out.txt\nend\n",
	)]);
	let started = std::time::Instant::now();
	let o = p.run(&["t"]);
	assert!(o.status.success(), "{}", err(&o));
	let elapsed = started.elapsed();
	assert!(
		elapsed.as_millis() < 2500,
		"three one-second iterations took {elapsed:?}, so they ran in turn"
	);
	let done = std::fs::read_to_string(p.dir.path().join("out.txt")).unwrap();
	assert_eq!(done.lines().count(), 3, "every iteration still ran: {done:?}");
}

#[test]
fn a_for_block_without_parallel_still_runs_in_order() {
	let p = project(&[(
		"runfiles/t.run",
		"for n in [\"1\", \"2\", \"3\"]\n\t$ echo {{ n }} >> out.txt\nend\n",
	)]);
	assert!(p.run(&["t"]).status.success());
	let done = std::fs::read_to_string(p.dir.path().join("out.txt")).unwrap();
	assert_eq!(done, "1\n2\n3\n", "source order, one at a time");
}

// ------------------------------------------------- installing completions

#[test]
fn bash_installs_a_file_that_is_loaded_on_demand_not_a_startup_hook() {
	// The bug this exists for: Ubuntu's `~/.profile` sources `.bashrc` before
	// it puts `~/.local/bin` on PATH, so a startup hook that shells out to
	// `run` found nothing and `eval` registered nothing, silently. A file in
	// bash-completion's directory is read when Tab is first pressed, by which
	// time PATH is complete.
	let p = project(&[(MARK, &marker("o"))]);
	let rc = p.home.path().join(".bashrc");
	std::fs::write(&rc, "# my own settings\n").unwrap();

	let o = p.run(&[":completions", "install", "bash"]);
	assert!(o.status.success(), "{}", err(&o));
	let installed = p.home.path().join(".local/share/bash-completion/completions/run");
	assert!(installed.is_file(), "{}", out(&o));
	assert!(
		std::fs::read_to_string(&installed)
			.unwrap()
			.contains("complete -F _run run")
	);
	assert_eq!(
		std::fs::read_to_string(&rc).unwrap(),
		"# my own settings\n",
		"and the profile is not touched at all"
	);

	assert!(p.run(&[":completions", "uninstall", "bash"]).status.success());
	assert!(!installed.exists());
}

#[test]
fn uninstalling_bash_also_removes_an_older_startup_hook() {
	// Upgrading must not leave the broken line behind.
	let p = project(&[(MARK, &marker("o"))]);
	let rc = p.home.path().join(".bashrc");
	std::fs::write(
		&rc,
		"# mine\n\n# runfile completions\neval \"$(run :completions output bash)\"\n",
	)
	.unwrap();
	let o = p.run(&[":completions", "uninstall", "bash"]);
	assert!(o.status.success(), "{}", err(&o));
	assert_eq!(std::fs::read_to_string(&rc).unwrap(), "# mine\n");
	assert!(out(&o).contains("older hook"), "{}", out(&o));
}

#[test]
fn a_profile_hook_names_the_binary_rather_than_trusting_the_path() {
	// Same hazard for zsh: at startup `run` may not be on PATH yet.
	let p = project(&[(MARK, &marker("o"))]);
	assert!(p.run(&[":completions", "install", "zsh"]).status.success());
	let rc = std::fs::read_to_string(p.home.path().join(".zshrc")).unwrap();
	assert!(rc.contains("# runfile completions"), "{rc}");
	assert!(rc.contains(":completions output zsh"), "{rc}");
	assert!(rc.contains('/'), "the hook names a path, not a bare `run`: {rc}");
}

#[test]
fn installing_twice_changes_nothing() {
	let p = project(&[(MARK, &marker("o"))]);
	p.run(&[":completions", "install", "zsh"]);
	let once = std::fs::read_to_string(p.home.path().join(".zshrc")).unwrap();
	let o = p.run(&[":completions", "install", "zsh"]);
	assert!(out(&o).contains("Already installed"), "{}", out(&o));
	assert_eq!(std::fs::read_to_string(p.home.path().join(".zshrc")).unwrap(), once);
}

#[test]
fn install_creates_a_profile_that_does_not_exist_yet() {
	let p = project(&[(MARK, &marker("o"))]);
	assert!(p.run(&[":completions", "install", "zsh"]).status.success());
	assert!(p.home.path().join(".zshrc").exists());
}

#[test]
fn fish_gets_a_file_of_its_own_rather_than_a_profile_line() {
	// Fish reads a completions directory, so the whole script goes there.
	let p = project(&[(MARK, &marker("o"))]);
	let o = p.run(&[":completions", "install", "fish"]);
	assert!(o.status.success(), "{}", err(&o));
	let path = p.home.path().join("fish/completions/run.fish");
	assert!(path.exists(), "{}", out(&o));
	assert!(std::fs::read_to_string(&path).unwrap().contains("run :complete"));

	assert!(p.run(&[":completions", "uninstall", "fish"]).status.success());
	assert!(!path.exists());
}

#[test]
fn uninstalling_what_was_never_installed_is_not_an_error() {
	let p = project(&[(MARK, &marker("o"))]);
	for shell in ["bash", "fish", "zsh"] {
		let o = p.run(&[":completions", "uninstall", shell]);
		assert!(o.status.success(), "{shell}: {}", err(&o));
		assert!(out(&o).contains("Nothing installed"), "{shell}: {}", out(&o));
	}
}

#[test]
fn output_prints_the_script_without_installing_it() {
	// The installed hook calls exactly this, so it cannot change shape.
	let p = project(&[(MARK, &marker("o"))]);
	let o = p.run(&[":completions", "output", "bash"]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(out(&o).contains("complete -F _run run"), "{}", out(&o));
	assert!(!p.home.path().join(".bashrc").exists(), "printing installs nothing");
}

#[test]
fn install_rejects_a_shell_it_does_not_know() {
	let p = project(&[(MARK, &marker("o"))]);
	let o = p.run(&[":completions", "install", "nushell"]);
	assert!(!o.status.success());
	assert!(err(&o).contains("bash, zsh, fish"), "{}", err(&o));
	assert!(err(&p.run(&[":completions", "install"])).contains("needs a shell"));
}

#[cfg(unix)]
#[test]
fn the_bash_script_completes_the_completions_actions() {
	let p = project(&[(MARK, &marker("o"))]);
	let mut got = complete_bash(&p, "run :completions ins");
	got.sort();
	assert_eq!(got, ["install"]);
	// And a shell at the next level.
	let mut shells = complete_bash(&p, "run :completions install ba");
	shells.sort();
	assert_eq!(shells, ["bash"]);
}

// ------------------------------------------------------------------- help

#[test]
fn help_is_available_from_the_flag_and_from_no_arguments() {
	// Neither should need a runfiles/ directory to exist.
	let p = project(&[]);
	for args in [vec!["--help"], vec!["-h"], vec![]] {
		let o = p.run(&args);
		assert!(o.status.success(), "{args:?}: {}", err(&o));
		assert!(out(&o).contains("run :list"), "{args:?}: {}", out(&o));
		assert!(out(&o).contains("Commands"), "{args:?}: {}", out(&o));
		assert!(out(&o).contains("Options"), "{args:?}: {}", out(&o));
	}
}

#[test]
fn the_main_help_does_not_list_a_subcommands_own_commands() {
	// One line per command, like every other entry; `:completions` explains
	// itself when you ask it.
	let p = project(&[]);
	let text = out(&p.run(&["--help"]));
	assert!(text.contains("run :completions <command>"), "{text}");
	assert!(!text.contains(":completions install"), "{text}");
	assert!(!text.contains(":env init"), "{text}");
}

#[test]
fn each_subcommand_explains_itself() {
	let p = project(&[]);
	for (args, expect) in [
		(vec![":env"], "run :env init"),
		(vec![":env", "--help"], "run :env init"),
		(vec![":completions"], "run :completions install"),
		(vec![":completions", "--help"], "run :completions output"),
		(vec![":generate"], "run :generate zed"),
		(vec![":generate", "--help"], "run :generate jetbrains"),
	] {
		let o = p.run(&args);
		assert!(o.status.success(), "{args:?}: {}", err(&o));
		assert!(out(&o).contains(expect), "{args:?}: {}", out(&o));
	}
}

#[test]
fn help_is_plain_when_it_is_not_going_to_a_terminal() {
	// Captured output is a pipe, so no escape codes may appear in it.
	let p = project(&[]);
	for args in [vec!["--help"], vec![":env"], vec![":generate"]] {
		assert!(
			!out(&p.run(&args)).contains('\x1b'),
			"{args:?} emitted colour into a pipe"
		);
	}
}

#[test]
fn there_is_no_version_command_any_more() {
	let p = project(&[(MARK, &marker("o"))]);
	let o = p.run(&[":version"]);
	assert!(!o.status.success());
	assert!(err(&o).contains("unknown command"), "{}", err(&o));
}
