use super::{Recorder, host_run, project, run_src};

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
	run_src("match \"zzz\"\ncase a\n\trun a\ndefault\n\trun fallback\nend\n", &d).unwrap();
	assert_eq!(d.calls(), vec!["fallback"]);
}

#[test]
fn match_without_a_default_lists_the_valid_cases() {
	let d = Recorder::default();
	let e = run_src("match \"zzz\"\ncase a\n\trun a\ncase b\n\trun b\nend\n", &d).unwrap_err();
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
	let e = run_src("for n in [\"a\"]\n\t.confirm = \"really?\"\n\trun {{ n }}\nend\n", &d).unwrap_err();
	assert!(e.to_string().contains("header-only"), "{e}");
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
	// The exported list drives editor completion; a name here that `extend`
	// rejects would be offered and then fail.
	for p in crate::props::PROPERTIES {
		let (name, block_ok) = (p.name, &p.block_scoped);
		// `.env` is the one property addressed by sub-key rather than set whole.
		let lhs = if name == "env" {
			"env.SOME_KEY".to_string()
		} else {
			(*name).to_string()
		};
		let src = format!(".{lhs} = \"x\"\n$ true\n");
		let ast = runfile_lang::parse(&src).unwrap_or_else(|e| panic!(".{name}: {e}"));
		let mut sc = runfile_lang::Scope::new();
		if let Err(crate::props::PropError::Unknown { .. }) =
			crate::props::Props::default().extend(&ast.body, &mut sc, false)
		{
			panic!("`.{name}` is exported for completion but not known");
		}
		// And the second column must match where it is actually allowed.
		let nested = crate::props::Props::default().extend(&ast.body, &mut sc, true);
		let allowed = !matches!(nested, Err(crate::props::PropError::NotBlockScoped { .. }));
		assert_eq!(allowed, *block_ok, "`.{name}` block-scoping is mislabelled");
	}
}
