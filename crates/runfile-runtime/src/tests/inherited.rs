//! What a target hands the target it `run`s, and what that target makes of it.
//!
//! The first half asserts the hand-over itself, with a recorder standing in for
//! the host: what a `run` is given from every place a property can sit, and
//! which half each value lands in. The second runs real targets, because what
//! matters in the end is what a called target's commands and `ENV.X` see.

use super::{Recorder, host_run, project, run_src};
use crate::env::Inherited;
use crate::props::Props;
use crate::run::{Runner, run_target_with};
use runfile_lang::eval::Scope;
use std::collections::HashMap;
use std::path::Path;

fn exported<'a>(env: &'a Inherited, key: &str) -> Option<&'a str> {
	env.exported.get(key).map(String::as_str)
}

fn default_of<'a>(env: &'a Inherited, key: &str) -> Option<&'a str> {
	env.defaults.get(key).map(String::as_str)
}

fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
	pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// A path as a `.run` string can hold it on every platform.
fn slashed(p: &Path) -> String {
	p.to_string_lossy().replace('\\', "/")
}

/// Walk `src` as a target another target ran with `inherited`.
fn walk_run_with(src: &str, inherited: Inherited, d: &Recorder) {
	let target = runfile_lang::parse(src).expect("parse");
	let mut scope = Scope::new();
	scope.env = inherited.environment();
	let mut r = Runner {
		scope,
		chain: Vec::new(),
		env: Vec::new(),
		anchor: std::env::temp_dir(),
		dispatch: d,
		interrupted: None,
		label: None,
		colour: None,
		dry_run: false,
		trace: Vec::new(),
	};
	let base = Props {
		inherited: Some(std::sync::Arc::new(inherited)),
		..Props::default()
	};
	run_target_with(&target, base, &mut r).expect("the walk");
}

/// A variable this process already has, with a plain name and a value no
/// test changes while others run.
fn plain_process_variable() -> (String, String) {
	std::env::vars()
		.find(|(k, v)| {
			!k.eq_ignore_ascii_case("PATH")
				// `with_home` swaps it for the tests that need a home of their own.
				&& !k.eq_ignore_ascii_case("HOME")
				&& !k.starts_with("RUNFILE_")
				&& !v.starts_with("encrypted:")
				&& k.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
				&& k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
		})
		.expect("a process with no plain environment variable at all")
}

// ------------------------------------------------------------------ the hand-over

#[test]
fn a_run_is_handed_the_environment_of_its_own_line() {
	let rec = Recorder::default();
	run_src(
		".env.RUNFILE_T_HEADER = \"header\"\n\
		 \n\
		 run first\n\
		 \n\
		 do\n\
		 \t.env.RUNFILE_T_BLOCK = \"block\"\n\
		 \n\
		 \trun inside\n\
		 end\n\
		 \n\
		 run after\n",
		&rec,
	)
	.unwrap();
	for target in ["first", "inside", "after"] {
		assert_eq!(
			exported(&rec.env_of(target), "RUNFILE_T_HEADER"),
			Some("header"),
			"{target}"
		);
	}
	assert_eq!(exported(&rec.env_of("inside"), "RUNFILE_T_BLOCK"), Some("block"));
	assert_eq!(exported(&rec.env_of("first"), "RUNFILE_T_BLOCK"), None, "not yet");
	assert_eq!(
		exported(&rec.env_of("after"), "RUNFILE_T_BLOCK"),
		None,
		"the block closed"
	);
}

#[test]
fn a_property_below_a_statement_is_handed_over_from_its_line_down() {
	let rec = Recorder::default();
	run_src("run before\n.env.RUNFILE_T_LATE = \"late\"\nrun after\n", &rec).unwrap();
	assert_eq!(exported(&rec.env_of("before"), "RUNFILE_T_LATE"), None);
	assert_eq!(exported(&rec.env_of("after"), "RUNFILE_T_LATE"), Some("late"));
}

