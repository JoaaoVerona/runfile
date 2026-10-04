//! Each rule, on what it must find and on the text nearest to it that it must
//! leave alone -- which is where a false report would come from -- and the
//! tables it reads, held to the functions they describe.

use crate::check::{Finding, RULES, check};
use crate::eval::{EvalError, Scope};
use crate::functions::{FUNCTIONS, call, call_with};
use crate::span::Span;
use crate::types::{ANY, BOOL, Kinds, LIST, NUM, SIGNATURES, STR, Ty};
use crate::value::{TypeError, Value};

fn parsed(src: &str) -> crate::Target {
	crate::parse(src).unwrap_or_else(|e| panic!("{src}\n{e}"))
}

fn findings(src: &str) -> Vec<Finding> {
	check(src, &parsed(src), Some(&[]), false)
}

/// The one finding in `src`, which has to be `rule`'s and cover `covers`.
fn one(src: &str, rule: &str, covers: &str) -> Finding {
	let f = findings(src);
	assert_eq!(f.len(), 1, "{src}\n{f:#?}");
	assert_eq!(f[0].rule, rule, "{src}\n{f:#?}");
	assert_eq!(&src[f[0].span.start..f[0].span.end], covers, "{src}\n{f:#?}");
	f[0].clone()
}

fn clean(src: &str) {
	let f = findings(src);
	assert!(f.is_empty(), "{src}\n{f:#?}");
}

fn fix(f: &Finding) -> &str {
	f.fix.as_deref().unwrap_or_default()
}

// ---- arity

#[test]
fn a_call_with_a_count_of_arguments_the_function_never_takes_is_found_at_its_name() {
	let f = one("print(length(\"a\", \"b\"))\n", "arity", "length");
	assert_eq!(f.message, "`length` takes 1 argument, and is given 2");
	one("let id = uuid(1)\n", "arity", "uuid");
	one("print(now(\"iso\", \"utc\"))\n", "arity", "now");
	one("exit(1, 2)\n", "arity", "exit");
	one("print(split(\"a,b\"))\n", "arity", "split");
	let f = one("print(substring(\"abc\"))\n", "arity", "substring");
	assert!(f.message.contains("takes 2 or 3 arguments"), "{}", f.message);
	one("print(try())\n", "arity", "try");
}

#[test]
fn a_call_given_what_the_function_takes_is_left_alone() {
	clean("print(concat(\"a\", \"b\", \"c\"))\n");
	clean("print(now())\nprint(now(\"unix\"))\n");
	clean("exit()\n");
	clean("print(join(\",\"))\n");
	clean("print(substring(\"abc\", 1, 1))\n");
	// A function that does not exist is the name check's, and not said twice.
	clean("print(exists(\"a\", \"b\"))\n");
}

// ---- wrong-type

#[test]
fn a_string_where_a_number_is_needed_is_found() {
	let f = one("print(ARG.port + 1)\n", "wrong-type", "ARG.port");
	assert_eq!(f.message, "`+` needs numbers, and `ARG.port` is a string");
	assert_eq!(fix(&f), "`number(ARG.port)` reads it as one");
	let f = one("let port = ARG.port\nprint(port * 2)\n", "wrong-type", "port");
	assert_eq!(fix(&f), "`number(port)` reads it as one");
	let f = one("print(\"v\" + 1)\n", "wrong-type", "\"v\"");
	assert_eq!(fix(&f), "`concat(\"v\", 1)` joins text; `+` only adds numbers");
	one("sleep(ARG.seconds)\n", "wrong-type", "ARG.seconds");
	one("retry ENV.ATTEMPTS\n\t$ true\nend\n", "wrong-type", "ENV.ATTEMPTS");
	one("retry 3 every ARG.wait\n\t$ true\nend\n", "wrong-type", "ARG.wait");
	one("print(-ENV.X)\n", "wrong-type", "ENV.X");
	one("print(ARGS < 2)\n", "wrong-type", "ARGS");
}

