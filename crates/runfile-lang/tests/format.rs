//! The formatter, against source written badly on purpose.

use runfile_lang::{fingerprint, format, parse};

/// Every rule at once, so the canonical shape is written down in one place.
#[test]
fn a_messy_file_comes_out_canonical() {
	let src = "\
# A messy file


.env.A=\"one\"
   .shell   =  \"bash\"
let x=concat( \"a\" ,\"b\" )
let y = [ 1,2 , 3 ]
if x==\"ab\"&&!contains(x,\"z\")
$ echo   {{ x }}
    for i in y
if i>1
$ echo big
else
$ echo small
end
    end
   else
$ echo no
end
match x
   case \"ab\"
$ echo m
default
$ echo d
end
";
	let want = "\
# A messy file

.env.A = \"one\"
.shell = \"bash\"
let x = concat(\"a\", \"b\")
let y = [1, 2, 3]
if x == \"ab\" && !contains(x, \"z\")
\t$ echo   {{ x }}
\tfor i in y
\t\tif i > 1
\t\t\t$ echo big
\t\telse
\t\t\t$ echo small
\t\tend
\tend
else
\t$ echo no
end
match x
case \"ab\"
\t$ echo m
default
\t$ echo d
end
";
	assert_eq!(format(src).unwrap(), want);
}

#[test]
fn shell_text_is_never_touched() {
	// The words after `$ ` are the shell's. Collapsing the spacing inside them
	// would be editing someone's command.
	let src = "$ echo  a   b\n$ printf '%s   %s\\n' x y\n";
	assert_eq!(format(src).unwrap(), src);
}

#[test]
fn a_shell_continuation_keeps_its_own_indentation() {
	// The parser hands the *raw* next line to the shell, indentation included,
	// so reindenting one would change the command.
	let src = "if true\n$ echo a \\\n      b \\\n      c\nend\n";
	let out = format(src).unwrap();
	assert!(out.contains("\t$ echo a \\\n      b \\\n      c\n"), "{out:?}");
}

#[test]
fn an_exec_body_keeps_its_shape_below_the_base_indent() {
	let src = "exec python3\n      import sys\n      if x:\n          go()\n\nend\n";
	let out = format(src).unwrap();
	assert_eq!(out, "exec python3\n\timport sys\n\tif x:\n\t    go()\n\nend\n");
}

#[test]
fn an_exec_commands_own_spacing_is_left_alone() {
	// `run` arguments are re-split on whitespace, so collapsing them is safe.
	// An `exec` command is not: its text reaches the shell as written.
	assert_eq!(
		format("exec sudo   tee  f\nx\nend\n").unwrap(),
		"exec sudo   tee  f\n\tx\nend\n"
	);
	assert_eq!(format("run other   a    b\n").unwrap(), "run other a b\n");
}

#[test]
fn an_exec_body_may_contain_its_own_end() {
	// The terminator is an `end` at the *opener's* indentation, so a ruby or
	// lua body does not close the block early.
	let src = "exec ruby\n\tif x\n\tend\nend\n";
	assert_eq!(format(src).unwrap(), src);
}

#[test]
fn a_capture_with_a_body_indents_like_a_block() {
	let src = "let out = exec sh\necho hi\nend\n";
	assert_eq!(format(src).unwrap(), "let out = exec sh\n\techo hi\nend\n");
}

#[test]
fn a_capture_to_end_of_line_is_left_as_shell() {
	let src = "let files = $ git diff --cached   --name-only\n";
	assert_eq!(format(src).unwrap(), src);
}

#[test]
fn a_string_is_copied_out_of_the_source_verbatim() {
	// Raw prefixes, escapes and interpolations all survive because tokens are
	// rendered from their spans rather than reconstructed.
	let src = "let a = r\"\\d+ \\\" x\"\nlet b = \"say \\\"hi\\\"\"\nlet c = \"{{ ARG.x ? \"w\" }}\"\n";
	assert_eq!(format(src).unwrap(), src);
}

#[test]
fn unary_and_binary_minus_are_told_apart() {
	assert_eq!(format("let a = 1 - -4\n").unwrap(), "let a = 1 - -4\n");
	assert_eq!(format("let a = 1-2\n").unwrap(), "let a = 1 - 2\n");
	assert_eq!(format("let a = -x\n").unwrap(), "let a = -x\n");
	assert_eq!(format("let a = f(-1, !b)\n").unwrap(), "let a = f(-1, !b)\n");
}

#[test]
fn indexing_and_calls_bind_tight_but_lists_and_groups_do_not() {
	assert_eq!(
		format("let a = split ( \"a.b\" , \".\" ) [ 1 ]\n").unwrap(),
		"let a = split(\"a.b\", \".\")[1]\n"
	);
	assert_eq!(format("let a = [1]+[2]\n").unwrap(), "let a = [1] + [2]\n");
	assert_eq!(format("let a = (1+2)*3\n").unwrap(), "let a = (1 + 2) * 3\n");
	assert_eq!(format("let a = ARG . x\n").unwrap(), "let a = ARG.x\n");
}

#[test]
fn a_multi_line_list_stays_multi_line() {
	// The parser joins it into one logical line; joining it in the output too
	// would turn a readable list into one very long line.
	let src = "let list = [\n\t\"a\",\n\t2,\n]\n";
	assert_eq!(format(src).unwrap(), src);

	// A `for` spills its list *before* the body starts, so the items belong to
	// the header's level and not to the loop's.
	let src = "for s in [\n\t\"a\",\n]\n\t$ echo {{ s }}\nend\n";
	assert_eq!(format(src).unwrap(), src);
}

#[test]
fn blank_lines_are_collapsed_and_the_file_ends_in_exactly_one_newline() {
	assert_eq!(format("\n\n$ a\n\n\n\n$ b\n\n\n").unwrap(), "$ a\n\n$ b\n");
	assert_eq!(format("$ a").unwrap(), "$ a\n");
}

#[test]
fn source_that_does_not_parse_is_refused() {
	// Reindenting a file whose blocks do not close is guesswork, and guessing
	// is how a formatter eats someone's work.
	let e = format("if x\n$ echo a\n").unwrap_err();
	assert!(e.to_string().contains("end"), "{e}");
}

#[test]
fn formatting_never_changes_what_a_file_means() {
	// The property that matters, asserted directly rather than trusted.
	for src in [
		"# c\n.shell=\"bash\"\nlet x=1+2\nif x>2\n$ echo {{ x }}\nend\n",
		"match ARG.a\ncase \"x\"\nrun other {{ ARG.a }} b\ndefault\n$ true\nend\n",
		"exec sh\n  set -e\n  echo hi\nend\n",
		"let a = r\"\\s+\"\nlet b = [1, 2][0]\n",
	] {
		let out = format(src).unwrap();
		assert_eq!(
			fingerprint(&parse(src).unwrap()),
			fingerprint(&parse(&out).unwrap()),
			"meaning changed:\n{src}\n->\n{out}"
		);
	}
}

#[test]
fn formatting_is_idempotent() {
	for src in [
		"# c\n\n\n.env.A=\"1\"\nif  x  ==  1\n$ a\nelse\n$ b\nend\n",
		"exec python3\n        x = 1\n        if x:\n            pass\nend\n",
		"let list = [\n\t1,\n]\n",
	] {
		let once = format(src).unwrap();
		let twice = format(&once).unwrap();
		assert_eq!(once, twice, "not stable:\n{once}");
	}
}
