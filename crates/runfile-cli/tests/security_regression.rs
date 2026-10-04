//! Security regressions from the 2026-10-03 audit (`security-audit-2026-10-03/`).
//!
//! Each test asserts the *secure* behaviour. Every one failed against 1.8.2 on
//! its own assertion, and passes once the finding it names is fixed -- so it is
//! that fix's acceptance criterion, and what keeps it fixed. Every payload only
//! creates a marker file inside the test's own temporary directory.

#![cfg(unix)]

use std::path::Path;
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

/// Mirrors `ci_detect::CI_ENV_VARS`, as `cli.rs` does: CI mode changes what the
/// runner reads and answers, so none of it may leak in from the shell.
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
			.env("APPDATA", self.home.path())
			.env_remove("RUNFILE_SKIP_PREPARE")
			.env_remove("RUNFILE_PRIVATE_KEYS")
			// A developer who trusts every directory would otherwise trust them
			// for the tests too.
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

fn bin_dir() -> &'static Path {
	Path::new(env!("CARGO_BIN_EXE_run")).parent().unwrap()
}

/// Source the generated bash completion and press Tab after `run `, the way
/// readline calls `_run` -- the same driving `cli.rs` does, cut down to a bare
/// Tab, which is all this needs.
fn press_tab_after_run(p: &Project) {
	let script = String::from_utf8_lossy(&p.run(&[":completions", "output", "bash"]).stdout).into_owned();
	press_tab_with(p, &script);
}

/// The `_run` that `run :completions install bash` wrote before the fix, word
/// for word. An install is a copy, and upgrading `run` does not replace it.
const BASH_INSTALLED_BY_1_8_2: &str = r#"COMP_WORDBREAKS="${COMP_WORDBREAKS//:/}"
_run() {
	local cur out
	cur="${COMP_WORDS[COMP_CWORD]}"
	out="$(run :complete "$COMP_CWORD" "${COMP_WORDS[@]}" 2>/dev/null)"
	COMPREPLY=()
	case "$out" in
		*"<dirs>"*) COMPREPLY=( $(compgen -d -- "$cur") ); out="${out/<dirs>/}" ;;
		*"<files>"*) COMPREPLY=( $(compgen -f -- "$cur") ); out="${out/<files>/}" ;;
	esac
	COMPREPLY+=( $(compgen -W "$out" -- "$cur") )
}
complete -F _run run
"#;

fn press_tab_with(p: &Project, script: &str) {
	let path = p.home.path().join("comp.bash");
	std::fs::write(&path, script).unwrap();
	let prog = format!("source {}\nCOMP_WORDS=(run '')\nCOMP_CWORD=1\n_run", path.display());
	Command::new("bash")
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
}

// ------------------------------------------------------- completion (Tab)

/// Audit finding: arbitrary command execution on Tab via bash completion.
/// `_run` hands the binary's candidates to `compgen -W`, which expands
/// `$(…)` and backticks in every word -- and a target's name is its file name.
#[test]
fn security_regression_pressing_tab_does_not_run_a_command_spelled_in_a_file_name() {
	let p = project(&[("runfiles/$(touch TAB_RAN).run", "$ true\n")]);
	press_tab_after_run(&p);
	assert!(
		!p.exists("TAB_RAN"),
		"pressing Tab after `run` ran the command spelled in a target's file name"
	);
}

/// A script installed by an older `run` still hands every candidate to
/// `compgen -W`. What keeps it safe after an upgrade is that the binary no
/// longer offers a name a shell would expand -- `$( )`, backticks or `<( )`.
#[test]
fn security_regression_the_script_an_older_run_installed_does_not_run_a_file_name() {
	for name in ["$(touch TAB_RAN)", "`touch TAB_RAN`", "a<(touch TAB_RAN)"] {
		let p = project(&[
			(&format!("runfiles/{name}.run"), "$ true\n"),
			("runfiles/build.run", "$ true\n"),
		]);
		press_tab_with(&p, BASH_INSTALLED_BY_1_8_2);
		assert!(
			!p.exists("TAB_RAN"),
			"Tab in a script installed by 1.8.2 ran the command spelled in `{name}`"
		);
	}
}

/// The same flaw through backticks, which `compgen -W` substitutes as well.
#[test]
fn security_regression_pressing_tab_does_not_run_a_backtick_in_a_file_name() {
	let p = project(&[("runfiles/`touch TAB_RAN`.run", "$ true\n")]);
	press_tab_after_run(&p);
	assert!(
		!p.exists("TAB_RAN"),
		"pressing Tab after `run` ran a backtick command spelled in a target's file name"
	);
}

