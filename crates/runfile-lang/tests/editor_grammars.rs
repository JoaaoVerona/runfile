//! The editor grammars' lists of function names, against the runner's.
//!
//! A call is coloured as a function in VS Code, and in the tree-sitter editors,
//! only when it names one the language has -- and a grammar can only know that
//! from a list of its own, written out. These hold each list to the runner in
//! both directions: a function the runner gains and a grammar does not is drawn
//! as an error there, and one the runner drops and a grammar keeps is drawn as a
//! call that works.

use std::collections::BTreeSet;
use std::path::Path;

fn repo(path: &str) -> String {
	let full = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(path);
	std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("{}: {e}", full.display()))
}

/// Every name a call may use: what `functions::exists` accepts.
fn runner() -> BTreeSet<String> {
	let mut out: BTreeSet<String> = runfile_lang::functions::names().map(str::to_string).collect();
	// Unlisted and unsuggested, since `?` replaced it, but it still answers.
	out.insert("try".into());
	for n in &out {
		assert!(runfile_lang::functions::exists(n), "`{n}` is listed but does not exist");
	}
	out
}

/// The names in `\b(?:(a|b|c)|(<any name>))…`: the first group.
fn alternation(pattern: &str) -> BTreeSet<String> {
	let from = pattern.find("(?:(").expect("the list opens the pattern") + "(?:(".len();
	let to = from
		+ pattern[from..]
			.find(")|(")
			.expect("the list closes before the fallback");
	pattern[from..to].split('|').map(str::to_string).collect()
}

#[test]
fn the_textmate_grammar_names_exactly_the_runner_s_functions() {
	let grammar: serde_json::Value =
		serde_json::from_str(&repo("editors/vscode/syntaxes/runfile.tmLanguage.json")).expect("valid JSON");
	for (rule, key) in [("function", "match"), ("capture-call", "begin")] {
		let pattern = grammar["repository"][rule][key]
			.as_str()
			.unwrap_or_else(|| panic!("no `{rule}.{key}`"));
		let names = alternation(pattern);
		for n in &names {
			assert!(
				runfile_lang::functions::exists(n),
				"the `{rule}` rule colours `{n}`, which is no function"
			);
		}
		assert_eq!(names, runner(), "the `{rule}` rule");
	}
}

#[test]
fn the_tree_sitter_queries_name_exactly_the_runner_s_functions() {
	let queries = repo("editors/tree-sitter/queries/highlights.scm");
	let mut lists = 0;
	for chunk in queries.split("(#any-of? ").skip(1) {
		let body = &chunk[..chunk.find("))").expect("an `#any-of?` list is closed")];
		let names: BTreeSet<String> = body.split('"').skip(1).step_by(2).map(str::to_string).collect();
		assert_eq!(names, runner(), "an `#any-of?` list in highlights.scm");
		lists += 1;
	}
	assert_eq!(lists, 2, "one for a call and one for a call holding a capture");
}
