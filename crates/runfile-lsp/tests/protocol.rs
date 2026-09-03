//! A whole LSP conversation, driven through the real transport.
//!
//! The unit tests cover analysis; this covers the wiring around it -- that a
//! client's messages produce the right replies, in the right order, framed the
//! way a client expects.

use serde_json::{Value, json};

use runfile_lsp::rpc::{read_message, write_message};
use runfile_lsp::server::{Server, path_to_uri};

/// Feed a scripted conversation through the server and collect what it sends.
fn converse(messages: &[Value]) -> Vec<Value> {
	let mut input = Vec::new();
	for m in messages {
		write_message(&mut input, m).unwrap();
	}
	let mut output = Vec::new();
	Server::new()
		.serve(&mut std::io::BufReader::new(&input[..]), &mut output)
		.expect("server ran");

	let mut r = std::io::BufReader::new(&output[..]);
	let mut out = Vec::new();
	while let Ok(v) = read_message(&mut r) {
		out.push(v);
	}
	out
}

fn did_open(uri: &str, text: &str) -> Value {
	json!({
		"jsonrpc": "2.0",
		"method": "textDocument/didOpen",
		"params": {"textDocument": {"uri": uri, "languageId": "runfile", "text": text}},
	})
}

fn diagnostics(v: &Value) -> &Vec<Value> {
	v["params"]["diagnostics"].as_array().expect("diagnostics")
}

#[test]
fn initialize_announces_what_the_server_can_do() {
	let out = converse(&[json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}})]);
	assert_eq!(out.len(), 1);
	assert_eq!(out[0]["id"], 1);
	let caps = &out[0]["result"]["capabilities"];
	assert_eq!(caps["textDocumentSync"], 1, "full sync");
	assert!(caps["completionProvider"].is_object());
	assert_eq!(caps["definitionProvider"], true);
}

#[test]
fn opening_a_clean_file_publishes_an_empty_list() {
	// Publishing an empty list is not the same as publishing nothing: it is
	// what clears the editor's previous errors.
	let out = converse(&[did_open("file:///x/runfiles/a.run", "# ok\n$ echo hi\n")]);
	assert_eq!(out.len(), 1);
	assert_eq!(out[0]["method"], "textDocument/publishDiagnostics");
	assert!(diagnostics(&out[0]).is_empty());
}

#[test]
fn opening_a_broken_file_publishes_the_error_with_its_position() {
	let out = converse(&[did_open("file:///x/runfiles/a.run", "$ ok\n.wach = \"y\"\n")]);
	let d = diagnostics(&out[0]);
	assert_eq!(d.len(), 1, "{d:?}");
	assert_eq!(d[0]["range"]["start"]["line"], 1);
	assert_eq!(d[0]["severity"], 1, "an error, not a hint");
	assert_eq!(d[0]["source"], "runfile");
	assert!(d[0]["message"].as_str().unwrap().contains(".watch"), "{d:?}");
}

#[test]
fn editing_republishes_and_a_fix_clears_the_error() {
	let uri = "file:///x/runfiles/a.run";
	let out = converse(&[
		did_open(uri, ".wach = \"y\"\n$ true\n"),
		json!({
			"jsonrpc": "2.0",
			"method": "textDocument/didChange",
			"params": {
				"textDocument": {"uri": uri},
				"contentChanges": [{"text": ".watch = \"y\"\n$ true\n"}],
			},
		}),
	]);
	assert_eq!(out.len(), 2);
	assert_eq!(diagnostics(&out[0]).len(), 1, "typo reported");
	assert!(diagnostics(&out[1]).is_empty(), "fix clears it");
}

#[test]
fn closing_a_file_clears_its_diagnostics() {
	let uri = "file:///x/runfiles/a.run";
	let out = converse(&[
		did_open(uri, ".wach = \"y\"\n$ true\n"),
		json!({
			"jsonrpc": "2.0",
			"method": "textDocument/didClose",
			"params": {"textDocument": {"uri": uri}},
		}),
	]);
	assert_eq!(out.len(), 2);
	assert!(diagnostics(&out[1]).is_empty(), "stale errors must not linger");
}

