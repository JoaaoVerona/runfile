//! Functions that touch the filesystem or a regex engine.

use crate::eval::{Scope, eval_boundary};
use crate::parser::parse_expr;
use crate::value::Value;
use std::path::Path;

fn in_dir(dir: &Path, src: &str) -> Result<Value, String> {
	let mut sc = Scope::new();
	sc.base_dir = dir.to_path_buf();
	let e = parse_expr(src, 0, 1).map_err(|x| x.to_string())?;
	eval_boundary(&e, &mut sc).map_err(|x| x.to_string())
}

fn fixture() -> tempfile::TempDir {
	let d = tempfile::TempDir::new().unwrap();
	std::fs::create_dir_all(d.path().join("a/b")).unwrap();
	std::fs::create_dir_all(d.path().join("node_modules/pkg")).unwrap();
	std::fs::write(d.path().join("a/one.yml"), "image: alpine:3\nimage: nginx:1\n").unwrap();
	std::fs::write(d.path().join("a/b/two.yml"), "x").unwrap();
	std::fs::write(d.path().join("node_modules/pkg/three.yml"), "x").unwrap();
	d
}

#[test]
fn glob_returns_a_list_of_forward_slash_relative_paths() {
	let d = fixture();
	let Value::List(items) = in_dir(d.path(), r#"glob("**/*.yml")"#).unwrap() else {
		panic!()
	};
	let mut got: Vec<String> = items.iter().map(|v| v.to_string()).collect();
	got.sort();
	assert_eq!(got, vec!["a/b/two.yml", "a/one.yml"], "node_modules is skipped");
}

