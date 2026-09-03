//! Line classification and the shapes a statement may take.

#[test]
fn a_binding_with_no_name_is_rejected() {
	// `let = 3` used to parse, binding the empty name.
	let e = crate::parse("let = 3\n").unwrap_err().to_string();
	assert!(e.contains("binding has no name"), "{e}");
	// A bare `= 3` never reaches the binding path -- nothing precedes the `=`,
	// so it is read as an expression and the lexer rejects it first. Different
	// message, same outcome.
	let e = crate::parse("= 3\n").unwrap_err().to_string();
	assert!(e.contains('='), "{e}");
}

#[test]
fn a_binding_name_must_look_like_a_name() {
	let e = crate::parse("let 2fast = 3\n").unwrap_err().to_string();
	assert!(e.contains("not a valid name"), "{e}");
}

#[test]
fn ordinary_binding_names_still_parse() {
	for name in ["x", "_x", "my-var", "a1"] {
		crate::parse(&format!("let {name} = 1\n")).unwrap_or_else(|e| panic!("{name}: {e}"));
	}
}

#[test]
fn a_block_closer_with_nothing_open_is_rejected() {
	// These used to parse as expression statements and fail much later with an
	// unrelated "not defined" message.
	for kw in ["end", "else", "case x", "default"] {
		let src = format!("$ true\n{kw}\n");
		let e = crate::parse(&src).unwrap_err().to_string();
		assert!(e.contains("closes a block, but none is open"), "{kw}: {e}");
	}
}

#[test]
fn closers_inside_their_own_block_are_unaffected() {
	crate::parse("if FLAG.x\n\t$ a\nelse\n\t$ b\nend\n").unwrap();
	crate::parse("match ARGS\ncase \"a\"\n\t$ a\ndefault\n\t$ b\nend\n").unwrap();
}

/// Every literal piece of shell text in a block, commands and bodies alike,
/// walking into control flow.
fn shell_texts(block: &crate::Block) -> Vec<String> {
	use crate::{InterpPart, Statement};
	let lit = |parts: &[InterpPart]| -> String {
		parts
			.iter()
			.map(|p| match p {
				InterpPart::Literal(s) => s.clone(),
				InterpPart::Expr(_) => "{{}}".to_string(),
			})
			.collect()
	};
	let mut out = Vec::new();
	for st in &block.statements {
		match st {
			Statement::Exec { command, body, .. } => {
				if let Some(c) = command {
					out.push(lit(c));
				}
				out.extend(body.iter().map(|l| lit(l)));
			}
			Statement::If { then, otherwise, .. } => {
				out.extend(shell_texts(then));
				if let Some(o) = otherwise {
					out.extend(shell_texts(o));
				}
			}
			Statement::For { body, .. } => out.extend(shell_texts(body)),
			Statement::Match { cases, default, .. } => {
				for c in cases {
					out.extend(shell_texts(&c.body));
				}
				if let Some(d) = default {
					out.extend(shell_texts(d));
				}
			}
			_ => {}
		}
	}
	out
}

#[test]
fn a_crlf_file_parses_exactly_like_its_lf_twin() {
	// The carriage return is line ending, not content. It used to reach exec
	// bodies as text, and made an indented `end` compare unequal to its
	// opener's indentation, so the block never closed.
	let lf = "# Described\nif true\n\texec sh\n\t\techo hi\n\tend\nend\n$ echo after\n";
	let crlf = lf.replace('\n', "\r\n");
	let a = crate::parse(lf).unwrap();
	let b = crate::parse(&crlf).unwrap_or_else(|e| panic!("CRLF: {e}"));
	assert_eq!(a.description, b.description);
	assert_eq!(shell_texts(&a.body), shell_texts(&b.body));
	assert!(
		shell_texts(&b.body).iter().all(|s| !s.contains('\r')),
		"no carriage return may reach a shell: {:?}",
		shell_texts(&b.body)
	);
}

#[test]
fn a_crlf_file_still_reports_the_right_line_numbers() {
	let e = crate::parse("$ ok\r\nlet = 3\r\n").unwrap_err().to_string();
	assert!(e.starts_with("line 2:"), "{e}");
}
