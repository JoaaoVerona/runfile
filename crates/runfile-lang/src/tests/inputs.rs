//! What a target reads, walked from the tree.

use crate::inputs::of;

fn reads(src: &str) -> (Vec<String>, Vec<String>, Vec<String>, bool) {
	let i = walk(src);
	(
		i.args.keys().cloned().collect(),
		i.flags.iter().cloned().collect(),
		i.env.keys().cloned().collect(),
		i.positional,
	)
}

fn walk(src: &str) -> crate::Inputs {
	of(&crate::parse(src).unwrap_or_else(|e| panic!("{src:?}: {e}")))
}

#[test]
fn a_name_in_a_comment_is_not_a_use() {
	// This is the one that mattered: a comment saying `ARG.legacy` used to
	// suppress the warning for a mistyped `--legacy`, silently.
	let (args, flags, ..) =
		reads("# Set ARG.legacy if you are on the old path.\n#\n# FLAG.verbose was removed.\n\nprint(ARG.env)\n");
	assert_eq!(args, ["env"]);
	assert!(flags.is_empty(), "{flags:?}");
}

#[test]
fn a_name_in_a_string_or_in_shell_text_is_not_a_use() {
	let (args, ..) =
		reads("let hint = \"pass ARG.region to override\"\n\n$ echo 'ARG.cluster is deprecated'\nprint(ARG.env)\n");
	assert_eq!(args, ["env"]);
}

#[test]
fn an_interpolation_inside_a_string_or_a_shell_line_is_a_use() {
	// The literal halves are somebody else's; the `{{ … }}` is ours.
	let (args, ..) = reads("let m = \"deploying {{ ARG.env }}\"\n\n$ deploy {{ ARG.region }}\n");
	assert_eq!(args, ["env", "region"]);
}

