use super::{Recorder, host_run, project, run_src};
use crate::RunError;

#[test]
fn run_dispatches_in_process_with_interpolated_target() {
	let d = Recorder::default();
	run_src("let ns = \"web\"\nrun {{ ns }}:build --release\n", &d).unwrap();
	assert_eq!(d.calls(), vec!["web:build --release"]);
}

#[test]
fn for_iterates_a_list_and_scopes_its_variable() {
	let d = Recorder::default();
	run_src("for n in [\"a\", \"b\"]\n\trun {{ n }}:build\nend\n", &d).unwrap();
	assert_eq!(d.calls(), vec!["a:build", "b:build"]);
}

#[test]
fn a_loop_variable_is_restored_afterwards() {
	let d = Recorder::default();
	let e = run_src("for n in [\"a\"]\n\trun {{ n }}\nend\nrun {{ n }}\n", &d).unwrap_err();
	assert!(e.to_string().contains("not defined"), "{e}");
}

#[test]
fn for_requires_a_list() {
	let d = Recorder::default();
	let e = run_src("for n in \"abc\"\n\trun {{ n }}\nend\n", &d).unwrap_err();
	assert!(e.to_string().contains("needs a list"), "{e}");
}

#[test]
fn if_takes_one_branch() {
	let d = Recorder::default();
	run_src("if RUN.os == \"linux\"\n\trun linux\nelse\n\trun other\nend\n", &d).unwrap();
	assert_eq!(d.calls(), vec!["linux"]);
}

#[test]
fn match_falls_through_to_default() {
	let d = Recorder::default();
	run_src("match \"zzz\"\ncase \"a\"\n\trun a\ndefault\n\trun fallback\nend\n", &d).unwrap();
	assert_eq!(d.calls(), vec!["fallback"]);
}

#[test]
fn match_without_a_default_lists_the_valid_cases() {
	let d = Recorder::default();
	let e = run_src("match \"zzz\"\ncase \"a\"\n\trun a\ncase \"b\"\n\trun b\nend\n", &d).unwrap_err();
	assert!(e.to_string().contains("a, b"), "{e}");
}

#[test]
fn block_properties_are_scoped_to_their_block() {
	// .ignore-errors inside the loop must not leak out to the statement after it.
	let d = Recorder::default();
	let e = run_src("for n in [\"a\"]\n\t.ignore-errors\n\t$ exit 1\nend\n$ exit 4\n", &d).unwrap_err();
	assert!(
		e.to_string().contains("status 4"),
		"outer failure still propagates: {e}"
	);
}

#[test]
fn a_header_only_property_is_rejected_inside_a_block() {
	let d = Recorder::default();
	let e = run_src("for n in [\"a\"]\n\t.watch = \"src/**\"\n\trun {{ n }}\nend\n", &d).unwrap_err();
	assert!(e.to_string().contains("header-only"), "{e}");
	// And a name that is not a property at all says so, rather than being
	// reported as a scope rule it was never subject to.
	let e = run_src("for n in [\"a\"]\n\t.ignore-error\n\trun {{ n }}\nend\n", &d).unwrap_err();
	assert!(e.to_string().contains("unknown property"), "{e}");
}

#[test]
fn an_unknown_property_names_itself() {
	let d = Recorder::default();
	let e = run_src(".nonsense = 1\n$ true\n", &d).unwrap_err();
	assert!(e.to_string().contains("nonsense"), "{e}");
}

// ---- host dispatch, cycle detection and _shared.run

#[test]
fn a_cycle_is_reported_with_the_chain_that_caused_it() {
	let d = project(&[("runfiles/a.run", "run b\n"), ("runfiles/b.run", "run a\n")]);
	let e = host_run(&d, "a").unwrap_err();
	assert!(e.to_string().contains("a -> b -> a"), "{e}");
}

#[test]
fn shared_bindings_and_properties_reach_every_target() {
	let d = project(&[
		("runfiles/_shared.run", ".env.SHARED = \"yes\"\nlet who = \"world\"\n"),
		(
			"runfiles/greet.run",
			"$ test \"$SHARED\" = yes\n$ test {{ who }} = world\n",
		),
	]);
	host_run(&d, "greet").expect("_shared.run is the globals analog");
}

