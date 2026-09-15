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

/// What sits above a document: the `_shared.run` files whose `let`s reach it,
/// as far as the server could read them.
#[derive(Debug, Clone, Copy)]
pub enum Chain<'a> {
	/// A target, under these shared files, outermost first.
	Target(&'a [runfile_lang::Target]),
	/// A `_shared.run` itself, under the ones above it.
	Shared(&'a [runfile_lang::Target]),
	/// One of them does not parse, so what they bind is not known -- and a
	/// name the document reads may be one of those. Only calls are checked:
	/// which functions exist is never in doubt.
	Unknown,
}

/// Diagnostics for one document.
///
/// `known_targets` is what `run <name>` may refer to; pass an empty slice when
/// the catalog is unknown, and target checks are skipped rather than guessed.
/// `machine_wide` says whether this document sits in the machine-wide
/// directory, which is the one place `.only-in-directories` means anything.
/// Without it an editor would accept a property the runner refuses -- the drift
/// that makes a language server worse than none. `chain` is what binds the
/// names the document does not bind itself.
pub fn diagnose(src: &str, known_targets: &[String], machine_wide: bool, chain: Chain<'_>) -> Vec<Diagnostic> {
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
	// The runner's own check, asked the way the runner asks it: a name
	// underlined here is a name a run refuses, and one left alone is not.
	let unresolved = match chain {
		Chain::Target(shared) => runfile_lang::resolve::of_chain(&ast, shared),
		Chain::Shared(above) => runfile_lang::resolve::of_shared(&ast, above),
		Chain::Unknown => runfile_lang::resolve::functions(&ast),
	};
	out.extend(unresolved.iter().map(|u| Diagnostic {
		range: name_range(src, u),
		message: strip_line_prefix(&u.to_string()),
		severity: Severity::Error,
	}));
	// The shell in the file, checked as a run checks it -- which is also what
	// knows whether a `_shared.run` above names the shell `$` lines use.
	let above = match chain {
		Chain::Target(files) | Chain::Shared(files) => Some(files),
		Chain::Unknown => None,
	};
	out.extend(
		runfile_shell::check(src, &ast, above)
			.iter()
			.map(|f| shell_diagnostic(src, f)),
	);
	out.sort_by_key(|d| (d.range.start_line, d.range.start_col));
	out
}

/// A shell finding, underlining exactly the text it is about, with what to write
/// instead on a line of its own.
fn shell_diagnostic(src: &str, f: &runfile_shell::Finding) -> Diagnostic {
	let text = Text::new(src);
	let at = |byte: usize| {
		let no = text.line_of(byte);
		let from = byte - text.starts[no - 1];
		let col = text.line(no).get(..from).map_or(0, |l| l.chars().count());
		(no - 1, col)
	};
	let (start_line, start_col) = at(f.span.start);
	let (end_line, end_col) = at(f.span.end);
	let mut message = format!("{} [{}]", f.message, f.rule);
	if let Some(fix) = &f.fix {
		message.push_str("\nfix: ");
		message.push_str(fix);
	}
	Diagnostic {
		range: Range {
			start_line,
			start_col,
			end_line,
			end_col,
		},
		message,
		severity: Severity::Error,
	}
}

/// Where to underline a name that does not resolve: the name, and nothing
/// around it.
///
/// A call or a read is placed exactly by the tree, which the text at that
/// offset spelling the name confirms. A reassignment's names carry no position
/// of their own, so its name is looked for as a whole word on its line. A name
/// found in neither place -- a tree that disagrees with its text -- underlines
/// the line rather than somewhere wrong.
fn name_range(src: &str, u: &runfile_lang::Unresolved) -> Range {
	let text = Text::new(src);
	let start = if src.get(u.span.start..u.span.start + u.name.len()) == Some(u.name.as_str()) {
		Some(u.span.start)
	} else {
		let from = text.starts.get(u.span.line.wrapping_sub(1)).copied();
		from.and_then(|from| word_in(text.line(u.span.line), &u.name).map(|i| from + i))
	};
	let Some(start) = start else {
		return whole_line(src, u.span.line);
	};
	let no = text.line_of(start);
	let line = text.line(no);
	let from = start - text.starts[no - 1];
	let col = |byte: usize| line.get(..byte).map_or(0, |l| l.chars().count());
	Range {
		start_line: no - 1,
		start_col: col(from),
		end_line: no - 1,
		end_col: col(from + u.name.len()),
	}
}

/// Where `word` first appears in `line` as a whole word.
fn word_in(line: &str, word: &str) -> Option<usize> {
	let part = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
	line.match_indices(word).map(|(i, _)| i).find(|&i| {
		!line[..i].chars().next_back().is_some_and(part) && !line[i + word.len()..].chars().next().is_some_and(part)
	})
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
		if head == runfile_discovery::SCOPE {
			let refused = if !machine_wide {
				Some(format!(
					"`.{head}` scopes the machine-wide directory; this target is part of the project"
				))
			} else if runfile_discovery::scope_entries(p).is_none() {
				// Discovery refuses it before anything runs, but only on its way
				// to a target -- and `run :lint` and an editor collect every file
				// whatever its scope, so neither would say it unless it is said
				// here.
				Some(runfile_discovery::UNREADABLE_SCOPE.to_string())
			} else {
				None
			};
			if let Some(message) = refused {
				out.push(Diagnostic {
					range: whole_line(src, p.span.line),
					message,
					severity: Severity::Error,
				});
			}
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

/// What kind of thing a completion is, which is the icon an editor draws beside
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
	Function,
	/// A binding, a loop variable, `ARGS`, a key of `RUN`.
	Variable,
	/// A source whose keys follow a dot: `ARG`, `ENV`, `FLAG`, `RUN`.
	Module,
	Property,
	/// A target name.
	Value,
	Keyword,
}

impl Kind {
	/// LSP's `CompletionItemKind`.
	pub fn lsp(self) -> u8 {
		match self {
			Kind::Function => 3,
			Kind::Variable => 6,
			Kind::Module => 9,
			Kind::Property => 10,
			Kind::Value => 12,
			Kind::Keyword => 14,
		}
	}
}

/// One offered completion: what to insert, and what to show beside it.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Item {
	pub label: String,
	pub detail: String,
	pub doc: String,
	pub kind: Kind,
	/// Where it sorts among matches that are otherwise as good, lowest first.
	/// An editor ranks by how well the typed prefix fits and only then by
	/// this, so it decides what comes first when `re` fits `region` and
	/// `read_file` equally well -- the name bound three lines up.
	pub rank: u8,
}

impl Item {
	fn new(label: &str, detail: &str, doc: &str, kind: Kind, rank: u8) -> Self {
		Self {
			label: label.into(),
			detail: detail.into(),
			doc: doc.into(),
			kind,
			rank,
		}
	}
}

/// What to offer at a position. The prefix already typed is matched by the
/// client, so everything applicable is returned.
#[derive(Debug, PartialEq, Eq)]
pub enum Completions {
	Items(Vec<Item>),
	/// Target names, which only discovery can list.
	Targets,
	None,
}

/// How a name came to be bound, which is what an editor says beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binder {
	Let,
	For,
	/// A top-level `let` in a `_shared.run` above the document.
	Shared,
}