#[test]
fn every_position_a_source_can_sit_in_is_walked() {
	let (args, flags, env, positional) = reads(concat!(
		"import_env_file(\".env.{{ ARG.a }}\")\n",
		"\n",
		"let x = ARG.b ? ENV.C\n",
		"\n",
		"if FLAG.d\n",
		"\tfor i in [ARG.e]\n",
		"\t\trun other {{ ARG.f }}\n",
		"\tend\n",
		"end\n",
		"\n",
		"do\n",
		"\tmatch ARG.g\n",
		"\t\tcase \"x\"\n",
		"\t\t\texec cat {{ ARG.h }}\n",
		"\t\t\t\t{{ ARG.i }}\n",
		"\t\t\tend\n",
		"\tend\n",
		"end\n",
		"\n",
		"retry number(ARG.j) every number(ARG.k)\n",
		"\t$ true\n",
		"else\n",
		"\tprint(ARG.l)\n",
		"end\n",
		"\n",
		"let doc = json\n",
		"\t{\"m\": {{ ARG.m }}}\n",
		"end\n",
		"\n",
		"let out = lines($ echo {{ ARG.n }})\n",
		"print(length(ARGS), concat(ARG.o))\n",
	));
	assert_eq!(
		args,
		["a", "b", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o"],
		"a position is not being walked"
	);
	assert_eq!(flags, ["d"]);
	assert_eq!(env, ["C"]);
	assert!(positional);
}

#[test]
fn a_name_is_reported_once_however_often_it_is_read() {
	let (args, ..) = reads("print(ARG.env)\nprint(ARG.env)\n$ echo {{ ARG.env }}\n");
	assert_eq!(args, ["env"]);
}

#[test]
fn a_chain_says_what_a_name_falls_back_to() {
	// `a ? b ? c` is left-associative, so the outermost fallback is what is
	// reached once everything before it is missing -- and that is the useful
	// thing to show.
	let i = walk("let p = ARG.port ? ENV.PORT ? \"3000\"\n");
	assert_eq!(i.args["port"].default.as_deref(), Some("3000"));
	assert!(!i.args["port"].required);
	assert_eq!(i.env["PORT"].default.as_deref(), Some("3000"), "guarded too");
	assert!(!i.env["PORT"].required);
}

#[test]
fn a_name_read_bare_is_required() {
	let i = walk("print(ARG.token)\n");
	assert!(i.args["token"].required);
	assert_eq!(i.args["token"].default, None);
}

#[test]
fn one_guarded_use_does_not_excuse_a_bare_one() {
	// The bare use is where a run fails, whatever the other one says.
	let i = walk("let a = ARG.x ? \"d\"\n\nprint(ARG.x)\n");
	assert!(i.args["x"].required);
	assert_eq!(
		i.args["x"].default.as_deref(),
		Some("d"),
		"and still shows the fallback"
	);
}

#[test]
fn a_fallback_that_is_not_a_literal_has_no_value_to_show() {
	let i = walk("let c = ARG.dir ? join_path(ENV.HOME, \".config\")\n");
	assert!(!i.args["dir"].required, "it is still guarded");
	assert_eq!(i.args["dir"].default, None);
}

#[test]
fn try_guards_the_same_way_a_chain_does() {
	let i = walk("let a = try(ARG.maybe)\n");
	assert!(!i.args["maybe"].required);
}

#[test]
fn the_right_hand_side_of_a_chain_is_not_guarded_by_it() {
	// `a ? b` catches a failure in `a`. If `b` fails, so does the expression.
	let i = walk("let a = ARG.x ? ARG.y\n");
	assert!(!i.args["x"].required);
	assert!(i.args["y"].required);
}

#[test]
fn every_kind_of_literal_can_be_a_default() {
	let i = walk("let a = ARG.n ? 8\nlet b = ARG.b ? true\nlet c = ARG.e ? \"\"\n");
	assert_eq!(i.args["n"].default.as_deref(), Some("8"));
	assert_eq!(i.args["b"].default.as_deref(), Some("true"));
	assert_eq!(
		i.args["e"].default.as_deref(),
		Some(""),
		"an empty default is a default"
	);
}

// ---- what a `one_of` says an input takes

fn strings(xs: &[&str]) -> Option<Vec<String>> {
	Some(xs.iter().map(|x| x.to_string()).collect())
}

#[test]
fn a_one_of_lists_what_the_input_it_checks_takes() {
	// The 38 checks of an input in the corpus are all this shape: a source or
	// the first positional, then the options written out.
	let i = walk(
		"let e = one_of(ARG.env, \"staging\", \"production\")\nlet m = one_of(ENV.MODE, [\"blue\", \"green\"])\nlet p = one_of(first(ARGS), \"major\", \"minor\")\n",
	);
	assert_eq!(i.args["env"].choices, strings(&["staging", "production"]));
	assert_eq!(
		i.env["MODE"].choices,
		strings(&["blue", "green"]),
		"a single list is the options"
	);
	assert_eq!(i.positional_choices, strings(&["major", "minor"]));
	assert_eq!(
		walk("let p = one_of(ARGS[0], \"a\")\n").positional_choices,
		strings(&["a"])
	);
}

#[test]
fn a_one_of_reaches_through_a_chain_and_keeps_its_default() {
	// Whichever side of the `?` the value comes from is what is checked.
	let i = walk("let e = one_of(ARG.env ? ENV.ENV ? \"staging\", \"staging\", \"production\")\n");
	for u in [&i.args["env"], &i.env["ENV"]] {
		assert_eq!(u.choices, strings(&["staging", "production"]), "{u:?}");
		assert_eq!(u.default.as_deref(), Some("staging"), "{u:?}");
		assert!(!u.required, "{u:?}");
	}
}

#[test]
fn only_a_list_written_out_and_checked_against_the_input_itself_is_shown() {
	// A computed option leaves no list to show; a number never equals the
	// string an input arrives as; a value worked out from the input is what
	// the options describe; and the whole of `ARGS`, or any word but the
	// first, is not the word the prompt for it starts with.
	for src in [
		"let x = one_of(ARG.x, \"a\", concat(\"b\", \"c\"))\n",
		"let x = one_of(ARG.x, \"a{{ ENV.B }}\", \"c\")\n",
		"let x = one_of(ARG.x, 1, 2)\n",
		"let x = one_of(lower(ARG.x), \"a\", \"b\")\n",
	] {
		assert_eq!(walk(src).args["x"].choices, None, "{src}");
	}
	for src in [
		"let x = one_of(ARGS, \"a\")\n",
		"let x = one_of(ARGS[1], \"a\")\n",
		"let x = one_of(last(ARGS), \"a\")\n",
	] {
		let i = walk(src);
		assert!(i.positional, "{src}");
		assert_eq!(i.positional_choices, None, "{src}");
	}
}

#[test]
fn a_use_without_a_check_does_not_undo_one_and_two_checks_must_agree() {
	let i = walk("print(ARG.x)\nlet a = one_of(ARG.x, \"a\", \"b\")\nlet b = one_of(ARG.x, \"b\", \"a\")\n");
	assert_eq!(
		i.args["x"].choices,
		strings(&["a", "b"]),
		"the same values, in any order"
	);
	// One accepts `a`, the other refuses it: neither list is the answer.
	let i = walk("let a = one_of(ARG.x, \"a\", \"b\")\nlet b = one_of(ARG.x, \"b\", \"c\")\n");
	assert_eq!(i.args["x"].choices, Some(Vec::new()));
}

#[test]
fn the_options_are_read_like_any_other_argument() {
	// They are inputs too when they read one, and are not checked by the call.
	let i = walk("let x = one_of(ARG.x, ARG.allowed)\n");
	assert!(i.args["allowed"].required);
	assert_eq!(i.args["allowed"].choices, None);
	assert_eq!(i.args["x"].choices, None, "a computed option");
}

// ---- a name the file has set is not an input

/// The environment names a target reads under a `_shared.run` chain.
fn env_under(src: &str, shared: &[&str]) -> Vec<String> {
	let target = crate::parse(src).unwrap_or_else(|e| panic!("{src:?}: {e}"));
	let chain: Vec<_> = shared.iter().map(|s| crate::parse(s).expect("parses")).collect();
	crate::inputs::of_chain(&target, &chain).env.keys().cloned().collect()
}

fn env(src: &str) -> Vec<String> {
	env_under(src, &[])
}

#[test]
fn a_name_the_file_has_set_is_not_an_input() {
	// `--help` said `PORT required` for this, and `--stdin-args` asked for it.
	// The assignment beats whatever the caller exports, so there is nothing for
	// them to supply: a value they pass is ignored.
	assert!(env("ENV.PORT = \"3000\"\nprint(ENV.PORT)\n").is_empty());
	assert!(env("ENV.PORT = \"3000\"\n$ serve --port {{ ENV.PORT }}\n").is_empty());
	// The same in any block below it.
	assert!(env("ENV.PORT = \"3000\"\nif true\n\tprint(ENV.PORT)\nend\n").is_empty());
}

#[test]
fn a_default_the_caller_may_override_is_still_one() {
	// Spelled out on purpose, which is how a target says the caller may set
	// it -- and what `--help` shows as `defaults to 3000`.
	let i = walk("ENV.PORT = ENV.PORT ? \"3000\"\nprint(ENV.PORT)\n");
	let port = &i.env["PORT"];
	assert_eq!(port.default.as_deref(), Some("3000"), "{port:?}");
	assert!(!port.required, "the chain catches it: {port:?}");
}

#[test]
fn an_assignment_reading_its_own_name_reads_the_callers() {
	// Its value is worked out before it is assigned, so the read inside it is
	// of whatever was there already.
	assert_eq!(env("ENV.P = concat(ENV.P, \":/x\")\nprint(ENV.P)\n"), ["P"]);
}

#[test]
fn an_assignment_covers_what_is_below_it_and_no_more() {
	// Above it the name is not set yet.
	assert_eq!(env("print(ENV.PORT)\nENV.PORT = \"3000\"\n$ true\n"), ["PORT"]);
	// Below it, it is.
	assert!(env("$ true\nENV.PORT = \"3000\"\nprint(ENV.PORT)\n").is_empty());
	// And one value reads the one set above it, but not the other way round.
	assert!(env("ENV.A = \"a\"\nENV.B = ENV.A\n$ true\n").is_empty());
	assert_eq!(env("ENV.B = ENV.A\nENV.A = \"a\"\n$ true\n"), ["A"]);
}

#[test]
fn a_name_counts_as_set_only_where_every_path_to_the_read_set_it() {
	// A change lasts past the block it is written in -- so after an `if` that
	// sets it on both sides, it is set; after one that sets it on one side, a
	// run down the other one still needs it from the caller.
	assert!(env("if FLAG.x\n\tENV.P = \"1\"\nelse\n\tENV.P = \"2\"\nend\nprint(ENV.P)\n").is_empty());
	assert_eq!(env("if FLAG.x\n\tENV.P = \"1\"\nend\nprint(ENV.P)\n"), ["P"]);
	// Inside the branch that set it, it is.
	assert!(env("if FLAG.x\n\tENV.P = \"1\"\n\tprint(ENV.P)\nend\n").is_empty());
	// A `match` needs every case, and a default -- without one, no case matching
	// fails the target, so the cases are every path there is.
	let cases = "match ARG.m\ncase \"a\"\n\tENV.P = \"1\"\ncase \"b\"\n\tENV.P = \"2\"\n";
	assert!(env(&format!("{cases}end\nprint(ENV.P)\n")).is_empty());
	assert_eq!(env(&format!("{cases}default\n\tprint(1)\nend\nprint(ENV.P)\n")), ["P"]);
}

#[test]
fn a_path_that_leaves_first_does_not_count_against_one_that_set_it() {
	// The usual guard: the branch that does not set it ends the target.
	assert!(env("if FLAG.x\n\tENV.P = \"1\"\nelse\n\texit(1)\nend\nprint(ENV.P)\n").is_empty());
	assert!(env("if !FLAG.x\n\terror(\"pass --x\")\nend\nENV.P = \"1\"\nprint(ENV.P)\n").is_empty());
}

#[test]
fn a_do_block_puts_the_environment_back_at_its_end() {
	assert!(env("do\n\tENV.P = \"1\"\n\tprint(ENV.P)\nend\n").is_empty());
	assert_eq!(env("do\n\tENV.P = \"1\"\nend\nprint(ENV.P)\n"), ["P"]);
}

#[test]
fn a_loop_may_run_no_passes_so_nothing_it_sets_is_certain_after_it() {
	for src in [
		"for x in ARGS\n\tENV.P = x\nend\nprint(ENV.P)\n",
		"while FLAG.x\n\tENV.P = \"1\"\nend\nprint(ENV.P)\n",
	] {
		assert_eq!(env(src), ["P"], "{src}");
	}
	// Its own body reads what it set above, on that pass.
	assert!(env("for x in ARGS\n\tENV.P = x\n\tprint(ENV.P)\nend\n").is_empty());
}

#[test]
fn a_retry_keeps_what_the_attempt_that_worked_set() {
	// Every attempt starts where the `retry` began, and the one that succeeds
	// is kept; the `else` runs from where it began too.
	assert!(env("retry 3\n\tENV.P = \"1\"\nend\nprint(ENV.P)\n").is_empty());
	assert_eq!(
		env("retry 3\n\tENV.P = \"1\"\nelse\n\tprint(1)\nend\nprint(ENV.P)\n"),
		["P"]
	);
	assert!(env("retry 3\n\tENV.P = \"1\"\nelse\n\tENV.P = \"2\"\nend\nprint(ENV.P)\n").is_empty());
}

#[test]
fn a_parallel_branch_sets_its_own_copy() {
	let src = "parallel for x in ARGS\n\tENV.P = x\n\tprint(ENV.P)\nend\nprint(ENV.P)\n";
	assert_eq!(env(src), ["P"], "read after the block, from the caller");
}

#[test]
fn under_ignore_errors_only_an_assignment_that_cannot_fail_counts() {
	// A forgiven failure leaves the name unset, so `--help` cannot promise it.
	assert_eq!(
		env("ignore_errors()\nENV.P = read_file(\"port\")\nprint(ENV.P)\n"),
		["P"]
	);
	assert!(env("ignore_errors()\nENV.P = \"3000\"\nprint(ENV.P)\n").is_empty());
	// A `do` puts it back, along with everything else.
	assert!(env("do\n\tignore_errors()\nend\nENV.P = read_file(\"port\")\nprint(ENV.P)\n").is_empty());
}

#[test]
fn a_name_a_shared_file_sets_is_not_an_input_for_the_target() {
	assert!(env_under("print(ENV.DIR)\n", &["ENV.DIR = \"x\"\n"]).is_empty());
	assert!(env_under("print(ENV.DIR)\n", &["let d = \"x\"\nENV.DIR = d\n"]).is_empty());
	// On every path through it, as anywhere else.
	assert_eq!(
		env_under("print(ENV.DIR)\n", &["if FLAG.x\n\tENV.DIR = \"x\"\nend\n"]),
		["DIR"]
	);
}

#[test]
fn what_a_shared_file_sets_reaches_the_rest_of_the_chain_and_the_target() {
	assert!(env_under("$ true\n", &["ENV.A = \"a\"\nlet x = ENV.A\n"]).is_empty());
	assert!(env_under("$ true\n", &["ENV.A = \"a\"\n", "let x = ENV.A\n"]).is_empty());
	assert!(env_under("ENV.B = ENV.A\n$ true\n", &["ENV.A = \"a\"\n"]).is_empty());
	// A property is read once the chain has run.
	assert!(env_under(".watch = ENV.A\n$ true\n", &["ENV.A = \"a\"\n"]).is_empty());
}

#[test]
fn a_name_an_env_file_might_set_is_still_an_input() {
	// Which names a file sets is only known once it is read, at run time, and
	// the file may well not exist -- a project whose `.env` is git-ignored
	// still has to run. Listing the name is the answer that is never a lie.
	assert_eq!(env("import_env_file(\".env\")\nprint(ENV.DB)\n"), ["DB"]);
}

#[test]
fn an_assignment_is_matched_by_the_exact_name_it_sets() {
	// `ENV.PORT` looks for `PORT` exactly before it tries another case, so a
	// caller's `PORT` still reaches it under an `ENV.port = …`.
	assert_eq!(env("ENV.port = \"3000\"\nprint(ENV.PORT)\n"), ["PORT"]);
}
