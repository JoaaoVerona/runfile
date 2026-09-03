use crate::ast::*;
use crate::parse;

fn t(s: &str) -> Target {
	parse(s).unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn exec_takes_an_arbitrary_command_not_just_a_shell() {
	let x = t("exec sudo tee /etc/conf\n\t[Journal]\n\tStorage=persistent\nend\n");
	let Statement::Exec {
		command: Some(cmd),
		body,
		..
	} = &x.body.statements[0]
	else {
		panic!()
	};
	let InterpPart::Literal(c) = &cmd[0] else { panic!() };
	assert_eq!(c, "sudo tee /etc/conf");
	assert_eq!(body.len(), 2);
}

#[test]
fn body_is_dedented_by_its_own_base_indent() {
	// Without this, `exec sudo tee file` writes a config full of leading tabs.
	let x = t("exec sudo tee f\n\t[Journal]\n\tStorage=persistent\nend\n");
	let Statement::Exec { body, .. } = &x.body.statements[0] else {
		panic!()
	};
	let InterpPart::Literal(first) = &body[0][0] else {
		panic!()
	};
	assert_eq!(first, "[Journal]", "no leading tab reaches the command");
}

#[test]
fn a_body_containing_end_does_not_close_the_block_early() {
	// The defect: `exec ruby` closed on the `end` of a Ruby method.
	let x = t("exec ruby\n\tdef f\n\t\tputs 1\n\tend\nend\n$ echo after\n");
	assert_eq!(x.body.statements.len(), 2, "the ruby end is body, not terminator");
	let Statement::Exec { body, .. } = &x.body.statements[0] else {
		panic!()
	};
	assert_eq!(body.len(), 3);
}

#[test]
fn indentation_style_is_not_mandated() {
	for indent in ["  ", "    ", "\t", "\t\t"] {
		let src = format!("exec ruby\n{indent}def f\n{indent}end\nend\n");
		let x = t(&src);
		let Statement::Exec { body, .. } = &x.body.statements[0] else {
			panic!()
		};
		assert_eq!(body.len(), 2, "indent {indent:?}");
	}
}

#[test]
fn nested_exec_closes_at_its_own_indentation() {
	let x = t("for i in [1]\n\texec ruby\n\t\tdef f\n\t\tend\n\tend\nend\n");
	let Statement::For { body, .. } = &x.body.statements[0] else {
		panic!()
	};
	assert_eq!(body.statements.len(), 1);
}

#[test]
fn mixing_indent_styles_within_one_block_is_an_error() {
	// Tabbed opener, spaced closer: an inconsistency inside one block, which
	// should fail loudly rather than mis-terminate somewhere later.
	let e = parse("for x in [1]\n\texec sh\n\t\techo hi\n    end\nend\n").unwrap_err();
	assert!(e.to_string().contains("never closed"), "{e}");
}

#[test]
fn exec_bodies_interpolate() {
	let x = t("exec node\n\tconsole.log(\"{{ v }}\")\nend\n");
	let Statement::Exec { body, .. } = &x.body.statements[0] else {
		panic!()
	};
	assert!(body[0].iter().any(|p| matches!(p, InterpPart::Expr(_))));
}
