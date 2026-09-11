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

#[test]
fn a_comment_does_not_change_what_a_target_fingerprints_as() {
	// The prepare gate re-triggers on a change to its setup target. Reflowing a
	// comment is not one, and hashing the file text said it was.
	let a = crate::parse("# Sets up\n$ install\n").unwrap();
	let b = crate::parse("# Sets up, at length\n#\n# Really.\n\n$ install\n").unwrap();
	assert_eq!(crate::fingerprint(&a), crate::fingerprint(&b));
}

#[test]
fn changing_what_a_target_does_changes_its_fingerprint() {
	let a = crate::parse("$ install\n").unwrap();
	for other in [
		"$ install --force\n",
		"$ uninstall\n",
		"$ install\n$ verify\n",
		"let x = 1\n$ install\n",
		// A field left out of the fingerprint at its default is still in it
		// when a file asks for it: detaching the command is a different target.
		"detach $ install\n",
	] {
		let b = crate::parse(other).unwrap();
		assert_ne!(crate::fingerprint(&a), crate::fingerprint(&b), "{other:?}");
	}
}

#[test]
fn an_unchanged_setup_keeps_the_fingerprint_it_was_recorded_with() {
	// A real `setup.run`, and the fingerprint the runner stored for it before
	// `Statement::Exec` grew `detach`. The field changes nothing about a file
	// that does not use it, yet rendered into the hash it moved every one --
	// and a prepare gate that trips on a runner upgrade asks every project on
	// the machine to set up again, for a change none of them made: 83 of the
	// author's 88 did. A field added the same way without an `UNASKED` entry
	// fails here, which is the point of pinning a hash from before it.
	let src = "# One-time per clone: activate the git hooks and install the desktop UI dependencies\n\n\
	           $ git config core.hooksPath .githooks\n\n\
	           do\n\t.workdir = \"libretether-desktop\"\n\n\t$ pnpm install\nend\n";
	let t = crate::parse(src).unwrap();
	assert_eq!(format!("{:x}", crate::fingerprint(&t)), "fc3ba8a69c88a636");
}

#[test]
fn a_comment_inside_a_shell_body_does_change_it() {
	// It is part of the text handed to the interpreter, not a note about it.
	let a = crate::parse("exec bash\n\techo hi\nend\n").unwrap();
	let b = crate::parse("exec bash\n\t# why\n\techo hi\nend\n").unwrap();
	assert_ne!(crate::fingerprint(&a), crate::fingerprint(&b));
}

#[test]
fn a_comment_shifts_every_position_after_it_and_none_of_them_count() {
	// Two kinds of positional detail live in the tree: spans, and the source
	// line of each `exec` body line. A comment moves both.
	let a = crate::parse("$ one\nexec sh\n\ttwo\nend\n").unwrap();
	let b = crate::parse("# note\n#\n$ one\nexec sh\n\ttwo\nend\n").unwrap();
	assert_eq!(crate::fingerprint(&a), crate::fingerprint(&b));
}

// ---- loops, and the two ways out of one

#[test]
fn break_and_continue_are_refused_outside_a_loop() {
	// Whether a line sits inside a loop is a question about the text, so it is
	// answered here and underlined in an editor rather than by a run that has
	// already done half the work.
	for kw in ["break", "continue"] {
		for src in [
			format!("{kw}\n"),
			format!("if true\n\t{kw}\nend\n"),
			format!("retry 3\n\t{kw}\nend\n"),
			format!("do\n\t{kw}\nend\n"),
		] {
			let e = crate::parse(&src).unwrap_err().to_string();
			assert!(e.contains("only meaningful inside"), "{src:?}: {e}");
		}
	}
}

#[test]
fn break_and_continue_are_fine_anywhere_inside_one() {
	for body in [
		"for x in [1]\n\tbreak\nend\n",
		"while true\n\tcontinue\nend\n",
		"until false\n\tbreak\nend\n",
		"loop\n\tbreak\nend\n",
		// Nested in anything, as long as a loop is somewhere above it.
		"for x in [1]\n\tif x == 1\n\t\tcontinue\n\tend\nend\n",
		"loop\n\tretry 3\n\t\tbreak\n\tend\nend\n",
		"for x in [1]\n\tmatch x\n\tcase \"1\"\n\t\tbreak\n\tend\nend\n",
	] {
		crate::parse(body).unwrap_or_else(|e| panic!("{body:?}: {e}"));
	}
	// And the depth comes back down: a loop that has closed does not license
	// what follows it.
	let e = crate::parse("for x in [1]\n\t$ true\nend\nbreak\n")
		.unwrap_err()
		.to_string();
	assert!(e.contains("only meaningful inside"), "{e}");
}

