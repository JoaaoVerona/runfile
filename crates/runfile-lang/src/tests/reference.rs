//! A `run` word naming a package: what it is read as, and what it is refused
//! for -- by the parser of the word, by the checker before a run, and by the
//! parser of `code_of(exec …)`, the line a package hands itself over with.

use crate::ast::{Expr, InterpPart, Statement};
use crate::reference::{Reference, Remote, Rev, is_reference, parse};

fn remote(word: &str) -> Remote {
	match parse(word) {
		Ok(Reference::Remote(r)) => r,
		other => panic!("{word}: {other:?}"),
	}
}

fn refused(word: &str) -> String {
	match parse(word) {
		Err(e) => e.0,
		Ok(r) => panic!("{word} was read as {r:?}"),
	}
}

#[test]
fn a_slash_is_what_makes_a_word_a_package() {
	for word in ["github.com/o/r@v1", "./tools/j18n", "../x", "/abs", ".", ".."] {
		assert!(is_reference(word), "{word}");
	}
	// A target is its file name, with `:` where its directories were.
	for word in ["build", "web:build", "_main", "a.b", "--help"] {
		assert!(!is_reference(word), "{word}");
	}
}

#[test]
fn a_repository_is_its_first_two_segments_and_the_rest_is_a_folder_in_it() {
	let r = remote("github.com/JoaaoVerona/j18n@v0.14.1");
	assert_eq!(
		(r.host.as_str(), r.repo.as_str(), r.dir.as_str()),
		("github.com", "JoaaoVerona/j18n", "")
	);
	assert_eq!(r.rev, Rev::Name("v0.14.1".into()));
	assert_eq!(r.clone_url(), "https://github.com/JoaaoVerona/j18n.git");
	assert_eq!(r.shown(), "github.com/JoaaoVerona/j18n@v0.14.1");

	let r = remote("github.com/JoaaoVerona/j18n/internal-packages/subdir@main");
	assert_eq!(
		(r.repo.as_str(), r.dir.as_str()),
		("JoaaoVerona/j18n", "internal-packages/subdir")
	);
	assert_eq!(r.shown(), "github.com/JoaaoVerona/j18n/internal-packages/subdir@main");
	assert_eq!(r.repository(), "github.com/JoaaoVerona/j18n");
}

#[test]
fn a_git_suffix_marks_where_a_nested_repository_ends() {
	// A GitLab subgroup has more than two segments, and only the author knows
	// where the repository stops and the folder begins.
	let r = remote("gitlab.com/group/sub/repo.git/tools/x@v2");
	assert_eq!((r.repo.as_str(), r.dir.as_str()), ("group/sub/repo", "tools/x"));
	assert_eq!(r.clone_url(), "https://gitlab.com/group/sub/repo.git");
	// On a two-segment repository it is only the clone URL's own spelling.
	let r = remote("git.joaoverona.com/joaaoverona/j18n.git@v1");
	assert_eq!((r.repo.as_str(), r.dir.as_str()), ("joaaoverona/j18n", ""));
}

#[test]
fn a_url_copied_from_a_browser_reads_the_same_and_the_host_is_case_blind() {
	let r = remote("https://GitHub.com/o/r/@v1");
	assert_eq!((r.host.as_str(), r.repo.as_str()), ("github.com", "o/r"));
	let r = remote("git.example.com:8443/o/r@v1");
	assert_eq!(r.host, "git.example.com:8443");
	assert_eq!(r.clone_url(), "https://git.example.com:8443/o/r.git");
}

#[test]
fn a_full_commit_is_a_commit_and_anything_else_is_a_name() {
	let sha = "E8E95D87B054CD7F081F88531CF85A54057A5D85";
	assert_eq!(
		remote(&format!("h.io/o/r@{sha}")).rev,
		Rev::Commit(sha.to_ascii_lowercase())
	);
	let sha256 = "a".repeat(64);
	assert_eq!(remote(&format!("h.io/o/r@{sha256}")).rev, Rev::Commit(sha256));
	// No host fetches by a prefix, so a short one is looked up as a name, and
	// the host's answer says what it looks like.
	assert_eq!(remote("h.io/o/r@e8e95d8").rev, Rev::Name("e8e95d8".into()));
	assert!(crate::reference::looks_abbreviated("e8e95d8"));
	assert!(!crate::reference::looks_abbreviated("2024"));
	assert_eq!(remote("h.io/o/r@feature/x").rev, Rev::Name("feature/x".into()));
}