#[test]
fn an_unknown_target_suggests_near_matches() {
	let d = project(&[("runfiles/build.run", "$ true\n")]);
	let e = host_run(&d, "buil").unwrap_err();
	assert!(e.to_string().contains("build"), "{e}");
}

#[test]
fn run_context_is_populated_from_the_environment() {
	let d = project(&[(
		"runfiles/ctx.run",
		"$ test {{ RUN.os }} != \"\"\n$ test {{ RUN.parent }} != \"\"\n",
	)]);
	host_run(&d, "ctx").expect("RUN.* resolves");
}

#[test]
fn namespaces_come_from_discovered_subprojects() {
	let d = project(&[
		(
			"runfiles/all.run",
			"for n in RUN.namespaces\n\trun {{ n }}:build\nend\n",
		),
		("api/runfiles/build.run", "$ true\n"),
		("web/runfiles/build.run", "$ true\n"),
	]);
	let trace = host_run(&d, "all").expect("fans out over discovered namespaces");
	assert_eq!(trace.len(), 2, "one per subproject, no declaration anywhere");
}

#[test]
fn every_exported_property_name_is_actually_known() {
	// The exported list drives editor completion and every static check; a name
	// here that `extend` rejects would be offered and then fail, and a column
	// that does not match what `extend` does is an editor disagreeing with the
	// runner. All three are asserted against the behaviour, not restated.
	// Machine-wide, so `.only-in-directories` is admissible here: it is the one
	// property whose legality depends on where the file was found rather than
	// on anything in the line.
	fn base() -> crate::props::Props {
		crate::props::Props {
			machine_wide: true,
			..Default::default()
		}
	}
	for p in crate::props::PROPERTIES {
		let name = p.name;
		// `.env` is the one property addressed by sub-key rather than set whole.
		let lhs = if name == "env" {
			"env.SOME_KEY".to_string()
		} else {
			(*name).to_string()
		};
		// A flag will not take a string, which is the point of the column.
		let rhs = if p.flag {
			"true".to_string()
		} else {
			"\"x\"".to_string()
		};
		let src = format!(".{lhs} = {rhs}\n$ true\n");
		let ast = runfile_lang::parse(&src).unwrap_or_else(|e| panic!(".{name}: {e}"));
		let mut sc = runfile_lang::Scope::new();
		match base().extend(&ast.body, &mut sc, false) {
			Err(crate::props::PropError::Unknown { .. }) => {
				panic!("`.{name}` is exported for completion but not known")
			}
			Err(e) => panic!("`.{name}` is exported but its own example shape fails: {e}"),
			Ok(_) => {}
		}
		// Column two: where a block allows it at all.
		let nested = base().extend(&ast.body, &mut sc, true);
		let allowed = !matches!(nested, Err(crate::props::PropError::NotBlockScoped { .. }));
		assert_eq!(allowed, p.block_scoped, "`.{name}` block-scoping is mislabelled");

		// Column three: whereabouts in one. Written below a statement rather
		// than above it.
		let below = runfile_lang::parse(&format!("$ true\n.{lhs} = {rhs}\n")).expect("parses");
		let trailing = base().extend(&below.body, &mut sc, false);
		let refused = matches!(trailing, Err(crate::props::PropError::NotInDeclaration { .. }));
		assert_eq!(refused, p.declaration_only, "`.{name}` declaration-only is mislabelled");

		// Column four: whether a constant that is not a bool is refused.
		let with_number = runfile_lang::parse(&format!(".{lhs} = 23\n$ true\n")).expect("parses");
		let numbered = base().extend(&with_number.body, &mut sc, false);
		let refused = matches!(numbered, Err(crate::props::PropError::NotABool { .. }));
		assert_eq!(refused, p.flag, "`.{name}` flag is mislabelled");
	}
}

// ---- `code_of(run …)`: a dispatch, scored

/// Dispatches nothing and fails the way it was told to.
struct Fails(fn() -> RunError);
impl crate::run::Dispatch for Fails {
	fn run(&self, _t: &str, _a: &[String], _c: &[String], _l: Option<&str>) -> Result<Vec<String>, RunError> {
		Err((self.0)())
	}
}

