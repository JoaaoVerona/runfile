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
		let Some((_, block_ok)) = PROPERTIES.iter().find(|(n, _)| n == head) else {
			out.push(Diagnostic {
				range: whole_line(src, p.span.line),
				message: format!("unknown property `.{head}`{}", nearest(head)),
				severity: Severity::Error,
			});
			continue;
		};
		if nested && !block_ok {
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
		.map(|(n, _)| (edits(name, n), *n))
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
#[derive(Debug, PartialEq, Eq)]
pub enum Completions {
	Properties(Vec<String>),
	Functions(Vec<String>),
	Targets,
	None,
}

/// Completion depends only on the line so far, which is what makes it usable
/// on a document that does not currently parse.
pub fn complete(line_prefix: &str) -> Completions {
	let t = line_prefix.trim_start();
	if let Some(rest) = t.strip_prefix('.')
		&& !rest.contains('=')
	{
		return Completions::Properties(PROPERTIES.iter().map(|(n, _)| (*n).to_string()).collect());
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
	Completions::Functions(
		runfile_lang::functions::FUNCTIONS
			.iter()
			.map(|s| s.to_string())
			.collect(),
	)
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
		assert!(p.contains(&"watch".to_string()));
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
		assert!(f.contains(&"to_upper".to_string()));
	}
}
