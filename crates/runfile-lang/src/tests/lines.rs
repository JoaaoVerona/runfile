use crate::ast::*;
use crate::parse;

fn t(s: &str) -> Target {
	parse(s).unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn leading_comment_block_is_the_description() {
	let x = t("# Short form\n# more detail\n\n$ echo hi\n");
	assert_eq!(x.description.as_deref(), Some("Short form\nmore detail"));
}

#[test]
fn editor_directives_are_not_description() {
	let x = t("# shellcheck disable=SC2086\n# Real description\n$ echo hi\n");
	assert_eq!(x.description.as_deref(), Some("Real description"));
}

#[test]
fn consecutive_shell_lines_are_one_process() {
	let x = t("$ cd web\n$ pnpm install\n");
	assert_eq!(x.body.statements.len(), 1);
	let Statement::Exec { command, body, .. } = &x.body.statements[0] else { panic!() };
	assert!(command.is_none(), "default shell");
	assert_eq!(body.len(), 2, "both lines share one shell, so cd persists");
}

#[test]
fn blanks_and_comments_do_not_split_a_shell_run() {
	// Reformatting a file must not silently change what shares a shell.
	let x = t("$ cd web\n\n# a note\n\n$ pnpm install\n");
	assert_eq!(x.body.statements.len(), 1);
}

#[test]
fn a_statement_does_split_a_shell_run() {
	let x = t("$ cd web\nlet v = \"1\"\n$ pnpm install\n");
	assert_eq!(x.body.statements.len(), 3);
}

#[test]
fn backslash_continues_a_shell_line() {
	let x = t("$ tar -c f \\\n    | docker run x\n$ echo done\n");
	let Statement::Exec { body, .. } = &x.body.statements[0] else { panic!() };
	assert_eq!(body.len(), 2, "the continuation is part of the first line");
}

#[test]
fn properties_attach_to_their_block() {
	let x = t(".parallel\nfor c in glob(\"*\")\n\t.ignore-errors\n\t$ echo {{ c }}\nend\n");
	assert_eq!(x.body.properties.len(), 1);
	let Statement::For { body, .. } = &x.body.statements[0] else { panic!() };
	assert_eq!(body.properties.len(), 1, "block-scoped, not hoisted");
}

#[test]
fn dotted_property_names_address_a_namespace() {
	let x = t(".env.MSYS_NO_PATHCONV = \"1\"\n$ echo hi\n");
	assert_eq!(x.body.properties[0].path, vec!["env", "MSYS_NO_PATHCONV"]);
}

#[test]
fn bare_property_has_no_value() {
	let x = t(".hide\n$ echo hi\n");
	assert!(x.body.properties[0].value.is_none());
}

#[test]
fn assignment_is_distinguished_from_comparison_and_calls() {
	let x = t("let a = 1\na = 2\nfoo(a)\n");
	assert!(matches!(x.body.statements[0], Statement::Let { .. }));
	assert!(matches!(x.body.statements[1], Statement::Assign { .. }));
	assert!(matches!(x.body.statements[2], Statement::Call { .. }));
}

#[test]
fn lists_may_span_lines() {
	let x = t("for s in [\n\t\"a\",\n\t\"b\",\n]\n\t$ echo {{ s }}\nend\n");
	let Statement::For { iter: Expr::List(items, _), .. } = &x.body.statements[0] else { panic!() };
	assert_eq!(items.len(), 2);
}

#[test]
fn run_is_in_process_dispatch_with_interpolated_target() {
	let x = t("run {{ ns }}:build --flag {{ v }}\n");
	let Statement::Run { target, args, .. } = &x.body.statements[0] else { panic!() };
	assert_eq!(args.len(), 2);
	assert!(matches!(target[0], InterpPart::Expr(_)));
}

#[test]
fn unclosed_block_names_the_opening_line() {
	let e = parse("for x in [1]\n\t$ echo hi\n").unwrap_err();
	assert!(e.to_string().contains("line 1"), "{e}");
}
