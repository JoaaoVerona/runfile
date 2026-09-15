//! `SHELL-CHECK-RULES.md` is the list of rules, and is held to the checker: the
//! same rules in the same order, each opening with the summary the checker
//! gives it, and every example doing what the document says it does. A rule
//! document that has drifted is worse than none -- it is where someone decides
//! whether a report is a bug.

use runfile_shell::{RULES, check};

fn document() -> String {
	let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../SHELL-CHECK-RULES.md");
	std::fs::read_to_string(path).expect("SHELL-CHECK-RULES.md")
}

/// Each rule's section: its name, and the text under its heading up to the next
/// heading of any level -- a `#` inside an example being a comment, not one.
fn sections(doc: &str) -> Vec<(String, String)> {
	let mut out: Vec<(String, String)> = Vec::new();
	let (mut open, mut fenced) = (false, false);
	for line in doc.lines() {
		if line.starts_with("```") {
			fenced = !fenced;
		} else if !fenced && line.starts_with('#') {
			let id = line.strip_prefix("### `").and_then(|r| r.strip_suffix('`'));
			if let Some(id) = id {
				out.push((id.to_string(), String::new()));
			}
			open = id.is_some();
			continue;
		}
		if open && let Some((_, body)) = out.last_mut() {
			body.push_str(line);
			body.push('\n');
		}
	}
	out
}

#[test]
fn the_document_lists_every_rule_in_order() {
	let listed: Vec<String> = sections(&document()).into_iter().map(|(id, _)| id).collect();
	let rules: Vec<&str> = RULES.iter().map(|r| r.id).collect();
	assert_eq!(listed, rules);
}

#[test]
fn each_rule_opens_with_its_summary() {
	for ((id, body), rule) in sections(&document()).iter().zip(RULES) {
		let first = body.lines().find(|l| !l.trim().is_empty()).unwrap_or_default();
		assert_eq!(first, rule.summary, "`{id}`");
	}
}

#[test]
fn every_example_does_what_the_document_says() {
	let mut examples = 0;
	for (id, body) in sections(&document()) {
		let mut lines = body.lines();
		while let Some(line) = lines.next() {
			if line != "```sh" {
				continue;
			}
			let block: Vec<&str> = lines.by_ref().take_while(|l| *l != "```").collect();
			let src = block.join("\n") + "\n";
			let file = runfile_lang::parse(&src).unwrap_or_else(|e| panic!("`{id}`: does not parse: {e}\n{src}"));
			let found = check(&src, &file, Some(&[]));
			match block.first().copied() {
				Some("# flagged") => {
					assert!(!found.is_empty(), "`{id}`: nothing found in\n{src}");
					assert!(found.iter().all(|f| f.rule == id), "`{id}`: {found:#?}\n{src}");
				}
				Some("# not flagged") => assert!(found.is_empty(), "`{id}`: {found:#?}\n{src}"),
				other => panic!("`{id}`: an example opens with `# flagged` or `# not flagged`, not {other:?}"),
			}
			examples += 1;
		}
	}
	assert!(examples >= 2 * RULES.len(), "only {examples} examples");
}