#[test]
fn a_condition_that_is_never_a_bool_is_found() {
	let f = one("if ENV.CI\n\tprint(\"ci\")\nend\n", "wrong-type", "ENV.CI");
	assert_eq!(f.message, "`if` needs `true` or `false`, and `ENV.CI` is a string");
	assert_eq!(fix(&f), "compare it, as `ENV.CI == \"true\"`");
	let f = one("while length(ARGS)\n\tbreak\nend\n", "wrong-type", "length(ARGS)");
	assert_eq!(fix(&f), "compare it, as `length(ARGS) != 0`");
	one("until ARGS\n\tbreak\nend\n", "wrong-type", "ARGS");
	one("if !ARG.force\n\tprint(\"x\")\nend\n", "wrong-type", "ARG.force");
	one("if FLAG.a && ENV.B\n\tprint(\"x\")\nend\n", "wrong-type", "ENV.B");
	one("if ENV.B || FLAG.a\n\tprint(\"x\")\nend\n", "wrong-type", "ENV.B");
}

#[test]
fn a_loop_an_index_or_an_unpacking_over_what_is_never_a_list_is_found() {
	let f = one("for f in ENV.FILES\n\tprint(f)\nend\n", "wrong-type", "ENV.FILES");
	assert_eq!(
		fix(&f),
		"`lines(ENV.FILES)` or `split(ENV.FILES, \",\")` makes a list of it"
	);
	one("print(ARG.name[0])\n", "wrong-type", "ARG.name");
	one("print(ARGS[\"a\"])\n", "wrong-type", "\"a\"");
	one("let a, b = ARG.pair\n", "wrong-type", "ARG.pair");
	one("for a, b in ARGS\n\tprint(a, b)\nend\n", "wrong-type", "ARGS");
}

#[test]
fn an_argument_of_a_type_the_function_refuses_is_found() {
	let f = one("print(join(ARGS, \" \"))\n", "wrong-type", "ARGS");
	assert_eq!(
		f.message,
		"the 1st argument of `join` has to be a string, and `ARGS` is a list"
	);
	assert_eq!(fix(&f), "the separator comes first: `join(\" \", ARGS)`");
	let f = one("print(length(FLAG.x))\n", "wrong-type", "FLAG.x");
	assert_eq!(f.message, "`length` takes a string or a list, and `FLAG.x` is a bool");
	one("print(first(ARG.names))\n", "wrong-type", "ARG.names");
	one("print(contains(\"abc\", 1))\n", "wrong-type", "1");
	one("print(to_upper(length(ARGS)))\n", "wrong-type", "length(ARGS)");
	one("let c = code_of(ARG.cmd)\n", "wrong-type", "ARG.cmd");
	one("printf(\"%d\\n\", ARG.n)\n", "wrong-type", "ARG.n");
}

#[test]
fn a_value_that_can_be_the_right_type_is_left_alone() {
	// Either, depending on whether the variable is set: sometimes right.
	clean("let port = ENV.PORT ? 3000\nprint(port + 1)\n");
	clean("let p = number(ARG.port)\nprint(p + 1)\n");
	// A name holds whatever any line binds it to.
	clean("let x = \"1\"\nif FLAG.n\n\tx = 2\nend\nprint(x + 1)\n");
	clean("for x in [1, \"a\"]\n\tprint(x + 1)\nend\n");
	clean("if FLAG.a\n\tprint(\"x\")\nend\nif $ true\n\tprint(\"y\")\nend\n");
	clean("for f in lines($ ls)\n\tprint(f)\nend\n");
	clean("print(contains(ARGS, 1))\n");
	// The right side is never asked.
	clean("if false && ENV.X\n\tprint(\"x\")\nend\nif true || ENV.X\n\tprint(\"y\")\nend\n");
	clean("printf(\"%s\\n\", ARGS)\n");
}