#[test]
fn a_folder_on_disk_is_written_as_a_path() {
	for word in [
		".",
		"..",
		"./tools/j18n",
		"../tools/j18n",
		"/opt/pkg",
		"C:/pkg",
		"C:\\pkg",
	] {
		assert_eq!(parse(word), Ok(Reference::Path(word.into())), "{word}");
	}
}

#[test]
fn a_reference_with_no_version_is_refused_with_one_to_write() {
	let m = refused("github.com/o/r");
	assert!(
		m.contains("names no version") && m.contains("github.com/o/r@v1.0.0"),
		"{m}"
	);
	assert!(refused("github.com/o/r@").contains("names no version"));
}

#[test]
fn an_address_that_is_not_https_is_refused_with_the_one_that_is() {
	let m = refused("http://github.com/o/r@v1");
	assert!(m.contains("`github.com/o/r@v1`"), "{m}");
	let m = refused("ssh://git@github.com/o/r.git");
	assert!(m.contains("`github.com/o/r@<version>`"), "{m}");
	let m = refused("git@github.com:o/r.git");
	assert!(
		m.contains("ssh address") && m.contains("`github.com/o/r@<version>`"),
		"{m}"
	);
}

#[test]
fn a_word_that_is_neither_a_path_nor_a_host_is_refused() {
	assert!(refused("tools/j18n").contains("starts with `./`"));
	assert!(refused("github.com/o@v1").contains("no repository"));
	assert!(refused("github.com/o/r$x@v1").contains("`r$x`"));
	assert!(refused("github.com/o/../r@v1").contains("`..`"));
	for rev in ["a..b", "-x", "x y", "x~1", "x^", "x:y", ".hidden", "x.lock", "x/"] {
		assert!(
			refused(&format!("github.com/o/r@{rev}")).contains("git accepts"),
			"{rev}"
		);
	}
}

fn statement(src: &str) -> Statement {
	crate::parse(src)
		.unwrap_or_else(|e| panic!("{src}\n{e}"))
		.body
		.statements
		.remove(0)
}

#[test]
fn code_of_holds_a_program_started_directly() {
	let Statement::Let { value, .. } =
		statement("let s = code_of(exec {{ join_path(d, \"j18n\") }} sync {{ ARGS }})\n")
	else {
		panic!("not a let");
	};
	let Expr::Call { name, args, .. } = value else {
		panic!("not a call");
	};
	assert_eq!(name, "code_of");
	let [Expr::Program { command, .. }] = &args[..] else {
		panic!("{args:?}");
	};
	// A `)` inside an interpolation is the expression's, and a comma after
	// `exec` is the program's: neither ends the call or splits its arguments.
	assert!(matches!(&command[0], InterpPart::Expr(Expr::Call { name, .. }) if name == "join_path"));
	assert!(matches!(&command[1], InterpPart::Literal(t) if t == " sync "));
	assert!(matches!(
		statement("code_of(exec printf %s,%s a b)\n"),
		Statement::Call { .. }
	));
}

#[test]
fn only_code_of_holds_a_program_and_its_parenthesis_has_to_be_the_last() {
	let e = crate::parse("let x = lines(exec ls)\n").unwrap_err().to_string();
	assert!(e.contains("cannot take an `exec` command"), "{e}");
	let e = crate::parse("let x = code_of(exec echo (a))\n")
		.unwrap_err()
		.to_string();
	assert!(e.contains("cannot be told from the one that closes"), "{e}");
	let e = crate::parse("let x = code_of(exec)\n").unwrap_err().to_string();
	assert!(e.contains("needs a program"), "{e}");
	// `exec` as a binding's name is still a binding.
	assert!(crate::parse("let exec = 1\nlet x = length(exec)\n").is_ok());
}

#[test]
fn a_program_is_left_as_written_by_the_formatter() {
	let src = "let s = code_of(exec tool   --x   {{ ARGS }})\n";
	assert_eq!(crate::format(src).unwrap(), src);
}
