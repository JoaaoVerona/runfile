//! The compiled `runfile-lsp` binary, driven as a subprocess.
//!
//! `protocol.rs` exercises the `Server` type; nothing there would notice a
//! broken `main.rs`, a renamed binary, or stdout that never flushes. This
//! ships in the release archive, so it is worth starting for real.

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
	let mut child = Command::new(env!("CARGO_BIN_EXE_runfile-lsp"))
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::null())
		.spawn()
		.expect("spawn runfile-lsp");
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
fn the_shipped_binary_answers_initialize_and_reports_a_problem() {
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
fn the_shipped_binary_exits_zero_when_the_client_says_goodbye() {
	// An editor closing down must not leave a failing process behind.
	let replies = talk(&[
		json!({"jsonrpc": "2.0", "id": 1, "method": "shutdown"}),
		json!({"jsonrpc": "2.0", "method": "exit"}),
	]);
	assert_eq!(replies.len(), 1);
	assert!(replies[0]["result"].is_null());
}
