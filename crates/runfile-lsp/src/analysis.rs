//! Everything the server knows about a document, as plain functions.
//!
//! Diagnostics come from the real parser rather than a second, approximate one,
//! so an editor can never disagree with what `run` does. Keeping this layer
//! free of protocol types is what lets it be tested without a client.

use runfile_lang::{InterpPart, Statement};
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
pub fn diagnose(src: &str, known_targets: &[String]) -> Vec<Diagnostic> {
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
	check_properties(&ast.body, false, &mut out, src);
	if !known_targets.is_empty() {
		check_target_calls(&ast.body, known_targets, &mut out, src);
	}
	out.sort_by_key(|d| (d.range.start_line, d.range.start_col));
	out
}

fn check_properties(block: &runfile_lang::Block, nested: bool, out: &mut Vec<Diagnostic>, src: &str) {
	for p in &block.properties {
		let Some(head) = p.path.first() else { continue };
		let Some(known) = PROPERTIES.iter().find(|p| p.name == head) else {
			out.push(Diagnostic {
				range: whole_line(src, p.span.line),
				message: format!("unknown property `.{head}`{}", nearest(head)),
				severity: Severity::Error,
			});
			continue;
		};
		if nested && !known.block_scoped {
			out.push(Diagnostic {
				range: whole_line(src, p.span.line),
				message: format!("`.{head}` is header-only and cannot be set inside a block"),
				severity: Severity::Error,
			});
		}
	}
	for st in &block.statements {
		for inner in sub_blocks(st) {
			check_properties(inner, true, out, src);
		}
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
		Statement::For { body, .. } => vec![body],
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
pub const RUN_KEYS: &[(&str, &str)] = &[
	("os", "`linux`, `mac` or `windows`."),
	("arch", "The CPU architecture, normalised."),
	("cwd", "The directory `run` was invoked from."),
	("file", "This target's own file."),
	(
		"parent",
		"The parent of `runfiles/`: the anchor every relative path resolves against.",
	),
	("namespaces", "The subproject namespaces in this project, as a list."),
];

/// The five roots a value can come from.
pub const SOURCES: &[(&str, &str)] = &[
	("ARG", "A `--name=value` argument."),
	("ENV", "An environment variable."),
	("FLAG", "Whether `--name` was passed, as a bool."),
	("RUN", "Context about this run."),
	("ARGS", "The positional arguments, as a list."),
];

/// Completion depends only on the line so far, which is what makes it usable
/// on a document that does not currently parse.
pub fn complete(line_prefix: &str) -> Completions {
	let t = line_prefix.trim_start();
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
						p.doc,
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
		return Completions::Sources(RUN_KEYS.iter().map(|(k, d)| Item::new(k, "run context", d)).collect());
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
		.map(|f| Item::new(f.name, f.signature, f.doc))
		.collect();
	items.extend(SOURCES.iter().map(|(n, d)| Item::new(n, "source", d)));
	Completions::Functions(items)
}

/// What to show when the pointer rests on `col` of `line`.
///
/// Everything a person can hover has a fixed meaning -- a property, a function,
/// a source -- so this reads the word under the cursor rather than the tree,
/// and keeps working while the document does not parse.
pub fn hover(line: &str, col: usize) -> Option<String> {
	let word = word_at(line, col)?;
	// `.name`, possibly dotted: `.env.PORT` is the `env` property.
	if let Some(rest) = word.strip_prefix('.') {
		let head = rest.split('.').next().unwrap_or(rest);
		let p = PROPERTIES.iter().find(|p| p.name == head)?;
		let scope = if p.block_scoped {
			"May be set inside an `if` / `for` / `match` block."
		} else {
			"Header-only: it belongs at the top of the file."
		};
		return Some(format!("`.{}`\n\n{}\n\n{scope}", p.name, p.doc));
	}
	// `ARG.name`, or a bare source root.
	if let Some((root, key)) = word.split_once('.')
		&& let Some((_, doc)) = SOURCES.iter().find(|(n, _)| *n == root)
	{
		if root == "RUN"
			&& let Some((k, d)) = RUN_KEYS.iter().find(|(k, _)| *k == key)
		{
			return Some(format!("`RUN.{k}`\n\n{d}"));
		}
		return Some(format!("`{root}.{key}`\n\n{doc}"));
	}
	if let Some((n, doc)) = SOURCES.iter().find(|(n, _)| *n == word) {
		return Some(format!("`{n}`\n\n{doc}"));
	}
	let f = runfile_lang::functions::FUNCTIONS.iter().find(|f| f.name == word)?;
	Some(format!("`{}`\n\n{}", f.signature, f.doc))
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
		diagnose(src, &[]).into_iter().map(|d| d.message).collect()
	}

	#[test]
	fn a_clean_file_has_no_diagnostics() {
		assert!(messages("# Builds\n.shell = \"bash\"\n$ echo hi\n").is_empty());
	}

	#[test]
	fn a_syntax_error_is_reported_on_its_own_line() {
		let d = diagnose("$ echo ok\nlet = 3\n", &[]);
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
		let m = messages("if FLAG.x\n\t.confirm = \"sure?\"\n\t$ true\nend\n");
		assert_eq!(m.len(), 1, "{m:?}");
		assert!(m[0].contains("header-only"), "{}", m[0]);
	}

	#[test]
	fn a_block_scoped_property_inside_a_block_is_fine() {
		assert!(messages("if FLAG.x\n\t.shell = \"bash\"\n\t$ true\nend\n").is_empty());
	}

	#[test]
	fn a_call_to_a_missing_target_is_reported() {
		let d = diagnose("run build\n", &["deploy".to_string()]);
		assert_eq!(d.len(), 1);
		assert!(d[0].message.contains("no target named `build`"), "{:?}", d[0]);
	}

	#[test]
	fn a_call_to_a_known_target_is_fine() {
		assert!(diagnose("run build\n", &["build".to_string()]).is_empty());
	}

	#[test]
	fn an_interpolated_target_name_is_not_guessed_at() {
		// It is only known at run time, so flagging it would be a false alarm.
		assert!(diagnose("run {{ ENV.NS }}:build\n", &["deploy".to_string()]).is_empty());
	}

	#[test]
	fn target_calls_are_checked_inside_blocks_too() {
		let src = "for x in [\"a\"]\n\trun nope\nend\n";
		let d = diagnose(src, &["yes".to_string()]);
		assert_eq!(d.len(), 1, "{d:?}");
		assert_eq!(d[0].range.start_line, 1);
	}

	#[test]
	fn without_a_catalog_target_calls_are_left_alone() {
		assert!(diagnose("run anything\n", &[]).is_empty());
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

	#[test]
	fn hover_explains_a_property_and_says_where_it_may_go() {
		let h = hover(".watch = \"src/**\"", 3).expect("hovers");
		assert!(h.contains("`.watch`"), "{h}");
		assert!(h.contains("Header-only"), "{h}");
		let h = hover(".shell = \"bash\"", 3).expect("hovers");
		assert!(h.contains("May be set inside"), "{h}");
	}

	#[test]
	fn hover_reads_a_dotted_property_as_its_head() {
		// `.env.PORT` is the `env` property with a sub-key.
		let h = hover(".env.PORT = \"3000\"", 6).expect("hovers");
		assert!(h.contains("`.env`"), "{h}");
	}

	#[test]
	fn hover_explains_a_function_by_its_signature() {
		let h = hover("let x = substring(s, 1)", 12).expect("hovers");
		assert!(h.contains("substring(s, start"), "{h}");
	}

	#[test]
	fn hover_explains_a_source_and_its_key() {
		let h = hover("$ echo {{ RUN.os }}", 15).expect("hovers");
		assert!(h.contains("`RUN.os`") && h.contains("linux"), "{h}");
		let h = hover("let e = ARG.env", 10).expect("hovers");
		assert!(h.contains("`ARG.env`") && h.contains("--name=value"), "{h}");
		let h = hover("let a = ARGS", 10).expect("hovers");
		assert!(h.contains("positional"), "{h}");
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
}