#[test]
fn a_loop_keyword_takes_only_what_it_has_a_use_for() {
	let e = crate::parse("loop true\n\tbreak\nend\n").unwrap_err().to_string();
	assert!(e.contains("`loop` takes nothing"), "{e}");
	let e = crate::parse("while\n\t$ true\nend\n").unwrap_err().to_string();
	assert!(e.contains("needs a condition"), "{e}");
	let e = crate::parse("for x in [1]\n\tbreak now\nend\n")
		.unwrap_err()
		.to_string();
	assert!(e.contains("takes nothing after it"), "{e}");
}

#[test]
fn a_conditional_loop_may_be_scored_on_a_command() {
	// The same rule an `if` follows, and what makes `until $ cmd` the wait
	// loop the corpus wrote by hand four times.
	let t = crate::parse("until $ curl -sf localhost\n\tsleep(1)\nend\n").unwrap();
	let [crate::Statement::Loop { test, .. }] = &t.body.statements[..] else {
		panic!("{:?}", t.body.statements)
	};
	assert!(
		matches!(test.cond(), Some(crate::Expr::Capture { .. })),
		"the condition is the command's status"
	);
}

// ---- `else if`

#[test]
fn an_else_if_chain_is_one_block_closed_by_one_end() {
	use crate::ast::Statement;
	let t = crate::parse("if a\n\t$ one\nelse if b\n\t$ two\nelse if c\n\t$ three\nelse\n\t$ four\nend\n").unwrap();
	// Each `else if` nests inside the one before it, so a walker that knows
	// `if` knows a chain without being told about one.
	let mut depth = 0;
	let mut at = &t.body.statements[0];
	loop {
		let Statement::If { otherwise, .. } = at else {
			panic!("{at:?}")
		};
		let Some(b) = otherwise else { break };
		match &b.statements[..] {
			[inner @ Statement::If { .. }] => {
				depth += 1;
				at = inner;
			}
			// The bare `else` at the end.
			_ => break,
		}
	}
	assert_eq!(depth, 2, "two `else if`s");
	// One `end` for the lot: another statement after it is at file level.
	let t = crate::parse("if a\n\t$ one\nelse if b\n\t$ two\nend\n$ after\n").unwrap();
	assert_eq!(t.body.statements.len(), 2);
}

#[test]
fn an_else_with_anything_but_if_after_it_is_refused() {
	// The trailing text used to be dropped without a word, so `else x > 1` ran
	// its block unconditionally.
	let e = crate::parse("if a\n\t$ one\nelse b\n\t$ two\nend\n")
		.unwrap_err()
		.to_string();
	assert!(e.contains("takes nothing after it, or `if"), "{e}");
	let e = crate::parse("if a\n\t$ one\nelse if\n\t$ two\nend\n")
		.unwrap_err()
		.to_string();
	assert!(e.contains("needs a condition"), "{e}");
	// `ifx` is a name, not the keyword.
	let e = crate::parse("if a\n\t$ one\nelse ifx\n\t$ two\nend\n")
		.unwrap_err()
		.to_string();
	assert!(e.contains("takes nothing after it"), "{e}");
}

// ---- unpacking

#[test]
fn a_binding_may_name_several_positions() {
	use crate::ast::Statement;
	let t = crate::parse("let a, _, c = [1, 2, 3]\n").unwrap();
	let [Statement::Let { names, .. }] = &t.body.statements[..] else {
		panic!("{:?}", t.body.statements)
	};
	assert_eq!(names, &["a", "_", "c"]);

	// `_` is a position rather than a name, so it may be written as often as
	// it is useful and cannot collide with itself.
	crate::parse("let _, _, x = [1, 2, 3]\n").unwrap();
	crate::parse("x, y = [1, 2]\n").unwrap();
	crate::parse("for k, v in pairs\n\t$ echo {{ k }}\nend\n").unwrap();

	// Every name still has to look like one.
	let e = crate::parse("let a, 2fast = [1, 2]\n").unwrap_err().to_string();
	assert!(e.contains("not a valid name"), "{e}");
	let e = crate::parse("let a, = [1, 2]\n").unwrap_err().to_string();
	assert!(e.contains("binding has no name"), "{e}");
}

// ---- `detach`

