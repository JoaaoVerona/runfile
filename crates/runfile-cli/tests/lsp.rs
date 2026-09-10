//! `run :lsp` -- the language server as the shipped binary serves it.
//!
//! `runfile-lsp/tests/protocol.rs` exercises the `Server` type; nothing there
//! would notice a `:lsp` arm that never dispatched, a runner that printed a
//! line of its own onto the stream LSP framing owns, or stdout that never
//! flushed. This lives beside the CLI's other tests because the subcommand is
//! now what ships -- there is no second binary to start.

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use runfile_lsp::rpc::{read_message, write_message};

/// Feed a conversation to the real binary and collect its replies.
///
/// Everything is written and stdin closed before reading, so neither side can
/// block on the other: the server reads until end of file.
fn talk(messages: &[Value]) -> Vec<Value> {
	let mut input = Vec::new();
	for m in messages {
		write_message(&mut input, m).expect("frame");
	}
	let mut child = Command::new(env!("CARGO_BIN_EXE_run"))
		.arg(":lsp")
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::null())
		.spawn()
		.expect("spawn run :lsp");
	child.stdin.take().expect("stdin").write_all(&input).expect("write");
	let out = child.wait_with_output().expect("wait");
	assert!(out.status.success(), "server exited with {}", out.status);

	let mut r = std::io::BufReader::new(&out.stdout[..]);
	let mut replies = Vec::new();
	while let Ok(v) = read_message(&mut r) {
		replies.push(v);
	}
	replies
}

fn did_open(uri: &str, text: &str) -> Value {
	json!({
		"jsonrpc": "2.0",
		"method": "textDocument/didOpen",
		"params": {"textDocument": {"uri": uri, "languageId": "runfile", "text": text}},
	})
}

#[test]
fn run_lsp_answers_initialize_and_reports_a_problem() {
	let replies = talk(&[
		json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
		did_open("file:///x/runfiles/a.run", ".wach = \"y\"\n$ true\n"),
	]);
	assert_eq!(replies.len(), 2, "{replies:?}");
	assert_eq!(replies[0]["id"], 1);
	assert_eq!(replies[0]["result"]["serverInfo"]["name"], "runfile-lsp");

	let diagnostics = replies[1]["params"]["diagnostics"].as_array().expect("diagnostics");
	assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
	assert!(
		diagnostics[0]["message"].as_str().unwrap().contains(".watch"),
		"{diagnostics:?}"
	);
}

#[test]
fn run_lsp_exits_zero_when_the_client_says_goodbye() {
	// An editor closing down must not leave a failing process behind.
	let replies = talk(&[
		json!({"jsonrpc": "2.0", "id": 1, "method": "shutdown"}),
		json!({"jsonrpc": "2.0", "method": "exit"}),
	]);
	assert_eq!(replies.len(), 1);
	assert!(replies[0]["result"].is_null());
}

#[test]
fn run_lsp_puts_nothing_of_its_own_on_stdout() {
	// The risk the subcommand introduces that a separate binary did not have:
	// `run` is a program that talks. An announcement, a `[runfile]` line, a
	// usage banner -- anything ahead of the first header desynchronises the
	// client for the rest of the session, and it would do so in a directory
	// with a project in it, which is every directory an editor opens.
	let dir = tempfile::tempdir().expect("tempdir");
	std::fs::create_dir(dir.path().join("runfiles")).expect("runfiles");
	std::fs::write(dir.path().join("runfiles/build.run"), "$ true\n").expect("target");

	let mut input = Vec::new();
	write_message(&mut input, &json!({"jsonrpc": "2.0", "id": 1, "method": "shutdown"})).expect("frame");
	write_message(&mut input, &json!({"jsonrpc": "2.0", "method": "exit"})).expect("frame");

	let mut child = Command::new(env!("CARGO_BIN_EXE_run"))
		.arg(":lsp")
		.current_dir(dir.path())
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::null())
		.spawn()
		.expect("spawn run :lsp");
	child.stdin.take().expect("stdin").write_all(&input).expect("write");
	let out = child.wait_with_output().expect("wait");

	assert!(
		out.stdout.starts_with(b"Content-Length:"),
		"stdout opens with something other than a frame: {:?}",
		String::from_utf8_lossy(&out.stdout[..out.stdout.len().min(120)])
	);
}
