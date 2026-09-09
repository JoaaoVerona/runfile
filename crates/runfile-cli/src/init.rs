//! `run :init` -- create `runfiles/` with one working target.
//!
//! Deliberately minimal: one file that runs, showing the three things every
//! target uses (a description, a property, a shell line) and nothing else.

use std::path::Path;

/// Quotes inside `{{ }}` are opaque to the surrounding string, so the nested
/// `"world"` needs no escaping -- which is exactly the sort of thing a starter
/// file should demonstrate rather than explain.
const EXAMPLE: &str = r#"# Say hello
#
# The first comment block is this target's description.

let who = ARG.name ? "world"

confirm("Greet {{ who }}?")
print("hello, {{ who }}")
"#;

pub fn init(dir: &Path) -> Result<String, String> {
	let runfiles = dir.join("runfiles");
	let example = runfiles.join("hello.run");
	if example.exists() {
		return Err(format!("{} already exists", example.display()));
	}
	std::fs::create_dir_all(&runfiles).map_err(|e| format!("{}: {e}", runfiles.display()))?;
	std::fs::write(&example, EXAMPLE).map_err(|e| format!("{}: {e}", example.display()))?;
	Ok(format!(
		"created {}\n\ntry it:\n  run hello\n  run hello --name=you\n",
		example.display()
	))
}

#[cfg(test)]
mod tests {
	use super::*;
	use tempfile::TempDir;

	#[test]
	fn the_generated_target_parses() {
		// The whole point of a starter file is that it works; a template that
		// does not parse would be found by users, not by us.
		let ast = runfile_lang::parse(EXAMPLE).expect("template must parse");
		assert_eq!(
			ast.description.as_deref().map(|d| d.lines().next().unwrap()),
			Some("Say hello")
		);
	}

	#[test]
	fn init_creates_a_runfiles_directory() {
		let d = TempDir::new().unwrap();
		init(d.path()).unwrap();
		assert!(d.path().join("runfiles/hello.run").is_file());
	}

	#[test]
	fn init_refuses_to_overwrite() {
		let d = TempDir::new().unwrap();
		init(d.path()).unwrap();
		std::fs::write(d.path().join("runfiles/hello.run"), "mine").unwrap();
		assert!(init(d.path()).is_err());
		assert_eq!(
			std::fs::read_to_string(d.path().join("runfiles/hello.run")).unwrap(),
			"mine"
		);
	}
}
