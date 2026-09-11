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

// ---- a name a `.env` property sets is not an input

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
fn a_name_a_property_has_set_is_not_an_input() {
	// `--help` said `PORT required` for this, and `--stdin-args` asked for it.
	// The property beats whatever the caller exports, so there is nothing for
	// them to supply: a value they pass is ignored.
	assert!(env(".env.PORT = \"3000\"\nprint(ENV.PORT)\n").is_empty());
	assert!(env(".env.PORT = \"3000\"\n$ serve --port {{ ENV.PORT }}\n").is_empty());
	// The same in any block below it.
	assert!(env(".env.PORT = \"3000\"\nif true\n\tprint(ENV.PORT)\nend\n").is_empty());
}

#[test]
fn a_default_the_caller_may_override_is_still_one() {
	// Spelled out on purpose, which is how a target says the caller may set
	// it -- and what `--help` shows as `defaults to 3000`.
	let i = walk(".env.PORT = ENV.PORT ? \"3000\"\nprint(ENV.PORT)\n");
	let port = &i.env["PORT"];
	assert_eq!(port.default.as_deref(), Some("3000"), "{port:?}");
	assert!(!port.required, "the chain catches it: {port:?}");
}

#[test]
fn a_property_reading_its_own_name_reads_the_callers() {
	// Its value is worked out before it is assigned, so the read inside it is
	// of whatever was there already.
	assert_eq!(env(".env.P = concat(ENV.P, \":/x\")\nprint(ENV.P)\n"), ["P"]);
}

#[test]
fn a_property_covers_only_what_is_below_it_and_inside_its_block() {
	// Above a trailing property the name is not set yet.
	assert_eq!(env("print(ENV.PORT)\n.env.PORT = \"3000\"\n$ true\n"), ["PORT"]);
	// Below it, it is.
	assert!(env("$ true\n.env.PORT = \"3000\"\nprint(ENV.PORT)\n").is_empty());
	// And a block's own property is undone when the block closes.
	assert_eq!(env("if true\n\t.env.PORT = \"3000\"\nend\nprint(ENV.PORT)\n"), ["PORT"]);
}

#[test]
fn a_header_value_reads_the_names_set_above_it() {
	// The runner applied a header as one step and built the environment after,
	// so `.env.B = ENV.A` read the caller's `A` -- and this listed `A`, which
	// was true. A value reads the environment as it stands at its own line
	// now, in a header as below a statement, so it is not an input either way.
	assert!(env(".env.A = \"a\"\n.env.B = ENV.A\n$ true\n").is_empty());
	assert!(env("$ true\n.env.A = \"a\"\n.env.B = ENV.A\n").is_empty());
	// In a nested block's header too.
	assert!(env("if true\n\t.env.A = \"a\"\n\t.env.B = ENV.A\n\t$ true\nend\n").is_empty());
	// Only what is above it: the other way round, `A` is still the caller's.
	assert_eq!(env(".env.B = ENV.A\n.env.A = \"a\"\n$ true\n"), ["A"]);
}

#[test]
fn a_name_a_shared_file_sets_is_not_an_input_for_the_target() {
	assert!(env_under("print(ENV.DIR)\n", &[".env.DIR = \"x\"\n"]).is_empty());
	// Below a `let` in the shared file too: it is folded, not walked, so every
	// property in it applies.
	assert!(env_under("print(ENV.DIR)\n", &["let d = \"x\"\n.env.DIR = d\n"]).is_empty());
}

#[test]
fn what_a_shared_file_sets_reaches_the_rest_of_the_chain_and_the_target_header() {
	// Its own `let` below it, a later shared file, and the target's own header
	// all read it. They used to see nothing the chain set, because the runner
	// had not built the environment yet when they were evaluated.
	assert!(env_under("$ true\n", &[".env.A = \"a\"\nlet x = ENV.A\n"]).is_empty());
	assert!(env_under("$ true\n", &[".env.A = \"a\"\n", "let x = ENV.A\n"]).is_empty());
	assert!(env_under(".env.B = ENV.A\n$ true\n", &[".env.A = \"a\"\n"]).is_empty());
}

#[test]
fn a_name_an_env_file_might_set_is_still_an_input() {
	// Which names a file sets is only known once it is read, at run time, and
	// the file may well not exist -- a project whose `.env` is git-ignored
	// still has to run. Listing the name is the answer that is never a lie.
	assert_eq!(env(".env-file = \".env\"\nprint(ENV.DB)\n"), ["DB"]);
}

#[test]
fn a_property_is_matched_by_the_exact_name_it_sets() {
	// `ENV.PORT` looks for `PORT` exactly before it tries another case, so a
	// caller's `PORT` still reaches it under a `.env.port`.
	assert_eq!(env(".env.port = \"3000\"\nprint(ENV.PORT)\n"), ["PORT"]);
}