// ------------------------------------------------ interpolation self-quoting

/// Audit finding: a self-quoted interpolation is executed as shell code inside
/// double quotes. `{{ x }}` single-quotes its value, and single quotes are
/// plain characters inside `"…"`, so `$(…)` in the value still expands. This is
/// the shape `SHELL-CHECK-RULES.md` lists as "not flagged". Either fix --
/// refusing the file before it runs, or quoting for the context the value
/// lands in -- leaves the marker unmade.
#[test]
fn security_regression_an_interpolation_inside_double_quotes_does_not_run_its_value() {
	let p = project(&[("runfiles/say.run", "$ echo \"Deploying {{ ARG.tag }}\"\n")]);
	let o = p.run(&["say", "--tag=$(touch VALUE_RAN)"]);
	assert!(
		!p.exists("VALUE_RAN"),
		"a value interpolated inside double quotes was run as a command: {}",
		String::from_utf8_lossy(&o.stdout)
	);
}

/// The `ssh host "cd {{ dir }} && …"` shape the rules document calls correct:
/// the local shell expands the value before the remote one ever sees it.
/// `sh -c` stands in for `ssh` so nothing leaves the machine.
#[test]
fn security_regression_a_command_string_for_another_shell_does_not_run_its_value_locally() {
	let p = project(&[("runfiles/remote.run", "$ sh -c \"cd {{ ARG.dir }} && true\"\n")]);
	let o = p.run(&["remote", "--dir=$(touch VALUE_RAN)"]);
	assert!(
		!p.exists("VALUE_RAN"),
		"the value was expanded by the local shell: {}",
		String::from_utf8_lossy(&o.stderr)
	);
}

/// Audit finding: an interpolation in shell arithmetic is evaluated as code.
/// Inside `$(( ))` the value is written the way it is inside double quotes, so
/// its `$( )` is text, and its own quotes are what arithmetic then refuses.
/// This covers `$(( ))`, `(( ))` and `$[ ]` only: an operand of `[[ -eq ]]`,
/// `let` or `declare -i` is evaluated after quote removal, and stays open.
#[test]
fn security_regression_an_interpolation_inside_shell_arithmetic_does_not_run_its_value() {
	// The arithmetic *expansions* are rendered so the shell cannot expand the
	// value; the arithmetic-*evaluating* positions are refused before the run.
	let bodies = [
		"$ echo $(( {{ ARG.n }} + 1 ))\n",
		"$ (( {{ ARG.n }} )) && echo yes\n",
		"$ [[ 1 -eq {{ ARG.n }} ]] && echo yes\n",
		"$ let x={{ ARG.n }}\n",
		"$ declare -i x={{ ARG.n }}\n",
	];
	for body in bodies {
		let p = project(&[("runfiles/inc.run", body)]);
		let o = p.run(&["inc", "--n=a[$(touch VALUE_RAN)]"]);
		assert!(
			!p.exists("VALUE_RAN"),
			"a value in a shell arithmetic position ran as a command: {body}{}",
			String::from_utf8_lossy(&o.stderr)
		);
	}
}

/// An unquoted heredoc expands `$(…)` the same way double quotes do.
#[test]
fn security_regression_an_interpolation_inside_a_heredoc_does_not_run_its_value() {
	let p = project(&[(
		"runfiles/write.run",
		"exec bash\n\tcat <<EOF\n\tname={{ ARG.name }}\n\tEOF\nend\n",
	)]);
	let o = p.run(&["write", "--name=$(touch VALUE_RAN)"]);
	assert!(
		!p.exists("VALUE_RAN"),
		"a value interpolated into a heredoc was run as a command: {}",
		String::from_utf8_lossy(&o.stdout)
	);
}

/// A body, and what it prints for a value.
type Form<'a> = (&'a str, &'a dyn Fn(&str) -> String);

