//! Source in, tree out.
//!
//! One `.run` file per shape the parser can produce, each beside the tree it
//! parses to. A change to the AST shows up as a diff a person can read, rather
//! than as a distant assertion failure -- which is what makes these worth
//! keeping as the language grows.
//!
//! Regenerate after a deliberate change:
//!
//! ```text
//! UPDATE_GOLDEN=1 cargo test -p runfile-lang --test golden
//! ```

use std::path::{Path, PathBuf};

fn golden_dir() -> PathBuf {
	Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// The tree, with every source position removed.
///
/// Spans and `exec` body line numbers shift when anything above them moves, so
/// leaving them in would make every file a diff about whitespace instead of
/// about structure.
fn render(src: &str) -> String {
	let ast = runfile_lang::parse(src).unwrap_or_else(|e| panic!("{e}"));
	let mut text = format!("{:#?}", ast);
	for (open, close) in [("Span {", '}'), ("lines: [", ']')] {
		let mut from = 0;
		while let Some(at) = text[from..].find(open).map(|i| i + from) {
			let after = at + open.len();
			let end = text[after..].find(close).map(|e| after + e + 1).unwrap_or(text.len());
			text.replace_range(at..end, open.trim_end_matches([' ', '{', '[', ':']));
			from = at + 4;
		}
	}
	// Collapse what the stripping leaves behind, so the files read cleanly.
	text.lines()
		.map(str::trim_end)
		.filter(|l| !l.trim().is_empty())
		.collect::<Vec<_>>()
		.join("\n")
		+ "\n"
}

#[test]
fn every_shape_parses_to_the_tree_beside_it() {
	let update = std::env::var_os("UPDATE_GOLDEN").is_some();
	let mut checked = 0;
	let mut entries: Vec<PathBuf> = std::fs::read_dir(golden_dir())
		.expect("tests/golden exists")
		.flatten()
		.map(|e| e.path())
		.filter(|p| p.extension().is_some_and(|x| x == "run"))
		.collect();
	entries.sort();

	for input in entries {
		let src = std::fs::read_to_string(&input).expect("read input");
		let actual = render(&src);
		let expected_path = input.with_extension("ast");
		if update {
			std::fs::write(&expected_path, &actual).expect("write golden");
			continue;
		}
		let expected = std::fs::read_to_string(&expected_path).unwrap_or_else(|_| {
			panic!(
				"{} has no tree beside it; regenerate with UPDATE_GOLDEN=1",
				input.display()
			)
		});
		assert_eq!(
			actual,
			expected,
			"\n{} parses differently than its golden file.\n\
			 If the change is deliberate: UPDATE_GOLDEN=1 cargo test -p runfile-lang --test golden\n",
			input.display()
		);
		checked += 1;
	}
	assert!(
		update || checked >= 8,
		"only {checked} shapes checked; files went missing"
	);
}

#[test]
fn a_golden_file_is_not_silently_orphaned() {
	// A `.ast` with no `.run` beside it is a leftover from a renamed shape, and
	// nothing else would ever notice.
	for entry in std::fs::read_dir(golden_dir()).expect("tests/golden").flatten() {
		let p = entry.path();
		if p.extension().is_some_and(|x| x == "ast") {
			assert!(p.with_extension("run").exists(), "{} has no input", p.display());
		}
	}
}