#[test]
fn glob_feeds_a_for_loop_directly() {
	// The old form needed a shell iterator; a list is iterable as-is.
	let d = fixture();
	assert!(matches!(in_dir(d.path(), r#"glob("a/*.yml")"#).unwrap(), Value::List(l) if l.len() == 1));
}

#[test]
fn read_and_write_anchor_to_the_base_dir() {
	let d = fixture();
	in_dir(d.path(), r#"write_file("out/made.txt", "hello")"#).unwrap();
	assert_eq!(std::fs::read_to_string(d.path().join("out/made.txt")).unwrap(), "hello");
	assert_eq!(
		in_dir(d.path(), r#"read_file("out/made.txt")"#).unwrap(),
		Value::Str("hello".into())
	);
}

#[test]
fn read_file_names_the_path_it_could_not_read() {
	let d = fixture();
	let e = in_dir(d.path(), r#"read_file("nope.txt")"#).unwrap_err();
	assert!(e.contains("nope.txt"), "{e}");
}

#[test]
fn file_exists_is_a_bool_not_an_error() {
	let d = fixture();
	assert_eq!(
		in_dir(d.path(), r#"file_exists("a/one.yml")"#).unwrap(),
		Value::Bool(true)
	);
	assert_eq!(in_dir(d.path(), r#"file_exists("nope")"#).unwrap(), Value::Bool(false));
}

#[test]
fn regex_capture_all_returns_every_match_as_a_list() {
	// The corpus target that pulls image names out of compose files.
	let d = fixture();
	let src = r#"regex_capture_all(read_file("a/one.yml"), r"image:\s*(\S+)", 1)"#;
	let Value::List(items) = in_dir(d.path(), src).unwrap() else {
		panic!()
	};
	let got: Vec<String> = items.iter().map(|v| v.to_string()).collect();
	assert_eq!(got, vec!["alpine:3", "nginx:1"]);
}

#[test]
fn regex_functions_report_a_bad_pattern_with_the_pattern() {
	let d = fixture();
	let e = in_dir(d.path(), r#"regex_matches("x", r"[")"#).unwrap_err();
	assert!(e.contains("bad regex"), "{e}");
}

#[test]
fn regex_matches_and_replace_use_raw_strings_without_escape_soup() {
	let d = fixture();
	assert_eq!(
		in_dir(d.path(), r#"regex_matches("v1.2", r"^v\d+\.\d+$")"#).unwrap(),
		Value::Bool(true)
	);
	assert_eq!(
		in_dir(d.path(), r#"regex_replace("a1b2", r"\d", "-")"#).unwrap(),
		Value::Str("a-b-".into())
	);
}

#[test]
fn base64_round_trips() {
	let d = fixture();
	assert_eq!(
		in_dir(d.path(), r#"base64_decode(base64_encode("hi"))"#).unwrap(),
		Value::Str("hi".into())
	);
	assert!(
		in_dir(d.path(), r#"base64_decode("!!!")"#)
			.unwrap_err()
			.contains("base64")
	);
}

#[test]
fn a_single_star_does_not_cross_a_directory_boundary() {
	// globset defaults the other way, so this is a deliberate setting -- and it
	// is the same rule target globs follow.
	let d = fixture();
	let Value::List(shallow) = in_dir(d.path(), r#"glob("a/*.yml")"#).unwrap() else {
		panic!()
	};
	let Value::List(deep) = in_dir(d.path(), r#"glob("a/**/*.yml")"#).unwrap() else {
		panic!()
	};
	assert_eq!(shallow.len(), 1, "a/*.yml is only a/one.yml, not a/b/two.yml");
	assert_eq!(
		deep.len(),
		2,
		"a/**/*.yml reaches both -- ** also matches zero directories"
	);
}

mod printf {
	use crate::functions::render_format;
	use crate::span::Span;
	use crate::value::Value;

	fn f(fmt: &str, args: &[Value]) -> Result<String, String> {
		render_format(fmt, args, Span::new(0, 0, 1)).map_err(|e| e.to_string())
	}

	fn s(x: &str) -> Value {
		Value::Str(x.into())
	}

	#[test]
	fn the_substitutions_it_has() {
		assert_eq!(f("%s", &[s("a")]).unwrap(), "a");
		assert_eq!(f("%d", &[Value::Num(42.0)]).unwrap(), "42");
		assert_eq!(f("%d", &[Value::Num(-7.0)]).unwrap(), "-7");
		assert_eq!(f("%f", &[Value::Num(1.5)]).unwrap(), "1.500000");
		assert_eq!(f("%.2f", &[Value::Num(1.005)]).unwrap(), "1.00");
		assert_eq!(f("%.0f", &[Value::Num(2.6)]).unwrap(), "3");
	}

	#[test]
	fn percent_escapes_itself_and_takes_no_value() {
		assert_eq!(f("100%%", &[]).unwrap(), "100%");
		assert_eq!(f("%d%% of %s", &[Value::Num(50.0), s("them")]).unwrap(), "50% of them");
	}

	#[test]
	fn text_around_the_substitutions_is_kept_as_it_stands() {
		assert_eq!(f("a\tb\n", &[]).unwrap(), "a\tb\n");
		assert_eq!(f("[%s]", &[s("")]).unwrap(), "[]");
	}

	#[test]
	fn any_value_has_an_s_form() {
		assert_eq!(f("%s", &[Value::Bool(true)]).unwrap(), "true");
		assert_eq!(f("%s", &[Value::Num(2.0)]).unwrap(), "2");
		assert_eq!(
			f("%s", &[Value::List(vec![s("a"), Value::List(vec![s("b")])])]).unwrap(),
			"a b"
		);
	}

	#[test]
	fn the_count_has_to_match_in_both_directions() {
		// A `%s` with nothing to put in it, or a value with no `%` to go to,
		// is a typo every time.
		assert!(f("%s %s", &[s("one")]).is_err());
		assert!(f("%s", &[s("a"), s("b")]).is_err());
		assert!(f("no substitutions", &[s("a")]).is_err());
	}

	#[test]
	fn a_wrong_type_is_refused_rather_than_coerced() {
		assert!(f("%d", &[s("12")]).is_err());
		assert!(f("%f", &[Value::Bool(true)]).is_err());
		assert!(f("%d", &[Value::Num(2.5)]).is_err());
	}

	#[test]
	fn a_malformed_format_says_what_is_wrong() {
		for (fmt, want) in [
			("%", "nothing to substitute"),
			("%q", "not a substitution"),
			("%.2d", "only for `%f`"),
			("%.f", "followed by a number"),
		] {
			let e = f(fmt, &[s("a")]).unwrap_err();
			assert!(e.contains(want), "{fmt:?} said {e:?}");
		}
	}
}
