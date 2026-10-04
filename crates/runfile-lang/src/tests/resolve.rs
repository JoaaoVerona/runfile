//! Names nothing defines, found before anything runs -- by the rules the runner
//! binds names by, since a check that disagrees with it either refuses a file
//! that runs or passes one that does not.

use crate::resolve::{self, Kind, Unresolved};

fn tree(src: &str) -> crate::Target {
	crate::parse(src).unwrap_or_else(|e| panic!("{e}\n\n{src}"))
}

fn check(src: &str) -> Vec<Unresolved> {
	resolve::of_chain(&tree(src), &[])
}

/// What does not resolve, as `name@line`, kind aside.
fn found(src: &str) -> Vec<String> {
	check(src)
		.iter()
		.map(|u| format!("{}@{}", u.name, u.span.line))
		.collect()
}

fn clean(src: &str) {
	let f = check(src);
	assert!(f.is_empty(), "{f:#?}\n\n{src}");
}

/// The one thing `src` names that does not resolve.
fn only(src: &str) -> Unresolved {
	let mut f = check(src);
	assert_eq!(f.len(), 1, "{f:#?}\n\n{src}");
	f.remove(0)
}

// ---- functions

#[test]
fn a_call_to_a_function_that_does_not_exist_is_found_wherever_it_sits() {
	for (src, line) in [
		("let x = exists(\"a\")\n", 1),
		("if FLAG.never\n\tif FLAG.twice\n\t\texists(\"a\")\n\tend\nend\n", 3),
		("$ echo {{ exists(\"a\") }}\n", 1),
		("print(\"{{ exists(\"a\") }}\")\n", 1),
		(".env.X = exists(\"a\")\n$ true\n", 1),
		("let doc = json\n\t{\"a\": {{ exists(\"a\") }}}\nend\n", 2),
		("exec python3\n\tprint({{ exists(\"a\") }})\nend\n", 2),
		("run deploy --to={{ exists(\"a\") }}\n", 1),
		("print(length(exists(\"a\")))\n", 1),
		("let xs = [\n\t1,\n\texists(\"a\"),\n]\n", 3),
		("let c = code_of(run {{ exists(\"a\") }})\n", 1),
	] {
		let u = only(src);
		assert_eq!(
			(u.kind, u.name.as_str(), u.span.line),
			(Kind::Function, "exists", line),
			"{src}"
		);
	}
	// The name a capture is handed to is a call like any other.
	assert_eq!(
		found("$ cargo run -- {{ rest(ARGS) }}\nlet files = rest($ git ls-files)\n"),
		["rest@1", "rest@2"]
	);
}

#[test]
fn an_unknown_function_is_underlined_as_its_name_and_nothing_else() {
	let src = "if true\n\tlet found = exists(\"/etc/hosts\")\nend\n";
	let u = only(src);
	assert_eq!(&src[u.span.start..u.span.end], "exists");
	assert_eq!(u.span.line, 2);
}

#[test]
fn an_unknown_function_names_the_one_that_was_probably_meant() {
	let said = |src: &str| only(src).to_string();
	let m = said("let x = exists(\"a\")\n");
	assert!(m.contains("unknown function `exists`"), "{m}");
	assert!(
		m.contains("`directory_exists` or `file_exists`"),
		"one word of a longer name is the better guess: {m}"
	);
	assert!(!m.contains("`exit`"), "two letters out is not offered beside it: {m}");
	assert!(said("print(lenght(\"ab\"))\n").contains("did you mean `length`?"));
	assert!(said("prnt(\"hi\")\n").contains("did you mean `print`?"));
	assert!(said("print(to_uper(\"a\"))\n").contains("`to_upper`"));
	assert!(
		!said("zzz(1)\n").contains("did you mean"),
		"nothing near is nothing said"
	);
}

#[test]
fn the_calls_answered_before_the_table_are_functions_too() {
	clean("let a = code_of($ true)\nlet b = code_of(run build)\nlet c = try(read_file(\"x\"))\nprint(a, b, c)\n");
	for f in crate::functions::FUNCTIONS {
		assert!(crate::functions::exists(f.name), "{}", f.name);
	}
}