/// Every place a value can sit in shell text, each with every value worth
/// fearing: nothing in a value may run, and what arrives has to be exactly the
/// value -- or, where the author's quotes keep the value's own quotes as text,
/// exactly that text, which is what a message shows and a command string for
/// another shell needs.
#[test]
fn security_regression_a_value_is_text_wherever_it_sits() {
	use runfile_lang::value::shell_quote;
	let values = [
		"plain",
		"my dir",
		"$(touch VALUE_RAN)",
		"`touch VALUE_RAN`",
		"a\"b",
		"it's",
		"back\\slash",
		"$HOME",
		"two\nlines",
		"\"; touch VALUE_RAN; \"",
		"'; touch VALUE_RAN; '",
		"\\$(touch VALUE_RAN)",
		"*",
	];
	// The body, and what it prints for a value: the value itself, or its
	// quoted word as text.
	let exact = |v: &str| format!("{v}\n");
	let quoted = |v: &str| format!("{}\n", shell_quote(v));
	let around = |v: &str| format!("'pre'{}'post'\n", shell_quote(v));
	let forms: [Form; 11] = [
		("$ printf '%s\\n' {{ ARG.v }}\n", &exact),
		("$ printf '%s\\n' \"{{ ARG.v }}\"\n", &quoted),
		("$ sh -c \"printf '%s\\\\n' {{ ARG.v }}\"\n", &exact),
		("$ printf '%s\\n' \"$(printf '%s' {{ ARG.v }})\"\n", &exact),
		("$ x=; printf '%s\\n' \"${x:-{{ ARG.v }}}\"\n", &quoted),
		("$ x=; printf '%s\\n' \"${x:-'pre{{ ARG.v }}post'}\"\n", &around),
		(
			"exec bash\n\tx=\n\tcat <<EOF\n\t${x:-{{ ARG.v }}}\n\tEOF\nend\n",
			&quoted,
		),
		("$ x=; y=${x:-'{{ ARG.v }}'}; printf '%s\\n' \"$y\"\n", &exact),
		("$ printf '%s\\n' \"`printf '%s' {{ ARG.v }}`\"\n", &exact),
		("exec bash\n\tcat <<EOF\n\t{{ ARG.v }}\n\tEOF\nend\n", &quoted),
		("exec bash\n\tcat <<'EOF'\n\t{{ ARG.v }}\n\tEOF\nend\n", &quoted),
	];
	for (body, expect) in forms {
		for v in values {
			let p = project(&[("runfiles/t.run", body)]);
			let o = p.run(&["t", &format!("--v={v}")]);
			let shown = format!("{body}with {v:?}");
			assert!(!p.exists("VALUE_RAN"), "a value ran as code: {shown}");
			assert!(o.status.success(), "{shown}\n{}", String::from_utf8_lossy(&o.stderr));
			assert_eq!(String::from_utf8_lossy(&o.stdout), expect(v), "{shown}");
		}
	}
}

/// A heredoc's body ends at the first line that is its delimiter, however that
/// line came to be -- so a value holding one ended the body early, and the
/// text below it ran as commands. A quoted heredoc is no defence: the body is
/// cut into lines before anything is quoted.
#[test]
fn security_regression_a_value_cannot_end_the_heredoc_it_is_in() {
	for delimiter in ["EOF", "'EOF'"] {
		let body = format!("exec bash\n\tcat <<{delimiter}\n\t{{{{ ARG.v }}}}\n\ttouch VALUE_RAN\n\tEOF\nend\n");
		let p = project(&[("runfiles/t.run", &body)]);
		// The last line of a quoted value carries its closing quote, so only a
		// whole value, or a line in the middle of one, can be the delimiter.
		for v in ["EOF", "x\nEOF\ny"] {
			let o = p.run(&["t", &format!("--v={v}")]);
			assert!(!o.status.success(), "{body}with {v:?} ran");
			assert!(
				String::from_utf8_lossy(&o.stderr).contains("ends the heredoc"),
				"{}",
				String::from_utf8_lossy(&o.stderr)
			);
			assert!(!p.exists("VALUE_RAN"), "{body}with {v:?}");
		}
		// The same value anywhere else in the body is only text.
		let o = p.run(&["t", "--v=EOFX"]);
		assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
	}
}

/// A comment ends at a line break, so a value holding one ended it, and the
/// rest of the value ran.
#[test]
fn security_regression_a_value_cannot_end_the_comment_it_is_in() {
	let p = project(&[("runfiles/t.run", "$ true # was {{ ARG.v }}\n")]);
	let o = p.run(&["t", "--v=x\ntouch VALUE_RAN #"]);
	assert!(!o.status.success());
	assert!(!p.exists("VALUE_RAN"));
	assert!(p.run(&["t", "--v=plain"]).status.success());
}