#[test]
fn every_block_form_hands_a_run_inside_it_its_own_environment() {
	// Each form reaches the environment through its own arm of the walker, so
	// each is asked rather than one standing in for the rest.
	let rec = Recorder::default();
	run_src(
		"if true\n\
		 \t.env.RUNFILE_T_IN = \"if\"\n\
		 \n\
		 \trun in-if\n\
		 end\n\
		 \n\
		 if false\n\
		 \trun never\n\
		 else\n\
		 \t.env.RUNFILE_T_IN = \"else\"\n\
		 \n\
		 \trun in-else\n\
		 end\n\
		 \n\
		 for n in [1]\n\
		 \t.env.RUNFILE_T_IN = \"for\"\n\
		 \n\
		 \trun in-for\n\
		 end\n\
		 \n\
		 match \"a\"\n\
		 \tcase \"a\"\n\
		 \t\t.env.RUNFILE_T_IN = \"match\"\n\
		 \n\
		 \t\trun in-match\n\
		 end\n\
		 \n\
		 retry 1\n\
		 \t.env.RUNFILE_T_IN = \"retry\"\n\
		 \n\
		 \trun in-retry\n\
		 end\n\
		 \n\
		 loop\n\
		 \t.env.RUNFILE_T_IN = \"loop\"\n\
		 \n\
		 \trun in-loop\n\
		 \tbreak\n\
		 end\n\
		 \n\
		 do\n\
		 \t.env.RUNFILE_T_IN = \"do\"\n\
		 \n\
		 \trun in-do\n\
		 end\n\
		 \n\
		 run outside\n",
		&rec,
	)
	.unwrap();
	for form in ["if", "else", "for", "match", "retry", "loop", "do"] {
		assert_eq!(
			exported(&rec.env_of(&format!("in-{form}")), "RUNFILE_T_IN"),
			Some(form),
			"{form}"
		);
	}
	assert_eq!(exported(&rec.env_of("outside"), "RUNFILE_T_IN"), None);
}

#[test]
fn code_of_a_run_is_handed_the_same_environment_as_a_run() {
	let rec = Recorder::default();
	run_src(".env.RUNFILE_T_X = \"x\"\n\nlet status = code_of(run scored)\n", &rec).unwrap();
	assert_eq!(exported(&rec.env_of("scored"), "RUNFILE_T_X"), Some("x"));
}

#[test]
fn each_parallel_branch_hands_over_the_environment_it_runs_in() {
	// A property between two branches is the second one's, and an iteration's
	// block is that iteration's -- so every branch's call is handed its own.
	let rec = Recorder::default();
	run_src(
		"parallel do\n\
		 \trun first\n\
		 \t.env.RUNFILE_T_BETWEEN = \"between\"\n\
		 \trun second\n\
		 end\n\
		 \n\
		 parallel for n in [\"a\", \"b\"]\n\
		 \tdo\n\
		 \t\t.env.RUNFILE_T_ITEM = n\n\
		 \n\
		 \t\trun item-{{ n }}\n\
		 \tend\n\
		 end\n",
		&rec,
	)
	.unwrap();
	assert_eq!(exported(&rec.env_of("first"), "RUNFILE_T_BETWEEN"), None);
	assert_eq!(exported(&rec.env_of("second"), "RUNFILE_T_BETWEEN"), Some("between"));
	assert_eq!(exported(&rec.env_of("item-a"), "RUNFILE_T_ITEM"), Some("a"));
	assert_eq!(exported(&rec.env_of("item-b"), "RUNFILE_T_ITEM"), Some("b"));
}

#[test]
fn a_value_only_an_env_file_supplied_is_handed_over_as_a_default() {
	let dir = tempfile::TempDir::new().unwrap();
	std::fs::write(
		dir.path().join(".env"),
		"RUNFILE_T_ONLY_FILE=file\nRUNFILE_T_BOTH=file\n",
	)
	.unwrap();
	let rec = Recorder::default();
	run_src(
		&format!(
			".env-file = \"{}\"\n.env.RUNFILE_T_BOTH = \"assigned\"\n\nrun child\n",
			slashed(&dir.path().join(".env"))
		),
		&rec,
	)
	.unwrap();
	let env = rec.env_of("child");
	assert_eq!(default_of(&env, "RUNFILE_T_ONLY_FILE"), Some("file"));
	assert_eq!(exported(&env, "RUNFILE_T_ONLY_FILE"), None);
	assert_eq!(
		exported(&env, "RUNFILE_T_BOTH"),
		Some("assigned"),
		"an assignment is exported"
	);
	assert_eq!(default_of(&env, "RUNFILE_T_BOTH"), None);
}

