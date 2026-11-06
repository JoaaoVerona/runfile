//! Security regressions from the 2026-11-06 audit (`security-audit-2026-11-06/`).
//!
//! Each test asserts the *secure* behaviour and names the finding it proves. They
//! are `#[ignore]`d while their findings are open, so the suite stays green; run
//! them with
//!
//!     run test -- -p runfile-cli --test security_regression_2026_11_06 -- --ignored
//!
//! and remove the `#[ignore]` in the commit that fixes the finding. Every payload
//! only creates a marker file inside the test's own temporary directory.

#![cfg(unix)]

use std::process::{Command, Output};
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

/// Mirrors `ci_detect::CI_ENV_VARS`, as `cli.rs` and `security_regression.rs` do.
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

impl Project {
	fn run(&self, args: &[&str]) -> Output {
		let mut c = Command::new(env!("CARGO_BIN_EXE_run"));
		c.args(args)
			.current_dir(self.dir.path())
			.env("HOME", self.home.path())
			.env("USERPROFILE", self.home.path())
			.env("RUNFILE_CONFIG_DIR", self.home.path())
			.env("XDG_CONFIG_HOME", self.home.path())
			.env("XDG_STATE_HOME", self.home.path())
			.env("XDG_DATA_HOME", self.home.path())
			.env("APPDATA", self.home.path())
			.env_remove("DBUS_SESSION_BUS_ADDRESS")
			.env_remove("RUNFILE_SKIP_PREPARE")
			.env_remove("RUNFILE_PRIVATE_KEYS")
			.env_remove("RUNFILE_SAFE_DIRECTORIES")
			.env_remove("FORCE_COLOR");
		for v in CI_VARS {
			c.env_remove(v);
		}
		c.output().expect("run binary")
	}

	fn exists(&self, name: &str) -> bool {
		self.dir.path().join(name).exists()
	}
}

fn text(o: &Output) -> String {
	format!(
		"status {:?}\nstdout:\n{}\nstderr:\n{}",
		o.status,
		String::from_utf8_lossy(&o.stdout),
		String::from_utf8_lossy(&o.stderr)
	)
}

fn on_path(program: &str) -> bool {
	std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
}

/// SA-001: a shell started through a wrapper is still a shell. `exec env bash`
/// reads its body exactly as `exec bash` does, but `body_is_shell` looks only at
/// the first word, so the body gets no quoting at all.
#[test]
fn security_regression_a_shell_started_through_a_wrapper_does_not_run_its_value() {
	for wrapper in ["/usr/bin/env bash", "env bash"] {
		let p = project(&[(
			"runfiles/deploy.run",
			&format!("exec {wrapper}\n\techo \"deploy {{{{ ARG.v }}}}\"\nend\n"),
		)]);
		let o = p.run(&["deploy", "--v=$(touch VALUE_RAN)"]);
		assert!(
			!p.exists("VALUE_RAN"),
			"`exec {wrapper}` ran an interpolated value as a command:\n{}",
			text(&o)
		);
	}
}

/// SA-002, the zsh half: `.shell = "zsh"` falls back to one single-quoted word
/// for every value, which inside the author's double quotes is the bug the
/// 2026-10-03 audit's SA-003 fixed for bash.
#[test]
fn security_regression_a_value_inside_double_quotes_under_zsh_does_not_run() {
	if !on_path("zsh") {
		eprintln!("zsh not installed; skipped");
		return;
	}
	let p = project(&[(
		"runfiles/say.run",
		".shell = \"zsh\"\n\n$ echo \"Deploying {{ ARG.v }}\"\n",
	)]);
	let o = p.run(&["say", "--v=$(touch VALUE_RAN)"]);
	assert!(
		!p.exists("VALUE_RAN"),
		"a value inside double quotes ran under zsh:\n{}",
		text(&o)
	);
}

/// SA-002, the bash half: a block the shell reader gives up on (`coproc`) gets
/// the same context-blind fallback, under the default shell.
#[test]
fn security_regression_a_block_the_reader_gives_up_on_does_not_run_its_value() {
	let p = project(&[("runfiles/say.run", "$ coproc cat\n$ echo \"x {{ ARG.v }}\"\n")]);
	let o = p.run(&["say", "--v=$(touch VALUE_RAN)"]);
	assert!(
		!p.exists("VALUE_RAN"),
		"a value inside double quotes ran in a block with a coproc:\n{}",
		text(&o)
	);
}

