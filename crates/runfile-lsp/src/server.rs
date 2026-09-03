//! Request dispatch. Synchronous and single-threaded: every operation is a
//! parse of one small file, so there is nothing to gain from concurrency and a
//! good deal of complexity to avoid.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::analysis::{self, Completions, Severity};
use crate::rpc::{ReadError, read_message, write_message};

/// What the `$` shorthand runs, which is what its body must be checked as.
/// The runner prefers bash and falls back to sh; sh is the stricter of the two,
/// so checking against it never lets a real problem through.
const DEFAULT_SHELL: &str = "sh";

/// Looked up on PATH. Absent shellcheck means no shell diagnostics, silently:
/// it is an enhancement, not a requirement.
const SHELLCHECK: &str = "shellcheck";

#[derive(Default)]
pub struct Server {
	/// Open documents, by URI. The client owns the text once a file is open, so
	/// what is on disk is irrelevant until it closes.
	docs: HashMap<String, String>,
	shutdown_requested: bool,
}

/// What a handled message produced: replies and notifications to send back.
type Outgoing = Vec<Value>;

impl Server {
	pub fn new() -> Self {
		Self::default()
	}

	/// Run until the client closes the stream or says goodbye.
	pub fn serve(&mut self, input: &mut impl BufRead, output: &mut impl Write) -> std::io::Result<()> {
		loop {
			let msg = match read_message(input) {
				Ok(m) => m,
				Err(ReadError::Eof) => return Ok(()),
				Err(ReadError::Io(e)) => return Err(e),
				// A malformed message is the client's problem; staying up is
				// better than taking the editor's language support down.
				Err(ReadError::Protocol(_)) => continue,
			};
			for out in self.handle(&msg) {
				write_message(output, &out)?;
			}
			if self.shutdown_requested && msg["method"] == "exit" {
				return Ok(());
			}
		}
	}

	/// Handle one message. Split from `serve` so the whole protocol can be
	/// tested without pipes.
	pub fn handle(&mut self, msg: &Value) -> Outgoing {
		let method = msg["method"].as_str().unwrap_or_default();
		let id = msg.get("id").cloned();
		match method {
			"initialize" => vec![reply(id, capabilities())],
			"initialized" => vec![],
			"shutdown" => {
				self.shutdown_requested = true;
				vec![reply(id, Value::Null)]
			}
			"exit" => vec![],
			"textDocument/didOpen" => {
				let uri = uri_of(&msg["params"]["textDocument"]);
				let text = msg["params"]["textDocument"]["text"].as_str().unwrap_or_default();
				self.docs.insert(uri.clone(), text.to_string());
				vec![self.diagnostics_for(&uri)]
			}
			"textDocument/didChange" => {
				let uri = uri_of(&msg["params"]["textDocument"]);
				// Full sync only -- see `capabilities`.
				if let Some(text) = msg["params"]["contentChanges"]
					.as_array()
					.and_then(|c| c.last())
					.and_then(|c| c["text"].as_str())
				{
					self.docs.insert(uri.clone(), text.to_string());
				}
				vec![self.diagnostics_for(&uri)]
			}
			"textDocument/didClose" => {
				let uri = uri_of(&msg["params"]["textDocument"]);
				self.docs.remove(&uri);
				// Clear what was published, or stale errors linger in the
				// editor's problem list after the file is gone.
				vec![publish(&uri, Vec::new())]
			}
			"textDocument/completion" => vec![reply(id, self.completion(&msg["params"]))],
			"textDocument/definition" => vec![reply(id, self.definition(&msg["params"]))],
			// Anything else: an unanswered request hangs the client, so refuse
			// rather than ignore.
			_ if id.is_some() => vec![reply(id, Value::Null)],
			_ => vec![],
		}
	}

	fn diagnostics_for(&self, uri: &str) -> Value {
		let src = self.docs.get(uri).map(String::as_str).unwrap_or_default();
		let targets = uri_to_path(uri).map(|p| target_names(&p)).unwrap_or_default();
		let mut all = analysis::diagnose(src, &targets);
		// Only when the file itself is sound: shellcheck on a document that does
		// not parse would report against text the runner never assembles.
		if all.is_empty() {
			all.extend(crate::shell::diagnose(src, DEFAULT_SHELL, SHELLCHECK));
		}
		let items: Vec<Value> = all
			.into_iter()
			.map(|d| {
				json!({
					"range": {
						"start": {"line": d.range.start_line, "character": d.range.start_col},
						"end": {"line": d.range.end_line, "character": d.range.end_col},
					},
					"severity": match d.severity {
						Severity::Error => 1,
						Severity::Warning => 2,
						Severity::Information => 3,
						Severity::Hint => 4,
					},
					"source": "runfile",
					"message": d.message,
				})
			})
			.collect();
		publish(uri, items)
	}