/// A name completion may offer, and the line that binds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
	pub name: String,
	/// That line as written, shown with the name so a reader sees what it holds.
	pub line: String,
	pub binder: Binder,
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

/// What may be written at zero-based line `no`, character `col`.
///
/// The line so far decides what *kind* of thing goes there -- a property after
/// a leading `.`, a target after `run `, nothing in shell text -- and the tree
/// decides which names are in scope and which blocks are open around it. The
/// document is usually mid-edit and not parsing, so the tree is of a repaired
/// copy; see [`parse_around`].
///
/// `shared` lists what the `_shared.run` files above the document bind, and is
/// only called where a name could go: answering it means discovery.
pub fn complete(src: &str, no: usize, col: usize, shared: impl FnOnce() -> Vec<Binding>) -> Completions {
	let line = src.lines().nth(no).unwrap_or("");
	let prefix = &line[..byte_at(line, col)];
	let body = bodies(src).into_iter().find(|b| b.lines.contains(&no));
	let foreign = body.is_some();
	let here = Place::of(src, no + 1, body.as_ref().map(|b| &b.whole));
	// Prose, not code. A comment runs to the end of its line, so a prefix that
	// has already passed the `#` is inside one -- and the runner's own rule is
	// what says which `#` that is, so a `#` in a string or in shell text still
	// completes as the code it is.
	if !foreign && lexer::comment_at(prefix).is_some() {
		return Completions::None;
	}
	match region(prefix, foreign) {
		Region::Text => Completions::None,
		Region::Interp(at) => expression(&prefix[at..], here.as_ref(), shared, false),
		Region::Code => code(prefix.trim_start(), here.as_ref(), shared),
	}
}

/// Keywords that are written after something rather than at the start of a
/// line, and so are offered only there: `in` after a loop's names, `every`
/// after a retry's count, `json` as a value, and `$` and `#`, which are
/// characters. `code_of` is a call, and goes wherever a function does.
const NOT_OPENERS: &[&str] = &["$", "#", "in", "every", "json", "code_of"];

/// A line of this language, up to the cursor, with its indentation dropped.
fn code(t: &str, here: Option<&Place>, shared: impl FnOnce() -> Vec<Binding>) -> Completions {
	// `.name` is a property until its `=`, and a value after it. A dotted name
	// addresses a sub-key -- `.env.PORT` -- whose names are the environment's.
	if let Some(rest) = t.strip_prefix('.') {
		return match rest.split_once('=') {
			Some((_, value)) => expression(value, here, shared, false),
			None if rest.contains(|c: char| c == '.' || c.is_whitespace()) => Completions::None,
			None => Completions::Items(properties()),
		};
	}
	// `run ` wants a target name; `run x ` is already past it, and what follows
	// is words handed over as written.
	if let Some(rest) = t.strip_prefix("run ") {
		return match rest.trim_start().contains(' ') {
			true => Completions::None,
			false => Completions::Targets,
		};
	}
	let (before, _) = split_word(t);
	let before = before.trim_end();
	if before.is_empty() {
		return statement(here, shared);
	}
	let spaced = format!("{before} ");
	match before.split_whitespace().next().unwrap_or_default() {
		// A name being introduced, which nothing here can guess.
		"let" if !before.contains('=') => Completions::None,
		"for" if before == "for" => Completions::None,
		"for" if !spaced.contains(" in ") => keywords(&["in"]),
		"retry" if before != "retry" && !spaced.contains(" every ") => keywords(&["every"]),
		"else" if before == "else" => keywords(&["if"]),
		"detach" if before == "detach" => keywords(&["exec"]),
		// A label is a quoted string, and `default` takes nothing.
		"case" | "default" => Completions::None,
		_ => expression(t, here, shared, opens_value(before)),
	}
}

/// The start of a line: a keyword, a call, or a name being rebound.
fn statement(here: Option<&Place>, shared: impl FnOnce() -> Vec<Binding>) -> Completions {
	let mut items: Vec<Item> = KEYWORDS
		.iter()
		.filter(|k| !NOT_OPENERS.contains(&k.name) && fits(k.name, here))
		.map(|k| keyword_item(k, Kind::Keyword, 0))
		.collect();
	items.extend(in_scope(here, shared, 1));
	items.extend(functions(2));
	Completions::Items(items)
}