/// Every `Exec` statement in a target, as (detached, body lines).
fn exec_shapes(src: &str) -> Vec<(bool, usize)> {
	let t = crate::parse(src).expect("parses");
	t.body
		.statements
		.iter()
		.filter_map(|st| match st {
			crate::Statement::Exec { detach, body, .. } => Some((*detach, body.len())),
			_ => None,
		})
		.collect()
}

#[test]
fn detach_marks_one_command_and_ends_the_run_it_opens() {
	// Contiguous `$` lines are one process, so folding on from a `detach $`
	// would silently detach whatever was written below it -- which is the
	// failure the marker exists to remove.
	assert_eq!(
		exec_shapes("$ a\n$ b\ndetach $ c\n$ d\n$ e\n"),
		vec![(false, 2), (true, 1), (false, 2)],
		"the pair, the detached one alone, then the pair below it"
	);
	// A backslash continuation is still one shell line, not a fold.
	assert_eq!(exec_shapes("detach $ a \\\n\tb\n$ c\n"), vec![(true, 1), (false, 1)]);
}

#[test]
fn detach_marks_an_exec_block_too() {
	assert_eq!(
		exec_shapes("detach exec node\n\tconsole.log(1)\nend\n"),
		vec![(true, 1)]
	);
	let t = crate::parse("detach exec node --harmony\n\tx\nend\n").expect("parses");
	let crate::Statement::Exec { command: Some(c), .. } = &t.body.statements[0] else {
		panic!("an exec block");
	};
	assert_eq!(
		c,
		&vec![crate::InterpPart::Literal("node --harmony".into())],
		"the marker is not part of the command"
	);
}

#[test]
fn detach_is_refused_on_a_dispatch() {
	// `run` happens in this process, so there is nothing to detach.
	let e = crate::parse("detach run build\n").unwrap_err().to_string();
	assert!(e.contains("dispatches in-process"), "{e}");
}

#[test]
fn a_statement_span_reaches_its_last_line() {
	// Where a block ends is in the tree nowhere else, and an editor completing
	// a loop variable has to know whether the cursor is still inside the loop.
	let src = "for x in [\"a\"]\n\tif x == \"a\"\n\t\t$ echo \\\n\t\t\tmore\n\tend\nend\nlet xs = [\n\t1,\n]\n$ a\n\n# note\nprint(xs)\n";
	let t = crate::parse(src).expect("parses");
	let lines = |s: &crate::Statement| (s.span().line, src[..s.span().end].matches('\n').count() + 1);
	let [f, l, run, p] = &t.body.statements[..] else {
		panic!("four statements: {:?}", t.body.statements)
	};
	assert_eq!(lines(f), (1, 6), "a block reaches its `end`");
	let crate::Statement::For { body, .. } = f else {
		panic!("a for")
	};
	let crate::Statement::If { then, .. } = &body.statements[0] else {
		panic!("an if")
	};
	assert_eq!(lines(&body.statements[0]), (2, 5), "and so does one inside it");
	assert_eq!(
		lines(&then.statements[0]),
		(3, 4),
		"a continued `$` line reaches its continuation"
	);
	assert_eq!(lines(l), (7, 9), "a spilled list reaches its `]`");
	assert_eq!(
		lines(run),
		(10, 10),
		"a run stops at its last `$` line, not the blanks after it"
	);
	assert_eq!(lines(p), (13, 13));
}

#[test]
fn detach_is_only_a_marker_in_front_of_a_command() {
	// Claimed narrowly, so it stays an ordinary name everywhere else.
	crate::parse("let detach = 5\ndetach = 6\n").expect("an ordinary binding and reassignment");
	crate::parse("let x = detach\n").expect("an ordinary read");
}

// ---- `parallel do` and `parallel for`

/// How many statements a `parallel do`'s body holds -- one per branch.
fn branches(src: &str) -> usize {
	let t = crate::parse(src).expect("parses");
	match &t.body.statements[0] {
		crate::Statement::Do {
			body, parallel: true, ..
		} => body.statements.len(),
		other => panic!("not a parallel do: {other:?}"),
	}
}

#[test]
fn every_dollar_line_directly_inside_parallel_do_is_a_branch_of_its_own() {
	// Folded, `$ lint` and `$ test` were one process, run one after the
	// other, under a block that said they were not.
	assert_eq!(branches("parallel do\n\t$ lint\n\t$ test\n\n\t$ build\nend\n"), 3);
	// A continuation is still one line.
	assert_eq!(branches("parallel do\n\t$ lint \\\n\t\t--fix\n\t$ test\nend\n"), 2);
}