	fn completion(&self, params: &Value) -> Value {
		let uri = uri_of(&params["textDocument"]);
		let line = params["position"]["line"].as_u64().unwrap_or(0) as usize;
		let col = params["position"]["character"].as_u64().unwrap_or(0) as usize;
		let src = self.docs.get(&uri).map(String::as_str).unwrap_or_default();
		let prefix = line_prefix(src, line, col);

		// Property = 10, Function = 3, Value = 12, in LSP's CompletionItemKind.
		let (items, kind): (Vec<String>, u8) = match analysis::complete(prefix) {
			Completions::Properties(p) => (p, 10),
			Completions::Functions(f) => (f, 3),
			Completions::Targets => {
				let t = uri_to_path(&uri).map(|p| target_names(&p)).unwrap_or_default();
				(t, 12)
			}
			Completions::None => return json!({"isIncomplete": false, "items": []}),
		};
		json!({
			"isIncomplete": false,
			"items": items.iter().map(|l| json!({"label": l, "kind": kind})).collect::<Vec<_>>(),
		})
	}

	/// `run <target>` jumps to that target's file. Targets are files, so
	/// "definition" is exact rather than a search.
	fn definition(&self, params: &Value) -> Value {
		let uri = uri_of(&params["textDocument"]);
		let line = params["position"]["line"].as_u64().unwrap_or(0) as usize;
		let src = self.docs.get(&uri).map(String::as_str).unwrap_or_default();
		let Some(text) = src.lines().nth(line) else {
			return Value::Null;
		};
		let Some(name) = text.trim().strip_prefix("run ") else {
			return Value::Null;
		};
		let Some(name) = name.split_whitespace().next() else {
			return Value::Null;
		};
		let Some(here) = uri_to_path(&uri) else {
			return Value::Null;
		};
		let Some(dir) = here.parent() else { return Value::Null };
		let Ok(cat) = runfile_discovery::discover(dir, dirs_home().as_deref()) else {
			return Value::Null;
		};
		match cat.resolve(name) {
			Some(t) => json!({
				"uri": path_to_uri(&t.path),
				"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}},
			}),
			None => Value::Null,
		}
	}
}

/// The text of `line` up to `col`, which is all completion needs.
fn line_prefix(src: &str, line: usize, col: usize) -> &str {
	let Some(text) = src.lines().nth(line) else { return "" };
	// `col` counts UTF-16 units in LSP, but clamping to a char boundary is
	// enough here: completion only looks at what kind of line this is.
	let end = text
		.char_indices()
		.map(|(i, _)| i)
		.chain(std::iter::once(text.len()))
		.take(col + 1)
		.last()
		.unwrap_or(0);
	&text[..end]
}

fn capabilities() -> Value {
	json!({
		"capabilities": {
			// Full sync: these files are small, and an incremental applier is
			// a source of drift for no measurable gain.
			"textDocumentSync": 1,
			"completionProvider": {"triggerCharacters": [".", " "]},
			"definitionProvider": true,
		},
		"serverInfo": {"name": "runfile-lsp", "version": env!("CARGO_PKG_VERSION")},
	})
}

fn target_names(doc: &Path) -> Vec<String> {
	let Some(dir) = doc.parent() else { return Vec::new() };
	match runfile_discovery::discover(dir, dirs_home().as_deref()) {
		Ok(c) => c.targets.keys().cloned().collect(),
		Err(_) => Vec::new(),
	}
}

fn dirs_home() -> Option<PathBuf> {
	std::env::var_os("HOME")
		.or_else(|| std::env::var_os("USERPROFILE"))
		.map(PathBuf::from)
}

fn reply(id: Option<Value>, result: Value) -> Value {
	json!({"jsonrpc": "2.0", "id": id.unwrap_or(Value::Null), "result": result})
}

fn publish(uri: &str, diagnostics: Vec<Value>) -> Value {
	json!({
		"jsonrpc": "2.0",
		"method": "textDocument/publishDiagnostics",
		"params": {"uri": uri, "diagnostics": diagnostics},
	})
}

fn uri_of(doc: &Value) -> String {
	doc["uri"].as_str().unwrap_or_default().to_string()
}

/// `file:///a/b%20c.run` -> `/a/b c.run`. Only `file:` is handled; anything
/// else has no path for discovery to walk.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
	let rest = uri.strip_prefix("file://")?;
	let mut out = String::with_capacity(rest.len());
	let mut bytes = rest.bytes();
	while let Some(b) = bytes.next() {
		if b == b'%' {
			let hi = bytes.next()?;
			let lo = bytes.next()?;
			let hex = |c: u8| (c as char).to_digit(16);
			out.push(((hex(hi)? << 4) | hex(lo)?) as u8 as char);
		} else {
			out.push(b as char);
		}
	}
	Some(PathBuf::from(out))
}

pub fn path_to_uri(p: &Path) -> String {
	let mut out = String::from("file://");
	for b in p.to_string_lossy().bytes() {
		match b {
			b'/' | b'-' | b'_' | b'.' | b'~' | b':' => out.push(b as char),
			b if b.is_ascii_alphanumeric() => out.push(b as char),
			b => out.push_str(&format!("%{b:02X}")),
		}
	}
	out
}
