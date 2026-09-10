//! Everything the server knows about a document, as plain functions.
//!
//! Diagnostics come from the real parser rather than a second, approximate one,
//! so an editor can never disagree with what `run` does. Keeping this layer
//! free of protocol types is what lets it be tested without a client.

use runfile_lang::{InterpPart, KEYWORDS, Statement, lexer};
use runfile_runtime::props::PROPERTIES;

/// A zero-based, half-open range, the way LSP counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
	pub start_line: usize,
	pub start_col: usize,
	pub end_line: usize,
	pub end_col: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
	pub range: Range,
	pub message: String,
	pub severity: Severity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
	Error,
	Warning,
	Information,
	Hint,
}

/// The whole of a line, which is the granularity the parser reports at.
fn whole_line(src: &str, line_1based: usize) -> Range {
	let idx = line_1based.saturating_sub(1);
	let len = src.lines().nth(idx).map(str::len).unwrap_or(0);
	Range {
		start_line: idx,
		start_col: 0,
		end_line: idx,
		end_col: len,
	}
}

/// `line 12: something went wrong` -> `(12, "something went wrong")`.
///
/// The parser's errors already carry the line in their text, so it is read back
/// out rather than duplicated in every variant.
fn split_line_prefix(msg: &str) -> (usize, String) {
	let rest = match msg.strip_prefix("line ") {
		Some(r) => r,
		None => return (1, msg.to_string()),
	};
	match rest.split_once(": ") {
		Some((n, tail)) => match n.parse() {
			Ok(n) => (n, tail.to_string()),
			Err(_) => (1, msg.to_string()),
		},
		None => (1, msg.to_string()),
	}
}

/// Diagnostics for one document.
///
/// `known_targets` is what `run <name>` may refer to; pass an empty slice when
/// the catalog is unknown, and target checks are skipped rather than guessed.
/// `machine_wide` says whether this document sits in the machine-wide
/// directory, which is the one place `.only-in-directories` means anything.
/// Without it an editor would accept a property the runner refuses -- the drift
/// that makes a language server worse than none.
pub fn diagnose(src: &str, known_targets: &[String], machine_wide: bool) -> Vec<Diagnostic> {
	let ast = match runfile_lang::parse(src) {
		Ok(a) => a,
		Err(e) => {
			// A syntax error stops everything: the rest of the file has no
			// reliable structure to check.
			let (line, message) = split_line_prefix(&e.to_string());
			return vec![Diagnostic {
				range: whole_line(src, line),
				message,
				severity: Severity::Error,
			}];
		}
	};

	let mut out = Vec::new();
	check_properties(&ast.body, false, machine_wide, &mut out, src);
	if !known_targets.is_empty() {
		check_target_calls(&ast.body, known_targets, &mut out, src);
	}
	out.sort_by_key(|d| (d.range.start_line, d.range.start_col));
	out
}

fn check_properties(
	block: &runfile_lang::Block,
	nested: bool,
	machine_wide: bool,
	out: &mut Vec<Diagnostic>,
	src: &str,
) {
	// Asked of the runner's own `check`, rather than described a second time
	// here: an editor that accepts a line the runner refuses is how the two
	// come to disagree about what a file means, and this file used to hold its
	// own copy of the scope rule for exactly that reason.
	let regions = block
		.declaration()
		.iter()
		.map(|p| (p, false))
		.chain(block.trailing().iter().map(|p| (p, true)));
	for (p, trailing) in regions {
		let Some(head) = p.path.first() else { continue };
		match runfile_runtime::props::check(p, nested, trailing) {
			Ok(_) => {}
			Err(runfile_runtime::props::PropError::Unknown { .. }) => {
				out.push(Diagnostic {
					range: whole_line(src, p.span.line),
					message: format!("unknown property `.{head}`{}", nearest(head)),
					severity: Severity::Error,
				});
				continue;
			}
			// Every other refusal already reads as a sentence about the line,
			// and carries its own line number, which the range says instead.
			Err(e) => {
				out.push(Diagnostic {
					range: whole_line(src, p.span.line),
					message: strip_line_prefix(&e.to_string()),
					severity: Severity::Error,
				});
				continue;
			}
		}
		if head == runfile_discovery::SCOPE && !machine_wide {
			out.push(Diagnostic {
				range: whole_line(src, p.span.line),
				message: format!("`.{head}` scopes the machine-wide directory; this target is part of the project"),
				severity: Severity::Error,
			});
		}
	}
	for st in &block.statements {
		for inner in sub_blocks(st) {
			check_properties(inner, true, machine_wide, out, src);
		}
	}
}

/// Drop the `line N: ` a `PropError` opens with. The runner prints one line
/// with no other place to say where it happened; a diagnostic has a range.
fn strip_line_prefix(msg: &str) -> String {
	match msg.strip_prefix("line ") {
		Some(rest) => match rest.split_once(": ") {
			Some((n, tail)) if n.chars().all(|c| c.is_ascii_digit()) => tail.to_string(),
			_ => msg.to_string(),
		},
		None => msg.to_string(),
	}
}

fn check_target_calls(block: &runfile_lang::Block, known: &[String], out: &mut Vec<Diagnostic>, src: &str) {
	for st in &block.statements {
		if let Statement::Run { target, span, .. } = st {
			// Only a literal name can be checked; an interpolated one is not
			// known until the target runs.
			if let [InterpPart::Literal(name)] = &target[..]
				&& !known.iter().any(|k| k == name)
			{
				out.push(Diagnostic {
					range: whole_line(src, span.line),
					message: format!("no target named `{name}`"),
					severity: Severity::Error,
				});
			}
		}
		for inner in sub_blocks(st) {
			check_target_calls(inner, known, out, src);
		}
	}
}

