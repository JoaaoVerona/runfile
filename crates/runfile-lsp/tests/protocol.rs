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
	assert_eq!(caps["documentFormattingProvider"], true, "format-on-save needs this");
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
fn a_line_that_does_nothing_is_underlined_where_it_is() {
	// The point of catching this at parsing: an editor shows it while the file
	// is being written, instead of a run finding it later.
	let out = converse(&[did_open("file:///x/runfiles/a.run", "$ ok\nif true\n\texit\nend\n")]);
	let d = diagnostics(&out[0]);
	assert_eq!(d.len(), 1, "{d:?}");
	assert_eq!(d[0]["range"]["start"]["line"], 2, "the bare word's own line");
	assert!(d[0]["message"].as_str().unwrap().contains("exit()"), "{d:?}");
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

// ---- shellcheck delegation
//
// Skipped where shellcheck is not installed: CI has it, a contributor's machine
// might not, and a missing enhancement must not read as a broken build.

fn have_shellcheck() -> bool {
	std::process::Command::new("shellcheck")
		.arg("--version")
		.stdout(std::process::Stdio::null())
		.stderr(std::process::Stdio::null())
		.status()
		.is_ok_and(|s| s.success())
}

#[test]
fn a_real_shell_mistake_is_reported_on_its_own_line() {
	if !have_shellcheck() {
		eprintln!("skipped: shellcheck not installed");
		return;
	}
	// SC2086: an unquoted expansion of something shellcheck cannot prove safe.
	// Line 3 of the file is line 2 of the script, because the comment between
	// them is transparent to the run -- the case a naive offset gets wrong.
	let src = "$ echo start\n# a note\n$ echo $UNSET_VAR\n";
	let out = converse(&[did_open("file:///x/runfiles/a.run", src)]);
	let d = diagnostics(&out[0]);
	assert_eq!(d.len(), 1, "{d:?}");
	assert_eq!(d[0]["range"]["start"]["line"], 2, "the line the author wrote");
	assert!(d[0]["message"].as_str().unwrap().contains("SC2086"), "{d:?}");
}

#[test]
fn clean_shell_produces_nothing() {
	if !have_shellcheck() {
		return;
	}
	let out = converse(&[did_open("file:///x/runfiles/a.run", "$ x=1\n$ echo \"$x\"\n")]);
	assert!(diagnostics(&out[0]).is_empty(), "{:?}", diagnostics(&out[0]));
}

#[test]
fn an_interpolation_does_not_itself_trip_shellcheck() {
	if !have_shellcheck() {
		return;
	}
	// It resolves to exactly one shell word, so a correct rendering produces no
	// quoting complaint. Leaving the braces in would produce several.
	let out = converse(&[did_open("file:///x/runfiles/a.run", "$ cp {{ ARG.src }} /tmp/\n")]);
	assert!(diagnostics(&out[0]).is_empty(), "{:?}", diagnostics(&out[0]));
}

#[test]
fn a_non_shell_exec_body_is_left_alone() {
	if !have_shellcheck() {
		return;
	}
	// Python that would be nonsense as shell must not be reported as such.
	let src = "exec python3\nx = [1, 2]\nprint(x)\nend\n";
	let out = converse(&[did_open("file:///x/runfiles/a.run", src)]);
	assert!(diagnostics(&out[0]).is_empty(), "{:?}", diagnostics(&out[0]));
}

#[test]
fn a_syntax_error_suppresses_shell_diagnostics() {
	if !have_shellcheck() {
		return;
	}
	// The document does not parse, so the shell text the runner would assemble
	// is not known; reporting on a guess would be noise on top of a real error.
	let src = ".wach = \"y\"\n$ echo $UNSET_VAR\n";
	let out = converse(&[did_open("file:///x/runfiles/a.run", src)]);
	let d = diagnostics(&out[0]);
	assert_eq!(d.len(), 1, "{d:?}");
	assert!(d[0]["message"].as_str().unwrap().contains("watch"), "{d:?}");
}

// ---- hover

#[test]
fn hover_is_advertised_and_answered() {
	let uri = "file:///x/runfiles/a.run";
	let out = converse(&[
		json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
		did_open(uri, ".watch = \"src/**\"\n$ echo {{ RUN.os }}\n"),
		json!({
			"jsonrpc": "2.0", "id": 2, "method": "textDocument/hover",
			"params": {"textDocument": {"uri": uri}, "position": {"line": 0, "character": 3}},
		}),
		json!({
			"jsonrpc": "2.0", "id": 3, "method": "textDocument/hover",
			"params": {"textDocument": {"uri": uri}, "position": {"line": 1, "character": 15}},
		}),
	]);
	assert_eq!(out[0]["result"]["capabilities"]["hoverProvider"], true);
	let property = out[2]["result"]["contents"]["value"].as_str().expect("markdown");
	assert!(property.contains("`.watch`"), "{property}");
	let source = out[3]["result"]["contents"]["value"].as_str().expect("markdown");
	assert!(source.contains("`RUN.os`"), "{source}");
}

#[test]
fn hovering_nothing_in_particular_returns_null() {
	// A hover with no answer must still be answered, or the client waits.
	let uri = "file:///x/runfiles/a.run";
	let out = converse(&[
		did_open(uri, "$ echo plain\n"),
		json!({
			"jsonrpc": "2.0", "id": 5, "method": "textDocument/hover",
			"params": {"textDocument": {"uri": uri}, "position": {"line": 0, "character": 8}},
		}),
	]);
	assert_eq!(out[1]["id"], 5);
	assert!(out[1]["result"].is_null());
}

#[test]
fn completion_carries_a_signature_and_documentation() {
	let uri = "file:///x/runfiles/a.run";
	let out = converse(&[
		did_open(uri, "let x = sub\n"),
		json!({
			"jsonrpc": "2.0", "id": 4, "method": "textDocument/completion",
			"params": {"textDocument": {"uri": uri}, "position": {"line": 0, "character": 11}},
		}),
	]);
	let items = out[1]["result"]["items"].as_array().expect("items");
	let sub = items
		.iter()
		.find(|i| i["label"] == "substring")
		.expect("substring is offered");
	assert!(sub["detail"].as_str().unwrap().starts_with("substring("), "{sub}");
	assert_eq!(sub["documentation"]["kind"], "markdown");
	assert!(!sub["documentation"]["value"].as_str().unwrap().is_empty());
}

fn formatting(uri: &str) -> Value {
	json!({
		"jsonrpc": "2.0", "id": 9, "method": "textDocument/formatting",
		"params": {"textDocument": {"uri": uri}, "options": {"tabSize": 4, "insertSpaces": false}},
	})
}

fn reply_to(out: &[Value], id: i64) -> &Value {
	out.iter().find(|m| m["id"] == id).expect("a reply")
}

#[test]
fn formatting_returns_one_edit_covering_the_whole_document() {
	// Whole-document because sync is whole-document. A minimal diff would be a
	// second description of the same change, and a chance for the two to
	// disagree.
	let uri = "file:///x/runfiles/a.run";
	let out = converse(&[did_open(uri, "let x=1\nif x\n$ a\nend\n"), formatting(uri)]);
	let edits = reply_to(&out, 9)["result"].as_array().expect("edits");
	assert_eq!(edits.len(), 1);
	assert_eq!(edits[0]["range"]["start"]["line"], 0);
	assert_eq!(
		edits[0]["newText"], "let x = 1\n\nif x\n\t$ a\nend\n",
		"the same shape `run :format` produces"
	);
}

#[test]
fn formatting_an_already_formatted_document_edits_nothing() {
	// An editor must not mark a file dirty on every save of a clean one.
	let uri = "file:///x/runfiles/a.run";
	let out = converse(&[did_open(uri, "$ echo hi\n"), formatting(uri)]);
	assert_eq!(reply_to(&out, 9)["result"].as_array().expect("edits").len(), 0);
}

#[test]
fn formatting_a_document_that_does_not_parse_is_answered_with_null() {
	// A file is unfinished for most of the time it is being written, and
	// format-on-save must not put a dialog in the way of that. Still an
	// answer, though: an unanswered request hangs the client.
	let uri = "file:///x/runfiles/a.run";
	let out = converse(&[did_open(uri, "if x\n$ a\n"), formatting(uri)]);
	assert!(reply_to(&out, 9)["result"].is_null());
}