#[test]
fn completion_offers_properties_after_a_dot() {
	let uri = "file:///x/runfiles/a.run";
	let out = converse(&[
		did_open(uri, ".wa\n"),
		json!({
			"jsonrpc": "2.0", "id": 7, "method": "textDocument/completion",
			"params": {"textDocument": {"uri": uri}, "position": {"line": 0, "character": 3}},
		}),
	]);
	let items = out[1]["result"]["items"].as_array().unwrap();
	let labels: Vec<&str> = items.iter().map(|i| i["label"].as_str().unwrap()).collect();
	assert!(labels.contains(&"watch"), "{labels:?}");
	assert_eq!(items[0]["kind"], 10, "property kind");
}

#[test]
fn an_unknown_request_is_answered_rather_than_left_hanging() {
	// A request with no reply stalls the client forever.
	let out = converse(&[json!({"jsonrpc": "2.0", "id": 9, "method": "textDocument/wat"})]);
	assert_eq!(out.len(), 1);
	assert_eq!(out[0]["id"], 9);
	assert!(out[0]["result"].is_null());
}

#[test]
fn an_unknown_notification_is_silently_ignored() {
	// No `id` means no reply is expected; answering would be a protocol error.
	assert!(converse(&[json!({"jsonrpc": "2.0", "method": "$/setTrace"})]).is_empty());
}

#[test]
fn shutdown_is_acknowledged_and_exit_ends_the_session() {
	let out = converse(&[
		json!({"jsonrpc": "2.0", "id": 1, "method": "shutdown"}),
		json!({"jsonrpc": "2.0", "method": "exit"}),
		// Never reached: the server stops at `exit`.
		json!({"jsonrpc": "2.0", "id": 2, "method": "initialize", "params": {}}),
	]);
	assert_eq!(out.len(), 1, "only the shutdown reply: {out:?}");
	assert_eq!(out[0]["id"], 1);
}

#[test]
fn a_target_that_exists_on_disk_is_not_reported_missing() {
	let d = tempfile::TempDir::new().unwrap();
	let dir = d.path().join("runfiles");
	std::fs::create_dir_all(&dir).unwrap();
	std::fs::write(dir.join("build.run"), "$ true\n").unwrap();
	let doc = dir.join("all.run");
	std::fs::write(&doc, "run build\n").unwrap();

	let out = converse(&[did_open(&path_to_uri(&doc), "run build\nrun nope\n")]);
	let d = diagnostics(&out[0]);
	assert_eq!(d.len(), 1, "only the missing one: {d:?}");
	assert!(d[0]["message"].as_str().unwrap().contains("nope"), "{d:?}");
}

#[test]
fn go_to_definition_lands_on_the_targets_own_file() {
	let d = tempfile::TempDir::new().unwrap();
	let dir = d.path().join("runfiles");
	std::fs::create_dir_all(&dir).unwrap();
	std::fs::write(dir.join("build.run"), "$ true\n").unwrap();
	let doc = dir.join("all.run");
	std::fs::write(&doc, "run build\n").unwrap();
	let uri = path_to_uri(&doc);

	let out = converse(&[
		did_open(&uri, "run build\n"),
		json!({
			"jsonrpc": "2.0", "id": 3, "method": "textDocument/definition",
			"params": {"textDocument": {"uri": uri}, "position": {"line": 0, "character": 5}},
		}),
	]);
	let target = out[1]["result"]["uri"].as_str().expect("a location");
	assert!(target.ends_with("build.run"), "{target}");
}

#[test]
fn a_uri_with_escapes_round_trips() {
	let p = std::path::Path::new("/tmp/a b/runfiles/x.run");
	let uri = path_to_uri(p);
	assert!(uri.contains("%20"), "{uri}");
	assert_eq!(runfile_lsp::server::uri_to_path(&uri).unwrap(), p);
}