/// Somewhere a value can start -- or, straight after a source's dot, what that
/// source holds. `opening` is the first word of a `let` or a reassignment's
/// value, the one place a `json` block or an `exec` capture may begin.
fn expression(text: &str, here: Option<&Place>, shared: impl FnOnce() -> Vec<Binding>, opening: bool) -> Completions {
	// The dispatch `code_of` may hold wants a target name, as `run ` does.
	if let Some((_, target)) = text.rsplit_once("code_of(run ")
		&& !target.contains([' ', ')'])
	{
		return Completions::Targets;
	}
	let word = text
		.rsplit(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-' || c == '.'))
		.next()
		.unwrap_or_default();
	// `RUN.` is the one source whose keys are known ahead of time. `ARG.` and
	// the rest hold whatever the caller passed, and nothing else has members.
	if word.starts_with("RUN.") {
		return Completions::Items(
			RUN_KEYS
				.iter()
				.map(|(k, d, e)| Item::new(k, "run context", &with_example(d, e), Kind::Variable, 0))
				.collect(),
		);
	}
	if word.contains('.') {
		return Completions::None;
	}
	let mut items = in_scope(here, shared, 0);
	if opening {
		items.extend(
			KEYWORDS
				.iter()
				.filter(|k| k.name == "json" || k.name == "exec")
				.map(|k| keyword_item(k, Kind::Keyword, 1)),
		);
	}
	items.extend(functions(2));
	items.extend(SOURCES.iter().map(|(n, d, e)| {
		// `ARGS` is one value; the rest are read through a dot.
		let kind = if *n == "ARGS" { Kind::Variable } else { Kind::Module };
		Item::new(n, "source", &with_example(d, e), kind, 2)
	}));
	Completions::Items(items)
}

fn properties() -> Vec<Item> {
	PROPERTIES
		.iter()
		.map(|p| {
			let detail = match p.block_scoped {
				true => "property",
				false => "property, header-only",
			};
			Item::new(p.name, detail, &with_example(p.doc, p.example), Kind::Property, 0)
		})
		.collect()
}

/// The library, and `code_of` with it: a keyword in that it may hold a `$` or a
/// `run`, and a call in every way someone writing one can see.
fn functions(rank: u8) -> impl Iterator<Item = Item> {
	runfile_lang::functions::FUNCTIONS
		.iter()
		.map(move |f| {
			Item::new(
				f.name,
				f.signature,
				&with_example(f.doc, f.example),
				Kind::Function,
				rank,
			)
		})
		.chain(
			KEYWORDS
				.iter()
				.filter(|k| k.name == "code_of")
				.map(move |k| keyword_item(k, Kind::Function, rank)),
		)
}

fn keyword_item(k: &runfile_lang::Keyword, kind: Kind, rank: u8) -> Item {
	Item::new(k.name, k.syntax, &with_example(k.doc, k.example), kind, rank)
}

/// Only these keywords, where nothing else can follow.
fn keywords(names: &[&str]) -> Completions {
	Completions::Items(
		KEYWORDS
			.iter()
			.filter(|k| names.contains(&k.name))
			.map(|k| keyword_item(k, Kind::Keyword, 0))
			.collect(),
	)
}

/// Every name in scope at the cursor: this document's, then the `_shared.run`
/// chain's, which a binding here shadows as it would at run time. The nearest
/// binding of a name is the one its value comes from, so each list is read
/// from the bottom up and only the first of a name is kept.
fn in_scope(here: Option<&Place>, shared: impl FnOnce() -> Vec<Binding>, rank: u8) -> Vec<Item> {
	let shared = shared();
	let local = here.map_or(&[][..], |p| &p.bound[..]);
	let mut seen = std::collections::HashSet::new();
	local
		.iter()
		.rev()
		.chain(shared.iter().rev())
		.filter(|b| seen.insert(b.name.as_str()))
		.map(|b| {
			let detail = match b.binder {
				Binder::Let => "binding",
				Binder::For => "loop variable",
				Binder::Shared => "binding from _shared.run",
			};
			Item::new(
				&b.name,
				detail,
				&format!("```runfile\n{}\n```", b.line),
				Kind::Variable,
				rank,
			)
		})
		.collect()
}

/// Whether a keyword may open the line the cursor is on, as far as the blocks
/// around it say. Without a tree to ask, every one may.
fn fits(keyword: &str, here: Option<&Place>) -> bool {
	let Some(p) = here else {
		return true;
	};
	let innermost = p.blocks.last();
	match keyword {
		"else" => matches!(innermost, Some(Opener::If | Opener::Retry)),
		"case" | "default" => innermost == Some(&Opener::Match),
		"break" | "continue" => p.blocks.contains(&Opener::Loop),
		"end" => innermost.is_some(),
		_ => true,
	}
}

/// The line split before the word being typed, which is the part a
/// completion replaces.
fn split_word(t: &str) -> (&str, &str) {
	let at = t
		.trim_end_matches(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-')
		.len();
	t.split_at(at)
}

/// Whether `before` ends where the value of a `let` or a reassignment begins.
fn opens_value(before: &str) -> bool {
	before
		.strip_suffix('=')
		.is_some_and(|b| !b.ends_with(['=', '!', '<', '>']))
}

/// Where the end of a line's prefix sits.
#[derive(Debug, PartialEq, Eq)]
enum Region {
	/// This language, outside any string.
	Code,
	/// Inside a `{{ … }}`, whose expression starts at this byte.
	Interp(usize),
	/// A string, a shell command, an `exec` body: text that is not ours to
	/// complete.
	Text,
}

/// Which region the end of `prefix` is in.
///
/// Scanned the way the lexer scans: a string skips `{{ … }}` whole, so a quote
/// inside one does not end it, and a backslash keeps a quote from closing it.
/// A `$` hands the rest of the line to a shell, and so does the command after
/// `exec`; in either, only an interpolation is ours again. `foreign` starts the
/// line in somebody else's text, which is what a body line is.
fn region(prefix: &str, foreign: bool) -> Region {
	#[derive(Clone, Copy, PartialEq)]
	enum In {
		Code,
		Str,
		Text,
		Interp,
	}
	let b = prefix.as_bytes();
	let mut open = vec![(if foreign { In::Text } else { In::Code }, 0)];
	let mut i = 0;
	while i < b.len() {
		let top = open.last().map_or(In::Code, |o| o.0);
		let rest = &b[i..];
		match top {
			In::Interp if rest.starts_with(b"}}") => {
				open.pop();
				i += 2;
			}
			In::Str | In::Text if rest.starts_with(b"\\{{") => i += 3,
			In::Str if b[i] == b'\\' => i += 2,
			_ if rest.starts_with(b"{{") => {
				open.push((In::Interp, i + 2));
				i += 2;
			}
			In::Str if b[i] == b'"' => {
				open.pop();
				i += 1;
			}
			In::Code | In::Interp if b[i] == b'"' => {
				open.push((In::Str, i));
				i += 1;
			}
			In::Code if b[i] == b'$' || starts_exec(prefix, i) => {
				open.push((In::Text, i));
				i += 1;
			}
			_ => i += 1,
		}
	}
	match open.last() {
		Some((In::Interp, at)) => Region::Interp(*at),
		Some((In::Code, _)) | None => Region::Code,
		Some(_) => Region::Text,
	}
}

/// Whether the `exec` keyword starts at byte `i`: first on its line, after a
/// `detach`, or as a value's first word. What follows it is a command line.
fn starts_exec(prefix: &str, i: usize) -> bool {
	if !prefix.as_bytes()[i..].starts_with(b"exec ") {
		return false;
	}
	// `e` is ASCII, so `i` is a character boundary.
	let before = prefix[..i].trim();
	before.is_empty() || before == "detach" || opens_value(before)
}

/// A block a line can sit in, as far as the keywords that may open that line
/// are concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opener {
	If,
	Retry,
	/// `for`, `while`, `until` and `loop`: what `break` and `continue` need.
	Loop,
	Match,
	Do,
}

/// What the tree says about one line of a document.
struct Place {
	/// Names bound above the line and still in scope on it, in source order.
	bound: Vec<Binding>,
	/// The blocks open around the line, outermost first.
	blocks: Vec<Opener>,
}

impl Place {
	/// `line` is one-based, the way the tree counts. `aside` is a statement to
	/// leave out, zero-based -- the body the line is in, if it is in one.
	fn of(src: &str, line: usize, aside: Option<&std::ops::Range<usize>>) -> Option<Place> {
		let (tree, text) = parse_around(src, line, aside)?;
		let mut p = Place::empty();
		p.walk(&tree.body, &Text::new(&text), line);
		Some(p)
	}

	fn empty() -> Place {
		Place {
			bound: Vec::new(),
			blocks: Vec::new(),
		}
	}

	fn walk(&mut self, block: &runfile_lang::Block, text: &Text, at: usize) {
		for st in &block.statements {
			let span = st.span();
			let (first, last) = (span.line, text.line_of(span.end));
			// Strictly between the line that opens a block and the one that
			// closes it.
			let inside = first < at && at < last;
			match st {
				// Bound once the statement is over, so a name is not offered
				// inside its own value. A reassignment introduces nothing.
				Statement::Let { names, .. } => {
					if last < at {
						self.bind(names, Binder::Let, text.line(first));
					}
				}
				// The runner puts a loop's names back as it leaves, so they are
				// in scope only inside it. A `let` is not: it outlives its block.
				Statement::For { names, body, .. } => {
					if inside {
						self.bind(names, Binder::For, text.line(first));
						self.blocks.push(Opener::Loop);
					}
					self.walk(body, text, at);
				}
				Statement::Loop { body, .. } | Statement::Do { body, .. } => {
					if inside {
						self.blocks.push(match st {
							Statement::Do { .. } => Opener::Do,
							_ => Opener::Loop,
						});
					}
					self.walk(body, text, at);
				}
				Statement::If { then, otherwise, .. }
				| Statement::Retry {
					body: then, otherwise, ..
				} => {
					if inside {
						self.blocks.push(match st {
							Statement::If { .. } => Opener::If,
							_ => Opener::Retry,
						});
					}
					self.walk(then, text, at);
					if let Some(o) = otherwise {
						self.walk(o, text, at);
					}
				}
				Statement::Match { cases, default, .. } => {
					if inside {
						self.blocks.push(Opener::Match);
					}
					for c in cases {
						self.walk(&c.body, text, at);
					}
					if let Some(d) = default {
						self.walk(d, text, at);
					}
				}
				_ => {}
			}
		}
	}

	fn bind(&mut self, names: &[String], binder: Binder, line: &str) {
		// `_` is a position thrown away, not a name.
		for n in names.iter().filter(|n| *n != "_") {
			self.bound.push(Binding {
				name: n.clone(),
				line: line.trim().to_string(),
				binder,
			});
		}
	}
}

/// A parsed text, for turning the tree's byte offsets back into lines.
struct Text<'a> {
	lines: Vec<&'a str>,
	/// The byte each line starts at.
	starts: Vec<usize>,
}

