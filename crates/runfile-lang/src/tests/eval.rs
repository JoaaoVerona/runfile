use crate::eval::{Scope, eval, eval_boundary};
use crate::parser::parse_expr;
use crate::value::Value;

fn sc() -> Scope {
	let mut s = Scope::new();
	s.args.insert("env".into(), "production".into());
	s.args.insert("count".into(), "3".into());
	s.env.insert("PORT".into(), "4003".into());
	s.flags.push("debug".into());
	s.positional = vec!["major".into(), "extra arg".into()];
	s.run.insert("os".into(), Value::Str("linux".into()));
	s.run.insert(
		"namespaces".into(),
		Value::List(vec![Value::Str("web".into()), Value::Str("api".into())]),
	);
	s.vars.insert("name".into(), Value::Str("app".into()));
	s.vars.insert("n".into(), Value::Num(7.0));
	s
}

fn v(src: &str) -> Value {
	let e = parse_expr(src, 0, 1).unwrap_or_else(|x| panic!("{src}: {x}"));
	eval_boundary(&e, &mut sc()).unwrap_or_else(|x| panic!("{src}: {x}"))
}

fn boom(src: &str) -> String {
	let e = parse_expr(src, 0, 1).unwrap_or_else(|x| panic!("{src}: {x}"));
	eval(&e, &mut sc()).expect_err(src).to_string()
}

#[test]
fn arithmetic_is_one_numeric_type() {
	assert_eq!(v("2 + 3"), Value::Num(5.0));
	assert_eq!(v("1 / 2"), Value::Num(0.5), "single number type, not integer division");
	assert_eq!(v("2 + 3").to_string(), "5", "integral values print without a fraction");
}