#[test]
fn a_variable_the_process_had_is_exported_even_where_a_file_names_it_too() {
	// The file never won it -- what was exported beats a file -- so it is not
	// the file's to hand over as a default.
	let (key, from_process) = plain_process_variable();
	let dir = tempfile::TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), format!("{key}=from_file\n")).unwrap();
	let rec = Recorder::default();
	run_src(
		&format!(".env-file = \"{}\"\n\nrun child\n", slashed(&dir.path().join(".env"))),
		&rec,
	)
	.unwrap();
	let env = rec.env_of("child");
	assert_eq!(exported(&env, &key), Some(from_process.as_str()));
	assert_eq!(default_of(&env, &key), None);
}

#[test]
fn path_is_always_handed_over_as_exported_with_an_add_path_in_front() {
	let dir = tempfile::TempDir::new().unwrap();
	let bin = slashed(&dir.path().join("bin"));
	let rec = Recorder::default();
	run_src(&format!(".add-path = \"{bin}\"\n\nrun child\n"), &rec).unwrap();
	let env = rec.env_of("child");
	let path = env
		.exported
		.iter()
		.find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
		.map(|(_, v)| v.replace('\\', "/"))
		.expect("PATH is exported");
	assert!(path.starts_with(&bin), "{path}");
	assert!(!env.defaults.keys().any(|k| k.eq_ignore_ascii_case("PATH")));
}

#[test]
fn what_a_target_was_run_with_is_handed_on_in_the_same_halves() {
	// A default stays a default however many targets it passes through, unless
	// one of them assigns it -- and a target's own file keeps it one.
	let dir = tempfile::TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "RUNFILE_T_REDONE=own-file\n").unwrap();
	let inherited = Inherited {
		exported: map(&[("RUNFILE_T_EXPORTED", "e"), ("PATH", "/caller/path")]),
		defaults: map(&[
			("RUNFILE_T_DEFAULT", "d"),
			("RUNFILE_T_REDONE", "d"),
			("RUNFILE_T_ASSIGNED", "d"),
		]),
	};
	let rec = Recorder::default();
	walk_run_with(
		&format!(
			".env-file = \"{}\"\n.env.RUNFILE_T_ASSIGNED = \"assigned\"\n\nrun grandchild\n",
			slashed(&dir.path().join(".env"))
		),
		inherited,
		&rec,
	);
	let env = rec.env_of("grandchild");
	assert_eq!(exported(&env, "RUNFILE_T_EXPORTED"), Some("e"));
	assert_eq!(exported(&env, "PATH"), Some("/caller/path"));
	assert_eq!(default_of(&env, "RUNFILE_T_DEFAULT"), Some("d"));
	assert_eq!(default_of(&env, "RUNFILE_T_REDONE"), Some("own-file"));
	assert_eq!(exported(&env, "RUNFILE_T_ASSIGNED"), Some("assigned"));
	assert_eq!(default_of(&env, "RUNFILE_T_ASSIGNED"), None);
}

#[test]
fn a_target_run_with_nothing_set_hands_on_exactly_what_it_was_run_with() {
	let inherited = Inherited {
		exported: map(&[("RUNFILE_T_EXPORTED", "e"), ("PATH", "/caller/path")]),
		defaults: map(&[("RUNFILE_T_DEFAULT", "d")]),
	};
	let rec = Recorder::default();
	walk_run_with("run grandchild\n", inherited.clone(), &rec);
	assert_eq!(rec.env_of("grandchild"), inherited);
}

// ------------------------------------------------------------ what the target sees

/// Write a script at `path` that prints `says`, runnable from `PATH`.
fn tool(d: &tempfile::TempDir, path: &str, says: &str) {
	let full = d.path().join(path);
	std::fs::create_dir_all(full.parent().unwrap()).unwrap();
	std::fs::write(&full, format!("#!/bin/sh\nprintf {says}\n")).unwrap();
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		std::fs::set_permissions(&full, std::fs::Permissions::from_mode(0o755)).unwrap();
	}
}