impl<'a> Text<'a> {
	fn new(text: &'a str) -> Self {
		Self {
			lines: text.split('\n').collect(),
			starts: std::iter::once(0)
				.chain(text.match_indices('\n').map(|(i, _)| i + 1))
				.collect(),
		}
	}

	/// The one-based line byte `at` falls on.
	fn line_of(&self, at: usize) -> usize {
		self.starts.partition_point(|&s| s <= at)
	}

	/// One-based line `n`.
	fn line(&self, n: usize) -> &'a str {
		self.lines.get(n.wrapping_sub(1)).copied().unwrap_or_default()
	}
}

/// A tree for a document that is usually in the middle of being written.
///
/// The line being typed is what most often stops a file parsing -- `print(re`
/// is not a statement yet -- and a block opened a moment ago has no `end` yet.
/// So the line the parser stops at is set aside and the parse tried again, as
/// often as it takes, and a block still open at the end is given up to three
/// `end`s. Every parse error names its line, which is what makes the repair
/// exact rather than a guess at what went wrong.
///
/// A line inside a body sets aside the whole statement up front: a `json`
/// block is checked as it is read and is not valid JSON halfway through a
/// line, and an `exec` with no `end` yet closes only on one at its own
/// indentation.
///
/// Blanking keeps every other line where it was, so a position in the tree is
/// still a position in the document. The text that parsed comes back with the
/// tree, since the tree's byte offsets index it rather than the document.
fn parse_around(
	src: &str,
	line: usize,
	aside: Option<&std::ops::Range<usize>>,
) -> Option<(runfile_lang::Target, String)> {
	let mut lines: Vec<&str> = src.split('\n').collect();
	for i in aside.cloned().unwrap_or_default() {
		lines[i] = "";
	}
	// Bounded, because a file broken on every line has nothing to offer.
	for _ in 0..16 {
		let mut text = lines.join("\n");
		// Every line up to the cursor's is whole, so an `end` added below
		// lands after it rather than on it.
		if !text.ends_with('\n') {
			text.push('\n');
		}
		while text.matches('\n').count() < line {
			text.push('\n');
		}
		let mut stopped = None;
		for closers in 0..=3 {
			match runfile_lang::parse(&text) {
				Ok(tree) => return Some((tree, text)),
				Err(e) if closers == 0 => stopped = error_line(&e),
				Err(_) => {}
			}
			text.push_str("end\n");
		}
		match lines.get_mut(stopped?.checked_sub(1)?) {
			Some(l) if !l.trim().is_empty() => *l = "",
			// Blank already, or one of the lines added: nothing left to set aside.
			_ => return None,
		}
	}
	None
}

/// The one-based line a parse error names. Every one names one -- `line N:`
/// opens both kinds -- but a message is text, so this does not insist.
fn error_line(e: &runfile_lang::ParseError) -> Option<usize> {
	let msg = e.to_string();
	msg.strip_prefix("line ")?.split_once(':')?.0.parse().ok()
}

/// A run of lines in somebody else's language, and the statement it is part of.
struct Body {
	/// Its own lines, zero-based: an `exec` body, a structured block's
	/// contents, or the lines a `$` line's backslash carries it on to.
	lines: std::ops::Range<usize>,
	/// The whole statement -- opener, body and `end` -- which is what is set
	/// aside when the cursor is in the body.
	whole: std::ops::Range<usize>,
}

/// Every body in a document, found a line at a time the way the parser finds
/// them: a `$` line continues while it ends in a backslash, and an `exec` or a
/// structured block closes on an `end` at its opener's own indentation, which
/// is the parser's own [`closes_body`](runfile_lang::parser::closes_body).
///
/// Read from the lines rather than the tree, because a body is where a
/// document spends most of its time not parsing.
fn bodies(src: &str) -> Vec<Body> {
	let raw: Vec<&str> = src.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
	let mut out = Vec::new();
	let mut i = 0;
	while i < raw.len() {
		let start = i;
		let trimmed = raw[i].trim();
		let lead = trimmed.strip_prefix("detach ").map_or(trimmed, str::trim_start);
		if lead == "$" || lead.starts_with("$ ") {
			let mut last = lead;
			while last.ends_with('\\') && i + 1 < raw.len() {
				i += 1;
				last = raw[i];
			}
			if i > start {
				out.push(Body {
					lines: start + 1..i + 1,
					whole: start..i + 1,
				});
			}
		} else if !trimmed.starts_with('#') && opens_body(lexer::code(lead)) {
			let indent = &raw[i][..raw[i].len() - raw[i].trim_start().len()];
			let mut end = i + 1;
			while end < raw.len() && !runfile_lang::parser::closes_body(raw[end], indent) {
				end += 1;
			}
			out.push(Body {
				lines: start + 1..end,
				whole: start..(end + 1).min(raw.len()),
			});
			i = end;
		}
		i += 1;
	}
	out
}

/// Whether a line opens a body: `exec <command>`, or a binding whose value is
/// an `exec` capture or a structured block.
fn opens_body(code: &str) -> bool {
	if code.starts_with("exec ") {
		return true;
	}
	let Some((lhs, rhs)) = code.split_once('=') else {
		return false;
	};
	let lhs = lhs.trim();
	let names = lhs.strip_prefix("let ").unwrap_or(lhs);
	let rhs = rhs.trim();
	names.split(',').all(|n| is_name(n.trim()))
		&& (rhs.starts_with("exec ") || runfile_lang::Structured::from_keyword(rhs).is_some())
}