#[test]
fn a_scored_dispatch_answers_with_the_status_the_target_would_have_exited_with() {
	// The same numbers `$ run <target>` yields, because dispatching in-process
	// is meant to stop re-execing the binary, not to mean something else.
	let d = project(&[
		("runfiles/ok.run", "$ true\n"),
		("runfiles/broken.run", "$ exit 7\n"),
		("runfiles/quits.run", "exit(3)\n"),
		(
			"runfiles/score.run",
			"let a = code_of(run ok)\nlet b = code_of(run broken)\nlet c = code_of(run quits)\n\
			 $ test {{ a }}{{ b }}{{ c }} = 013\n",
		),
	]);
	// A failing command is the 1 the CLI reports -- a target is not its last
	// command, so there is no other status it could honestly carry -- and
	// `exit(3)` is the 3 it asked for. Neither stops the caller: the `$ test`
	// below them runs, and it is what proves the numbers.
	host_run(&d, "score").expect("a status is an answer, not a failure");
}

#[test]
fn a_refusal_inside_a_scored_dispatch_still_stops_the_caller() {
	// Ctrl+C and someone answering no to `confirm()` are a person stopping the
	// run, not a target reporting how it went. Scoring those 1 and carrying on
	// would be doing the very thing that was declined.
	for make in [
		(|| RunError::Cancelled) as fn() -> RunError,
		|| RunError::Interrupted,
		|| RunError::Eval(runfile_lang::eval::EvalError::Cancelled { line: 1 }),
	] {
		let e = run_src("let c = code_of(run deploy)\n$ true\n", &Fails(make)).unwrap_err();
		assert!(e.is_refusal(), "{e}");
	}
	// Everything else is a number, so the run goes on.
	let d = Fails(|| RunError::ForNeedsList {
		actual: "string",
		line: 1,
	});
	run_src("let c = code_of(run deploy)\n$ test {{ c }} = 1\n", &d).expect("a failure is scored 1");
}

#[test]
fn a_scored_dispatch_passes_its_arguments_and_splices_its_trace() {
	let d = Recorder::default();
	run_src("let n = \"web\"\nlet c = code_of(run build:{{ n }} --env=prod)\n", &d).expect("dispatches");
	assert_eq!(d.calls(), ["build:web --env=prod"]);

	// Under `--dry-run` the child's trace belongs where the call appeared, the
	// same as a `run` statement's.
	let p = project(&[
		("runfiles/dep.run", "$ echo hi\n"),
		("runfiles/top.run", "let c = code_of(run dep)\n$ echo bye\n"),
	]);
	let cat = runfile_discovery::discover(p.path(), None).unwrap();
	let mut h = crate::dispatch::Host::new(&cat);
	h.dry_run = true;
	h.run("top", &[]).expect("previews");
	let trace = h.trace.lock().expect("trace").join("\n");
	assert!(
		trace.find("echo hi") < trace.find("echo bye"),
		"the dependency's trace comes first: {trace}"
	);
}

// ---- `.only-in-directories`, which only the machine-wide directory may set

#[test]
fn only_in_directories_is_refused_in_a_project_file() {
	// It was stored in `Props` and read by nothing, so a project file naming
	// directories was accepted, offered by completion, and did nothing at all.
	// A scope that silently did not apply is a target offered where it was
	// meant to be hidden, found out by somebody else.
	let d = project(&[("runfiles/deploy.run", ".only-in-directories = \"sub\"\n$ true\n")]);
	let e = host_run(&d, "deploy").expect_err("a project target has no such question to answer");
	assert!(e.to_string().contains("machine-wide"), "{e}");
}

#[test]
fn only_in_directories_is_accepted_in_a_machine_wide_file() {
	// Discovery has already acted on it by the time the target runs, so the
	// runtime's only job is not to object.
	let home = tempfile::TempDir::new().unwrap();
	std::fs::create_dir_all(home.path().join(".runfiles")).unwrap();
	std::fs::create_dir_all(home.path().join("work")).unwrap();
	std::fs::write(
		home.path().join(".runfiles/deploy.run"),
		".only-in-directories = \"work\"\n$ true\n",
	)
	.unwrap();

	super::with_home(home.path(), || {
		let cat = runfile_discovery::discover(&home.path().join("work"), Some(home.path())).unwrap();
		let mut h = crate::dispatch::Host::new(&cat);
		h.assume_yes = true;
		h.run("deploy", &[])
			.expect("the one place the property means something");
	});
}

