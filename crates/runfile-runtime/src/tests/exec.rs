use super::{Recorder, run_src};

#[test]
fn a_shell_run_is_one_process_so_cd_persists() {
	// The Make wart this design set out to remove.
	let d = Recorder::default();
	run_src("$ cd /\n$ pwd > /dev/null\n", &d).expect("both lines share a shell");
}

#[test]
fn a_statement_between_shell_lines_starts_a_new_process() {
	let d = Recorder::default();
	let trace = run_src("$ cd /\nlet x = 1\n$ pwd > /dev/null\n", &d).unwrap();
	assert_eq!(trace.len(), 2, "two separate shells");
}

#[test]
fn exec_pipes_the_body_to_an_arbitrary_command() {
	let dir = std::env::temp_dir().join("runfile-exec-test");
	let _ = std::fs::remove_file(&dir);
	let src = format!("exec tee {}\n\thello\n\tthere\nend\n", dir.display());
	let d = Recorder::default();
	run_src(&src, &d).expect("tee runs");
	let got = std::fs::read_to_string(&dir).unwrap();
	assert_eq!(got, "hello\nthere", "body reaches the command's stdin, dedented");
	let _ = std::fs::remove_file(&dir);
}

#[test]
fn a_failing_command_stops_the_target() {
	let d = Recorder::default();
	let e = run_src("$ exit 3\n$ echo unreachable\n", &d).unwrap_err();
	assert!(e.to_string().contains("status 3"), "{e}");
}

#[test]
fn ignore_errors_lets_the_target_continue() {
	let d = Recorder::default();
	run_src(".ignore-errors\n$ exit 3\n\nlet x = 1\n$ true\n", &d).expect("failure is swallowed");
}

#[test]
fn capture_takes_stdout_with_one_trailing_newline_stripped() {
	let d = Recorder::default();
	// echo appends a newline; the capture must not keep it.
	run_src("let v = $ echo hi\n$ test {{ v }} = hi\n", &d).expect("trailing newline stripped");
	run_src("let v = $ printf x\n$ test {{ v }} = x\n", &d).expect("no newline to strip");
}

#[test]
fn interpolation_quotes_so_a_dollar_cannot_expand() {
	// If the value were pasted raw, $HOME would expand and the test would fail.
	let d = Recorder::default();
	run_src("let v = \"$HOME\"\n$ test {{ v }} = '$HOME'\n", &d).expect("quoted, not expanded");
}

#[test]
fn an_explicitly_named_shell_gets_the_same_stop_on_failure_as_dollar() {
	// `$ x` must behave exactly like `exec sh` with x as its body.
	let d = Recorder::default();
	let a = run_src("$ false\n$ echo unreachable\n", &d).unwrap_err();
	let b = run_src("exec sh\n\tfalse\n\techo unreachable\nend\n", &d).unwrap_err();
	assert!(a.to_string().contains("status 1"), "{a}");
	assert!(b.to_string().contains("status 1"), "{b}");
}

#[test]
fn a_non_shell_command_is_spawned_verbatim() {
	// The program here is tee, not a shell, so nothing is injected into its
	// arguments -- the same reasoning keeps `exec ssh host bash` untouched.
	let f = std::env::temp_dir().join("runfile-verbatim-test");
	let _ = std::fs::remove_file(&f);
	let d = Recorder::default();
	run_src(&format!("exec tee {}\n\tline\nend\n", f.display()), &d).expect("tee runs");
	assert_eq!(std::fs::read_to_string(&f).unwrap(), "line");
	let _ = std::fs::remove_file(&f);
}

#[test]
fn env_properties_reach_the_process() {
	let d = Recorder::default();
	run_src(".env.GREETING = \"hi\"\n$ test \"$GREETING\" = hi\n", &d).expect("env is set");
}

