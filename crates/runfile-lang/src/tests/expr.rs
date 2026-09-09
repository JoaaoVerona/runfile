use crate::ast::*;
use crate::parser::parse_expr;

fn p(s: &str) -> Expr {
	parse_expr(s, 0, 1).unwrap_or_else(|e| panic!("{s:?}: {e}"))
}

#[test]
fn precedence_is_c_like() {
	let Expr::Binary {
		op: BinaryOp::Add, rhs, ..
	} = p("1 + 2 * 3")
	else {
		panic!()
	};
	assert!(matches!(*rhs, Expr::Binary { op: BinaryOp::Mul, .. }));
}

#[test]
fn comparison_binds_looser_than_arithmetic() {
	assert!(matches!(p("1 + 1 == 2"), Expr::Binary { op: BinaryOp::Eq, .. }));
}

#[test]
fn chain_is_the_loosest() {
	assert!(matches!(p("ARG.a ? ENV.B ? \"d\""), Expr::Chain { .. }));
}

#[test]
fn quotes_inside_an_interpolation_do_not_end_the_string() {
	// The defect that failed five real files: a regex-based lexer stops at the
	// quote before `development`.
	let e = p(r#""a.{{ one_of(ARG.env, "development", "production") }}.b""#);
	let Expr::Str(parts, _) = e else { panic!("not a string") };
	assert_eq!(parts.len(), 3, "literal, interpolation, literal");
	assert!(matches!(parts[1], InterpPart::Expr(Expr::Call { .. })));
}

#[test]
fn raw_strings_keep_the_backslash_but_still_terminate_on_a_bare_quote() {
	// Python's rule. Fully inert backslashes would make a quote impossible in a
	// regex, which the release target needs.
	let Expr::Str(parts, _) = p(r#"r"(?m)^version = \"([^\"]+)\"""#) else {
		panic!()
	};
	let InterpPart::Literal(t) = &parts[0] else { panic!() };
	assert!(t.contains(r#"\""#), "backslash retained in value: {t:?}");
}

#[test]
fn escapes_are_processed_in_normal_strings() {
	let Expr::Str(parts, _) = p(r#""a\"b\nc""#) else {
		panic!()
	};
	let InterpPart::Literal(t) = &parts[0] else { panic!() };
	assert_eq!(t, "a\"b\nc");
}

#[test]
fn sources_and_lists() {
	assert!(matches!(
		p("ARGS"),
		Expr::Source {
			kind: SourceKind::Args,
			..
		}
	));
	assert!(matches!(
		p("RUN.namespaces"),
		Expr::Source {
			kind: SourceKind::Run,
			..
		}
	));
	let Expr::List(items, _) = p(r#"["a", ["b"], 1.5, true]"#) else {
		panic!()
	};
	assert_eq!(items.len(), 4);
}

#[test]
fn index_and_call_bind_tightest() {
	assert!(matches!(
		p("number(parts[0]) + 1"),
		Expr::Binary { op: BinaryOp::Add, .. }
	));
}

#[test]
fn a_bare_source_prefix_is_rejected() {
	assert!(parse_expr("ARG", 0, 1).is_err());
}

#[test]
fn a_list_nests_to_any_depth() {
	let Expr::List(items, _) = p(r#"[[1, 2], [3, 4, [5, 6]], 7, "a"]"#) else {
		panic!()
	};
	assert_eq!(items.len(), 4);
	let Expr::List(row, _) = &items[1] else { panic!() };
	assert_eq!(row.len(), 3);
	assert!(matches!(row[2], Expr::List(..)));
	assert!(matches!(items[2], Expr::Number(..)));
	assert!(matches!(items[3], Expr::Str(..)));
}

#[test]
fn an_empty_list_nests_too() {
	let Expr::List(items, _) = p("[[], [[]]]") else {
		panic!()
	};
	let Expr::List(inner, _) = &items[0] else { panic!() };
	assert!(inner.is_empty());
	let Expr::List(outer, _) = &items[1] else { panic!() };
	assert!(matches!(outer[0], Expr::List(..)));
}

#[test]
fn an_element_is_a_whole_expression() {
	// Nothing about a list restricts what may sit in one, and that holds at
	// every level.
	let Expr::List(items, _) = p(r#"[[1 + 2, concat("a", "b")], [ARG.x ? "d"]]"#) else {
		panic!()
	};
	let Expr::List(row, _) = &items[0] else { panic!() };
	assert!(matches!(row[0], Expr::Binary { .. }));
	assert!(matches!(row[1], Expr::Call { .. }));
}

#[test]
fn indexing_chains_through_the_levels() {
	// `xs[1][2]` is an index of an index, not a two-argument form.
	let Expr::Index { base, .. } = p("xs[1][2]") else {
		panic!()
	};
	assert!(matches!(*base, Expr::Index { .. }));
}

#[test]
fn a_list_literal_can_be_indexed_where_it_stands() {
	assert!(matches!(p("[[1, 2]][0][1]"), Expr::Index { .. }));
}
