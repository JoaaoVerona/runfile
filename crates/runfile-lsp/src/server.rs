//! Request dispatch. Synchronous and single-threaded: every operation is a
//! parse of one small file, so there is nothing to gain from concurrency and a
//! good deal of complexity to avoid.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::analysis::{self, Completions, Severity};
use crate::rpc::{ReadError, read_message, write_message};

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
			"textDocument/hover" => vec![reply(id, self.hover(&msg["params"]))],
			"textDocument/definition" => vec![reply(id, self.definition(&msg["params"]))],
			"textDocument/formatting" => vec![reply(id, self.formatting(&msg["params"]))],
			// Anything else: an unanswered request hangs the client, so refuse
			// rather than ignore.
			_ if id.is_some() => vec![reply(id, Value::Null)],
			_ => vec![],
		}
	}

	/// One edit replacing the whole document.
	///
	/// Whole-document because sync is whole-document: a minimal diff would be
	/// a second description of the same change, and a chance for the two to
	/// disagree. A document that does not parse is answered with `null` rather
	/// than an error -- a file is unfinished for most of the time it is being
	/// written, and format-on-save must not put a dialog in the way of that.
	fn formatting(&self, params: &Value) -> Value {
		let uri = params["textDocument"]["uri"].as_str().unwrap_or_default();
		let Some(src) = self.docs.get(uri) else {
			return Value::Null;
		};
		let Ok(out) = runfile_lang::format(src) else {
			return Value::Null;
		};
		if out == *src {
			// No edits at all, so an editor marks nothing dirty.
			return json!([]);
		}
		// An end position past the last line covers the document whatever its
		// line endings are, which is what every server does here.
		json!([{
			"range": {
				"start": {"line": 0, "character": 0},
				"end": {"line": src.lines().count() + 1, "character": 0},
			},
			"newText": out,
		}])
	}

	fn diagnostics_for(&self, uri: &str) -> Value {
		let src = self.docs.get(uri).map(String::as_str).unwrap_or_default();
		let path = uri_to_path(uri);
		let cat = path.as_deref().and_then(|p| self.catalog(p));
		// What `run :lint` says of the file on disk, said of the editor's copy --
		// with every other open document read as the editor has it too.
		let all = crate::document::diagnostics(src, path.as_deref(), cat.as_ref(), &|p| self.text_of(p));
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
		let here = uri_to_path(&uri);
		// Asked only where a name could go, since answering is discovery.
		let shared = || here.as_deref().map(|p| self.shared_bindings(p)).unwrap_or_default();

		let items = match analysis::complete(src, line, col, shared) {
			Completions::Items(items) => items,
			Completions::Targets => here
				.as_deref()
				.and_then(|p| self.catalog(p).map(|cat| crate::document::target_names(&cat, p)))
				.unwrap_or_default()
				.into_iter()
				.map(|n| analysis::Item {
					label: n,
					detail: "target".into(),
					doc: String::new(),
					kind: analysis::Kind::Value,
					rank: 0,
				})
				.collect(),
			Completions::None => Vec::new(),
		};
		json!({
			"isIncomplete": false,
			"items": items
				.iter()
				.map(|i| json!({
					"label": i.label,
					"kind": i.kind.lsp(),
					"detail": i.detail,
					"documentation": {"kind": "markdown", "value": i.doc},
					// Compared only between items the typed prefix fits
					// equally well, which is exactly where it should decide.
					"sortText": format!("{}{}", i.rank, i.label),
				}))
				.collect::<Vec<_>>(),
		})
	}

	/// What the `_shared.run` files above this document bind, outermost first,
	/// so an inner file's binding of a name comes later and wins.
	fn shared_bindings(&self, here: &Path) -> Vec<analysis::Binding> {
		self.shared_chain(here)
			.iter()
			.filter_map(|p| self.text_of(p))
			.flat_map(|src| analysis::shared_bindings(&src))
			.collect()
	}

	/// Every `_shared.run` that applies to this document, outermost first.
	fn shared_chain(&self, here: &Path) -> Vec<PathBuf> {
		self.catalog(here)
			.map(|cat| crate::document::shared_chain(&cat, here))
			.unwrap_or_default()
	}

	/// A file's text as the editor has it when it is open, unsaved edits and
	/// all, and as it is on disk when it is not.
	fn text_of(&self, path: &Path) -> Option<String> {
		match self.docs.get(&path_to_uri(path)) {
			Some(open) => Some(open.clone()),
			None => runfile_discovery::read_runfile(path).ok(),
		}
	}

	/// What the word under the pointer means.
	fn hover(&self, params: &Value) -> Value {
		let uri = uri_of(&params["textDocument"]);
		let line = params["position"]["line"].as_u64().unwrap_or(0) as usize;
		let col = params["position"]["character"].as_u64().unwrap_or(0) as usize;
		let src = self.docs.get(&uri).map(String::as_str).unwrap_or_default();
		let Some(text) = src.lines().nth(line) else {
			return Value::Null;
		};
		// A package's reference: what it is, from the cache, which an editor
		// reads and never fills.
		if let Some(word) = analysis::dispatched_at(text, col)
			&& runfile_lang::reference::is_reference(word)
			&& let Some(here) = uri_to_path(&uri)
		{
			let md = self.package_card(&here, word);
			return json!({"contents": {"kind": "markdown", "value": md}});
		}
		match analysis::hover(text, col) {
			Some(md) => json!({"contents": {"kind": "markdown", "value": md}}),
			None => Value::Null,
		}
	}

	/// `run <target>` jumps to that target's file. Targets are files, so
	/// "definition" is exact rather than a search.
	fn definition(&self, params: &Value) -> Value {
		let uri = uri_of(&params["textDocument"]);
		let no = params["position"]["line"].as_u64().unwrap_or(0) as usize;
		let col = params["position"]["character"].as_u64().unwrap_or(0) as usize;
		let src = self.docs.get(&uri).map(String::as_str).unwrap_or_default();
		let Some(here) = uri_to_path(&uri) else {
			return Value::Null;
		};
		match analysis::definition(src, no, col) {
			Some(analysis::Ref::Here { line, character }) => at(&uri, line, character),
			// A package opens at its `_main.run`, when it is on this machine.
			Some(analysis::Ref::Target(name)) if runfile_lang::reference::is_reference(&name) => self
				.package_entry(&here, &name)
				.map(|p| at(&path_to_uri(&p), 0, 0))
				.unwrap_or(Value::Null),
			Some(analysis::Ref::Target(name)) => self
				.catalog(&here)
				.and_then(|cat| cat.resolve(&name).map(|t| at(&path_to_uri(&t.path), 0, 0)))
				.unwrap_or(Value::Null),
			// A name this file does not bind: look up the `_shared.run` chain,
			// innermost first, which is the order they layer in.
			Some(analysis::Ref::Shared(name)) => self.in_shared(&here, &name),
			None => Value::Null,
		}
	}

	/// The project a document sits in, as `run :lint` sees it: every file on
	/// disk, whatever a machine-wide one names. A scoped file is opened from
	/// outside its scope as a rule, and has a `_shared.run` and siblings all the
	/// same -- see [`runfile_discovery::discover_unscoped`].
	fn catalog(&self, here: &std::path::Path) -> Option<runfile_discovery::Catalog> {
		let dir = here.parent()?;
		runfile_discovery::discover_unscoped(dir, dirs_home().as_deref()).ok()
	}

	/// The `_main.run` a package reference opens, without fetching anything: a
	/// folder relative to the file's anchor, where a line starts unless a
	/// `set_cwd()` moved it, or a repository as the cache last had it.
	fn package_entry(&self, here: &std::path::Path, word: &str) -> Option<std::path::PathBuf> {
		use runfile_lang::reference::{ENTRY, Reference};
		let root = match runfile_lang::reference::parse(word).ok()? {
			Reference::Path(p) => anchor_of(here)?.join(p),
			Reference::Remote(r) => runfile_remote::Cache::open_default()?.cached(&r)?.root,
		};
		let entry = root.join("runfiles").join(format!("{ENTRY}.run"));
		entry.is_file().then_some(entry)
	}

	/// What hovering a package reference says: what the package says it is,
	/// and what the cache holds of it -- or why it can never be fetched.
	fn package_card(&self, here: &std::path::Path, word: &str) -> String {
		use runfile_lang::reference::Reference;
		// Everything here that came from the document or the cache is fenced:
		// the reference can hold a backtick, and a description is whoever wrote
		// the package's (SA-039).
		let heading = analysis::fenced(&format!("run {word}"));
		let reference = match runfile_lang::reference::parse(word) {
			Ok(r) => r,
			Err(e) => return format!("{heading}\n{}", analysis::fenced_text(&e.0)),
		};
		let about = |entry: &std::path::Path| {
			runfile_discovery::read_runfile(entry)
				.ok()
				.and_then(|src| runfile_lang::parse(&src).ok())
				.and_then(|t| t.description)
				.map(|d| analysis::fenced_text(&d))
		};
		// The tag is a name git accepts, which a backtick can be part of.
		match (&reference, self.package_entry(here, word)) {
			(Reference::Remote(r), Some(entry)) => {
				let cache = runfile_remote::Cache::open_default();
				let at = cache
					.and_then(|c| c.cached(r))
					.map(|p| match p.version.as_str() {
						"" => format!("Fetched at {}.", runfile_remote::short(&p.commit)),
						tag => format!(
							"Fetched at {}, tag\n{}",
							runfile_remote::short(&p.commit),
							analysis::fenced_text(tag)
						),
					})
					.unwrap_or_default();
				format!("{heading}\n{}\n\n{at}", about(&entry).unwrap_or_default())
			}
			(Reference::Remote(r), None) => format!(
				"{heading}\nNot fetched on this machine yet. Running it fetches it, and so does\n{}",
				analysis::fenced(&format!("run :packages fetch {}", r.shown()))
			),
			(Reference::Path(_), Some(entry)) => format!("{heading}\n{}", about(&entry).unwrap_or_default()),
			(Reference::Path(_), None) => format!("{heading}\nThere is no `runfiles/_main.run` in that folder."),
		}
	}

	/// Where a `_shared.run` above this file binds `name`.
	///
	/// The one place a reader most needs taking to: a shared binding applies to
	/// every target in its directory and appears nowhere in the file using it.
	fn in_shared(&self, here: &std::path::Path, name: &str) -> Value {
		for path in self.shared_chain(here).into_iter().rev() {
			let Some(src) = self.text_of(&path) else {
				continue;
			};
			if let Some((line, character)) = analysis::binding_in(&src, name, None) {
				return at(&path_to_uri(&path), line, character);
			}
		}
		Value::Null
	}
}

fn capabilities() -> Value {
	json!({
		"capabilities": {
			// Full sync: these files are small, and an incremental applier is
			// a source of drift for no measurable gain.
			"textDocumentSync": 1,
			"completionProvider": {"triggerCharacters": [".", " "]},
			"hoverProvider": true,
			"definitionProvider": true,
			// Format-on-save works through this: an editor asks for edits
			// before writing, and the answer is the same `run :lint`
			// produces, so a file cannot come out of an editor in a shape the
			// CLI would then change.
			"documentFormattingProvider": true,
		},
		"serverInfo": {"name": "runfile-lsp", "version": env!("CARGO_PKG_VERSION")},
	})
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

/// One LSP location, which is all a jump needs.
fn at(uri: &str, line: usize, character: usize) -> Value {
	json!({
		"uri": uri,
		"range": {
			"start": {"line": line, "character": character},
			"end": {"line": line, "character": character},
		},
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

/// The folder holding the `runfiles/` a file is in: its anchor.
fn anchor_of(file: &std::path::Path) -> Option<std::path::PathBuf> {
	file.ancestors()
		.find(|d| d.file_name().is_some_and(|n| n == "runfiles"))
		.and_then(|d| d.parent())
		.map(std::path::Path::to_path_buf)
}