#[test]
fn an_env_file_is_loaded_before_the_body_is_evaluated() {
	// This is why `.env-file` is header-only: {{ ENV.x }} has to see it.
	let dir = std::env::temp_dir().join("runfile-envfile-test");
	std::fs::create_dir_all(&dir).unwrap();
	std::fs::write(dir.join("vals.env"), "FROM_FILE=loaded\n").unwrap();
	let d = Recorder::default();
	let target = runfile_lang::parse(".env-file = \"vals.env\"\n$ test {{ ENV.FROM_FILE }} = loaded\n").expect("parse");
	let mut r = crate::run::Runner {
		chain: Vec::new(),
		scope: runfile_lang::eval::Scope::new(),
		env: Vec::new(),
		anchor: dir.clone(),
		dispatch: &d,
		assume_yes: true,
		prompt: None,
		interrupted: None,
		label: None,
		dry_run: false,
		trace: Vec::new(),
	};
	crate::run::run_target(&target, &mut r).expect("env file value is visible to {{ ENV.x }}");
	let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn confirm_cancels_when_there_is_nobody_to_ask() {
	let target = runfile_lang::parse(".confirm = \"proceed?\"\n$ true\n").unwrap();
	let d = Recorder::default();
	let mut r = crate::run::Runner {
		chain: Vec::new(),
		scope: runfile_lang::eval::Scope::new(),
		env: Vec::new(),
		anchor: std::env::temp_dir(),
		dispatch: &d,
		assume_yes: false,
		prompt: None,
		interrupted: None,
		label: None,
		dry_run: false,
		trace: Vec::new(),
	};
	assert!(
		crate::run::run_target(&target, &mut r).is_err(),
		"no prompt, no consent"
	);
}

#[test]
fn confirm_interpolates_its_message() {
	let asked = std::sync::Mutex::new(String::new());
	let ask = |m: &str| {
		*asked.lock().expect("asked") = m.to_string();
		true
	};
	let target = runfile_lang::parse(".confirm = \"wipe {{ ARG.env }}?\"\n$ true\n").unwrap();
	let d = Recorder::default();
	let mut scope = runfile_lang::eval::Scope::new();
	scope.args.insert("env".into(), "production".into());
	let mut r = crate::run::Runner {
		chain: Vec::new(),
		scope,
		env: Vec::new(),
		anchor: std::env::temp_dir(),
		dispatch: &d,
		assume_yes: false,
		prompt: Some(&ask),
		interrupted: None,
		label: None,
		dry_run: false,
		trace: Vec::new(),
	};
	crate::run::run_target(&target, &mut r).expect("consent given");
	assert_eq!(
		*asked.lock().expect("asked"),
		"wipe production?",
		"the field could not interpolate before"
	);
}

#[test]
fn a_header_property_cannot_see_a_body_binding() {
	// Header properties resolve before any statement runs -- that ordering is
	// what lets `.env-file` feed `{{ ENV.x }}` -- so they see sources, not lets.
	let target = runfile_lang::parse("let e = \"x\"\n.confirm = \"{{ e }}?\"\n$ true\n").unwrap();
	let d = Recorder::default();
	let mut r = crate::run::Runner {
		chain: Vec::new(),
		scope: runfile_lang::eval::Scope::new(),
		env: Vec::new(),
		anchor: std::env::temp_dir(),
		dispatch: &d,
		assume_yes: true,
		prompt: None,
		interrupted: None,
		label: None,
		dry_run: false,
		trace: Vec::new(),
	};
	let e = crate::run::run_target(&target, &mut r).unwrap_err();
	assert!(e.to_string().contains("not defined"), "{e}");
}

#[test]
fn a_capture_as_a_condition_asks_whether_the_command_succeeded() {
	let d = Recorder::default();
	let t = run_src("if $ true\n\trun yes\nend\n", &d).unwrap();
	assert_eq!(d.calls(), vec!["yes"], "{t:?}");

	let d = Recorder::default();
	run_src("if $ false\n\trun yes\nelse\n\trun no\nend\n", &d).unwrap();
	assert_eq!(d.calls(), vec!["no"]);
}

#[test]
fn a_failing_condition_is_an_answer_and_not_a_failure() {
	// A `$` line that exits non-zero stops the target. The whole point of
	// `if $ cmd` is that this one does not.
	let d = Recorder::default();
	run_src("if $ false\n\trun yes\nend\nrun after\n", &d).expect("the run continues");
	assert_eq!(d.calls(), vec!["after"]);
}

#[test]
fn a_capture_as_a_match_subject_dispatches_on_the_exit_code() {
	let d = Recorder::default();
	run_src(
		"match $ sh -c 'exit 3'\ncase \"0\"\n\trun zero\ncase \"3\"\n\trun three\ndefault\n\trun other\nend\n",
		&d,
	)
	.unwrap();
	assert_eq!(d.calls(), vec!["three"]);

	let d = Recorder::default();
	run_src(
		"match $ sh -c 'exit 9'\ncase \"0\"\n\trun zero\ndefault\n\trun other\nend\n",
		&d,
	)
	.unwrap();
	assert_eq!(d.calls(), vec!["other"]);
}

#[test]
fn code_of_yields_the_status_and_lets_the_run_carry_on() {
	let d = Recorder::default();
	run_src("let c = code_of($ true)\nif c == 0\n\trun ok\nend\n", &d).unwrap();
	assert_eq!(d.calls(), vec!["ok"]);

	let d = Recorder::default();
	run_src(
		"let c = code_of($ sh -c 'exit 7')\nmatch c\ncase \"7\"\n\trun seven\ndefault\n\trun other\nend\n",
		&d,
	)
	.expect("a non-zero code is a value, not a failure");
	assert_eq!(d.calls(), vec!["seven"]);
}

#[test]
fn a_bare_code_of_runs_the_command_and_ignores_how_it_went() {
	let d = Recorder::default();
	run_src("code_of($ false)\nrun after\n", &d).expect("it does not stop the target");
	assert_eq!(d.calls(), vec!["after"]);
}

#[test]
fn the_shell_property_decides_what_runs_a_line() {
	// It was parsed, stored, documented and offered in completion, and read by
	// nothing: `.shell = "sh"` ran under bash and said nothing about it.
	let d = Recorder::default();
	let trace = run_src(".shell = \"sh\"\n$ test -z \"$BASH_VERSION\"\n", &d);
	assert!(trace.is_ok(), "sh has no BASH_VERSION: {trace:?}");

	let d = Recorder::default();
	assert!(
		run_src(".shell = \"bash\"\n$ test -n \"$BASH_VERSION\"\n", &d).is_ok(),
		"and bash does"
	);
}

#[test]
fn a_capture_condition_uses_the_same_shell_as_a_line() {
	// `if $ cmd` is a `$` line asked a question; it must not quietly be a
	// different shell from the lines around it.
	let d = Recorder::default();
	run_src(
		".shell = \"sh\"\nif $ test -z \"$BASH_VERSION\"\n\trun sh\nelse\n\trun bash\nend\n",
		&d,
	)
	.unwrap();
	assert_eq!(d.calls(), vec!["sh"]);
}

#[test]
fn retry_stops_at_the_first_success() {
	let d = Recorder::default();
	run_src("retry 5\n\t$ true\n\trun once\nend\n", &d).unwrap();
	assert_eq!(d.calls(), vec!["once"], "not retried after it worked");
}

#[test]
fn retry_runs_the_else_when_it_never_succeeds() {
	let d = Recorder::default();
	run_src("retry 3\n\t$ false\nelse\n\trun gave-up\nend\nrun after\n", &d).unwrap();
	assert_eq!(d.calls(), vec!["gave-up", "after"], "and the target carries on");
}

#[test]
fn retry_without_an_else_fails_with_the_last_error() {
	let d = Recorder::default();
	let e = run_src("retry 2\n\t$ false\nend\nrun after\n", &d).unwrap_err();
	assert!(e.to_string().contains("exited with status 1"), "{e}");
	assert!(d.calls().is_empty(), "the statement after it does not run");
}

#[test]
fn retry_sees_failures_that_ignore_errors_would_forgive() {
	// Otherwise the body always "succeeds" and a retry runs exactly once,
	// which is the least useful way for this to be wrong.
	let d = Recorder::default();
	run_src(
		".ignore-errors = true\nretry 3\n\t$ false\nelse\n\trun gave-up\nend\n",
		&d,
	)
	.unwrap();
	assert_eq!(d.calls(), vec!["gave-up"]);
}

#[test]
fn an_exit_inside_a_retry_is_not_retried() {
	// It is an instruction to stop, not a failure to have another go at.
	let d = Recorder::default();
	let e = run_src("retry 5\n\t$ echo trying\n\texit(4)\nend\n", &d).unwrap_err();
	assert_eq!(e.exit_code(), Some(4), "{e}");
}

#[test]
fn retry_is_refused_inside_a_parallel_block() {
	let d = Recorder::default();
	let e = run_src(".parallel = true\nretry 2\n\t$ false\nend\n", &d).unwrap_err();
	assert!(e.to_string().contains("cannot be inside a `.parallel`"), "{e}");
}

#[test]
fn each_command_announces_itself_as_it_runs() {
	// A block of `$` lines is one process, so the runner cannot see when each
	// command starts -- it used to print all of them before any of them ran,
	// and a failure then landed at the end naming nothing.
	let d = Recorder::default();
	let trace = run_src("$ echo one\n$ echo two\n$ echo three\n", &d);
	assert!(trace.is_ok(), "{trace:?}");
}

#[test]
fn a_multi_line_shell_construct_is_left_whole() {
	// A `for` spread over several `$` lines is one command to the shell; a
	// line inserted into the middle of it would cut it in half.
	let d = Recorder::default();
	run_src("$ for f in a b; do\n$ echo $f\n$ done\n", &d).expect("the loop still parses");

	let d = Recorder::default();
	run_src("$ if true; then\n$ echo yes\n$ fi\n", &d).expect("and so does the if");
}

#[test]
fn a_quote_spanning_lines_is_left_whole() {
	let d = Recorder::default();
	run_src("$ x='one\n$ two'\n$ test -n \"$x\"\n", &d).expect("the quote closes on the second line");
}

#[test]
fn a_heredoc_is_left_whole() {
	let d = Recorder::default();
	run_src("$ cat <<'EOF' >/dev/null\n$ body\n$ EOF\n", &d).expect("the heredoc body is not commands");
}

#[test]
fn a_failure_names_the_command_and_never_the_shell() {
	// `$ docker compose up -d` failing used to be reported as
	// "`/usr/bin/bash` exited with status 1", which names an implementation
	// detail and nothing a person can act on.
	let d = Recorder::default();
	let e = run_src("$ false\n", &d).unwrap_err().to_string();
	assert!(e.contains("`false` exited with status 1"), "{e}");
	assert!(!e.contains("bash"), "{e}");

	// Several lines share one shell and `-e` stops at the one that failed,
	// which the runner cannot see -- but each announced itself as it ran.
	let d = Recorder::default();
	let e = run_src("$ true\n$ false\n", &d).unwrap_err().to_string();
	assert!(e.contains("the command above exited with status 1"), "{e}");
	assert!(!e.contains("bash"), "{e}");

	// A block the runner could not take apart is announced whole, so there is
	// no single line above to point at: it names the block instead. Claiming
	// "the command above" here would point at the block's last line, which is
	// not the one that stopped.
	let d = Recorder::default();
	let e = run_src("$ for f in a b; do\n$ test -f nope\n$ done\n", &d)
		.unwrap_err()
		.to_string();
	assert!(e.contains("a command in `for f in a b; do"), "{e}");
	assert!(!e.contains("above"), "{e}");

	// A real command still names itself.
	let d = Recorder::default();
	let e = run_src("exec sh\n\texit 3\nend\n", &d).unwrap_err().to_string();
	assert!(e.contains("exited with status 3"), "{e}");
}
