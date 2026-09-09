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
		".env-file = \".env.{{ ARG.a }}\"\n",
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