#[test]
fn a_name_the_chain_binds_carries_its_type_and_one_that_is_unknown_is_anything() {
	let shared = parsed("let port = ARG.port\n");
	let src = "print(port + 1)\n";
	let f = check(src, &parsed(src), Some(std::slice::from_ref(&shared)), false);
	assert_eq!(f.len(), 1, "{f:#?}");
	assert_eq!(f[0].rule, "wrong-type");
	// When the chain cannot be read, a name could hold anything it bound.
	let src = "let port = ARG.port\nprint(port + 1)\nprint(ARG.x + 1)\n";
	let f = check(src, &parsed(src), None, false);
	assert_eq!(f.len(), 1, "only the source, which no chain can change: {f:#?}");
	assert_eq!(&src[f[0].span.start..f[0].span.end], "ARG.x");
}

#[test]
fn what_a_value_that_always_fails_is_then_asked_is_not_reported_as_well() {
	// The `+` fails, so the `if` never sees what it would have made.
	one("if ARG.x + 1\n\tprint(\"x\")\nend\n", "wrong-type", "ARG.x");
}

// ---- never-equal

#[test]
fn a_comparison_across_types_is_found() {
	let f = one("if ARG.n == 3\n\tprint(\"x\")\nend\n", "never-equal", "ARG.n == 3");
	assert_eq!(
		f.message,
		"`ARG.n == 3` is always false: `ARG.n` is a string and `3` is a number, and a value never equals one of another type"
	);
	assert_eq!(fix(&f), "`number(ARG.n)` reads it as a number");
	let f = one(
		"let status = code_of($ make)\nif status != \"0\"\n\texit(1)\nend\n",
		"never-equal",
		"status != \"0\"",
	);
	assert!(f.message.contains("always true"), "{}", f.message);
	assert_eq!(fix(&f), "write `0` without the quotes");
	let f = one(
		"let notes = regex_matches(\"x\", \"y\")\nif notes == \"false\"\n\texit(1)\nend\n",
		"never-equal",
		"notes == \"false\"",
	);
	assert_eq!(fix(&f), "write `false` without the quotes");
}

#[test]
fn a_platform_compared_with_a_name_it_never_has_is_found() {
	let f = one(
		"if RUN.os == \"darwin\"\n\tprint(\"x\")\nend\n",
		"never-equal",
		"RUN.os == \"darwin\"",
	);
	assert_eq!(
		f.message,
		"`RUN.os == \"darwin\"` is always false: `RUN.os` is only ever `linux`, `mac` or `windows`"
	);
	assert_eq!(fix(&f), "write `\"mac\"`");
	let f = one(
		"if \"x86_64\" != RUN.arch\n\texit(1)\nend\n",
		"never-equal",
		"\"x86_64\" != RUN.arch",
	);
	assert_eq!(fix(&f), "write `\"x86-64\"`");
	one(
		"let os = RUN.os\nif os == \"Linux\"\n\tprint(\"x\")\nend\n",
		"never-equal",
		"os == \"Linux\"",
	);
}

#[test]
fn a_comparison_that_can_hold_is_left_alone() {
	clean("if RUN.os == \"mac\"\n\tprint(\"x\")\nend\n");
	clean("if ENV.CI == \"true\"\n\tprint(\"x\")\nend\n");
	clean("let v = ENV.V ? 1\nif v == 1\n\tprint(\"x\")\nend\n");
	clean("if RUN.os == ENV.OS\n\tprint(\"x\")\nend\n");
	// A setting edited by hand is how this is written, and not a mistake.
	clean("let mode = \"dev\"\nif mode == \"prod\"\n\tprint(\"x\")\nend\n");
	// A name bound to anything but the platform could hold anything.
	clean("let os = RUN.os\nif FLAG.x\n\tos = ENV.OS\nend\nif os == \"darwin\"\n\tprint(\"x\")\nend\n");
}

// ---- unreachable-case