#[test]
fn a_run_key_that_does_not_exist_is_found_whatever_the_chain() {
	let f = only("print(RUN.oss)\n");
	assert_eq!(f.kind, Kind::RunKey);
	assert_eq!(f.to_string(), "line 1: unknown `RUN.oss`; did you mean `RUN.os`?");
	// Which keys exist does not depend on what a shared file binds.
	let f = resolve::functions(&tree("if FLAG.x\n\tprint(RUN.namespace)\nend\n"));
	assert_eq!(f.len(), 1, "{f:#?}");
	assert_eq!(
		f[0].to_string(),
		"line 2: unknown `RUN.namespace`; did you mean `RUN.namespaces`?"
	);
	clean("print(RUN.os, RUN.arch, RUN.cwd, RUN.file, RUN.parent, RUN.namespaces, RUN.user)\n");
}

// ---- names

#[test]
fn a_name_read_before_anything_binds_it_is_found() {
	assert_eq!(found("print(region)\n"), ["region@1"]);
	clean("let region = \"eu\"\nprint(region)\n");
	assert_eq!(found("print(region)\nlet region = \"eu\"\n"), ["region@1"]);
	// A `let` reads what was bound before it, not itself.
	assert_eq!(found("let n = n + 1\n"), ["n@1"]);
	assert_eq!(found("let a, b = [b, 1]\n"), ["b@1"]);
}

#[test]
fn a_name_is_found_in_every_position_one_can_sit_in() {
	for src in [
		"$ echo {{ ghost }}\n",
		"print(\"{{ ghost }}\")\n",
		".workdir = ghost\n$ true\n",
		"if ghost\nend\n",
		"while ghost\nend\n",
		"match ghost\ndefault\n\t$ true\nend\n",
		"for x in ghost\nend\n",
		"retry ghost every 1\n\t$ true\nend\n",
		"run {{ ghost }}:build\n",
		"let c = code_of(run build --x={{ ghost }})\n",
		"let out = $ echo {{ ghost }}\n",
		"exec sh {{ ghost }}\n\techo\nend\n",
		"let doc = json\n\t[{{ ghost }}]\nend\n",
		"let first = [ghost][0]\n",
		"let lines = lines($ ls {{ ghost }})\n",
	] {
		let u = only(src);
		assert_eq!((u.kind, u.name.as_str()), (Kind::Name, "ghost"), "{src}");
	}
}

#[test]
fn a_read_under_a_fallback_is_still_a_read_of_nothing() {
	// The fallback is taken every time, so the name the author meant is never
	// read -- which is the slip, not an optional value.
	let u = only("let region = \"eu\"\nprint(regoin ? \"us\")\n");
	assert_eq!(u.name, "regoin");
	assert!(u.to_string().contains("did you mean `region`?"), "{u}");
	assert_eq!(found("print(try(ghost))\n"), ["ghost@1"]);
}

#[test]
fn sources_and_literals_are_not_names_and_underscore_binds_nothing() {
	clean("print(ARG.x ? \"\", ENV.Y ? \"\", FLAG.z, RUN.os, ARGS, true, false, 1)\n");
	assert_eq!(found("let _, b = [1, 2]\nprint(b, _)\n"), ["_@2"]);
}

#[test]
fn a_function_named_without_its_parentheses_says_so() {
	let u = only("let n = length\n");
	assert!(
		u.to_string().contains("`length` is a function; call it as `length()`"),
		"{u}"
	);
	// And a name bound after a function is a binding like any other.
	clean("let first = 1\nprint(first)\n");
}

#[test]
fn a_near_miss_of_a_bare_source_or_a_literal_is_suggested() {
	assert!(only("print(args)\n").to_string().contains("did you mean `ARGS`?"));
	assert!(only("let ok = True\n").to_string().contains("did you mean `true`?"));
}

// ---- the runner's scoping rules

#[test]
fn a_let_inside_a_block_outlives_the_block() {
	clean("if FLAG.x\n\tlet v = 1\nend\nprint(v)\n");
	clean("do\n\tlet v = 1\nend\nprint(v)\n");
	clean("match RUN.os\ncase \"linux\"\n\tlet v = 1\ndefault\n\tlet w = 2\nend\nprint(v, w)\n");
	clean("retry 3\n\t$ true\nelse\n\tlet v = 1\nend\nprint(v)\n");
	// Either branch of an `if` counts: which one runs is the run's question.
	clean("if FLAG.x\n\tlet v = 1\nelse if FLAG.y\n\tlet v = 2\nend\nprint(v)\n");
}