/// SA-008 (the 2026-10-03 audit's SA-008 fix, incomplete): a preview must not
/// read outside the project through a symlink the repository committed.
#[test]
fn security_regression_dry_run_does_not_read_outside_the_project_through_a_symlink() {
	let outside = TempDir::new().unwrap();
	std::fs::write(outside.path().join("secret.txt"), "OUTSIDE-SECRET").unwrap();
	let p = project(&[("runfiles/t.run", "print(\"got:\", read_file(\"link/secret.txt\"))\n")]);
	std::os::unix::fs::symlink(outside.path(), p.dir.path().join("link")).unwrap();
	let o = p.run(&["--dry-run", "t"]);
	assert!(
		!String::from_utf8_lossy(&o.stdout).contains("OUTSIDE-SECRET"),
		"a preview read a file outside the project through a symlink:\n{}",
		text(&o)
	);
}

/// SA-007: `:generate` must not write through a symlinked `.vscode/` into a
/// directory outside the project, as `:lint` already refuses to (the 2026-10-03
/// audit's SA-021).
#[test]
fn security_regression_generate_does_not_write_outside_the_project_through_a_symlink() {
	let outside = TempDir::new().unwrap();
	let p = project(&[("runfiles/build.run", "# Build\n$ true\n")]);
	std::os::unix::fs::symlink(outside.path(), p.dir.path().join(".vscode")).unwrap();
	let o = p.run(&[":generate", "vscode"]);
	assert!(
		!outside.path().join("tasks.json").exists(),
		"`:generate vscode` wrote tasks.json through a symlinked .vscode/ outside the project:\n{}",
		text(&o)
	);
}

/// SA-005: a value interpolated into a `$` line must not sit in the shell's
/// argv, where every local user can read it from /proc/<pid>/cmdline.
#[test]
#[cfg(target_os = "linux")]
fn security_regression_an_interpolated_value_is_not_in_the_shells_argv() {
	let p = project(&[(
		"runfiles/t.run",
		"$ : {{ ARG.token }}; tr '\\0' '\\n' < /proc/$$/cmdline\n",
	)]);
	let o = p.run(&["t", "--token=TOKEN-MARKER-1234"]);
	assert!(o.status.success(), "{}", text(&o));
	assert!(
		!String::from_utf8_lossy(&o.stdout).contains("TOKEN-MARKER-1234"),
		"the interpolated value is readable in the shell's /proc/<pid>/cmdline:\n{}",
		text(&o)
	);
}

/// SA-006: a value handed to another target with `run w {{ x }}` is a value. Data
/// spelled `--force` must arrive as a positional, not set the callee's flag.
#[test]
fn security_regression_a_value_forwarded_to_a_target_does_not_become_its_flag() {
	let p = project(&[
		("runfiles/caller.run", "# caller\nrun w {{ ARG.name }}\n"),
		(
			"runfiles/w.run",
			"# w\nif FLAG.force\n\tprint(\"FORCE SET\")\nend\nprint(\"args:\", join(\"|\", ARGS))\n",
		),
	]);
	let o = p.run(&["caller", "--name=--force"]);
	let out = String::from_utf8_lossy(&o.stdout);
	assert!(
		!out.contains("FORCE SET"),
		"data forwarded through `run w {{{{ x }}}}` set the callee's FLAG.force:\n{}",
		text(&o)
	);
	assert!(
		out.contains("args: --force"),
		"the value should arrive as a positional:\n{}",
		text(&o)
	);
}

/// SA-021 (the 2026-10-03 audit's SA-022 fix, incomplete): one deeply nested
/// file must not abort `run :list --json`, which the VS Code extension runs on
/// folder open.
#[test]
#[ignore = "security regression: SA-021 open (audit 2026-11-06)"]
fn security_regression_a_deeply_nested_file_does_not_abort_list() {
	let depth = 4000;
	let mut expr = String::from("\"x\"");
	for _ in 0..depth {
		expr = format!("\"{{{{ {expr} }}}}\"");
	}
	let p = project(&[
		("runfiles/deep.run", &format!("# deep\nprint({expr})\n")),
		("runfiles/ok.run", "# ok\n$ true\n"),
	]);
	let o = p.run(&[":list", "--json"]);
	assert!(
		o.status.code().is_some(),
		"`run :list --json` was killed by a signal on a deeply nested file (stack overflow):\n{}",
		text(&o)
	);
}