#[test]
fn a_case_no_subject_can_reach_is_found_at_its_label() {
	let f = one(
		"match ARG.env\ncase \"dev\"\n\tprint(1)\ncase \"dev\"\n\tprint(2)\nend\n",
		"unreachable-case",
		"\"dev\"",
	);
	assert!(f.span.line == 4, "the second: {f:#?}");
	assert!(f.message.contains("line 2"), "{}", f.message);
	let f = one(
		"match RUN.os\ncase \"linux\"\n\tprint(1)\ncase \"darwin\"\n\tprint(2)\nend\n",
		"unreachable-case",
		"\"darwin\"",
	);
	assert_eq!(fix(&f), "write `case \"mac\"`");
	one(
		"match $ make\ncase \"0\"\n\tprint(1)\ncase \"ok\"\n\tprint(2)\nend\n",
		"unreachable-case",
		"\"ok\"",
	);
	one(
		"match FLAG.verbose\ncase \"yes\"\n\tprint(1)\ndefault\n\tprint(2)\nend\n",
		"unreachable-case",
		"\"yes\"",
	);
	one(
		"match length(ARGS)\ncase \"one\"\n\tprint(1)\ndefault\n\tprint(2)\nend\n",
		"unreachable-case",
		"\"one\"",
	);
}

#[test]
fn a_case_the_subject_can_reach_is_left_alone() {
	clean("match $ make\ncase \"0\"\n\tprint(1)\ncase \"-1\"\n\tprint(2)\nend\n");
	clean("match FLAG.x\ncase \"true\"\n\tprint(1)\ncase \"false\"\n\tprint(2)\nend\n");
	clean("match length(ARGS)\ncase \"3\"\n\tprint(1)\ncase \"2.5\"\n\tprint(2)\nend\n");
	clean("match ARGS\ncase \"a b\"\n\tprint(1)\nend\n");
	clean("match ENV.X\ncase \"anything\"\n\tprint(1)\nend\n");
	clean("match RUN.arch\ncase \"x86-64\"\n\tprint(1)\ncase \"arm64\"\n\tprint(2)\nend\n");
}

// ---- invalid-literal

#[test]
fn a_pattern_or_format_that_never_works_is_found() {
	let f = one("print(regex_matches(\"a\", \"(\"))\n", "invalid-literal", "\"(\"");
	assert!(f.message.contains("unclosed group"), "{}", f.message);
	one(
		"for f in glob(\"src/[a\")\n\tprint(f)\nend\n",
		"invalid-literal",
		"\"src/[a\"",
	);
	let f = one(
		"printf(\"%s %s\\n\", \"only one\")\n",
		"invalid-literal",
		"\"%s %s\\n\"",
	);
	assert!(f.message.contains("more substitutions"), "{}", f.message);
	one("printf(\"%q\", 1)\n", "invalid-literal", "\"%q\"");
	one("printf(\"%d\", 2.5)\n", "invalid-literal", "2.5");
	let f = one("print(now(\"tuesday\"))\n", "invalid-literal", "\"tuesday\"");
	assert!(f.message.contains("not a time format"), "{}", f.message);
}

#[test]
fn a_number_that_is_never_accepted_is_found() {
	let f = one("let xs = [1, 2]\nprint(xs[-1])\n", "invalid-literal", "-1");
	assert_eq!(fix(&f), "`last(xs)` is the last item");
	one("print(substring(\"abc\", -1))\n", "invalid-literal", "-1");
	one("print(repeat(\"a\", 1.5))\n", "invalid-literal", "1.5");
	one("print(range(0.5))\n", "invalid-literal", "0.5");
	one("sleep(-1)\n", "invalid-literal", "-1");
	one("print(regex_capture(\"a\", \"(a)\", -1))\n", "invalid-literal", "-1");
	one("print(7 % 0)\n", "invalid-literal", "0");
	one("let a, b, c = [1, 2]\n", "invalid-literal", "[1, 2]");
	one("for a, b in [[1], [2, 3]]\n\tprint(a)\nend\n", "invalid-literal", "[1]");
}