#[test]
fn inside_a_branch_the_dollar_lines_fold_again() {
	// A branch is ordinary code; only the block's own statements are split.
	let t = crate::parse("parallel do\n\tdo\n\t\t$ a\n\t\t$ b\n\tend\nend\n").unwrap();
	let crate::Statement::Do { body, .. } = &t.body.statements[0] else {
		panic!()
	};
	let crate::Statement::Do { body: inner, .. } = &body.statements[0] else {
		panic!()
	};
	assert_eq!(inner.statements.len(), 1, "one process, as anywhere else");
}

#[test]
fn a_parallel_for_body_is_ordinary_code() {
	let t = crate::parse("parallel for s in [\"a\"]\n\t$ build\n\t$ push\nend\n").unwrap();
	let crate::Statement::For { body, parallel, .. } = &t.body.statements[0] else {
		panic!()
	};
	assert!(parallel);
	assert_eq!(
		body.statements.len(),
		1,
		"an iteration's `$` lines fold, as in any body"
	);
}

#[test]
fn a_let_directly_inside_parallel_do_is_refused_unless_it_binds_nothing() {
	let e = crate::parse("parallel do\n\tlet x = 1\n\t$ true\nend\n")
		.unwrap_err()
		.to_string();
	assert!(e.contains("binds nothing anyone can read"), "{e}");
	// `let _` binds nothing, so it is only a call run for its effect.
	crate::parse("parallel do\n\tlet _ = print(\"x\")\nend\n").expect("a discarded value is a branch like a call");
}

#[test]
fn a_branch_may_reassign_only_what_it_bound_itself() {
	for ok in [
		"parallel for n in [1]\n\tn = n + 1\nend\n",
		"parallel for n in [1]\n\tlet t = 0\n\tt = t + n\nend\n",
		"parallel do\n\tdo\n\t\tlet t = 0\n\t\tt = 1\n\tend\nend\n",
		"parallel do\n\tfor i in [1]\n\t\ti = 2\n\tend\nend\n",
		// Outside any parallel block, nothing changes.
		"let t = 0\nfor n in [1]\n\tt = t + n\nend\n",
	] {
		crate::parse(ok).unwrap_or_else(|e| panic!("{ok:?}: {e}"));
	}
	for bad in [
		"let t = 0\nparallel for n in [1]\n\tt = t + n\nend\n",
		"let t = 0\nparallel do\n\tdo\n\t\tt = 1\n\tend\nend\n",
		// A loop variable is put back when its loop ends, so after it the name
		// is whatever it was outside.
		"let i = 0\nparallel do\n\tdo\n\t\tfor i in [1]\n\t\tend\n\t\ti = 2\n\tend\nend\n",
		// A nested parallel branch is a branch of its own.
		"parallel for n in [1]\n\tlet t = 0\n\tparallel do\n\t\tdo\n\t\t\tt = 1\n\t\tend\n\tend\nend\n",
	] {
		let e = crate::parse(bad).expect_err(bad).to_string();
		assert!(e.contains("bound outside this parallel branch"), "{bad:?}: {e}");
	}
}

#[test]
fn break_and_continue_cannot_reach_past_a_parallel_block() {
	let e = crate::parse("parallel for n in [1]\n\tbreak\nend\n")
		.unwrap_err()
		.to_string();
	assert!(e.contains("cannot stop a `parallel for`"), "{e}");
	crate::parse("parallel for n in [1]\n\tcontinue\nend\n").expect("continue ends one iteration");
	let e = crate::parse("for n in [1]\n\tparallel do\n\t\tcontinue\n\tend\nend\n")
		.unwrap_err()
		.to_string();
	assert!(e.contains("cannot reach a loop outside"), "{e}");
	// A loop inside a branch is the branch's own to leave.
	crate::parse("parallel do\n\tfor n in [1]\n\t\tbreak\n\tend\nend\n").expect("its own loop");
	crate::parse("parallel for s in [1]\n\tfor n in [1]\n\t\tbreak\n\tend\nend\n").expect("its own loop");
}

#[test]
fn parallel_is_claimed_only_in_front_of_a_keyword() {
	let e = crate::parse("parallel if true\n\t$ x\nend\n").unwrap_err().to_string();
	assert!(e.contains("takes `do` or `for`"), "{e}");
	crate::parse("let parallel = 5\nparallel = 6\n").expect("an ordinary name everywhere else");
}