#[test]
fn a_machine_wide_file_may_scope_itself_even_when_it_is_the_local_directory() {
	// The runtime asked `Origin`, which answers how discovery *reached* the
	// file, not where it lives. `$HOME/runfiles` is collected as `Local` when
	// the run started at or below the home directory, so every target in the
	// machine-wide directory refused its own scope with "this target is part of
	// the project". It asks the path now -- the same question the language
	// server asks, so the two cannot answer differently.
	let home = tempfile::TempDir::new().unwrap();
	let g = home.path().join("runfiles");
	std::fs::create_dir_all(&g).unwrap();
	std::fs::write(g.join("deploy.run"), ".only-in-directories = \".\"\n$ true\n").unwrap();

	super::with_home(home.path(), || {
		let cat = runfile_discovery::discover(home.path(), Some(home.path())).unwrap();
		let mut h = crate::dispatch::Host::new(&cat);
		h.assume_yes = true;
		h.run("deploy", &[])
			.expect("the machine-wide directory may scope itself wherever it was reached from");
	});
}

// ---- flags

/// `.ignore-errors = <rhs>`, applied against a scope holding `ENV.F`.
fn flag_with(rhs: &str, f: Option<&str>) -> Result<crate::props::Props, crate::props::PropError> {
	let ast = runfile_lang::parse(&format!(".ignore-errors = {rhs}\n$ true\n")).expect("parses");
	let mut sc = runfile_lang::Scope::new();
	if let Some(v) = f {
		sc.env.insert("F".into(), v.into());
	}
	crate::props::Props::default().extend(&ast.body, &mut sc, false)
}

#[test]
fn a_flag_written_with_a_constant_that_is_not_a_bool_is_refused() {
	// It used to be read as `false` and say nothing: `matches!(v, Bool(true))`
	// answered every other value the same way, so a flag that did not take
	// effect was found out later, if at all. A constant has one reading and
	// this is it.
	for rhs in ["23", "\"abc\"", "[]", "\"\""] {
		let e = flag_with(rhs, None).expect_err(rhs);
		assert!(matches!(e, crate::props::PropError::NotABool { .. }), "{rhs}: {e}");
	}
	// The near miss is worth naming: the quotes are the whole mistake.
	let e = flag_with("\"true\"", None).unwrap_err();
	assert!(e.to_string().contains("write `true` without the quotes"), "{e}");
	let e = flag_with("\"0\"", None).unwrap_err();
	assert!(e.to_string().contains("write `false`"), "{e}");
	// And a bool is what it takes.
	assert!(flag_with("true", None).expect("a bool literal").ignore_errors);
	assert!(!flag_with("false", None).expect("a bool literal").ignore_errors);
}

#[test]
fn a_flag_worked_out_during_the_run_is_left_to_the_run() {
	// The rule is about constants, not about types: `ENV.F` is a string on
	// every platform there is, and refusing it statically would leave a flag
	// with no way to be decided by anything outside the file.
	for (v, want) in [("true", true), ("1", true), ("TRUE", true), (" true ", true)] {
		let p = flag_with("ENV.F", Some(v)).unwrap_or_else(|e| panic!("F={v}: {e}"));
		assert_eq!(p.ignore_errors, want, "F={v}");
	}
	for v in ["false", "0"] {
		assert!(!flag_with("ENV.F", Some(v)).unwrap().ignore_errors, "F={v}");
	}
	// An interpolated string is not a constant either, so it takes the same path.
	assert!(flag_with("\"{{ ENV.F }}\"", Some("true")).unwrap().ignore_errors);
	// And a `?` chain ending in a literal is how a default is written.
	assert!(!flag_with("ARG.f ? ENV.F ? \"false\"", None).unwrap().ignore_errors);
}

#[test]
fn a_flag_that_resolves_to_neither_says_so_rather_than_reading_as_false() {
	// The dynamic half of the same rule. Silently off is the failure worth
	// pinning: nothing about the run says the line did not take effect.
	for v in ["yes", "", "on", "2"] {
		let e = flag_with("ENV.F", Some(v)).expect_err(v);
		assert!(matches!(e, crate::props::PropError::FlagValue { .. }), "F={v}: {e}");
	}
}