fn sub_blocks(st: &Statement) -> Vec<&runfile_lang::Block> {
	match st {
		Statement::If { then, otherwise, .. } => {
			let mut v = vec![then];
			v.extend(otherwise.iter());
			v
		}
		Statement::For { body, .. } | Statement::Loop { body, .. } | Statement::Do { body, .. } => vec![body],
		Statement::Retry { body, otherwise, .. } => {
			let mut v = vec![body];
			v.extend(otherwise.iter());
			v
		}
		Statement::Match { cases, default, .. } => {
			let mut v: Vec<&runfile_lang::Block> = cases.iter().map(|c| &c.body).collect();
			v.extend(default.iter());
			v
		}
		_ => Vec::new(),
	}
}

/// A "did you mean" suffix, or nothing. One edit apart is close enough to be
/// worth suggesting; more than that and a wrong guess is worse than silence.
fn nearest(name: &str) -> String {
	let best = PROPERTIES
		.iter()
		.map(|p| (edits(name, p.name), p.name))
		.filter(|(d, _)| *d <= 2)
		.min_by_key(|(d, _)| *d);
	match best {
		Some((_, n)) => format!("; did you mean `.{n}`?"),
		None => String::new(),
	}
}

fn edits(a: &str, b: &str) -> usize {
	let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
	let mut prev: Vec<usize> = (0..=b.len()).collect();
	let mut cur = vec![0; b.len() + 1];
	for i in 1..=a.len() {
		cur[0] = i;
		for j in 1..=b.len() {
			let sub = prev[j - 1] + usize::from(a[i - 1] != b[j - 1]);
			cur[j] = sub.min(prev[j] + 1).min(cur[j - 1] + 1);
		}
		std::mem::swap(&mut prev, &mut cur);
	}
	prev[b.len()]
}

/// What to offer at `position`. The prefix already typed is matched by the
/// client, so everything applicable is returned.
/// One offered completion: what to insert, and what to show beside it.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Item {
	pub label: String,
	pub detail: String,
	pub doc: String,
}

impl Item {
	fn new(label: &str, detail: &str, doc: &str) -> Self {
		Self {
			label: label.into(),
			detail: detail.into(),
			doc: doc.into(),
		}
	}
}

#[derive(Debug, PartialEq, Eq)]
pub enum Completions {
	Properties(Vec<Item>),
	Functions(Vec<Item>),
	/// The keys of one source, e.g. everything after `RUN.`.
	Sources(Vec<Item>),
	Targets,
	None,
}

/// What `RUN.` offers, and what each one means. The only source with a fixed
/// set of keys: `ARG`, `ENV` and `FLAG` are whatever the caller passed.
pub const RUN_KEYS: &[(&str, &str, &str)] = &[
	(
		"os",
		"`linux`, `mac` or `windows`.",
		"if RUN.os == \"windows\"\n\t$ ./build.ps1\nend",
	),
	(
		"arch",
		"The CPU architecture, normalised.",
		"if RUN.arch == \"x86-64\"\n\tlet image = \"amd64\"\nend",
	),
	(
		"cwd",
		"The directory `run` was invoked from — which is not the anchor. A machine-wide target uses 		 it to act on the project in front of it.",
		".workdir = RUN.cwd",
	),
	(
		"file",
		"This target's own file.",
		"print(\"defined in {{ RUN.file }}\")",
	),
	(
		"parent",
		"The parent of `runfiles/`: the anchor every relative path resolves against — `glob`, 		 `read_file`, `.env-file`, `.add-path` and the working directory.",
		"$ ln -sfn {{ RUN.parent }}/extension ~/.local/share/gnome-shell/extensions/mine",
	),
	(
		"namespaces",
		"The subproject namespaces in this project, as a list — one per `*/runfiles/` directory found 		 below the anchor.",
		"for ns in RUN.namespaces\n\trun {{ ns }}:build\nend",
	),
	(
		"user",
		"The name of the user running this.",
		"$ loginctl show-user {{ RUN.user }} -p Linger",
	),
];

/// The five roots a value can come from.
pub const SOURCES: &[(&str, &str, &str)] = &[
	(
		"ARG",
		"A `--name=value` argument. Only that spelling: `--name value` is a flag plus a positional, 		 because nothing declares which names take a value. Pair it with `?` to give it a default.",
		"let port = ARG.port ? ENV.PORT ? \"3000\"",
	),
	(
		"ENV",
		"An environment variable, including anything an `.env-file` or `.env.NAME` put there.",
		"let home = ENV.HOME\n\n.env-file = \".env.production\"\n$ deploy --token {{ ENV.API_TOKEN }}",
	),
	(
		"FLAG",
		"Whether `--name` was passed, as a bool. A flag needs no value and defaults to false.",
		"if FLAG.apk\n\t$ ./gradlew :app:assembleRelease\nelse\n\t$ ./gradlew :app:bundleRelease\nend",
	),
	(
		"RUN",
		"Context about this run: `os`, `arch`, `cwd`, `file`, `parent`, `namespaces`, `user`.",
		"match RUN.os\n\tcase \"linux\"\n\t\t$ ./install.sh\n\tcase \"windows\"\n\t\t$ ./install.ps1\nend",
	),
	(
		"ARGS",
		"The positional arguments, as a list — everything that was not a `--flag` or `--key=value`, 		 plus everything after a bare `--`.",
		"let part = one_of(first(ARGS), \"major\", \"minor\", \"patch\")\n\n$ cargo test {{ ARGS }}",
	),
];