/// What a `_shared.run` binds for the targets below it: its top-level `let`s,
/// in order. Only those -- the rest of a shared file's statements never run,
/// so a `let` inside an `if` there binds nothing.
pub fn shared_bindings(src: &str) -> Vec<Binding> {
	let Ok(tree) = runfile_lang::parse(src) else {
		return Vec::new();
	};
	let text = Text::new(src);
	let mut p = Place::empty();
	for st in &tree.body.statements {
		if let Statement::Let { names, span, .. } = st {
			p.bind(names, Binder::Shared, text.line(span.line));
		}
	}
	p.bound
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
		plain(src).into_iter().map(|d| d.message).collect()
	}

	/// Diagnostics for a target with nothing above it.
	fn plain(src: &str) -> Vec<Diagnostic> {
		diagnose(src, &[], false, Chain::Target(&[]))
	}

	#[test]
	fn the_parallel_property_is_underlined_with_where_it_went() {
		let m = messages(".parallel\n$ true\n");
		assert_eq!(m.len(), 1, "{m:?}");
		assert!(m[0].contains("`parallel do"), "the message is the fix: {}", m[0]);
	}

	#[test]
	fn a_parallel_block_s_rules_are_underlined_where_they_are_broken() {
		// The parser's, so an editor says so while the block is being written.
		for (src, says) in [
			("parallel do\n\tlet x = 1\nend\n", "binds nothing"),
			(
				"let n = 0\nparallel for i in [1]\n\tn = i\nend\n",
				"bound outside this parallel branch",
			),
			("parallel for i in [1]\n\tbreak\nend\n", "cannot stop a `parallel for`"),
		] {
			let m = messages(src);
			assert!(m.iter().any(|m| m.contains(says)), "{src}: {m:?}");
		}
		assert!(messages("parallel do\n\t$ true\n\t$ true\nend\n").is_empty());
	}

	#[test]
	fn a_clean_file_has_no_diagnostics() {
		assert!(messages("# Builds\n.shell = \"bash\"\n$ echo hi\n").is_empty());
	}

	#[test]
	fn a_flag_given_a_constant_that_is_not_a_bool_is_underlined() {
		// The runner's own rule, asked of the runner: these two used to be
		// described in two places, and the scope rule had already drifted once.
		for src in [".ignore-errors = 23\n$ true\n", ".logging = \"abc\"\n$ true\n"] {
			let m = messages(src);
			assert_eq!(m.len(), 1, "{src}: {m:?}");
			assert!(m[0].contains("is a flag and takes a bool"), "{}", m[0]);
		}
		assert!(
			messages(".ignore-errors = \"true\"\n$ true\n")[0].contains("without the quotes"),
			"the near miss is named"
		);
		// A value the run works out is not the parser's business.
		assert!(messages(".ignore-errors = ENV.CI\n$ true\n").is_empty());
		assert!(messages(".ignore-errors = \"{{ ENV.CI }}\"\n$ true\n").is_empty());
	}

	#[test]
	fn a_property_that_describes_the_block_is_underlined_below_the_first_statement() {
		let m = messages("$ true\n.watch = \"src/**\"\n$ true\n");
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
		let d = plain(src);
		assert_eq!(d.len(), 1, "{d:?}");
		assert!(d[0].message.contains("machine-wide"), "{}", d[0].message);
		assert!(
			diagnose(src, &[], true, Chain::Target(&[])).is_empty(),
			"the one place it means something"
		);
	}

	#[test]
	fn a_scope_that_cannot_be_read_is_underlined_where_it_is_written() {
		// Discovery refuses it before anything runs, but only on its way to a
		// target, and nothing that checks files collects them by scope -- so this
		// is the only place it is said about the file.
		let machine_wide = |src: &str| diagnose(src, &[], true, Chain::Target(&[]));
		for src in [
			".only-in-directories = \"{{ ENV.WORK }}/acme\"\n$ true\n",
			".only-in-directories = [\"work/acme\", ENV.WORK]\n$ true\n",
		] {
			let d = machine_wide(src);
			assert_eq!(d.len(), 1, "{src:?}: {d:?}");
			assert!(d[0].message.contains("literal string"), "{}", d[0].message);
			assert_eq!(d[0].range.start_line, 0, "{d:?}");
		}
		assert!(
			machine_wide(".only-in-directories = [\"work/acme\", \"~/work/zed\"]\n$ true\n").is_empty(),
			"literals are what can be read"
		);
		// In a project file, the one thing worth saying is that it does not belong.
		let d = plain(".only-in-directories = \"{{ ENV.WORK }}\"\n$ true\n");
		assert_eq!(d.len(), 1, "{d:?}");
		assert!(d[0].message.contains("machine-wide"), "{}", d[0].message);
	}

	#[test]
	fn a_syntax_error_is_reported_on_its_own_line() {
		let d = plain("$ echo ok\nlet = 3\n");
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
		assert!(messages("if FLAG.x\n\t.workdir = \"web\"\n\t$ true\nend\n").is_empty());
	}

	#[test]
	fn shell_names_one_of_the_posix_shells_and_says_so_when_it_does_not() {
		let m = messages(".shell = \"pwsh\"\n$ true\n");
		assert_eq!(m.len(), 1, "{m:?}");
		assert!(m[0].contains("write `exec pwsh`"), "the fix is the message: {}", m[0]);
		assert!(
			messages(".shell = \"busybox sh\"\n$ true\n").is_empty(),
			"a shell with an argument"
		);
		assert!(messages(".shell = \"/usr/bin/bash\"\n$ true\n").is_empty(), "by path");
		// Worked out during the run, so not the parser's business.
		assert!(messages(".shell = ARG.sh\n$ true\n").is_empty());
	}

	#[test]
	fn a_call_to_a_missing_target_is_reported() {
		let d = diagnose("run build\n", &["deploy".to_string()], false, Chain::Target(&[]));
		assert_eq!(d.len(), 1);
		assert!(d[0].message.contains("no target named `build`"), "{:?}", d[0]);
	}

	#[test]
	fn a_call_to_a_known_target_is_fine() {
		assert!(diagnose("run build\n", &["build".to_string()], false, Chain::Target(&[])).is_empty());
	}

	#[test]
	fn an_interpolated_target_name_is_not_guessed_at() {
		// It is only known at run time, so flagging it would be a false alarm.
		assert!(
			diagnose(
				"run {{ ENV.NS }}:build\n",
				&["deploy".to_string()],
				false,
				Chain::Target(&[])
			)
			.is_empty()
		);
	}

	#[test]
	fn target_calls_are_checked_inside_blocks_too() {
		let src = "for x in [\"a\"]\n\trun nope\nend\n";
		let d = diagnose(src, &["yes".to_string()], false, Chain::Target(&[]));
		assert_eq!(d.len(), 1, "{d:?}");
		assert_eq!(d[0].range.start_line, 1);
	}

	#[test]
	fn without_a_catalog_target_calls_are_left_alone() {
		assert!(plain("run anything\n").is_empty());
	}

	// ---- names that do not resolve

	fn range(start_line: usize, start_col: usize, end_col: usize) -> Range {
		Range {
			start_line,
			start_col,
			end_line: start_line,
			end_col,
		}
	}

	#[test]
	fn a_call_to_a_function_that_does_not_exist_is_underlined_at_its_name() {
		let d = plain("if true\n\tlet found = exists(\"/etc/hosts\")\nend\n");
		assert_eq!(d.len(), 1, "{d:?}");
		assert_eq!(d[0].range, range(1, 13, 19));
		assert_eq!(d[0].severity, Severity::Error);
		assert_eq!(
			d[0].message,
			"unknown function `exists`; did you mean `directory_exists` or `file_exists`?"
		);
	}

	#[test]
	fn a_name_nothing_binds_is_underlined_and_one_the_shared_chain_binds_is_not() {
		let shared = [runfile_lang::parse("let region = \"eu\"\n").unwrap()];
		let d = diagnose("print(region, regoin)\n", &[], false, Chain::Target(&shared));
		assert_eq!(d.len(), 1, "{d:?}");
		assert_eq!(d[0].range, range(0, 14, 20));
		assert_eq!(d[0].message, "`regoin` is not defined; did you mean `region`?");
		// Without the chain, the name it binds is not bound either.
		assert_eq!(plain("print(region, regoin)\n").len(), 2);
	}

	#[test]
	fn a_reassignment_of_nothing_is_underlined_at_the_name_on_its_line() {
		let d = plain("let total = 0\n\ttotl = total + 1\n");
		assert_eq!(d.len(), 1, "{d:?}");
		assert_eq!(d[0].range, range(1, 1, 5));
		assert!(d[0].message.contains("did you mean `total`?"), "{}", d[0].message);
	}

	#[test]
	fn a_name_on_a_later_line_of_a_list_is_underlined_on_that_line() {
		let d = plain("let xs = [\n\t\"a\", # first\n\tghost,\n]\n");
		assert_eq!(d.len(), 1, "{d:?}");
		assert_eq!(d[0].range, range(2, 1, 6));
		// Past a character that is two bytes and one column.
		let d = plain("print(\"é\", ghost)\n");
		assert_eq!(d[0].range, range(0, 11, 16));
	}

	#[test]
	fn with_the_chain_unreadable_only_calls_are_underlined() {
		let d = diagnose("print(region)\nnope()\n", &[], false, Chain::Unknown);
		assert_eq!(d.len(), 1, "{d:?}");
		assert!(d[0].message.contains("unknown function `nope`"), "{}", d[0].message);
	}

	#[test]
	fn a_shared_file_is_checked_the_way_the_runner_folds_it() {
		// Its `if` never runs, so nothing inside binds anything for the lines below.
		let src = "if FLAG.x\n\tlet a = 1\nend\nlet b = a ? \"x\"\n";
		let d = diagnose(src, &[], false, Chain::Shared(&[]));
		assert_eq!(d.len(), 1, "{d:?}");
		assert_eq!(d[0].range, range(3, 8, 9));
		assert!(plain(src).is_empty(), "in a target, it would have run");
	}

	/// Completion at the end of a one-line document.
	fn complete_line(line: &str) -> Completions {
		complete(line, 0, line.chars().count(), Vec::new)
	}

	/// Completion where `‸` sits in `src`, with the marker taken out.
	fn complete_at(src: &str) -> Completions {
		complete_with(src, Vec::new)
	}

	fn complete_with(src: &str, shared: impl FnOnce() -> Vec<Binding>) -> Completions {
		let (no, line) = src
			.lines()
			.enumerate()
			.find(|(_, l)| l.contains('‸'))
			.expect("a cursor");
		let col = line[..line.find('‸').unwrap()].chars().count();
		complete(&src.replace('‸', ""), no, col, shared)
	}

	fn items(c: Completions) -> Vec<Item> {
		match c {
			Completions::Items(items) => items,
			other => panic!("expected items, got {other:?}"),
		}
	}

	/// The labels offered, or none when the answer is not a list at all.
	fn labels(c: Completions) -> Vec<String> {
		match c {
			Completions::Items(items) => items.into_iter().map(|i| i.label).collect(),
			_ => Vec::new(),
		}
	}

	fn has(c: Completions, want: &[&str], not: &[&str]) {
		let got = labels(c);
		for w in want {
			assert!(got.iter().any(|g| g == w), "{w} missing from {got:?}");
		}
		for n in not {
			assert!(!got.iter().any(|g| g == n), "{n} offered in {got:?}");
		}
	}

	#[test]
	fn a_leading_dot_completes_properties() {
		let p = items(complete_line("  .wa"));
		let watch = p.iter().find(|i| i.label == "watch").expect("watch is offered");
		assert_eq!(watch.detail, "property, header-only");
		assert_eq!(watch.kind, Kind::Property);
		assert!(!watch.doc.is_empty(), "and says what it does");
	}

	#[test]
	fn a_property_that_already_has_a_value_completes_nothing_more() {
		assert_eq!(complete_line(".shell = \"ba"), Completions::None, "inside a string");
	}

	#[test]
	fn a_property_value_is_an_expression_and_a_sub_key_is_the_environments() {
		has(
			complete_at("let dir = \"web\"\n.workdir = d‸\n"),
			&["dir", "to_upper", "ENV"],
			&["watch", "json"],
		);
		// `.env.` is followed by a variable's name, which nothing here knows.
		assert_eq!(complete_line(".env."), Completions::None);
	}

	#[test]
	fn run_completes_target_names_until_one_is_chosen() {
		assert_eq!(complete_line("run de"), Completions::Targets);
		// Its arguments are words, handed over as written.
		assert_eq!(complete_line("run deploy --x"), Completions::None);
		// The dispatch `code_of` holds is the same one.
		assert_eq!(complete_line("let c = code_of(run bu"), Completions::Targets);
		assert_ne!(complete_line("let c = code_of(run build) + "), Completions::Targets);
	}

	#[test]
	fn a_shell_line_offers_nothing() {
		assert_eq!(complete_line("$ echo "), Completions::None);
		assert_eq!(complete_line("let out = $ git "), Completions::None, "a capture");
		assert_eq!(complete_line("for f in lines($ git ls-"), Completions::None);
		assert_eq!(complete_line("exec python3 -"), Completions::None, "an `exec` command");
		assert_eq!(complete_line("let out = exec jq "), Completions::None);
	}

	#[test]
	fn an_expression_offers_functions() {
		let f = items(complete_line("let x = to_"));
		let upper = f.iter().find(|i| i.label == "to_upper").expect("to_upper is offered");
		assert_eq!(upper.detail, "to_upper(s)", "the signature is the detail");
		assert_eq!(upper.kind, Kind::Function);
		assert!(!upper.doc.is_empty());
	}

	// ---- keywords

	#[test]
	fn a_line_start_offers_the_keywords_that_open_a_line() {
		let got = items(complete_at("let x = 1\n‸\n"));
		let exec = got.iter().find(|i| i.label == "exec").expect("exec is offered");
		assert_eq!(exec.kind, Kind::Keyword);
		assert!(
			exec.doc.contains("```runfile"),
			"with the hover's example: {}",
			exec.doc
		);
		has(
			complete_at("let x = 1\n‸\n"),
			&[
				"let", "if", "for", "while", "until", "loop", "do", "match", "retry", "exec", "detach", "run",
			],
			// Written after something else, or characters rather than words.
			&["in", "every", "json", "$", "#"],
		);
		// And while one is half typed.
		has(complete_at("\tfo‸\n"), &["for"], &[]);
	}

	#[test]
	fn a_keyword_that_closes_or_continues_a_block_is_offered_only_inside_one() {
		// At the top level each of these is a parse error.
		has(
			complete_at("‸\n"),
			&["let"],
			&["end", "else", "case", "default", "break", "continue"],
		);
		has(
			complete_at("for x in xs\n\t‸\nend\n"),
			&["break", "continue", "end"],
			&["else", "case"],
		);
		has(
			complete_at("if FLAG.x\n\t‸\nend\n"),
			&["else", "end"],
			&["break", "case"],
		);
		// `break` leaves the loop around an `if`; `else` belongs to the `if`.
		has(
			complete_at("for x in xs\n\tif FLAG.x\n\t\t‸\n\tend\nend\n"),
			&["break", "else", "end"],
			&[],
		);
		has(
			complete_at("match RUN.os\n\tcase \"linux\"\n\t\t$ true\n\t‸\nend\n"),
			&["case", "default", "end"],
			&["else"],
		);
		has(complete_at("retry 3\n\t$ true\n‸\nend\n"), &["else", "end"], &["break"]);
		has(complete_at("do\n\t‸\nend\n"), &["end"], &["else"]);
	}

	#[test]
	fn a_block_opened_a_moment_ago_counts_before_its_end_is_written() {
		// No `end` yet: the file does not parse as it stands.
		has(complete_at("while FLAG.x\n\t‸"), &["break", "end"], &[]);
	}

	#[test]
	fn a_header_offers_what_comes_next_in_it() {
		assert_eq!(labels(complete_line("for x i")), ["in"]);
		assert_eq!(labels(complete_line("for a, b ")), ["in"]);
		assert_eq!(labels(complete_line("retry 30 ev")), ["every"]);
		assert_eq!(labels(complete_line("else i")), ["if"]);
		assert_eq!(labels(complete_line("detach ex")), ["exec"]);
		// A name being introduced is nothing anyone can guess.
		assert_eq!(complete_line("let "), Completions::None);
		assert_eq!(complete_line("let a, "), Completions::None);
		assert_eq!(complete_line("for "), Completions::None);
		assert_eq!(complete_line("case "), Completions::None);
		// Past the keyword, an ordinary value.
		has(complete_line("for x in "), &["glob", "ARGS"], &["in"]);
		has(complete_line("retry 30 every "), &["number"], &["every"]);
		has(complete_line("else if "), &["file_exists"], &["if"]);
	}

	#[test]
	fn a_value_may_open_with_a_json_block_or_an_exec_capture() {
		has(complete_line("let doc = "), &["json", "exec", "to_upper"], &["for"]);
		has(complete_line("doc = js"), &["json"], &[]);
		// Anywhere else in an expression they are not values.
		has(complete_line("print("), &["to_upper"], &["json", "exec"]);
		has(complete_line("if x == "), &["to_upper"], &["json", "exec"]);
		has(complete_line("let ok = x != "), &["to_upper"], &["json", "exec"]);
	}

	#[test]
	fn code_of_is_offered_as_the_call_it_is() {
		let got = items(complete_line("let c = co"));
		let c = got
			.iter()
			.find(|i| i.label == "code_of")
			.expect("offered with the functions");
		assert_eq!(c.kind, Kind::Function);
		has(complete_at("‸\n"), &["code_of"], &[]);
	}

	// ---- names

	#[test]
	fn a_binding_above_the_cursor_is_offered_with_the_line_that_binds_it() {
		let got = items(complete_at("let region = \"eu\"\nprint(‸)\n"));
		let r = got.iter().find(|i| i.label == "region").expect("region is offered");
		assert_eq!((r.kind, r.detail.as_str()), (Kind::Variable, "binding"));
		assert!(r.doc.contains("let region = \"eu\""), "{}", r.doc);
		// Unpacked names each count, and `_` is not one.
		has(
			complete_at("let major, _, patch = split(v, \".\")\nprint(‸)\n"),
			&["major", "patch"],
			&["_"],
		);
	}

	#[test]
	fn a_name_is_not_offered_above_its_let_or_inside_its_own_value() {
		has(complete_at("print(‸)\nlet later = 1\n"), &["print"], &["later"]);
		has(complete_at("let early = 1\nlet me = ea‸\n"), &["early"], &["me"]);
		// A spilled list is one statement, and not over until its `]`.
		has(complete_at("let xs = [\n\t‸\n]\n"), &["to_upper"], &["xs"]);
	}

	#[test]
	fn a_loop_variable_is_in_scope_only_inside_its_loop_and_a_let_outlives_its_block() {
		let got = items(complete_at("for host in hosts\n\tprint(‸)\nend\n"));
		let h = got.iter().find(|i| i.label == "host").expect("the loop's name");
		assert_eq!(h.detail, "loop variable");
		has(
			complete_at("for host in hosts\n\tlet seen = host\nend\nprint(‸)\n"),
			// The runner puts a loop's names back as it leaves; a `let` inside
			// any block is still bound after its `end`.
			&["seen"],
			&["host"],
		);
		has(
			complete_at("for a, b in pairs\n\tfor c in xs\n\t\tprint(‸)\n\tend\nend\n"),
			&["a", "b", "c"],
			&[],
		);
	}

	#[test]
	fn the_nearest_binding_of_a_name_is_the_one_offered() {
		let got = items(complete_at("let v = 1\nlet v = 2\nprint(‸)\n"));
		let v: Vec<&Item> = got.iter().filter(|i| i.label == "v").collect();
		assert_eq!(v.len(), 1, "once: {v:?}");
		assert!(v[0].doc.contains("let v = 2"), "{}", v[0].doc);
	}

	#[test]
	fn a_line_being_typed_does_not_hide_the_names_above_it() {
		// `print(re` is not a statement yet, so the file does not parse.
		has(complete_at("let region = \"eu\"\nprint(re‸\n"), &["region"], &[]);
		// Nor, with no `end`, does the loop around it.
		has(complete_at("for host in hosts\n\tprint(ho‸"), &["host"], &[]);
	}

	#[test]
	fn every_line_the_parser_stops_at_is_set_aside() {
		// Broken in three places, none of them the cursor's. Each parse error
		// names its line, so each is blanked in turn until the rest parses --
		// and the loop around the cursor is still a loop.
		let src = "let region = \"eu\"\nlet = 3\nfor host in hosts\n\t$ deploy {{ ho\n\t‸\n\tx(\nend\n";
		has(complete_at(src), &["region", "host", "break", "end"], &[]);
		// Where a body is being written, the whole statement is set aside: a
		// `json` block is checked as it is read, and is not valid JSON yet.
		has(
			complete_at("let n = 1\nif FLAG.x\n\tlet doc = json\n\t\t{\"a\": {{ ‸ }},\n"),
			&["n"],
			&[],
		);
	}

	#[test]
	fn a_shared_binding_is_offered_and_a_local_one_shadows_it() {
		let shared = || {
			vec![
				Binding {
					name: "osvImage".into(),
					line: "let osvImage = \"ghcr.io/google/osv-scanner\"".into(),
					binder: Binder::Shared,
				},
				Binding {
					name: "region".into(),
					line: "let region = \"us\"".into(),
					binder: Binder::Shared,
				},
			]
		};
		let got = items(complete_with("let region = \"eu\"\nprint(‸)\n", shared));
		let osv = got.iter().find(|i| i.label == "osvImage").expect("the shared one");
		assert_eq!(osv.detail, "binding from _shared.run");
		let region: Vec<&Item> = got.iter().filter(|i| i.label == "region").collect();
		assert_eq!(region.len(), 1, "{region:?}");
		assert_eq!(region[0].detail, "binding", "this file's, which shadows the shared one");
	}

	#[test]
	fn the_shared_chain_is_only_asked_for_where_a_name_can_go() {
		// Answering it means discovery, and the files it finds read from disk.
		let never = || -> Vec<Binding> { panic!("asked for shared bindings") };
		for line in [".wa", "$ echo ", "run de", "# note", "let ", "for x i", "let x = RUN."] {
			complete(line, 0, line.chars().count(), never);
		}
	}

	#[test]
	fn shared_bindings_are_the_top_level_lets() {
		// A shared file's other statements never run, so a `let` inside one of
		// them binds nothing for anybody.
		let b = shared_bindings("let a = 1\nlet b, _ = [1, 2]\nif FLAG.x\n\tlet c = 3\nend\n");
		let names: Vec<&str> = b.iter().map(|b| b.name.as_str()).collect();
		assert_eq!(names, ["a", "b"]);
		assert_eq!(b[0].line, "let a = 1");
		assert!(shared_bindings("let = broken\n").is_empty());
	}

	#[test]
	fn a_name_ranks_above_a_function_in_a_value_and_a_keyword_first_on_a_line() {
		let rank = |c: Completions, label: &str| {
			items(c)
				.into_iter()
				.find(|i| i.label == label)
				.unwrap_or_else(|| panic!("{label}"))
				.rank
		};
		let value = "let region = \"eu\"\nlet r = re‸\n";
		assert!(rank(complete_at(value), "region") < rank(complete_at(value), "read_file"));
		let line = "let retries = 3\nre‸\n";
		assert!(rank(complete_at(line), "retry") < rank(complete_at(line), "retries"));
		assert!(rank(complete_at(line), "retries") < rank(complete_at(line), "read_file"));
	}

	// ---- text that is not ours

	#[test]
	fn an_interpolation_on_a_shell_line_completes_as_an_expression() {
		let src = "let region = \"eu\"\n$ deploy --region {{ re‸\n";
		has(complete_at(src), &["region", "to_upper", "ENV"], &["for"]);
		assert_eq!(complete_at("let region = \"eu\"\n$ deploy re‸\n"), Completions::None);
		assert_eq!(
			complete_at("let region = \"eu\"\n$ deploy {{ region }} re‸\n"),
			Completions::None,
			"past the `}}` it is the shell's again"
		);
		// A `run` argument interpolates by the same rule.
		has(
			complete_at("let region = \"eu\"\nrun deploy {{ re‸\n"),
			&["region"],
			&[],
		);
	}

	#[test]
	fn a_string_offers_nothing_but_its_interpolations() {
		assert_eq!(complete_line("print(\"hel"), Completions::None);
		assert_eq!(complete_line("print(\"a \\\" b"), Completions::None, "an escaped quote");
		has(complete_at("let name = \"x\"\nprint(\"hi {{ na‸\n"), &["name"], &[]);
		// A quote inside an interpolation does not end the string around it.
		has(
			complete_at("let name = \"x\"\nprint(\"{{ join(xs, \",\") }} {{ na‸\n"),
			&["name"],
			&[],
		);
	}

	#[test]
	fn a_body_in_another_language_offers_nothing_but_its_interpolations() {
		assert_eq!(complete_at("exec python3\n\tim‸\nend\n"), Completions::None);
		has(
			complete_at("let n = 1\nexec python3\n\tprint({{ n‸ }})\nend\n"),
			&["n"],
			&[],
		);
		assert_eq!(complete_at("let out = exec jq .\n\tke‸\nend\n"), Completions::None);
		assert_eq!(complete_at("let doc = json\n\t{\"a\": tr‸}\nend\n"), Completions::None);
		has(
			complete_at("let n = 1\nlet doc = json\n\t{\"a\": {{ ‸ }}}\nend\n"),
			&["n"],
			&[],
		);
		// Where a `$` line's backslash carries it on to.
		assert_eq!(complete_at("$ docker run \\\n\t--rm im‸\n"), Completions::None);
		// A blank line inside a run is where a statement may yet go.
		has(complete_at("$ a\n‸\n$ b\n"), &["let"], &[]);
	}

	// ---- sources and hover

	#[test]
	fn an_expression_offers_the_source_roots_too() {
		let f = items(complete_line("let x = AR"));
		let args = f
			.iter()
			.find(|i| i.label == "ARGS")
			.expect("a source can start an expression");
		assert_eq!(args.kind, Kind::Variable, "one value");
		let arg = f.iter().find(|i| i.label == "ARG").expect("ARG");
		assert_eq!(arg.kind, Kind::Module, "read through a dot");
		// Not at the start of a line: a line that is only a value does nothing.
		has(complete_at("‸\n"), &["print"], &["ARG", "ENV"]);
	}

	#[test]
	fn run_dot_offers_the_keys_it_actually_has() {
		// The one source whose keys are fixed; `ARG` and `ENV` are whatever the
		// caller passed, so there is nothing to offer.
		let s = items(complete_line("$ echo {{ RUN."));
		let labels: Vec<&str> = s.iter().map(|i| i.label.as_str()).collect();
		assert!(labels.contains(&"os"), "{labels:?}");
		assert!(labels.contains(&"namespaces"), "{labels:?}");
		assert!(!labels.contains(&"nonsense"));
		assert_eq!(complete_line("let x = ARG."), Completions::None);
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
		let h = hover(".workdir = \"web\"", 3).expect("hovers");
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
		let fns = items(complete_line("let x = to_"));
		let up = fns.iter().find(|i| i.label == "to_upper").expect("to_upper");
		assert_eq!(up.detail, "to_upper(s)");
		assert!(up.doc.contains("```runfile"), "{}", up.doc);
		let props = items(complete_line(".work"));
		let wd = props.iter().find(|i| i.label == "workdir").expect("workdir");
		assert!(wd.doc.contains("```runfile"), "{}", wd.doc);
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
		assert!(matches!(complete_line("let x = 1 # about pri"), Completions::None));
		assert!(matches!(complete_line("# a note on con"), Completions::None));
		// Still code: the `#` is the shell's, and the `{{ … }}` is ours.
		assert!(!matches!(complete_line("$ echo # {{ RUN."), Completions::None));
	}
}