#[test]
fn a_literal_that_works_is_left_alone() {
	clean("print(regex_matches(\"a\", \"(a)|b\"))\n");
	clean("printf(\"%d%% of %s, %.2f\\n\", 50, \"them\", 1.5)\n");
	clean("print(now(\"unix\"))\n");
	clean("let xs = [1]\nprint(xs[0])\nprint(range(-2, 2))\nprint(substring(\"abc\", 0, 2))\n");
	clean("let d = number(ARG.d)\nprint(1 / d)\n");
	clean("let a, b = [1, 2, 3]\n");
}

// ---- capture-position

#[test]
fn a_run_where_nothing_can_run_it_is_found() {
	let f = one(
		"if contains(\"main\", $ git branch)\n\tprint(\"x\")\nend\n",
		"capture-position",
		"$ git branch",
	);
	assert!(f.message.starts_with("`if` runs a `$` run only when"), "{}", f.message);
	one(
		"if code_of($ make)\n\tprint(\"x\")\nend\n",
		"capture-position",
		"$ make",
	);
	one(
		"match code_of($ make)\ncase \"0\"\n\tprint(\"x\")\nend\n",
		"capture-position",
		"$ make",
	);
	for (src, covers) in [
		("let branch = $ git branch\n", "$ git branch"),
		("let files = lines($ ls)\n", "$ ls"),
		("let c = code_of(run build)\n", "run build"),
	] {
		let f = check(src, &parsed(src), Some(&[]), true);
		assert_eq!(f.len(), 1, "{src}\n{f:#?}");
		assert_eq!(f[0].rule, "capture-position");
		assert_eq!(&src[f[0].span.start..f[0].span.end], covers);
	}
}

#[test]
fn a_run_where_the_runner_runs_one_is_left_alone() {
	clean("if $ true\n\tprint(\"x\")\nend\n");
	clean("let files = lines($ ls)\nlet c = code_of($ true)\ncode_of($ false)\nlet d = code_of(run build)\n");
	clean("match $ make\ncase \"0\"\n\tprint(1)\nend\n");
	clean("parallel for f in lines($ ls)\n\tprint(f)\nend\n");
	// In a shared file, only what runs: its top-level `let`s.
	let src = "if FLAG.x\n\tlet branch = $ git branch\nend\n$ echo never runs\n";
	assert!(check(src, &parsed(src), Some(&[]), true).is_empty());
}

// ---- glued-list

#[test]
fn a_list_with_a_literal_glued_onto_its_end_is_found() {
	let f = one(
		"let dirs = glob(\"build/*\")\n$ rm -rf {{ dirs }}/cache\n",
		"glued-list",
		"dirs",
	);
	assert_eq!(fix(&f), "put a space before `/cache`, or give the list its own word");
	// ARGS is a list too.
	one("$ cat {{ ARGS }}.log\n", "glued-list", "ARGS");
}

#[test]
fn a_list_on_its_own_or_glued_only_on_the_left_is_left_alone() {
	// A space after it: its own word, the ordinary way to pass a list.
	clean("let dirs = glob(\"build/*\")\n$ rm -rf {{ dirs }}\n");
	clean("$ tauri build {{ ARGS }} -- --no-default-features\n");
	// Built into a word with a prefix: the "zero or one positional" idiom.
	clean("$ go build -Dexec.args={{ ARGS }}\n");
	clean("$ inst={{ ARGS }}; systemctl restart \"r${inst:+-$inst}\"\n");
	// Sometimes a string, so its string form is fine glued.
	clean("let d = ARG.d ? \"x\"\n$ rm -rf {{ d }}/cache\n");
	// A string list element indexed out is a string, not a list.
	clean("let xs = [\"a\"]\n$ echo {{ xs[0] }}/x\n");
	// An `exec` body is not a `$` line: a list there is the author's language.
	clean("let xs = glob(\"*\")\nexec python3\n\tprint({{ xs }}.count)\nend\n");
}