#[test]
fn nothing_coerces() {
	assert!(
		boom(r#""a" + 1"#).contains("concat"),
		"the error should point at concat"
	);
	assert_eq!(v(r#""1" == 1"#), Value::Bool(false), "equality never coerces");
	assert!(boom("ARG.count + 1").contains("needs numbers"));
	assert_eq!(v("number(ARG.count) + 1"), Value::Num(4.0));
}

#[test]
fn concat_joins_and_stringifies() {
	assert_eq!(v(r#"concat("v", 1, ".", 0)"#), Value::Str("v1.0".into()));
}

#[test]
fn division_by_zero_is_an_error() {
	assert!(boom("1 / 0").contains("zero"));
}

#[test]
fn sources_resolve_by_kind() {
	assert_eq!(v("ARG.env"), Value::Str("production".into()));
	assert_eq!(v("ENV.PORT"), Value::Str("4003".into()));
	assert_eq!(v("FLAG.debug"), Value::Bool(true));
	assert_eq!(v("FLAG.absent"), Value::Bool(false));
	assert_eq!(v("RUN.os"), Value::Str("linux".into()));
	assert!(matches!(v("RUN.namespaces"), Value::List(l) if l.len() == 2));
	assert!(matches!(v("ARGS"), Value::List(l) if l.len() == 2));
}

#[test]
fn a_chain_falls_through_to_the_next_link() {
	assert_eq!(v(r#"ARG.missing ? "fallback""#), Value::Str("fallback".into()));
	assert_eq!(v(r#"ARG.env ? "fallback""#), Value::Str("production".into()));
}

#[test]
fn missing_inputs_name_themselves() {
	assert!(boom("ARG.nope").contains("--nope"));
	assert!(boom("ENV.NOPE").contains("NOPE"));
	assert!(boom("undefined_thing").contains("not defined"));
}

#[test]
fn lists_index_and_measure() {
	assert_eq!(v(r#"["a", "b", "c"][1]"#), Value::Str("b".into()));
	assert_eq!(v(r#"length(["a", "b"])"#), Value::Num(2.0));
	assert!(boom(r#"["a"][5]"#).contains("out of range"));
}

#[test]
fn split_and_lines_return_lists() {
	assert!(matches!(v(r#"split("1.2.3", ".")"#), Value::List(l) if l.len() == 3));
	assert!(
		matches!(v("lines(\"a\\nb\\n\\nc\")"), Value::List(l) if l.len() == 3),
		"blank lines dropped"
	);
}

#[test]
fn the_semver_bump_reads_as_arithmetic() {
	// The target that motivated operators: add(nth(cur, '.', '0'), '1') before.
	let mut s = sc();
	s.vars.insert("cur".into(), Value::Str("1.4.9".into()));
	let e = parse_expr(r#"concat(number(split(cur, ".")[0]) + 1, ".0.0")"#, 0, 1).unwrap();
	assert_eq!(eval(&e, &mut s).unwrap(), Value::Str("2.0.0".into()));
}

#[test]
fn short_circuit_skips_the_right_side() {
	// `undefined_thing` would error if it were evaluated.
	assert_eq!(v("false && undefined_thing"), Value::Bool(false));
	assert_eq!(v("true || undefined_thing"), Value::Bool(true));
}

#[test]
fn conditions_must_be_boolean() {
	assert!(boom(r#""yes" && true"#).contains("true or false"));
}

#[test]
fn one_of_validates_and_lists_the_options() {
	assert_eq!(
		v(r#"one_of(ARG.env, "development", "production")"#),
		Value::Str("production".into())
	);
	let msg = boom(r#"one_of(ARG.count, "a", "b")"#);
	assert!(msg.contains("a, b"), "the error lists valid options: {msg}");
}

#[test]
fn try_recovers_to_an_empty_string() {
	assert_eq!(v("try(ARG.nope)"), Value::Str(String::new()));
	assert_eq!(v(r#"try(ARG.nope) ? "fallback""#), Value::Str("fallback".into()));
}

#[test]
fn interpolation_quotes_itself_and_expands_lists() {
	use crate::ast::InterpPart;
	use crate::eval::interpolate_shell;
	let mut s = sc();
	s.vars.insert("f".into(), Value::Str("a file.txt".into()));
	s.vars.insert(
		"many".into(),
		Value::List(vec![Value::Str("a b".into()), Value::Str("c".into())]),
	);

	let one = parse_expr("f", 0, 1).unwrap();
	let got = interpolate_shell(&[InterpPart::Literal("rm ".into()), InterpPart::Expr(one)], &mut s).unwrap();
	assert_eq!(got, "rm 'a file.txt'", "a string is one quoted argument");

	let list = parse_expr("many", 0, 1).unwrap();
	let got = interpolate_shell(&[InterpPart::Literal("rm ".into()), InterpPart::Expr(list)], &mut s).unwrap();
	assert_eq!(got, "rm 'a b' c", "a list is several arguments, each quoted");
}

#[test]
fn shell_quoting_defuses_expansion() {
	use crate::value::shell_quote;
	assert_eq!(shell_quote("$HOME"), "'$HOME'", "a bare $ would otherwise expand");
	assert_eq!(shell_quote("it's"), r#"'it'\''s'"#);
	assert_eq!(shell_quote("plain.txt"), "plain.txt", "nothing to quote");
}

#[test]
fn every_exported_function_name_is_actually_dispatched() {
	// The exported list drives editor completion. A name here that `call` does
	// not know would be offered and then fail, so the list is checked against
	// the dispatcher rather than trusted.
	for f in crate::functions::FUNCTIONS {
		let name = f.name;
		// Filesystem functions are matched on name *and* arity, so a name is
		// only unknown if every plausible arity rejects it. Any other outcome
		// -- a type complaint, a missing file -- means it was recognised.
		let known = (0..=4).any(|n| {
			let args = vec!["\"x\""; n].join(", ");
			let e = parse_expr(&format!("{name}({args})"), 0, 1).unwrap_or_else(|x| panic!("{name}: {x}"));
			// Calling every function for real means calling the writing ones
			// for real: this probe used to leave a file named `x` in the crate
			// root and a trail of temp artifacts. `dry_run` makes them no-ops
			// while still proving the name dispatches, since the dry-run arms
			// are dispatch arms too.
			let mut sc = sc();
			sc.dry_run = true;
			sc.base_dir = std::env::temp_dir();
			!matches!(eval(&e, &mut sc), Err(crate::EvalError::UnknownFunction { .. }))
		});
		assert!(known, "`{name}` is exported for completion but not dispatched");
	}
}

#[test]
fn number_of_a_number_is_itself() {
	assert_eq!(v("number(3)"), Value::Num(3.0));
	assert_eq!(v("number(length([1, 2]))"), Value::Num(2.0));
	assert_eq!(v("number(\"4.5\")"), Value::Num(4.5));
}

#[test]
fn every_dispatched_function_name_is_also_exported() {
	// The counterpart of the test above, and the one that was missing: `min`
	// worked but was absent from the list, so completion never offered it and
	// nothing noticed. Reads the dispatchers' own match arms.
	//
	// Bounded to `call` and `call_io`, because other functions in the file match
	// on strings too -- `now_formatted` on its format names -- and those are not
	// function names.
	let src = include_str!("../functions.rs");
	let mut missing: Vec<String> = Vec::new();
	let mut seen = 0usize;
	let mut inside = false;
	for line in src.lines() {
		if line.starts_with("fn ") || line.starts_with("pub fn ") || line.starts_with("pub(crate) fn ") {
			inside = line.contains(" call(") || line.contains(" call_io(");
			continue;
		}
		if !inside {
			continue;
		}
		let Some(rest) = line.trim().strip_prefix('"') else {
			continue;
		};
		let Some((name, after)) = rest.split_once('"') else {
			continue;
		};
		// A dispatch arm: `"name" => …` or `"name" if … => …`.
		if !after.contains("=>") {
			continue;
		}
		if !name
			.chars()
			.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
		{
			continue;
		}
		seen += 1;
		if !crate::functions::FUNCTIONS.iter().any(|f| f.name == name) && !missing.iter().any(|m| m == name) {
			missing.push(name.to_string());
		}
	}
	// A scan that stops matching would pass by finding nothing at all.
	assert!(
		seen > 40,
		"the scan found only {seen} dispatch arms; it has stopped working"
	);
	assert!(
		missing.is_empty(),
		"dispatched but not exported for completion: {missing:?}"
	);
}

#[test]
fn min_and_max_both_exist() {
	assert_eq!(v("min(3, 1, 2)"), Value::Num(1.0));
	assert_eq!(v("max(3, 1, 2)"), Value::Num(3.0));
}

#[test]
fn join_path_uses_the_platform_separator() {
	let joined = v("join_path(\"a\", \"b\", \"c\")");
	let expected = std::path::Path::new("a").join("b").join("c");
	assert_eq!(joined, Value::Str(expected.to_string_lossy().into_owned()));
	// An absolute later segment replaces what came before, as `Path::join` does.
	assert_eq!(v("join_path(\"a\", \"/b\")"), Value::Str("/b".into()));
}

// ---- the functions that were missed, restored

#[test]
fn capitalize_titles_every_word() {
	assert_eq!(
		v("capitalize(\"hello wide world\")"),
		Value::Str("Hello Wide World".into())
	);
	assert_eq!(v("capitalize(\"\")"), Value::Str(String::new()));
	// Only the first character changes; the rest of a word is left alone.
	assert_eq!(v("capitalize(\"iPhone x\")"), Value::Str("IPhone X".into()));
}

#[test]
fn substring_counts_characters_not_bytes() {
	assert_eq!(v("substring(\"hello\", 1)"), Value::Str("ello".into()));
	assert_eq!(v("substring(\"hello\", 1, 3)"), Value::Str("ell".into()));
	// A multi-byte character must not be cut in half.
	assert_eq!(v("substring(\"héllo\", 1, 2)"), Value::Str("él".into()));
	assert_eq!(v("substring(\"hi\", 9)"), Value::Str(String::new()));
}

#[test]
fn escape_renders_a_string_on_one_line() {
	assert_eq!(v("escape(\"a\\nb\")"), Value::Str("a\\nb".into()));
	assert_eq!(v("escape(\"say \\\"hi\\\"\")"), Value::Str("say \\\"hi\\\"".into()));
}

#[test]
fn repeat_refuses_an_absurd_count() {
	assert_eq!(v("repeat(\"ab\", 3)"), Value::Str("ababab".into()));
	assert_eq!(v("repeat(\"ab\", 0)"), Value::Str(String::new()));
	assert!(boom("repeat(\"ab\", 100000000)").contains("more than"));
	assert!(boom("repeat(\"ab\", -1)").contains("non-negative"));
}

#[test]
fn url_encoding_round_trips() {
	assert_eq!(v("url_encode(\"a b/c?d\")"), Value::Str("a%20b%2Fc%3Fd".into()));
	assert_eq!(v("url_decode(\"a%20b%2Fc\")"), Value::Str("a b/c".into()));
	assert_eq!(v("url_decode(url_encode(\"héllo &=\"))"), Value::Str("héllo &=".into()));
	assert!(boom("url_decode(\"a%zz\")").contains("percent-encoding"));
}

#[test]
fn the_hashes_match_their_published_vectors() {
	assert_eq!(
		v("sha256(\"abc\")"),
		Value::Str("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into())
	);
	assert_eq!(v("md5(\"abc\")"), Value::Str("900150983cd24fb0d6963f7d28e17f72".into()));
}

#[test]
fn a_uuid_looks_like_one_and_never_repeats() {
	let Value::Str(a) = v("uuid()") else { panic!("a string") };
	let Value::Str(b) = v("uuid()") else { panic!("a string") };
	assert_ne!(a, b);
	assert_eq!(a.len(), 36);
	let parts: Vec<&str> = a.split('-').collect();
	assert_eq!(parts.iter().map(|p| p.len()).collect::<Vec<_>>(), [8, 4, 4, 4, 12]);
	assert!(a.starts_with(|c: char| c.is_ascii_hexdigit()));
	assert_eq!(&parts[2][..1], "4", "version 4");
	assert!("89ab".contains(&parts[3][..1]), "variant 1: {a}");
}

#[test]
fn now_formats_the_clock_without_a_date_library() {
	let Value::Str(iso) = v("now()") else {
		panic!("a string")
	};
	assert_eq!(iso.len(), 20, "{iso}");
	assert!(iso.ends_with('Z') && iso.contains('T'), "{iso}");
	let Value::Str(unix) = v("now(\"unix\")") else {
		panic!("a string")
	};
	assert!(unix.parse::<u64>().unwrap() > 1_700_000_000, "{unix}");
	assert_eq!(v("now(\"date\")").to_string().len(), 10);
	assert!(boom("now(\"tuesday\")").contains("unknown time format"));
}

#[test]
fn the_calendar_conversion_matches_known_dates() {
	// The part of `now` worth pinning: the epoch, a leap day, and a year
	// boundary, none of which a clock-reading test would ever exercise.
	for (secs, expect) in [
		(0i64, (1970, 1, 1, 0, 0, 0)),
		(951_782_400, (2000, 2, 29, 0, 0, 0)),
		(1_709_164_800, (2024, 2, 29, 0, 0, 0)),
		(1_735_689_599, (2024, 12, 31, 23, 59, 59)),
	] {
		assert_eq!(crate::functions::civil_parts_for_test(secs), expect, "at {secs}");
	}
}

#[test]
fn json_get_reads_a_dotted_path() {
	let doc = "{\"users\": [{\"name\": \"ana\", \"admin\": true}], \"n\": 2}";
	assert_eq!(
		v(&format!("json_get(\"{}\", \"users.0.name\")", doc.replace('"', "\\\""))),
		Value::Str("ana".into())
	);
	let g = |p: &str| v(&format!("json_get(\"{}\", \"{p}\")", doc.replace('"', "\\\"")));
	assert_eq!(g("n"), Value::Num(2.0), "a number comes back as one");
	assert_eq!(g("users.0.admin"), Value::Bool(true));
	assert!(boom(&format!("json_get(\"{}\", \"nope\")", doc.replace('"', "\\\""))).contains("no value at"));
	assert!(boom("json_get(\"not json\", \"a\")").contains("json_get"));
}

#[test]
fn json_set_writes_a_path_and_builds_what_is_missing() {
	let set = |doc: &str, path: &str, val: &str| {
		v(&format!(
			"json_set(\"{}\", \"{path}\", \"{}\")",
			doc.replace('"', "\\\""),
			val.replace('"', "\\\"")
		))
	};
	assert_eq!(
		set("{\"a\":1}", "a", "2"),
		Value::Str("{\"a\":2}".into()),
		"a value that parses as JSON goes in as JSON"
	);
	assert_eq!(
		set("{}", "name", "bob"),
		Value::Str("{\"name\":\"bob\"}".into()),
		"and anything else as a string"
	);
	assert_eq!(
		set("{}", "a.b", "1"),
		Value::Str("{\"a\":{\"b\":1}}".into()),
		"objects are created on demand"
	);
	assert_eq!(
		set("{}", "a.0", "1"),
		Value::Str("{\"a\":[1]}".into()),
		"a numeric segment makes an array"
	);
}

#[test]
fn power_rejects_a_result_that_is_not_a_number() {
	assert_eq!(v("power(2, 10)"), Value::Num(1024.0));
	assert_eq!(v("power(9, 0.5)"), Value::Num(3.0));
	assert!(boom("power(0, -1)").contains("finite"));
}

#[test]
fn an_env_name_matches_ignoring_case() {
	// Windows environment variables are case-insensitive and POSIX ones are
	// not, so one file has to work on both. Six targets in the corpus read
	// `ENV.port` against a `PORT=` line, and an exact-only lookup broke them.
	let mut s = sc();
	s.env.insert("PORT".into(), "4003".into());
	let e = parse_expr("ENV.port", 0, 1).unwrap();
	assert_eq!(eval_boundary(&e, &mut s).unwrap(), Value::Str("4003".into()));
}

#[test]
fn an_exact_env_name_still_wins() {
	// Only reachable on a platform that allows both, but the fallback must not
	// make the exact one ambiguous.
	let mut s = sc();
	s.env.insert("Path".into(), "mixed".into());
	s.env.insert("PATH".into(), "upper".into());
	let e = parse_expr("ENV.PATH", 0, 1).unwrap();
	assert_eq!(eval_boundary(&e, &mut s).unwrap(), Value::Str("upper".into()));
}

#[test]
fn a_genuinely_absent_env_name_is_still_an_error() {
	let mut s = sc();
	s.env.insert("PORT".into(), "1".into());
	let e = parse_expr("ENV.nowhere", 0, 1).unwrap();
	assert!(eval(&e, &mut s).is_err());
}

#[test]
fn exit_carries_a_status_out_rather_than_producing_a_value() {
	// It leaves as an error because that is the only way out of an
	// expression. What it carries is a status, not a message.
	let at = |src: &str| {
		let e = parse_expr(src, 0, 1).unwrap_or_else(|x| panic!("{src}: {x}"));
		match eval(&e, &mut sc()) {
			Err(crate::EvalError::Exit { code, .. }) => code,
			other => panic!("{src}: expected an exit, got {other:?}"),
		}
	};
	assert_eq!(at("exit()"), 0, "no argument means 0");
	assert_eq!(at("exit(0)"), 0);
	assert_eq!(at("exit(3)"), 3);
	// Truncation to 255 is the operating system's, so the value is carried
	// whole and a test of it does not depend on the platform.
	assert_eq!(at("exit(-1)"), -1);
	assert_eq!(
		at("exit(number(ARG.count))"),
		3,
		"the argument is an ordinary expression"
	);
}

#[test]
fn nothing_catches_an_exit() {
	// `try` and `?` are for failures to fall back from. This is not one: a
	// target that says stop must stop.
	for src in ["try(exit(4))", "exit(4) ? 9"] {
		let e = parse_expr(src, 0, 1).unwrap();
		match eval(&e, &mut sc()) {
			Err(crate::EvalError::Exit { code: 4, .. }) => {}
			other => panic!("{src} swallowed it: {other:?}"),
		}
	}
}

#[test]
fn exit_takes_at_most_one_argument() {
	assert!(boom("exit(1, 2)").contains("0 or 1 arguments"));
}

#[test]
fn a_function_named_without_parentheses_says_so() {
	// Every call is written with parentheses, `exit()` included. A bare name
	// is a call someone forgot to finish, and reporting it as an unknown
	// binding would send them looking for a `let` that was never missing.
	for name in ["exit", "uuid", "now"] {
		let e = parse_expr(name, 0, 1).unwrap();
		let msg = eval(&e, &mut sc()).expect_err(name).to_string();
		assert!(msg.contains(&format!("call it as `{name}()`")), "{name}: {msg}");
	}
	assert!(
		boom("nope").contains("is not defined"),
		"an unknown name still reads that way"
	);
}

#[test]
fn a_binding_may_be_named_after_a_function() {
	// The check is at evaluation, not parsing, so a bound name is found first
	// and `let first = …` stays legal.
	let mut s = sc();
	s.vars.insert("first".into(), Value::Num(1.0));
	let e = parse_expr("first", 0, 1).unwrap();
	assert_eq!(eval(&e, &mut s).unwrap(), Value::Num(1.0));
}

#[test]
fn a_case_label_must_be_quoted() {
	// `RUN.os` is a string like any other, so a label compared against it is
	// a string too -- one rule, not a bare-word exception.
	let e = crate::parse(
		"match RUN.os
case linux
$ a
end
",
	)
	.unwrap_err()
	.to_string();
	assert!(e.contains("must be quoted"), "{e}");
	assert!(e.contains("case \"linux\""), "the fix is in the message: {e}");
	assert!(
		crate::parse(
			"match RUN.os
case \"linux\"
$ a
end
"
		)
		.is_ok()
	);
}

#[test]
fn a_statement_that_computes_and_discards_is_rejected() {
	// A line that is only a value is always a mistake -- most often a call
	// with the parentheses left off. Caught at parsing, so an editor
	// underlines it rather than a run finding it later.
	for (src, want) in [
		("exit\n", "call it as `exit()`"),
		("uuid\n", "call it as `uuid()`"),
		("abc\n", "`abc` is a value, not something to run"),
		("35\n", "computes a value and discards it"),
		("\"hello\"\n", "computes a value and discards it"),
		("true\n", "computes a value and discards it"),
		("[1, 2]\n", "computes a value and discards it"),
		("x + 1\n", "computes a value and discards it"),
		("ARG.x\n", "an input is a value"),
		("x[0]\n", "computes a value and discards it"),
	] {
		let e = crate::parse(src).unwrap_err().to_string();
		assert!(e.contains(want), "{src:?}: wanted {want:?}, got {e}");
		assert!(
			e.contains("does nothing") || e.contains("is a function"),
			"{src:?}: {e}"
		);
	}
}

#[test]
fn a_statement_that_reaches_a_call_is_kept() {
	// Only a call can do anything, but it need not be the whole expression:
	// a chain reaches one, and so does either side of `&&`.
	for src in [
		"write_file(\"a\", \"b\")\n",
		"decrypt(ARG.x) ? \"fallback\"\n",
		"file_exists(\"a\") && write_file(\"b\", \"c\")\n",
		"error(\"stop\")\n",
		"exit()\n",
		"exit(1)\n",
		// A statement in a block is checked the same way.
		"if true\n\twrite_file(\"a\", \"b\")\nend\n",
		// A call anywhere is enough: this one is pointless, but the rule is
		// deliberately conservative -- rejecting it would mean deciding which
		// functions are pure, and `write_file(…)[0]` is the same shape.
		"split(\"a\", \"b\")[0]\n",
	] {
		crate::parse(src).unwrap_or_else(|e| panic!("{src:?}: {e}"));
	}
}

#[test]
fn an_inert_statement_is_caught_inside_a_block_too() {
	let e = crate::parse("if true\n\texit\nend\n").unwrap_err().to_string();
	assert!(e.contains("line 2"), "the line is the one at fault: {e}");
	assert!(e.contains("exit()"), "{e}");
}
