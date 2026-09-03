use super::{Recorder, run_src};

#[test]
fn a_shell_run_is_one_process_so_cd_persists() {
	// The Make wart this design set out to remove.
	let mut d = Recorder::default();
	run_src("$ cd /\n$ pwd > /dev/null\n", &mut d).expect("both lines share a shell");
}

#[test]
fn a_statement_between_shell_lines_starts_a_new_process() {
	let mut d = Recorder::default();
	let trace = run_src("$ cd /\nlet x = 1\n$ pwd > /dev/null\n", &mut d).unwrap();
	assert_eq!(trace.len(), 2, "two separate shells");
}

#[test]
fn exec_pipes_the_body_to_an_arbitrary_command() {
	let dir = std::env::temp_dir().join("runfile-exec-test");
	let _ = std::fs::remove_file(&dir);
	let src = format!("exec tee {}\n\thello\n\tthere\nend\n", dir.display());
	let mut d = Recorder::default();
	run_src(&src, &mut d).expect("tee runs");
	let got = std::fs::read_to_string(&dir).unwrap();
	assert_eq!(got, "hello\nthere", "body reaches the command's stdin, dedented");
	let _ = std::fs::remove_file(&dir);
}

#[test]
fn a_failing_command_stops_the_target() {
	let mut d = Recorder::default();
	let e = run_src("$ exit 3\n$ echo unreachable\n", &mut d).unwrap_err();
	assert!(e.to_string().contains("status 3"), "{e}");
}

#[test]
fn ignore_errors_lets_the_target_continue() {
	let mut d = Recorder::default();
	run_src(".ignore-errors\n$ exit 3\n\nlet x = 1\n$ true\n", &mut d).expect("failure is swallowed");
}

#[test]
fn capture_takes_stdout_with_one_trailing_newline_stripped() {
	let mut d = Recorder::default();
	// echo appends a newline; the capture must not keep it.
	run_src("let v = $ echo hi\n$ test {{ v }} = hi\n", &mut d).expect("trailing newline stripped");
	run_src("let v = $ printf x\n$ test {{ v }} = x\n", &mut d).expect("no newline to strip");
}

#[test]
fn interpolation_quotes_so_a_dollar_cannot_expand() {
	// If the value were pasted raw, $HOME would expand and the test would fail.
	let mut d = Recorder::default();
	run_src("let v = \"$HOME\"\n$ test {{ v }} = '$HOME'\n", &mut d).expect("quoted, not expanded");
}