/// A data-driven delay or length that `Duration`/arithmetic cannot hold used to
/// abort the process (exit 101 in debug, a wrapped length in release), and the
/// abort skipped temp-file cleanup. Each now ends with a clean error or a
/// clamped result, never a panic -- exit 101 and 134 are the failure here.
#[test]
fn security_regression_a_data_driven_number_does_not_abort_the_process() {
	let cases: &[(&str, &str, &str)] = &[
		("sleep", "$ echo start\nsleep(number(ARG.n))\n", "1e300"),
		("retry", "retry 2 every number(ARG.n)\n\t$ false\nend\n", "1e300"),
		(
			"slice",
			"let xs = [\"a\", \"b\"]\nprint(join(\",\", slice(xs, 0, number(ARG.n))))\n",
			"1e20",
		),
	];
	for (name, body, n) in cases {
		let p = project(&[(&format!("runfiles/{name}.run"), body)]);
		let o = p.run(&[name, &format!("--n={n}")]);
		let code = o.status.code();
		assert!(
			code != Some(101) && code != Some(134),
			"{name} aborted ({code:?}): {}",
			String::from_utf8_lossy(&o.stderr)
		);
	}
}

/// An empty list interpolated with a path suffix glued onto it collapses the
/// path -- `rm -rf {{ dirs }}/cache` becomes `rm -rf /cache`. `run :lint`
/// refuses the glued shape before it can run (audit SA-005).
#[test]
fn security_regression_lint_flags_a_list_glued_to_a_path_suffix() {
	let p = project(&[(
		"runfiles/clean.run",
		"let dirs = glob(\"build/*\")\n$ rm -rf {{ dirs }}/cache\n",
	)]);
	let o = p.run(&[":lint", "--check"]);
	assert!(!o.status.success(), "lint should fail");
	let out = format!(
		"{}{}",
		String::from_utf8_lossy(&o.stdout),
		String::from_utf8_lossy(&o.stderr)
	);
	assert!(out.contains("glued-list"), "{out}");
	// A list in its own word draws no such finding.
	let ok = project(&[(
		"runfiles/clean.run",
		"let dirs = glob(\"build/*\")\n$ rm -rf {{ dirs }}\n",
	)]);
	let o = ok.run(&[":lint", "--check"]);
	let out = format!(
		"{}{}",
		String::from_utf8_lossy(&o.stdout),
		String::from_utf8_lossy(&o.stderr)
	);
	assert!(!out.contains("glued-list"), "{out}");
}

/// `run --dry-run` is documented as a safe preview of a repository before you
/// decide to run it. It must not read files outside the project, probe for them,
/// or unlock the credential store to decrypt the repository's values (audit
/// SA-008). A read inside the project still works, so the preview stays useful.
#[test]
fn security_regression_dry_run_does_not_read_outside_the_project_or_touch_the_keyring() {
	let p = project(&[(
		"runfiles/t.run",
		"print(\"abs:\", read_file(\"/etc/hostname\"))\n\
		 print(\"exists:\", file_exists(\"/etc/hostname\"))\n\
		 print(\"in:\", read_file(\"in.txt\"))\n",
	)]);
	std::fs::write(p.dir.path().join("in.txt"), "in-repo").unwrap();
	let o = p.run(&["--dry-run", "t"]);
	let out = String::from_utf8_lossy(&o.stdout);
	assert!(
		out.contains("abs: <contents of /etc/hostname>"),
		"outside read was not a placeholder: {out}"
	);
	assert!(out.contains("exists: false"), "outside path was probed: {out}");
	assert!(
		out.contains("in: in-repo"),
		"an in-project read should still work: {out}"
	);

	// An encrypted `.env-file` must not be read or decrypted during a preview:
	// that would reach the keyring. With DBUS pointed nowhere a real decrypt
	// would error or hang; the preview completes instead.
	let enc = project(&[
		("runfiles/d.run", ".env-file = \".env\"\nprint(\"done\")\n"),
		(".env", "RUNFILE_ENCRYPTION_PUBLIC_KEY=deadbeef\nSECRET=encrypted:abc\n"),
	]);
	let o = enc.run(&["--dry-run", "d"]);
	assert!(
		o.status.success(),
		"preview reached the keyring: {}",
		String::from_utf8_lossy(&o.stderr)
	);
	assert!(String::from_utf8_lossy(&o.stdout).contains("done"));
}
