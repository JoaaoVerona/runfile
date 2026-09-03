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
