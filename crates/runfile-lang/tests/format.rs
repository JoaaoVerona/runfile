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
\tcase \"ab\"
\t\t$ echo m
\tdefault
\t\t$ echo d
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

#[test]
fn a_case_sits_inside_its_match() {
	// `match`/`case`/`end` at one level read as three separate things. A case
	// is part of the match, and the indentation should say so.
	let src = "match x\ncase \"a\"\n$ one\ndefault\n$ two\nend\n";
	assert_eq!(
		format(src).unwrap(),
		"match x\n\tcase \"a\"\n\t\t$ one\n\tdefault\n\t\t$ two\nend\n"
	);
}

#[test]
fn blank_lines_are_placed_where_a_file_needs_air() {
	// The description is a paragraph of its own; a run of `let`s is one group;
	// a block stands out from what runs before it; and after an `end` comes a
	// new thought.
	let src = "# What this does\n.shell = \"bash\"\nlet a = 1\nlet b = 2\n$ echo hi\nif a\n$ x\nend\n$ done\n";
	assert_eq!(
		format(src).unwrap(),
		"# What this does\n\n.shell = \"bash\"\n\nlet a = 1\nlet b = 2\n\n$ echo hi\n\nif a\n\t$ x\nend\n\n$ done\n"
	);
}

#[test]
fn a_comment_stays_with_the_statement_it_is_about() {
	// The blank goes above the comment, not between it and what it explains.
	let src = "$ first\n# why this next bit\nif a\n$ x\nend\n";
	assert_eq!(
		format(src).unwrap(),
		"$ first\n\n# why this next bit\nif a\n\t$ x\nend\n"
	);
}

#[test]
fn nothing_is_pushed_away_from_the_block_it_opens_or_closes() {
	let src = "if a\nlet x = 1\nelse\nlet y = 2\nend\n";
	assert_eq!(format(src).unwrap(), "if a\n\tlet x = 1\nelse\n\tlet y = 2\nend\n");
}

#[test]
fn the_authors_own_blank_lines_are_kept() {
	// The rules add; they do not argue with someone who wanted a break here.
	let src = "let a = 1\n\nlet b = 2\n";
	assert_eq!(format(src).unwrap(), src);
}

#[test]
fn a_json_block_is_laid_out_rather_than_left_alone() {
	// An `exec` body is somebody else's language and stays as written; a
	// `json` body is a format this runner knows, so it gets the same
	// treatment as everything else in the file.
	let src = "let p = json\n  {\"a\":[1,2],\"b\":{\"c\":true},\"d\":[],\"e\":{}}\nend\n";
	assert_eq!(
		format(src).unwrap(),
		"let p = json\n\t{\n\t\t\"a\": [\n\t\t\t1,\n\t\t\t2\n\t\t],\n\t\t\"b\": {\n\t\t\t\"c\": true\n\t\t},\n\t\t\"d\": [],\n\t\t\"e\": {}\n\t}\nend\n"
	);
}

#[test]
fn laying_out_a_json_block_is_idempotent() {
	let src = "let p = json\n{\"a\":[1,{\"b\":[[]]}],\"c\":null}\nend\n";
	let once = format(src).unwrap();
	assert_eq!(format(&once).unwrap(), once);
}

#[test]
fn an_interpolation_survives_the_layout_whole() {
	// It is one JSON value, and a value is a token like any other -- but its
	// insides are the language's, not the format's, so nothing inside the
	// braces may be touched.
	let src = "let p = json\n{\"n\":{{ number(ARG.n) }},\"s\":{{ ARG.a ? \"x, y\" }}}\nend\n";
	let out = format(src).unwrap();
	assert!(out.contains("\"n\": {{ number(ARG.n) }}"), "{out}");
	assert!(out.contains("\"s\": {{ ARG.a ? \"x, y\" }}"), "{out}");
}

#[test]
fn a_string_keeps_its_own_escapes_and_spacing() {
	// Tokens are copied from the source, never re-serialised, so a `\u0041`
	// escape is not rewritten as the letter it stands for and the spaces
	// inside a string are left alone.
	let src = "let p = json\n{\"a\":\"x  y\\u0041\\\"z\"}\nend\n";
	let out = format(src).unwrap();
	assert!(out.contains("\"a\": \"x  y\\u0041\\\"z\""), "{out}");
}