#[test]
fn a_loop_s_names_end_with_the_loop() {
	let u = only("for host in [\"a\"]\n\tprint(host)\nend\nprint(host)\n");
	assert_eq!(u.span.line, 4);
	assert!(u.to_string().contains("a loop's names end with the loop"), "{u}");
	// Bound before it, the name is put back rather than taken away.
	clean("let host = \"z\"\nfor host in [\"a\"]\n\tprint(host)\nend\nprint(host)\n");
	// What the body binds is not the loop's own, and outlives it.
	clean("for host in [\"a\"]\n\tlet seen = host\nend\nprint(seen)\n");
	let u = only("while FLAG.x\n\tfor a, b in [[1, 2]]\n\tend\n\tprint(b)\nend\n");
	assert_eq!(u.name, "b");
}

#[test]
fn a_pass_reads_what_an_earlier_pass_bound() {
	clean("for i in range(3)\n\tif i > 0\n\t\tprint(prev)\n\tend\n\tlet prev = i\nend\n");
	clean("let i = 0\nwhile i < 3\n\tprint(last ? \"none\")\n\tlet last = i\n\ti = i + 1\nend\n");
	clean("retry 3\n\tprint(tried ? \"first\")\n\tlet tried = true\n\t$ false\nend\n");
	// The condition is asked again after each pass, of what the passes bound.
	clean("until ready ? false\n\tlet ready = true\nend\n");
	// Not carried out of a parallel iteration into the next one, though.
	assert_eq!(
		found("parallel for i in [1, 2]\n\tprint(prev)\n\tlet prev = i\nend\n"),
		["prev@2"]
	);
}

#[test]
fn a_property_at_the_top_of_a_loop_s_body_cannot_read_the_loop_s_names() {
	// The runner works the body's properties out once, before the first item is
	// bound -- so this is `dir is not defined` the moment it runs.
	let u = only("for dir in [\"web\"]\n\t.workdir = dir\n\t$ make\nend\n");
	assert_eq!((u.name.as_str(), u.span.line), ("dir", 2));
	assert!(u.to_string().contains("below the body's first statement"), "{u}");
	// Not even a binding of the name from above the loop: a `for` puts it aside.
	assert_eq!(
		found("let dir = \"x\"\nfor dir in [\"web\"]\n\t.workdir = dir\n\t$ make\nend\n"),
		["dir@3"]
	);
	// Below a statement, it is applied where it sits.
	clean("for dir in [\"web\"]\n\t$ true\n\t.workdir = dir\n\t$ make\nend\n");
	// A `parallel for` does not put an outer binding aside.
	assert_eq!(
		found("parallel for dir in [\"web\"]\n\t.workdir = dir\n\t$ make\nend\n"),
		["dir@2"]
	);
	clean("let dir = \"x\"\nparallel for dir in [\"web\"]\n\t.workdir = dir\n\t$ make\nend\n");
}

#[test]
fn what_a_parallel_branch_binds_does_not_reach_past_it() {
	let src = "parallel do\n\tdo\n\t\tlet a = 1\n\t\tprint(a)\n\tend\n\tprint(a)\nend\nprint(a)\n";
	assert_eq!(found(src), ["a@6", "a@8"], "a sibling, then the code after the block");
	let u = &check(src)[1];
	assert!(u.to_string().contains("inside a `parallel` branch"), "{u}");
	let u = only("parallel for i in [1]\n\tlet x = i\nend\nprint(x)\n");
	assert!(u.to_string().contains("inside a `parallel` branch"), "{u}");
	let u = only("parallel for i in [1]\n\tprint(i)\nend\nprint(i)\n");
	assert!(u.to_string().contains("a loop's names end"), "{u}");
}

#[test]
fn a_reassignment_of_a_name_nothing_bound_is_found() {
	let u = only("count = 1\n");
	assert_eq!((u.kind, u.name.as_str()), (Kind::Rebind, "count"));
	assert!(u.to_string().contains("bind it with `let count = …`"), "{u}");
	clean("let count = 0\ncount = count + 1\n");
	clean("if FLAG.x\n\tlet count = 0\nend\ncount = 1\n");
	// The slip it exists for: the name meant keeps its old value.
	let u = only("let total = 0\ntotl = total + 1\n");
	assert!(u.to_string().contains("did you mean `total`?"), "{u}");
	// One missing `let` is one message, not one per line that reads it.
	assert_eq!(found("n = 1\nprint(n)\nprint(n)\n"), ["n@1"]);
}

// ---- the shared chain

