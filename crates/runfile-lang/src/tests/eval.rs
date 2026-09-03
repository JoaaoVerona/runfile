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
	for name in crate::functions::FUNCTIONS {
		// Filesystem functions are matched on name *and* arity, so a name is
		// only unknown if every plausible arity rejects it. Any other outcome
		// -- a type complaint, a missing file -- means it was recognised.
		let known = (0..=4).any(|n| {
			let args = vec!["\"x\""; n].join(", ");
			let e = parse_expr(&format!("{name}({args})"), 0, 1).unwrap_or_else(|x| panic!("{name}: {x}"));
			!matches!(eval(&e, &mut sc()), Err(crate::EvalError::UnknownFunction { .. }))
		});
		assert!(known, "`{name}` is exported for completion but not dispatched");
	}
}