#[test]
fn an_exec_body_that_looks_like_json_is_still_left_alone() {
	let src = "let p = exec cat\n  {\"a\":1}\nend\n";
	assert_eq!(format(src).unwrap(), "let p = exec cat\n\t{\"a\":1}\nend\n");
}

#[test]
fn a_json_blocks_layout_is_not_part_of_its_fingerprint() {
	// This is what lets the formatter lay one out at all: it checks its own
	// work by comparing fingerprints, and the prepare gate uses the same
	// number, so reformatting a `setup.run` must not re-trigger it.
	let a = parse("let p = json\n{\"a\":[1,2]}\nend\n").unwrap();
	let b = parse("let p = json\n\t{\n\t\t\"a\": [\n\t\t\t1,\n\t\t\t2\n\t\t]\n\t}\nend\n").unwrap();
	assert_eq!(fingerprint(&a), fingerprint(&b));
}

#[test]
fn but_what_a_json_block_says_is() {
	let a = parse("let p = json\n{\"a\": [1, 2]}\nend\n").unwrap();
	for other in [
		"let p = json\n{\"a\": [1, 3]}\nend\n",
		"let p = json\n{\"b\": [1, 2]}\nend\n",
		"let p = json\n{\"a\": [1, 2, 3]}\nend\n",
		"let p = json\n{\"a\": [12]}\nend\n",
		"let p = json\n{\"a\": [{{ ARG.x }}, 2]}\nend\n",
	] {
		let b = parse(other).unwrap();
		assert_ne!(fingerprint(&a), fingerprint(&b), "{other}");
	}
}

#[test]
fn a_spilled_list_is_indented_by_how_deep_it_is() {
	// One level per bracket still open. A `]` sits with the `[` it answers,
	// not with what was inside it.
	let src = "let deep = [\n[\n1,\n2\n],\n[3, [4]],\n]\n";
	assert_eq!(
		format(src).unwrap(),
		"let deep = [\n\t[\n\t\t1,\n\t\t2\n\t],\n\t[3, [4]],\n]\n"
	);
}

#[test]
fn a_flat_spilled_list_is_unchanged_by_that() {
	let src = "let xs = [\n\"a\",\n\"b\",\n]\n";
	assert_eq!(format(src).unwrap(), "let xs = [\n\t\"a\",\n\t\"b\",\n]\n");
}

#[test]
fn laying_out_a_nested_list_is_idempotent() {
	let src = "let deep = [\n[[1], [2, [3]]],\n[\n4\n],\n]\n";
	let once = format(src).unwrap();
	assert_eq!(format(&once).unwrap(), once);
}

#[test]
fn a_bracket_inside_a_string_does_not_indent_the_rest_of_the_list() {
	let src = "let xs = [\n\"a[b\",\n\"c\",\n]\n";
	assert_eq!(format(src).unwrap(), "let xs = [\n\t\"a[b\",\n\t\"c\",\n]\n");
}

#[test]
fn a_for_header_spills_its_nested_list_at_the_headers_level() {
	// The list belongs to the header, not to the body it opens.
	let src = "for row in [\n[\"a\", \"1\"],\n[\"b\", \"2\"],\n]\n$ echo {{ row[0] }}\nend\n";
	assert_eq!(
		format(src).unwrap(),
		"for row in [\n\t[\"a\", \"1\"],\n\t[\"b\", \"2\"],\n]\n\t$ echo {{ row[0] }}\nend\n"
	);
}

#[test]
fn a_call_holding_a_capture_is_left_as_written() {
	// Everything from the `$` on is the shell's text. The formatter knew this
	// about `code_of` and had to be told about every other call.
	for src in [
		"let files = lines($ git diff --cached --name-only -- \"*.rs\")\n",
		"code_of($ docker volume create app-data)\n",
		"for f in lines($ git ls-files '*.sh')\n\t$ sh -n {{ f }}\nend\n",
	] {
		assert_eq!(format(src).unwrap(), src, "{src:?}");
	}
}

#[test]
fn a_scored_dispatch_is_spaced_like_a_run_statement() {
	// `code_of(run …)` holds the same words a `run` statement does, not an
	// expression, so they are separated by one space and otherwise untouched.
	assert_eq!(
		format("let c = code_of(run  web:build   --env={{ e }})\n").unwrap(),
		"let c = code_of(run web:build --env={{ e }})\n"
	);
	for src in [
		"code_of(run test)\n",
		"let c = code_of(run test --filter={{ ARG.filter }})\n",
	] {
		assert_eq!(format(src).unwrap(), src, "{src:?}");
	}
}