// ---- the documentation

#[test]
fn every_rule_is_named_once() {
	let mut ids: Vec<&str> = RULES.iter().map(|r| r.id).collect();
	ids.sort_unstable();
	ids.dedup();
	assert_eq!(ids.len(), RULES.len());
}

#[test]
fn every_documented_example_is_free_of_findings() {
	// Hover and completion show these, and a person copies them out.
	for f in FUNCTIONS {
		let src = format!("# what {} does\n\n{}\n", f.name, strip_results(f.example));
		let Ok(t) = crate::parse(&src) else { continue };
		let found = check(&src, &t, None, false);
		assert!(found.is_empty(), "the example for `{}`: {found:#?}", f.name);
	}
}

/// An example with its `  # result` annotations dropped.
fn strip_results(example: &str) -> String {
	example
		.lines()
		.map(|l| match l.find("  #") {
			Some(k) if !l.trim_start().starts_with('$') => &l[..k],
			_ => l,
		})
		.collect::<Vec<_>>()
		.join("\n")
}

// ---- the tables, held to the functions they describe

#[test]
fn every_function_has_one_signature_and_every_signature_a_function() {
	let mut names: Vec<&str> = SIGNATURES.iter().map(|s| s.name).collect();
	assert!(
		names.windows(2).all(|w| w[0] < w[1]),
		"sorted, since a lookup searches them by halves"
	);
	let mut functions: Vec<&str> = FUNCTIONS.iter().map(|f| f.name).chain(["code_of", "try"]).collect();
	functions.sort_unstable();
	names.sort_unstable();
	assert_eq!(names, functions);
}

/// A value of `kind` that `name` is likely to accept at argument `at` of `n`,
/// so a call fails only for the reason being asked about.
fn sample(name: &str, at: usize, n: usize, kind: Kinds) -> Value {
	let s = |t: &str| Value::Str(t.to_string());
	match kind {
		STR => match (name, at) {
			("printf", 0) => s(&vec!["%s"; n.saturating_sub(1)].join(" ")),
			("json_format", 1) => s("  "),
			(n, 0) if n.starts_with("json_") => s("{\"a\": [1]}"),
			(n, 1) if n.starts_with("json_") => s("a"),
			("now", _) => s("iso"),
			("base64_decode", _) => s("YQ=="),
			("number", _) => s("12"),
			("write_file", 0) => s("out.txt"),
			("temp_file", 1) => s("txt"),
			("glob", _) => s("*.none"),
			_ => s("a"),
		},
		NUM => Value::Num(match (name, at) {
			("sleep" | "exit", _) | ("substring", 1) | ("regex_capture" | "regex_capture_all", 2) => 0.0,
			_ => 1.0,
		}),
		BOOL => Value::Bool(true),
		_ => Value::List(vec![s("a")]),
	}
}

/// Call a function the way a run does, in a directory of its own.
fn call_for_real(name: &str, args: Vec<Value>, dir: &std::path::Path) -> Result<Value, EvalError> {
	let mut sc = Scope::new();
	sc.assume_yes = true;
	sc.base_dir = dir.to_path_buf();
	let sp = Span::new(0, 0, 1);
	let out = match name {
		// Evaluated before the table is: it has to catch what its argument does.
		"try" => {
			let exprs: Vec<crate::Expr> = args
				.iter()
				.map(|_| crate::Expr::Str(vec![crate::InterpPart::Literal("a".into())], sp))
				.collect();
			call(name, &exprs, &mut sc, sp)
		}
		_ => call_with(name, args, &mut sc, sp),
	};
	for made in sc.temps.take() {
		let _ = std::fs::remove_file(&made).or_else(|_| std::fs::remove_dir_all(&made));
	}
	out
}