#[test]
fn a_shared_file_binds_its_top_level_lets_for_the_files_below_it() {
	let shared = [tree("let region = \"eu\"\nif FLAG.x\n\tlet hidden = 1\nend\n")];
	let target = tree("print(region)\nprint(hidden)\n");
	let f: Vec<String> = resolve::of_chain(&target, &shared)
		.iter()
		.map(|u| u.name.clone())
		.collect();
	assert_eq!(f, ["hidden"], "a `let` inside a block in a shared file never runs");
	// And a nested shared file sees the one above it.
	let inner = tree("let zone = concat(region, \"-1\")\n");
	assert!(resolve::of_shared(&inner, &shared[..]).is_empty());
	assert_eq!(resolve::of_shared(&inner, &[]).len(), 1);
}

#[test]
fn a_shared_file_is_checked_the_way_it_is_folded() {
	let file = tree("if FLAG.x\n\tlet a = 1\n\tprint(a)\nend\nlet b = a ? \"x\"\nprint(nope)\n.env.B = b\n");
	let f: Vec<String> = resolve::of_shared(&file, &[])
		.iter()
		.map(|u| format!("{}@{}", u.name, u.span.line))
		.collect();
	// Inside the `if` its own `let` counts; after it, nothing the `if` bound
	// does, since it never runs. What never runs is still checked.
	assert_eq!(f, ["a@5", "nope@6"]);
}

#[test]
fn with_the_chain_unknown_only_calls_are_checked() {
	let f = resolve::functions(&tree("print(region)\nnope(1)\n"));
	assert_eq!(f.len(), 1, "{f:#?}");
	assert_eq!((f[0].kind, f[0].name.as_str()), (Kind::Function, "nope"));
}

// ---- the documentation

#[test]
fn every_documented_example_calls_only_functions_that_exist() {
	// Hover and completion show these, and a person copies them out.
	let examples = crate::functions::FUNCTIONS
		.iter()
		.map(|f| (f.name, f.example))
		.chain(crate::KEYWORDS.iter().map(|k| (k.name, k.example)));
	for (name, example) in examples {
		let Ok(t) = crate::parse(example) else { continue };
		let wrong = resolve::functions(&t);
		assert!(wrong.is_empty(), "the example for `{name}`: {wrong:#?}");
	}
}

#[test]
fn a_suggestion_is_only_offered_when_something_is_near() {
	assert_eq!(resolve::suggest("x", ["y", "xs"]), None, "too short to guess at");
	assert_eq!(resolve::suggest("cnt", ["count"]), None);
	assert_eq!(
		resolve::suggest("ARGS", ["ARGS"]),
		None,
		"a name is not a near miss of itself"
	);
	assert_eq!(
		resolve::suggest("upper", ["to_upper", "to_lower"]).as_deref(),
		Some("did you mean `to_upper`?")
	);
	assert_eq!(
		resolve::suggest("exists", ["file_exists", "directory_exists", "exit", "dir_exists"]).as_deref(),
		Some("did you mean `dir_exists`, `directory_exists` or `file_exists`?")
	);
}

#[test]
fn edits_within_is_bounded_and_agrees_with_full_distance_when_near() {
	use crate::resolve::{edits, edits_within};
	// Within the bound it is the exact distance; far apart it stops early and
	// reports `max + 1` rather than computing the whole table (audit SA-024).
	assert_eq!(edits_within("kitten", "sitting", 3), 3);
	assert_eq!(edits_within("kitten", "sitting", 2), 3, "past the bound → max+1");
	assert_eq!(edits_within("abc", "abc", 2), 0);
	// A length gap larger than the bound is rejected without walking the table.
	let long_a = "a".repeat(5000);
	let long_b = "b".repeat(4000);
	assert_eq!(edits_within(&long_a, &long_b, 2), 3);
	// `edits` is still the exact distance (the LSP sorts completions by it).
	assert_eq!(edits("exists", "exit"), 2);
}

#[test]
fn suggest_over_many_long_candidates_stays_cheap_and_correct() {
	use crate::resolve::suggest;
	// The real hint is unchanged for a plausible typo (one edit away)...
	assert_eq!(
		suggest("buld", ["build", "test", "deploy"]).as_deref(),
		Some("did you mean `build`?")
	);
	// ...and a flood of long, near-identical unknown names does not blow up:
	// the length gate and banded distance keep each comparison cheap (SA-024).
	let names: Vec<String> = (0..5000).map(|i| format!("{}{i}", "z".repeat(200))).collect();
	let got = suggest(&"q".repeat(200), names.iter().map(String::as_str));
	assert!(got.is_none(), "no near match among long names");
}