fn read(d: &tempfile::TempDir, name: &str) -> String {
	std::fs::read_to_string(d.path().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn a_called_target_runs_with_its_callers_env_env_file_and_add_path() {
	// Both readers: the commands it runs, and `ENV.X`.
	let d = project(&[
		(".env", "RUNFILE_T_FILE=from-file\n"),
		(
			"runfiles/caller.run",
			".env-file = \".env\"\n.env.RUNFILE_T_ASSIGNED = \"from-caller\"\n.add-path = \"bin\"\n\nrun _child\n",
		),
		(
			"runfiles/_child.run",
			"$ printf '%s|%s|%s' \"$RUNFILE_T_FILE\" \"$RUNFILE_T_ASSIGNED\" \"$(runfile-t-tool)\" > commands.txt\n\
			 write_file(\"reads.txt\", concat(ENV.RUNFILE_T_FILE, \"|\", ENV.RUNFILE_T_ASSIGNED))\n",
		),
	]);
	tool(&d, "bin/runfile-t-tool", "tool-ran");
	host_run(&d, "caller").unwrap();
	assert_eq!(read(&d, "commands.txt"), "from-file|from-caller|tool-ran");
	assert_eq!(read(&d, "reads.txt"), "from-file|from-caller");
}

#[test]
fn a_called_targets_own_env_file_replaces_what_only_its_callers_file_supplied() {
	// The case the two halves exist for. Handed over whole, as `$ run test`
	// hands it, the `.env` both targets read would beat `test`'s own
	// `.env.test`, and `ci` would test against the development database.
	let d = project(&[
		(".env", "RUNFILE_T_DB=dev\n"),
		(".env.test", "RUNFILE_T_DB=test\n"),
		("runfiles/_shared.run", ".env-file = \".env\"\n"),
		(
			"runfiles/test.run",
			".env-file = \".env.test\"\n\n$ printf '%s' \"$RUNFILE_T_DB\" > db.txt\n",
		),
		("runfiles/ci.run", "run test\n"),
	]);
	host_run(&d, "test").unwrap();
	assert_eq!(read(&d, "db.txt"), "test", "run on its own");
	std::fs::remove_file(d.path().join("db.txt")).unwrap();
	host_run(&d, "ci").unwrap();
	assert_eq!(read(&d, "db.txt"), "test", "run by `ci`");
}

#[test]
fn a_subprojects_own_env_file_replaces_what_only_the_roots_file_supplied() {
	let d = project(&[
		(".env", "RUNFILE_T_DB=root\nRUNFILE_T_ROOT_ONLY=root\n"),
		("web/.env", "RUNFILE_T_DB=web\n"),
		("runfiles/all.run", ".env-file = \".env\"\n\nrun web:test\n"),
		(
			"web/runfiles/test.run",
			".env-file = \".env\"\n\n$ printf '%s|%s' \"$RUNFILE_T_DB\" \"$RUNFILE_T_ROOT_ONLY\" > seen.txt\n",
		),
	]);
	host_run(&d, "all").unwrap();
	assert_eq!(read(&d, "web/seen.txt"), "web|root");
}

#[test]
fn what_a_caller_assigns_beats_the_called_targets_env_file() {
	// An assignment is exported, and what was exported beats a file -- the
	// rule a shell's own variables follow.
	let d = project(&[
		(".env.child", "RUNFILE_T_PORT=from-child-file\n"),
		(
			"runfiles/caller.run",
			".env.RUNFILE_T_PORT = \"from-caller\"\n\nrun _child\n",
		),
		(
			"runfiles/_child.run",
			".env-file = \".env.child\"\n\n$ printf '%s' \"$RUNFILE_T_PORT\" > port.txt\n",
		),
	]);
	host_run(&d, "caller").unwrap();
	assert_eq!(read(&d, "port.txt"), "from-caller");
}

#[test]
fn a_called_targets_own_assignment_beats_everything_it_was_run_with() {
	let d = project(&[
		(".env", "RUNFILE_T_FROM_FILE=caller-file\n"),
		(
			"runfiles/caller.run",
			".env-file = \".env\"\n.env.RUNFILE_T_ASSIGNED = \"caller\"\n\nrun _child\n",
		),
		(
			"runfiles/_child.run",
			".env.RUNFILE_T_FROM_FILE = \"child\"\n.env.RUNFILE_T_ASSIGNED = \"child\"\n\n\
			 $ printf '%s|%s' \"$RUNFILE_T_FROM_FILE\" \"$RUNFILE_T_ASSIGNED\" > seen.txt\n",
		),
	]);
	host_run(&d, "caller").unwrap();
	assert_eq!(read(&d, "seen.txt"), "child|child");
}

#[test]
fn a_called_target_defaults_what_its_caller_did_not_set() {
	// `ENV.PORT ? "3000"` is how a target says the caller may override it, and
	// a target that runs it is a caller like any other.
	let d = project(&[
		(
			"runfiles/_serve.run",
			".env.RUNFILE_T_PORT = ENV.RUNFILE_T_PORT ? \"3000\"\n\n$ printf '%s\\n' \"$RUNFILE_T_PORT\" >> ports.txt\n",
		),
		(
			"runfiles/both.run",
			"run _serve\n\ndo\n\t.env.RUNFILE_T_PORT = \"4000\"\n\n\trun _serve\nend\n",
		),
	]);
	host_run(&d, "both").unwrap();
	assert_eq!(read(&d, "ports.txt"), "3000\n4000\n");
}

#[test]
fn nothing_a_called_target_sets_reaches_its_caller_or_the_next_target_it_runs() {
	let d = project(&[
		(".env.leak", "RUNFILE_T_LEAK_FILE=leaked\n"),
		(
			"runfiles/_sets.run",
			".env.RUNFILE_T_LEAK = \"leaked\"\n.env-file = \".env.leak\"\n.add-path = \"leak-bin\"\n\n$ true\n",
		),
		(
			"runfiles/_reads.run",
			"$ printf '%s|%s' \"${RUNFILE_T_LEAK:-unset}\" \"${RUNFILE_T_LEAK_FILE:-unset}\" > reads.txt\n",
		),
		(
			"runfiles/caller.run",
			"run _sets\nrun _reads\n\n\
			 $ printf '%s|%s' \"${RUNFILE_T_LEAK:-unset}\" \"${RUNFILE_T_LEAK_FILE:-unset}\" > caller.txt\n\
			 write_file(\"path.txt\", ENV.PATH)\n",
		),
	]);
	host_run(&d, "caller").unwrap();
	assert_eq!(read(&d, "reads.txt"), "unset|unset");
	assert_eq!(read(&d, "caller.txt"), "unset|unset");
	assert!(!read(&d, "path.txt").contains("leak-bin"), "{}", read(&d, "path.txt"));
}

#[test]
fn every_level_is_run_with_the_one_above_it_and_layers_its_own() {
	let d = project(&[
		(
			"runfiles/a.run",
			".env.RUNFILE_T_A = \"a\"\n.env.RUNFILE_T_OVER = \"a\"\n\nrun _b\n",
		),
		(
			"runfiles/_b.run",
			".env.RUNFILE_T_B = \"b\"\n.env.RUNFILE_T_OVER = \"b\"\n\nrun _c\n",
		),
		(
			"runfiles/_c.run",
			"$ printf '%s|%s|%s' \"$RUNFILE_T_A\" \"$RUNFILE_T_B\" \"$RUNFILE_T_OVER\" > c.txt\n",
		),
	]);
	host_run(&d, "a").unwrap();
	assert_eq!(read(&d, "c.txt"), "a|b|b");
}

#[test]
fn a_file_value_stays_a_default_however_many_levels_it_travels() {
	let d = project(&[
		(".env.a", "RUNFILE_T_K=a-file\n"),
		(".env.c", "RUNFILE_T_K=c-file\n"),
		("runfiles/a.run", ".env-file = \".env.a\"\n\nrun _b\n"),
		("runfiles/_b.run", "$ printf '%s' \"$RUNFILE_T_K\" > b.txt\n\nrun _c\n"),
		(
			"runfiles/_c.run",
			".env-file = \".env.c\"\n\n$ printf '%s' \"$RUNFILE_T_K\" > c.txt\n",
		),
	]);
	host_run(&d, "a").unwrap();
	assert_eq!(read(&d, "b.txt"), "a-file", "passed through untouched");
	assert_eq!(read(&d, "c.txt"), "c-file", "and still a default two levels down");
}

#[test]
fn a_called_targets_shared_file_and_header_read_what_it_was_run_with() {
	// Both are read before any of the called target's own properties has built
	// anything, which is the environment it was handed and nothing else.
	let d = project(&[
		(".env.eu", "RUNFILE_T_ENDPOINT=eu.example\n"),
		(
			"runfiles/deploy/_shared.run",
			"let region = ENV.RUNFILE_T_REGION\n.env.RUNFILE_T_BUCKET = concat(\"logs-\", region)\n",
		),
		(
			"runfiles/deploy/_push.run",
			".env-file = \".env.{{ ENV.RUNFILE_T_STAGE }}\"\n\
			 .env.RUNFILE_T_URL = concat(\"https://\", ENV.RUNFILE_T_ENDPOINT)\n\n\
			 $ printf '%s|%s' \"$RUNFILE_T_BUCKET\" \"$RUNFILE_T_URL\" > push.txt\n",
		),
		(
			"runfiles/release.run",
			".env.RUNFILE_T_REGION = \"eu\"\n.env.RUNFILE_T_STAGE = \"eu\"\n\nrun deploy:_push\n",
		),
	]);
	host_run(&d, "release").unwrap();
	assert_eq!(read(&d, "push.txt"), "logs-eu|https://eu.example");
}

#[test]
fn a_rebuild_part_way_through_a_called_targets_header_keeps_what_it_was_run_with() {
	// A value below an `.env-file` has the environment rebuilt for it. Rebuilt
	// from the process's environment, the caller's value would be gone by the
	// line that reads it -- in the shared file and in the header alike.
	let d = project(&[
		(".env.shared", "RUNFILE_T_SHARED_FILE=1\n"),
		(".env.own", "RUNFILE_T_OWN_FILE=1\n"),
		(
			"runfiles/child/_shared.run",
			".env-file = \".env.shared\"\nlet copied = ENV.RUNFILE_T_FROM_CALLER ? \"missing\"\n.env.RUNFILE_T_IN_SHARED = copied\n",
		),
		(
			"runfiles/child/_run.run",
			".env-file = \".env.own\"\n.env.RUNFILE_T_IN_HEADER = ENV.RUNFILE_T_FROM_CALLER ? \"missing\"\n\n\
			 $ printf '%s|%s' \"$RUNFILE_T_IN_SHARED\" \"$RUNFILE_T_IN_HEADER\" > copied.txt\n",
		),
		(
			"runfiles/caller.run",
			".env.RUNFILE_T_FROM_CALLER = \"handed-over\"\n\nrun child:_run\n",
		),
	]);
	host_run(&d, "caller").unwrap();
	assert_eq!(read(&d, "copied.txt"), "handed-over|handed-over");
}

#[test]
fn a_called_targets_blocks_and_later_properties_keep_what_it_was_run_with() {
	// A block and a property below a statement each rebuild the environment,
	// and each has to build on what the target was run with.
	let d = project(&[
		(
			"runfiles/_child.run",
			"do\n\t.env.RUNFILE_T_INNER = \"inner\"\n\n\t$ printf '%s|%s' \"$RUNFILE_T_OUTER\" \"$RUNFILE_T_INNER\" > block.txt\nend\n\n\
			 $ true\n.env.RUNFILE_T_LATER = \"later\"\n\n\
			 $ printf '%s|%s' \"$RUNFILE_T_OUTER\" \"$RUNFILE_T_LATER\" > later.txt\n",
		),
		(
			"runfiles/caller.run",
			".env.RUNFILE_T_OUTER = \"outer\"\n\nrun _child\n",
		),
	]);
	host_run(&d, "caller").unwrap();
	assert_eq!(read(&d, "block.txt"), "outer|inner");
	assert_eq!(read(&d, "later.txt"), "outer|later");
}

#[test]
fn a_subproject_finds_its_own_tools_first_and_the_roots_after_them() {
	// A caller's `.add-path` was resolved against the caller's own directory
	// before it was handed over, and a called target's goes in front of it: a
	// subproject's `node_modules/.bin` wins over the root's that called it.
	let d = project(&[
		("runfiles/build.run", ".add-path = \"bin\"\n\nrun web:build\n"),
		(
			"web/runfiles/build.run",
			".add-path = \"bin\"\n\n$ printf '%s|%s' \"$(runfile-t-tool)\" \"$(runfile-t-root-only)\" > tools.txt\n",
		),
	]);
	tool(&d, "bin/runfile-t-tool", "root");
	tool(&d, "bin/runfile-t-root-only", "root-only");
	tool(&d, "web/bin/runfile-t-tool", "web");
	host_run(&d, "build").unwrap();
	assert_eq!(read(&d, "web/tools.txt"), "web|root-only");
}

#[test]
fn a_directory_both_levels_add_is_on_the_called_targets_path_once() {
	// A `_shared.run` puts it on PATH for the caller, and again for the target
	// it runs, which was run with the caller's PATH.
	let d = project(&[
		("runfiles/_shared.run", ".add-path = \"bin\"\n"),
		("runfiles/caller.run", "run _child\n"),
		("runfiles/_child.run", "write_file(\"path.txt\", ENV.PATH)\n"),
	]);
	host_run(&d, "caller").unwrap();
	let path = read(&d, "path.txt");
	let sep = if cfg!(windows) { ';' } else { ':' };
	let entries: Vec<&str> = path.split(sep).collect();
	assert!(entries[0].replace('\\', "/").ends_with("/bin"), "{path}");
	assert_eq!(
		entries.iter().filter(|e| **e == entries[0]).count(),
		1,
		"added twice: {path}"
	);
}

#[test]
fn a_scored_run_runs_the_target_with_the_callers_environment() {
	let d = project(&[
		(
			"runfiles/caller.run",
			".env.RUNFILE_T_X = \"x\"\n\nlet status = code_of(run _child)\n\n$ test {{ status }} = 0\n",
		),
		("runfiles/_child.run", "$ test \"$RUNFILE_T_X\" = x\n"),
	]);
	host_run(&d, "caller").expect("the child saw `x`, so it answered 0");
}

#[test]
fn each_parallel_branch_runs_its_target_with_its_own_environment() {
	let d = project(&[
		(
			"runfiles/all.run",
			"parallel for name in [\"one\", \"two\"]\n\tdo\n\t\t.env.RUNFILE_T_NAME = name\n\n\t\trun _write\n\tend\nend\n",
		),
		(
			"runfiles/_write.run",
			"$ printf '%s' \"$RUNFILE_T_NAME\" > \"out-$RUNFILE_T_NAME.txt\"\n",
		),
	]);
	host_run(&d, "all").unwrap();
	assert_eq!(read(&d, "out-one.txt"), "one");
	assert_eq!(read(&d, "out-two.txt"), "two");
}

#[test]
fn a_preview_shows_a_called_targets_commands_with_its_callers_environment() {
	let d = project(&[
		("runfiles/caller.run", ".env.RUNFILE_T_WHO = \"caller\"\n\nrun _child\n"),
		("runfiles/_child.run", "$ echo {{ ENV.RUNFILE_T_WHO }}\n"),
	]);
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let mut h = crate::dispatch::Host::new(&cat);
	h.assume_yes = true;
	h.dry_run = true;
	h.run("caller", &[]).unwrap();
	let trace = h.trace.lock().expect("trace").clone();
	assert!(trace.iter().any(|t| t.contains("echo caller")), "{trace:?}");
}

#[test]
fn a_target_run_on_its_own_still_reads_only_the_process_environment() {
	// Nothing ran it, so there is nothing to have been handed: a target that
	// reads what its callers set fails when run on its own, as it always did.
	let d = project(&[(
		"runfiles/_child.run",
		"$ printf '%s' \"$RUNFILE_T_NEVER_SET\"\nprint(ENV.RUNFILE_T_NEVER_SET)\n",
	)]);
	let e = host_run(&d, "_child").expect_err("nothing set it");
	assert!(e.to_string().contains("RUNFILE_T_NEVER_SET"), "{e}");
}