fn kind_of(v: &Value) -> Kinds {
	match v {
		Value::Str(_) => STR,
		Value::Num(_) => NUM,
		Value::Bool(_) => BOOL,
		Value::List(_) => LIST,
	}
}

#[test]
fn every_count_of_arguments_the_table_refuses_is_refused_by_the_function() {
	let dir = tempfile::tempdir().unwrap();
	for sig in SIGNATURES.iter().filter(|s| s.name != "code_of") {
		for n in 0..=5 {
			// What prints would print past the test harness, which captures
			// only its own macros; the refusals are what is being asked about.
			if matches!(sig.name, "print" | "printf") && sig.takes(n) {
				continue;
			}
			let tys = vec![Ty::ANY; n];
			let args: Vec<Value> = (0..n)
				.map(|at| {
					let accepted = sig.accepts(at, &tys);
					let kind = [STR, NUM, LIST, BOOL]
						.into_iter()
						.find(|k| accepted & k != 0)
						.unwrap_or(STR);
					sample(sig.name, at, n, kind)
				})
				.collect();
			let got = call_for_real(sig.name, args, dir.path());
			let refused = matches!(got, Err(EvalError::Arity { .. } | EvalError::UnknownFunction { .. }));
			assert_eq!(
				refused,
				!sig.takes(n),
				"`{}` with {n} arguments: the table says {}, and the call gave {got:?}",
				sig.name,
				sig.arity()
			);
		}
	}
}

#[test]
fn every_type_the_table_refuses_is_refused_by_the_function_and_what_it_answers_is_listed() {
	// A type the table refuses has to fail when it runs -- a mistake the other
	// way is a report about a call that works -- and what a call answers with has
	// to be something the table says it can, or a type worked out from it could
	// leave out what a value really is.
	let dir = tempfile::tempdir().unwrap();
	let kinds = [STR, NUM, BOOL, LIST];
	for sig in SIGNATURES.iter().filter(|s| s.name != "code_of") {
		let most = sig.max.unwrap_or(sig.min + 2).min(3);
		for n in sig.min..=most {
			for combo in 0..4usize.pow(n as u32) {
				let picked: Vec<Kinds> = (0..n).map(|k| kinds[(combo / 4usize.pow(k as u32)) % 4]).collect();
				let tys: Vec<Ty> = picked.iter().map(|k| Ty::of(*k)).collect();
				let refused = (0..n).any(|k| tys[k].refused(sig.accepts(k, &tys)));
				if matches!(sig.name, "print" | "printf") && !refused {
					continue;
				}
				let args: Vec<Value> = picked
					.iter()
					.enumerate()
					.map(|(k, kind)| sample(sig.name, k, n, *kind))
					.collect();
				let items: Vec<Ty> = args
					.iter()
					.map(|v| match v {
						Value::List(xs) => Ty::list(xs.iter().fold(0, |k, x| k | kind_of(x))),
						other => Ty::of(kind_of(other)),
					})
					.collect();
				let got = call_for_real(sig.name, args, dir.path());
				let type_error = matches!(
					got,
					Err(EvalError::Type {
						source: TypeError::Expected { .. },
						..
					})
				);
				assert_eq!(
					refused,
					type_error,
					"`{}` given {}: the table {} it, and the call gave {got:?}",
					sig.name,
					picked
						.iter()
						.map(|k| crate::types::named(*k))
						.collect::<Vec<_>>()
						.join(", "),
					if refused { "refuses" } else { "takes" },
				);
				if let Ok(v) = &got {
					let answer = sig.answer(&items);
					assert!(
						answer.kinds & kind_of(v) != 0,
						"`{}` answered {v:?}, which the table does not list",
						sig.name
					);
					if let Value::List(xs) = v {
						for x in xs {
							assert!(
								answer.items & kind_of(x) != 0,
								"`{}` answered a list holding {x:?}, which the table does not list",
								sig.name
							);
						}
					}
				}
			}
		}
	}
	let _ = ANY;
}