/// A completion's documentation: the prose, then the same worked example the
/// hover shows. The label and detail already carry the name and signature, so
/// this does not repeat them.
fn with_example(doc: &str, example: &str) -> String {
	if example.is_empty() {
		return doc.into();
	}
	format!("{doc}\n\n```runfile\n{example}\n```")
}

/// Completion depends only on the line so far, which is what makes it usable
/// on a document that does not currently parse.
pub fn complete(line_prefix: &str) -> Completions {
	let t = line_prefix.trim_start();
	// Prose, not code. A comment runs to the end of its line, so a prefix that
	// has already passed the `#` is inside one -- and the runner's own rule is
	// what says which `#` that is, so a `#` in a string or in shell text still
	// completes as the code it is.
	if lexer::comment_at(line_prefix).is_some() {
		return Completions::None;
	}
	if let Some(rest) = t.strip_prefix('.')
		&& !rest.contains('=')
	{
		return Completions::Properties(
			PROPERTIES
				.iter()
				.map(|p| {
					Item::new(
						p.name,
						if p.block_scoped {
							"property"
						} else {
							"property, header-only"
						},
						&with_example(p.doc, p.example),
					)
				})
				.collect(),
		);
	}
	// `RUN.` is the one source whose keys are known ahead of time.
	if t.ends_with("RUN.")
		|| t.rsplit(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
			.next()
			.is_some_and(|w| w.starts_with("RUN."))
	{
		return Completions::Sources(
			RUN_KEYS
				.iter()
				.map(|(k, d, e)| Item::new(k, "run context", &with_example(d, e)))
				.collect(),
		);
	}
	// `run ` wants a target name; `run x ` is already past it.
	if let Some(rest) = t.strip_prefix("run ")
		&& !rest.trim_start().contains(' ')
	{
		return Completions::Targets;
	}
	// A shell line is the shell's business, not the language's.
	if t.starts_with("$ ") || t == "$" {
		return Completions::None;
	}
	// Functions and the source roots share the same position: both are things
	// an expression can start with.
	let mut items: Vec<Item> = runfile_lang::functions::FUNCTIONS
		.iter()
		.map(|f| Item::new(f.name, f.signature, &with_example(f.doc, f.example)))
		.collect();
	items.extend(
		SOURCES
			.iter()
			.map(|(n, d, e)| Item::new(n, "source", &with_example(d, e))),
	);
	Completions::Functions(items)
}

/// What to show when the pointer rests on `col` of `line`.
///
/// Everything a person can hover has a fixed meaning -- a property, a function,
/// a source -- so this reads the word under the cursor rather than the tree,
/// and keeps working while the document does not parse.
/// What ctrl+click on a word should open.
///
/// Split this way so the analysis stays pure: finding the *name* is a question
/// about one document, and finding the *file* it lives in needs discovery,
/// which the server has.
#[derive(Debug, PartialEq, Eq)]
pub enum Ref {
	/// `run <target>`: another target's file.
	Target(String),
	/// A binding in this document, at a zero-based position.
	Here { line: usize, character: usize },
	/// A name this document does not bind. It may come from a `_shared.run`
	/// above it, which is exactly where a reader cannot see it and most wants
	/// to be taken.
	Shared(String),
}

/// Where the word at `col` on line `no` is defined, if anywhere.
///
/// Reads the tree, so a name inside a comment or a string is not mistaken for
/// a use -- and the nearest binding **at or above** the cursor wins, which is
/// what shadowing and rebinding mean when they happen.
pub fn definition(src: &str, no: usize, col: usize) -> Option<Ref> {
	let line = src.lines().nth(no)?;

	// `run <target>` is a jump to a file. Tested by position rather than by
	// word: a namespaced name like `build:release` is two words to anything
	// that treats `:` as a separator, and clicking either half means the same
	// thing. So does clicking the keyword.
	//
	// Asked before `word_at`, because the `:` between the halves is not part
	// of a word at all: reading the word first meant that clicking the one
	// character in the middle of a namespaced target answered nothing.
	if let Some((at, name)) = dispatch_on(line)
		&& at.contains(&col)
	{
		return Some(Ref::Target(name.trim_matches('"').to_string()));
	}
	let word = word_at(line, col)?;
	if !is_name(word) {
		return None;
	}
	match binding_in(src, word, Some(no)) {
		Some((line, character)) => Some(Ref::Here { line, character }),
		// Not bound here: a `_shared.run` above may bind it.
		None => Some(Ref::Shared(word.to_string())),
	}
}

/// The clickable span of a dispatch on this line, and the target it names.
///
/// Both spellings, because they are the same dispatch and clicking either
/// means the same thing: the `run` statement, and the one `code_of` may hold.
/// The span runs from the keyword through the target, so the space between
/// them counts too -- and for a statement it reaches back to column zero,
/// since clicking an indented line's indentation means that line.
fn dispatch_on(line: &str) -> Option<(std::ops::Range<usize>, &str)> {
	let indent = line.len() - line.trim_start().len();
	// A statement is the whole line; a `code_of` holds one anywhere on it,
	// where the `)` closing the call also ends the target.
	let (kw, start, paren) = if line.trim_start().starts_with("run ") {
		(indent, 0, false)
	} else {
		let k = line.find("code_of(run ")? + "code_of(".len();
		(k, k, true)
	};
	let rest = line.get(kw + 3..)?;
	let word = rest.trim_start();
	let end = word
		.find(|c: char| c.is_whitespace() || (paren && c == ')'))
		.unwrap_or(word.len());
	let name = &word[..end];
	let from = kw + 3 + (rest.len() - word.len());
	(!name.is_empty()).then(|| (start..from + name.len(), name))
}

/// Where `name` is bound in this document, as a zero-based position.
///
/// `before` is the line the cursor is on, when there is one: the nearest
/// binding at or above it wins, which is what shadowing and rebinding mean.
/// Without one -- reading a `_shared.run` as a whole -- the last binding wins.
pub fn binding_in(src: &str, name: &str, before: Option<usize>) -> Option<(usize, usize)> {
	let ast = runfile_lang::parse(src).ok()?;
	let ceiling = before.map_or(usize::MAX, |n| n + 1);
	let mut found: Option<usize> = None;
	bindings(&ast.body, name, &mut |at| {
		if at <= ceiling && found.is_none_or(|best| at > best) {
			found = Some(at);
		}
	});
	let at = found?;
	let text = src.lines().nth(at - 1)?;
	Some((at - 1, text.find(name).unwrap_or(0)))
}

/// Every line that binds `name`, reported to `hit` as a one-based line number.
///
/// `let` and `for` both bind; a bare reassignment does not, since the `let` is
/// where the name was introduced and is what a reader is looking for.
fn bindings(b: &runfile_lang::ast::Block, name: &str, hit: &mut impl FnMut(usize)) {
	use runfile_lang::ast::Statement;
	for s in &b.statements {
		match s {
			// A destructuring `let` or `for` binds several names on one line;
			// `_` is a position rather than a name and binds nothing.
			Statement::Let { names, span, .. } if names.iter().any(|n| n == name) => hit(span.line),
			Statement::For { names, body, span, .. } => {
				if names.iter().any(|n| n == name) {
					hit(span.line);
				}
				bindings(body, name, hit);
			}
			Statement::Do { body, .. } | Statement::Loop { body, .. } => bindings(body, name, hit),
			Statement::If { then, otherwise, .. } => {
				bindings(then, name, hit);
				if let Some(o) = otherwise {
					bindings(o, name, hit);
				}
			}
			Statement::Retry { body, otherwise, .. } => {
				bindings(body, name, hit);
				if let Some(o) = otherwise {
					bindings(o, name, hit);
				}
			}
			Statement::Match { cases, default, .. } => {
				for c in cases {
					bindings(&c.body, name, hit);
				}
				if let Some(d) = default {
					bindings(d, name, hit);
				}
			}
			_ => {}
		}
	}
}

fn is_name(word: &str) -> bool {
	!word.is_empty()
		&& word.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
		&& word.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

/// A hover: a heading someone can read as a signature, a sentence or two of
/// prose, and a worked example.
///
/// The example is the part that earns the popup. A signature says the shape of
/// a call and a sentence says its purpose; neither answers "what do I type
/// here", which is what a person hovering a name is usually asking. Fenced as
/// `runfile`, so an editor colours it with the same grammar as the file.
fn card(heading: &str, doc: &str, example: &str, footer: Option<&str>) -> String {
	let mut out = format!("```runfile\n{heading}\n```\n\n{doc}");
	if !example.is_empty() {
		out.push_str(&format!("\n\n**Example**\n\n```runfile\n{example}\n```"));
	}
	if let Some(f) = footer {
		out.push_str(&format!("\n\n---\n\n{f}"));
	}
	out
}

pub fn hover(line: &str, col: usize) -> Option<String> {
	// Anywhere at or past where a comment starts is prose, so the comment is
	// the only thing there is to say about it. Answered first, and by the
	// runner's own rule: a `#` in a string or in shell text does not start one,
	// and nothing written inside one is code -- which is what stops the lookups
	// below from offering the `print` card for the word `print` in a sentence
	// about printing, or the `$` card for a `$` in a remark about a shell line.
	if let Some(at) = lexer::comment_at(line)
		&& byte_at(line, col) >= at
	{
		let k = KEYWORDS.iter().find(|k| k.name == "#")?;
		return Some(card(k.syntax, k.doc, k.example, None));
	}
	// `$` is punctuation rather than a word, so it is found by looking at the
	// character rather than by `word_at`. It is the first thing anyone meets
	// in a runfile and had nothing to say for itself.
	if line.chars().nth(col) == Some('$') && is_shell_marker(line, col) {
		let k = KEYWORDS.iter().find(|k| k.name == "$")?;
		return Some(card(k.syntax, k.doc, k.example, None));
	}
	let word = word_at(line, col)?;
	// `.name`, possibly dotted: `.env.PORT` is the `env` property.
	if let Some(rest) = word.strip_prefix('.') {
		let head = rest.split('.').next().unwrap_or(rest);
		let p = PROPERTIES.iter().find(|p| p.name == head)?;
		let mut notes = vec![if p.block_scoped {
			"Block-scoped: it may also be set inside an `if` / `for` / `match` block, and applies to that block."
		} else {
			"Header-only: it belongs at the top of the file, before any statement."
		}];
		// Only worth saying of a block-scoped one: for a header-only property
		// the line above has already said it.
		if p.declaration_only && p.block_scoped {
			notes.push("It describes the whole block, so it has to be written above the block's first statement.");
		}
		if p.flag {
			notes.push(
				"A flag: write it bare for `= true`, or give it a bool -- or an expression that resolves to one.",
			);
		}
		return Some(card(&format!(".{}", p.name), p.doc, p.example, Some(&notes.join(" "))));
	}
	// `ARG.name`, or a bare source root.
	if let Some((root, key)) = word.split_once('.')
		&& let Some((_, doc, example)) = SOURCES.iter().find(|(n, _, _)| *n == root)
	{
		if root == "RUN"
			&& let Some((k, d, e)) = RUN_KEYS.iter().find(|(k, _, _)| *k == key)
		{
			return Some(card(&format!("RUN.{k}"), d, e, None));
		}
		return Some(card(&format!("{root}.{key}"), doc, example, None));
	}
	if let Some((n, doc, example)) = SOURCES.iter().find(|(n, _, _)| *n == word) {
		return Some(card(n, doc, example, None));
	}
	if let Some(f) = runfile_lang::functions::FUNCTIONS.iter().find(|f| f.name == word) {
		return Some(card(f.signature, f.doc, f.example, None));
	}
	let k = KEYWORDS.iter().find(|k| k.name == word)?;
	Some(card(k.syntax, k.doc, k.example, None))
}

/// The byte offset of character `col`, or the end of the line past it.
fn byte_at(line: &str, col: usize) -> usize {
	line.char_indices().nth(col).map_or(line.len(), |(i, _)| i)
}

/// Whether the `$` at `col` is the shell marker rather than a `$` inside a
/// command -- `$HOME`, `$(date)` and the `$` in a regex are not this one.
///
/// It is the marker when only whitespace precedes it, or when it follows one
/// of the keywords a command may stand behind -- `if`, `match`, and the two
/// loops that ask the same question of one.
fn is_shell_marker(line: &str, col: usize) -> bool {
	let before = line[..byte_at(line, col)].trim_end();
	before.is_empty() || ["if", "match", "while", "until"].iter().any(|k| before.ends_with(k))
}

/// The identifier-ish word around `col`, including a leading `.` and any dots
/// inside it, so `.env.PORT` and `RUN.os` each come back whole.
fn word_at(line: &str, col: usize) -> Option<&str> {
	let part = |c: char| c.is_alphanumeric() || c == '_' || c == '-' || c == '.';
	let chars: Vec<(usize, char)> = line.char_indices().collect();
	if chars.is_empty() {
		return None;
	}
	// Resting just past the end of a word still hovers it.
	let at = col.min(chars.len() - 1);
	if !part(chars[at].1) {
		return None;
	}
	let mut start = at;
	while start > 0 && part(chars[start - 1].1) {
		start -= 1;
	}
	let mut end = at;
	while end + 1 < chars.len() && part(chars[end + 1].1) {
		end += 1;
	}
	let (from, to) = (chars[start].0, chars[end].0 + chars[end].1.len_utf8());
	Some(line[from..to].trim_end_matches('.'))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn messages(src: &str) -> Vec<String> {
		diagnose(src, &[], false).into_iter().map(|d| d.message).collect()
	}

	#[test]
	fn a_clean_file_has_no_diagnostics() {
		assert!(messages("# Builds\n.shell = \"bash\"\n$ echo hi\n").is_empty());
	}

	#[test]
	fn a_flag_given_a_constant_that_is_not_a_bool_is_underlined() {
		// The runner's own rule, asked of the runner: these two used to be
		// described in two places, and the scope rule had already drifted once.
		for src in [".parallel = 23\n$ true\n", ".detach = \"abc\"\n$ true\n"] {
			let m = messages(src);
			assert_eq!(m.len(), 1, "{src}: {m:?}");
			assert!(m[0].contains("is a flag and takes a bool"), "{}", m[0]);
		}
		assert!(
			messages(".parallel = \"true\"\n$ true\n")[0].contains("without the quotes"),
			"the near miss is named"
		);
		// A value the run works out is not the parser's business.
		assert!(messages(".parallel = ENV.CI\n$ true\n").is_empty());
		assert!(messages(".parallel = \"{{ ENV.CI }}\"\n$ true\n").is_empty());
	}

	#[test]
	fn a_property_that_describes_the_block_is_underlined_below_the_first_statement() {
		let m = messages("$ true\n.parallel\n$ true\n");
		assert_eq!(m.len(), 1, "{m:?}");
		assert!(m[0].contains("above the block's first statement"), "{}", m[0]);
		// One that takes effect from where it sits is fine there.
		assert!(messages("$ true\n.workdir = \"web\"\n$ true\n").is_empty());
	}

	#[test]
	fn a_diagnostic_does_not_repeat_the_line_number_the_range_already_carries() {
		let m = messages("if true\n\t.watch = \"x\"\n\t$ true\nend\n");
		assert!(!m[0].starts_with("line "), "{}", m[0]);
	}

	#[test]
	fn only_in_directories_is_refused_in_a_project_file_and_accepted_in_a_machine_wide_one() {
		// An editor that accepts what the runner refuses is worse than none:
		// the file underlines clean and then fails when somebody runs it.
		let src = ".only-in-directories = \"sub\"\n$ true\n";
		let d = diagnose(src, &[], false);
		assert_eq!(d.len(), 1, "{d:?}");
		assert!(d[0].message.contains("machine-wide"), "{}", d[0].message);
		assert!(diagnose(src, &[], true).is_empty(), "the one place it means something");
	}

	#[test]
	fn a_syntax_error_is_reported_on_its_own_line() {
		let d = diagnose("$ echo ok\nlet = 3\n", &[], false);
		assert_eq!(d.len(), 1, "{d:?}");
		assert_eq!(d[0].range.start_line, 1, "zero-based line 1 is the second line");
	}

	#[test]
	fn an_unknown_property_suggests_the_nearest_real_one() {
		let m = messages(".wach = \"src/**\"\n$ true\n");
		assert_eq!(m.len(), 1, "{m:?}");
		assert!(m[0].contains("did you mean `.watch`"), "{}", m[0]);
	}

	#[test]
	fn an_unknown_property_with_no_near_match_just_says_so() {
		let m = messages(".quixotic = \"1\"\n$ true\n");
		assert!(m[0].starts_with("unknown property `.quixotic`"), "{}", m[0]);
		assert!(!m[0].contains("did you mean"), "{}", m[0]);
	}

	#[test]
	fn a_header_only_property_inside_a_block_is_reported() {
		let m = messages("if FLAG.x\n\t.watch = \"src/**\"\n\t$ true\nend\n");
		assert_eq!(m.len(), 1, "{m:?}");
		assert!(m[0].contains("header-only"), "{}", m[0]);
	}

	#[test]
	fn a_block_scoped_property_inside_a_block_is_fine() {
		assert!(messages("if FLAG.x\n\t.shell = \"bash\"\n\t$ true\nend\n").is_empty());
	}

	#[test]
	fn a_call_to_a_missing_target_is_reported() {
		let d = diagnose("run build\n", &["deploy".to_string()], false);
		assert_eq!(d.len(), 1);
		assert!(d[0].message.contains("no target named `build`"), "{:?}", d[0]);
	}

	#[test]
	fn a_call_to_a_known_target_is_fine() {
		assert!(diagnose("run build\n", &["build".to_string()], false).is_empty());
	}

	#[test]
	fn an_interpolated_target_name_is_not_guessed_at() {
		// It is only known at run time, so flagging it would be a false alarm.
		assert!(diagnose("run {{ ENV.NS }}:build\n", &["deploy".to_string()], false).is_empty());
	}

	#[test]
	fn target_calls_are_checked_inside_blocks_too() {
		let src = "for x in [\"a\"]\n\trun nope\nend\n";
		let d = diagnose(src, &["yes".to_string()], false);
		assert_eq!(d.len(), 1, "{d:?}");
		assert_eq!(d[0].range.start_line, 1);
	}

	#[test]
	fn without_a_catalog_target_calls_are_left_alone() {
		assert!(diagnose("run anything\n", &[], false).is_empty());
	}

	#[test]
	fn a_leading_dot_completes_properties() {
		let Completions::Properties(p) = complete("  .wa") else {
			panic!("expected properties")
		};
		let watch = p.iter().find(|i| i.label == "watch").expect("watch is offered");
		assert_eq!(watch.detail, "property, header-only");
		assert!(!watch.doc.is_empty(), "and says what it does");
	}

	#[test]
	fn a_property_that_already_has_a_value_completes_nothing_more() {
		assert!(!matches!(complete(".shell = \"ba"), Completions::Properties(_)));
	}

	#[test]
	fn run_completes_target_names_until_one_is_chosen() {
		assert_eq!(complete("run de"), Completions::Targets);
		assert_ne!(complete("run deploy --x"), Completions::Targets);
	}

	#[test]
	fn a_shell_line_offers_nothing() {
		assert_eq!(complete("$ echo "), Completions::None);
	}

	#[test]
	fn an_expression_offers_functions() {
		let Completions::Functions(f) = complete("let x = to_") else {
			panic!("expected functions")
		};
		let upper = f.iter().find(|i| i.label == "to_upper").expect("to_upper is offered");
		assert_eq!(upper.detail, "to_upper(s)", "the signature is the detail");
		assert!(!upper.doc.is_empty());
	}

	// ---- sources and hover

	#[test]
	fn an_expression_offers_the_source_roots_too() {
		let Completions::Functions(f) = complete("let x = AR") else {
			panic!("expected functions")
		};
		assert!(f.iter().any(|i| i.label == "ARGS"), "a source can start an expression");
		assert!(f.iter().any(|i| i.label == "ARG"));
	}

	#[test]
	fn run_dot_offers_the_keys_it_actually_has() {
		// The one source whose keys are fixed; `ARG` and `ENV` are whatever the
		// caller passed, so there is nothing to offer.
		let Completions::Sources(s) = complete("$ echo {{ RUN.") else {
			panic!("expected sources")
		};
		let labels: Vec<&str> = s.iter().map(|i| i.label.as_str()).collect();
		assert!(labels.contains(&"os"), "{labels:?}");
		assert!(labels.contains(&"namespaces"), "{labels:?}");
		assert!(!labels.contains(&"nonsense"));
	}

	// ---- go to definition

	#[test]
	fn a_binding_is_found_where_it_was_let() {
		let src = "let region = \"eu\"\n\n$ deploy {{ region }}\n";
		assert_eq!(definition(src, 2, 13), Some(Ref::Here { line: 0, character: 4 }));
	}

	#[test]
	fn a_loop_variable_points_at_its_for() {
		let src = "for compose in glob(\"*.yml\")\n\t$ docker compose -f {{ compose }} pull\nend\n";
		assert_eq!(definition(src, 1, 24), Some(Ref::Here { line: 0, character: 4 }));
	}

	#[test]
	fn the_nearest_binding_at_or_above_the_cursor_wins() {
		// A later `let` of the same name is a different binding, and is not
		// what an earlier use refers to.
		let src = "let x = 1\nprint(x)\nlet x = 2\nprint(x)\n";
		assert_eq!(definition(src, 1, 6), Some(Ref::Here { line: 0, character: 4 }));
		assert_eq!(definition(src, 3, 6), Some(Ref::Here { line: 2, character: 4 }));
	}

	#[test]
	fn a_binding_inside_a_block_is_found_from_below_it() {
		let src = "if true\n\tlet inner = 1\n\tprint(inner)\nend\n";
		assert_eq!(definition(src, 2, 8), Some(Ref::Here { line: 1, character: 5 }));
	}

	#[test]
	fn a_name_this_file_does_not_bind_is_looked_for_in_the_shared_chain() {
		// Where a reader most needs taking: a `_shared.run` binding applies to
		// every target in its directory and appears nowhere in the file using
		// it.
		let src = "$ docker run {{ osvImage }}\n";
		assert_eq!(definition(src, 0, 18), Some(Ref::Shared("osvImage".into())));
	}

	#[test]
	fn clicking_a_run_statement_opens_that_target() {
		let src = "run build:release --locked\n";
		// 9 is the `:`, which is no part of a word: reading the word first
		// made the one character in the middle of the name answer nothing.
		for col in [0, 2, 5, 9, 12] {
			assert_eq!(
				definition(src, 0, col),
				Some(Ref::Target("build:release".into())),
				"at {col}"
			);
		}
	}

	#[test]
	fn clicking_a_dispatch_inside_code_of_opens_that_target_too() {
		// `code_of(run x)` is the same dispatch the statement spells, so it is
		// the same jump. The statement's rule is anchored to the start of the
		// line, which this is not.
		let src = "let c = code_of(run build:release)\n";
		//         0123456789...      ^16   ^20
		for col in [16, 18, 20, 25, 30, 32] {
			assert_eq!(
				definition(src, 0, col),
				Some(Ref::Target("build:release".into())),
				"at {col}"
			);
		}
		// The call around it is not the dispatch, and neither is the binding.
		assert_eq!(definition(src, 0, 4), Some(Ref::Here { line: 0, character: 4 }));
		assert_ne!(definition(src, 0, 10), Some(Ref::Target("build:release".into())));
		// With arguments, and as a statement of its own.
		assert_eq!(
			definition("code_of(run web:build --env=prod)\n", 0, 12),
			Some(Ref::Target("web:build".into()))
		);
	}

	#[test]
	fn a_name_in_a_comment_or_a_string_is_not_a_binding() {
		// The tree has no comments in it, and a `let` written inside a string
		// binds nothing.
		let src = "# let region = \"eu\"\nlet hint = \"let region = x\"\n\nprint(region)\n";
		assert_eq!(definition(src, 3, 7), Some(Ref::Shared("region".into())));
	}

	#[test]
	fn definition_on_nothing_in_particular_says_nothing() {
		assert!(definition("$ echo hi\n", 0, 0).is_none(), "punctuation");
		assert!(definition("let x = 1\n", 0, 40).is_none(), "past the end");
		assert!(definition("", 0, 0).is_none(), "an empty document");
	}

	#[test]
	fn binding_in_reads_a_whole_document_when_there_is_no_cursor() {
		// How a `_shared.run` is read: as a file, not relative to a cursor in
		// some other document, so the last binding wins.
		let src = "let a = 1\nlet a = 2\n";
		assert_eq!(binding_in(src, "a", None), Some((1, 4)));
	}

	#[test]
	fn hover_explains_a_property_and_says_where_it_may_go() {
		let h = hover(".watch = \"src/**\"", 3).expect("hovers");
		assert!(h.contains(".watch"), "{h}");
		assert!(h.contains("Header-only"), "{h}");
		let h = hover(".shell = \"bash\"", 3).expect("hovers");
		assert!(h.contains("Block-scoped"), "{h}");
	}

	#[test]
	fn hover_reads_a_dotted_property_as_its_head() {
		// `.env.PORT` is the `env` property with a sub-key.
		let h = hover(".env.PORT = \"3000\"", 6).expect("hovers");
		assert!(h.contains(".env"), "{h}");
	}

	#[test]
	fn hover_explains_a_function_by_its_signature() {
		let h = hover("let x = substring(s, 1)", 12).expect("hovers");
		assert!(h.contains("substring(s, start"), "{h}");
	}

	#[test]
	fn hover_explains_a_source_and_its_key() {
		let h = hover("$ echo {{ RUN.os }}", 15).expect("hovers");
		assert!(h.contains("RUN.os") && h.contains("linux"), "{h}");
		let h = hover("let e = ARG.env", 10).expect("hovers");
		assert!(h.contains("ARG.env") && h.contains("--name=value"), "{h}");
		let h = hover("let a = ARGS", 10).expect("hovers");
		assert!(h.contains("positional"), "{h}");
	}

	#[test]
	fn completion_carries_the_example_too() {
		// The same question, one keystroke earlier.
		let Completions::Functions(items) = complete("let x = to_") else {
			panic!("expected functions")
		};
		let up = items.iter().find(|i| i.label == "to_upper").expect("to_upper");
		assert_eq!(up.detail, "to_upper(s)");
		assert!(up.doc.contains("```runfile"), "{}", up.doc);
		let Completions::Properties(props) = complete(".par") else {
			panic!("expected properties")
		};
		let par = props.iter().find(|i| i.label == "parallel").expect("parallel");
		assert!(par.doc.contains("```runfile"), "{}", par.doc);
	}

	#[test]
	fn every_hover_carries_a_worked_example() {
		// The example is what earns the popup: a signature says the shape of a
		// call and a sentence says its purpose, and neither answers "what do I
		// type here".
		for (src, col) in [
			("let x = substring(s, 1)", 12),
			(".watch = \"src/**\"", 3),
			("$ echo {{ RUN.parent }}", 15),
			("let e = ARG.env", 10),
			("for f in xs", 1),
			("$ echo hi", 0),
		] {
			let h = hover(src, col).unwrap_or_else(|| panic!("{src:?} at {col} hovers nothing"));
			assert!(h.contains("**Example**"), "{src:?}: {h}");
			// Fenced as the language, so an editor colours it like the file.
			assert!(h.matches("```runfile").count() >= 2, "{src:?}: {h}");
		}
	}

	#[test]
	fn the_line_forms_explain_themselves() {
		// `$`, `exec` and `run` are what a person meets first, and had nothing
		// to say for themselves before.
		let h = hover("$ docker build .", 0).expect("hovers the marker");
		assert!(h.contains("still set on the next"), "{h}");
		let h = hover("exec python3", 2).expect("hovers exec");
		assert!(h.contains("stdin"), "{h}");
		let h = hover("run build --env=prod", 1).expect("hovers run");
		assert!(h.contains("in this process"), "{h}");
		for (src, col, want) in [
			("retry 30 every 2", 2, "again while it fails"),
			("retry 30 every 2", 10, "between attempts"),
			("match RUN.os", 2, "quoted string"),
			("\tcase \"linux\"", 2, "quoted string"),
			("\tdefault", 3, "no `case` matched"),
			("let x = 1", 1, "Bind a value"),
			("end", 1, "Closes the nearest open block"),
			("let p = json", 9, "one JSON value"),
			("if code_of($ mkdir x) != 0", 6, "exit status"),
			// A dispatch inside it hovers as both: the call and the keyword.
			("let c = code_of(run test)", 12, "exit status"),
			("let c = code_of(run test)", 18, "in this process"),
		] {
			let h = hover(src, col).unwrap_or_else(|| panic!("{src:?} hovers nothing"));
			assert!(h.contains(want), "{src:?} did not mention {want:?}: {h}");
		}
	}

	#[test]
	fn only_the_shell_marker_hovers_as_one() {
		// A `$` inside a command is the shell's, not ours.
		assert!(
			hover("  $ echo hi", 2).is_some(),
			"leading whitespace is still the marker"
		);
		assert!(hover("if $ test -f x", 3).is_some(), "a condition's marker");
		assert!(hover("match $ curl -fsS url", 6).is_some(), "a subject's marker");
		assert!(hover("$ echo $HOME", 7).is_none(), "a shell variable is not the marker");
		assert!(
			hover("$ d=$(date)", 4).is_none(),
			"a command substitution is not the marker"
		);
	}

	#[test]
	fn hover_on_nothing_in_particular_says_nothing() {
		assert!(hover("$ echo hello", 6).is_none(), "an unknown word");
		assert!(hover("   ", 1).is_none(), "whitespace");
		assert!(hover("", 0).is_none(), "an empty line");
		assert!(hover("let x = 1", 40).is_none(), "past the end");
	}

	#[test]
	fn hover_finds_the_whole_word_from_anywhere_inside_it() {
		for col in 9..=13 {
			assert!(hover("let x = to_upper(s)", col).is_some(), "at {col}");
		}
	}

	#[test]
	fn a_comment_hovers_as_one_and_hides_the_words_inside_it() {
		// `print` written in a sentence about printing is prose. Offering its
		// card there is the same mistake as scanning the text for `ARG.`.
		let h = hover("let x = 1 # call print() first", 19).expect("hovers");
		assert!(h.contains("# <text>"), "{h}");
		let h = hover("let x = 1 # note", 10).expect("hovers the `#` itself");
		assert!(h.contains("# <text>"), "{h}");
		// A `$` inside a remark about one is prose too.
		let h = hover("let x = 1 # like if $ cmd", 20).expect("hovers");
		assert!(h.contains("# <text>"), "{h}");
		// The two regions that own their own `#`.
		let h = hover("$ echo {{ RUN.os }} # a shell comment", 15).expect("hovers");
		assert!(h.contains("RUN.os"), "{h}");
		let h = hover("let x = to_upper(\"#a\")", 12).expect("hovers");
		assert!(h.contains("to_upper"), "{h}");
	}

	#[test]
	fn nothing_completes_inside_a_comment() {
		assert!(matches!(complete("let x = 1 # about pri"), Completions::None));
		assert!(matches!(complete("# a note on con"), Completions::None));
		// Still code: the `#` is the shell's, and the `{{ … }}` is ours.
		assert!(!matches!(complete("$ echo # {{ RUN."), Completions::None));
	}
}
