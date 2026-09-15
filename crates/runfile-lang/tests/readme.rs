//! The examples in `README.md` are runfiles, and are held to the same standard
//! as the ones in `runfiles/`.
//!
//! Documentation that has drifted from the language is worse than none: a
//! reader copies it, it does not parse, and they conclude the tool is broken.
//! Every ```sh block is parsed and re-formatted, so an example cannot be stale
//! and cannot show a shape `run :lint` would immediately undo.

/// Every ```sh block in the README, with the line it starts on.
fn examples() -> Vec<(usize, String)> {
	let readme = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../README.md"))
		.expect("README.md");
	let mut out = Vec::new();
	let mut lines = readme.lines().enumerate();
	while let Some((no, line)) = lines.next() {
		if line.trim_end() != "```sh" {
			continue;
		}
		let mut body = String::new();
		for (_, l) in lines.by_ref() {
			if l.trim_end() == "```" {
				break;
			}
			body.push_str(l);
			body.push('\n');
		}
		out.push((no + 2, body));
	}
	out
}

#[test]
fn every_example_is_a_runfile() {
	let found = examples();
	assert!(found.len() > 10, "expected the README to be full of examples");
	for (line, body) in found {
		runfile_lang::parse(&body).unwrap_or_else(|e| panic!("README.md:{line}: does not parse: {e}\n\n{body}"));
	}
}

#[test]
fn every_example_calls_only_functions_that_exist() {
	// A reader copies an example out, and one that calls a function the
	// language does not have is refused the moment it runs. A fragment's names
	// are not held to the same standard -- it reads what the lines around it
	// would have bound -- but which functions exist never depends on that.
	for (line, body) in examples() {
		let tree = runfile_lang::parse(&body).unwrap_or_else(|e| panic!("README.md:{line}: {e}"));
		let wrong: Vec<String> = runfile_lang::resolve::functions(&tree)
			.iter()
			.map(ToString::to_string)
			.collect();
		assert!(wrong.is_empty(), "README.md:{line}: {}", wrong.join("; "));
	}
}

#[test]
fn every_whole_file_example_names_only_what_it_binds() {
	// The gallery's examples open with the path they live at, and are whole
	// files: nothing around them binds anything, so every name has to resolve.
	for (line, body) in examples() {
		if !body.starts_with("# runfiles/") || body.starts_with("# runfiles/_shared.run") {
			continue;
		}
		let tree = runfile_lang::parse(&body).unwrap_or_else(|e| panic!("README.md:{line}: {e}"));
		let wrong: Vec<String> = runfile_lang::resolve::of_chain(&tree, &[])
			.iter()
			.map(ToString::to_string)
			.collect();
		assert!(wrong.is_empty(), "README.md:{line}: {}", wrong.join("; "));
	}
}

#[test]
fn every_example_is_already_formatted() {
	for (line, body) in examples() {
		let formatted = runfile_lang::format(&body).unwrap_or_else(|e| panic!("README.md:{line}: {e}"));
		assert_eq!(
			formatted, body,
			"README.md:{line}: not in the shape `run :lint` produces"
		);
	}
}
