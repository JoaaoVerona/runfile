use super::*;

// ── JSON5 UTF-8 and edge case tests ─────────────────────────────────

#[test]
fn json5_utf8_values() {
	let input = r#"{"msg": "日本語 🎉 café",}"#;
	let val: serde_json::Value = from_json_str(input).unwrap();
	assert_eq!(val["msg"], "日本語 🎉 café");
}

#[test]
fn json5_utf8_in_arrays() {
	let input = r#"{"a": ["α", "β", "γ",], "b": [1, 2,],}"#;
	let val: serde_json::Value = from_json_str(input).unwrap();
	assert_eq!(val["a"][0], "α");
	assert_eq!(val["a"][2], "γ");
}

// ── Fix #9: file size limit ───────────────────────────────────────

#[test]
fn parse_rejects_oversized_file() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("huge.json");
	// Create a file slightly over the limit
	let size = (crate::MAX_RUNFILE_SIZE + 1) as usize;
	let content = " ".repeat(size);
	std::fs::write(&path, content).unwrap();

	let result = parse_runfile_from_path(&path);
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(err.contains("too large"), "error should mention size: {err}");
}

#[test]
fn parse_rejects_oversized_file_partial() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("huge.json");
	let size = (crate::MAX_RUNFILE_SIZE + 1) as usize;
	std::fs::write(&path, " ".repeat(size)).unwrap();

	let result = parse_runfile_from_path_partial(&path);
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(err.contains("too large"), "error should mention size: {err}");
}

#[test]
fn parse_accepts_file_at_size_limit() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("maxsize.json");
	// A valid but padded Runfile at exactly the limit
	let json = r#"{"$schema":"v0","targets":{"a":{"commands":["echo"]}}}"#;
	let padding = " ".repeat(crate::MAX_RUNFILE_SIZE as usize - json.len());
	let content = format!("{json}{padding}");
	assert!(content.len() as u64 <= crate::MAX_RUNFILE_SIZE);
	std::fs::write(&path, content).unwrap();

	// Should not fail with FileTooLarge (may fail with JSON parse error from padding, that's ok)
	let result = parse_runfile_from_path(&path);
	match result {
		Ok(_) => {} // valid
		Err(ParseError::FileTooLarge(..)) => panic!("file at limit should not be rejected"),
		Err(_) => {} // other parse errors are fine
	}
}

#[test]
fn parse_accepts_small_file() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("small.json");
	let json = r#"{"$schema":"v0","targets":{"a":{"commands":["echo hi"]}}}"#;
	std::fs::write(&path, json).unwrap();
	let result = parse_runfile_from_path(&path);
	assert!(result.is_ok());
}

#[test]
fn file_size_error_includes_actual_size() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("big.json");
	let size = crate::MAX_RUNFILE_SIZE + 42;
	std::fs::write(&path, " ".repeat(size as usize)).unwrap();

	let result = parse_runfile_from_path(&path);
	let err = result.unwrap_err().to_string();
	assert!(
		err.contains(&size.to_string()),
		"error should include actual size: {err}"
	);
}

#[test]
fn parse_nonexistent_file_gives_io_error_not_size_error() {
	let result = parse_runfile_from_path(std::path::Path::new("/nonexistent/path/Runfile.json"));
	assert!(result.is_err());
	match result.unwrap_err() {
		ParseError::Io(_) => {} // expected
		other => panic!("expected Io error, got: {other}"),
	}
}

// ── Include path traversal tests ──────────────────────────────────

#[test]
fn include_within_project_succeeds() {
	let dir = TempDir::new().unwrap();

	// Create sub/included.json
	let sub = dir.path().join("sub");
	std::fs::create_dir(&sub).unwrap();
	std::fs::write(
		sub.join("included.json"),
		r#"{ "$schema": "x", "targets": { "included": { "commands": ["echo included"] } } }"#,
	)
	.unwrap();

	// Create root Runfile that includes sub/included.json
	let root_path = dir.path().join(RUNFILE_NAME);
	std::fs::write(
		&root_path,
		r#"{ "$schema": "x", "includes": ["sub/included.json"], "targets": { "root": { "commands": ["echo root"] } } }"#,
	)
	.unwrap();

	let runfile = parse_runfile_from_path(&root_path).unwrap();
	let mut state = crate::merge::MergeState::new();
	let canonical = std::fs::canonicalize(&root_path).unwrap();
	let mut visited = std::collections::HashSet::new();
	visited.insert(canonical.clone());

	let result = crate::merge::resolve_includes(
		&runfile,
		&canonical,
		&mut state,
		&mut visited,
		crate::SourceKind::Included,
	);
	assert!(result.is_ok(), "include within project should succeed");
	assert!(state.targets.contains_key("included"));
}

#[test]
fn include_relative_path_outside_project_allowed() {
	// Sibling-directory includes (`../shared/...`) are a legitimate layout for
	// sharing one Runfile across sibling projects. There is no containment
	// check: a Runfile already runs arbitrary shell commands, so restricting
	// which files it may *read* bought no real security.
	let dir = TempDir::new().unwrap();

	let shared = dir.path().join("shared");
	std::fs::create_dir(&shared).unwrap();
	std::fs::write(
		shared.join("common.json"),
		r#"{ "$schema": "x", "targets": { "common": { "commands": ["echo common"] } } }"#,
	)
	.unwrap();

	let project = dir.path().join("project");
	std::fs::create_dir(&project).unwrap();
	let root_path = project.join(RUNFILE_NAME);
	std::fs::write(
		&root_path,
		r#"{ "$schema": "x", "includes": ["../shared/common.json"], "targets": { "safe": { "commands": ["echo safe"] } } }"#,
	)
	.unwrap();

	let runfile = parse_runfile_from_path(&root_path).unwrap();
	let mut state = crate::merge::MergeState::new();
	let canonical = std::fs::canonicalize(&root_path).unwrap();
	let mut visited = std::collections::HashSet::new();
	visited.insert(canonical.clone());

	let result = crate::merge::resolve_includes(
		&runfile,
		&canonical,
		&mut state,
		&mut visited,
		crate::SourceKind::Included,
	);
	assert!(
		result.is_ok(),
		"sibling-directory include should be allowed: {result:?}"
	);
	assert!(state.targets.contains_key("common"));
}

#[test]
fn include_absolute_path_outside_project_allowed() {
	let dir = TempDir::new().unwrap();

	let outside = dir.path().join("outside");
	std::fs::create_dir(&outside).unwrap();
	let outside_file = outside.join("external.json");
	std::fs::write(
		&outside_file,
		r#"{ "$schema": "x", "targets": { "ext": { "commands": ["echo ext"] } } }"#,
	)
	.unwrap();

	let project = dir.path().join("project2");
	std::fs::create_dir(&project).unwrap();
	let root_path = project.join(RUNFILE_NAME);
	let include_path = outside_file.to_string_lossy().replace('\\', "/");
	std::fs::write(
		&root_path,
		format!(
			r#"{{ "$schema": "x", "includes": ["{include_path}"], "targets": {{ "safe": {{ "commands": ["echo safe"] }} }} }}"#,
		),
	)
	.unwrap();

	let runfile = parse_runfile_from_path(&root_path).unwrap();
	let mut state = crate::merge::MergeState::new();
	let canonical = std::fs::canonicalize(&root_path).unwrap();
	let mut visited = std::collections::HashSet::new();
	visited.insert(canonical.clone());

	let result = crate::merge::resolve_includes(
		&runfile,
		&canonical,
		&mut state,
		&mut visited,
		crate::SourceKind::Included,
	);
	assert!(result.is_ok(), "absolute path outside project should be allowed");
	assert!(state.targets.contains_key("ext"));
}

// ── Include namespacing ───────────────────────────────────────────

/// Set up a temp directory with a root Runfile that includes a child file,
/// run `resolve_includes`, and return the resulting `MergeState` so the test
/// can inspect renamed targets and rewritten `@target` references.
fn run_namespace_include(root_json: &str, files: &[(&str, &str)]) -> crate::merge::MergeState {
	let dir = TempDir::new().unwrap();
	for (rel, body) in files {
		let path = dir.path().join(rel);
		if let Some(parent) = path.parent() {
			std::fs::create_dir_all(parent).unwrap();
		}
		std::fs::write(path, body).unwrap();
	}
	let root_path = dir.path().join(RUNFILE_NAME);
	std::fs::write(&root_path, root_json).unwrap();
	let runfile = parse_runfile_from_path(&root_path).unwrap();
	let mut state = crate::merge::MergeState::new();
	let canonical = std::fs::canonicalize(&root_path).unwrap();
	let mut visited = std::collections::HashSet::new();
	visited.insert(canonical.clone());
	crate::merge::resolve_includes(
		&runfile,
		&canonical,
		&mut state,
		&mut visited,
		crate::SourceKind::Included,
	)
	.unwrap();
	state
}

#[test]
fn parse_include_string_form() {
	let json = r#"{
        "$schema": "x",
        "includes": ["a.json", "b.json"],
        "targets": { "root": { "commands": ["echo root"] } }
    }"#;
	let rf = parse_runfile(json).unwrap();
	let inc = rf.includes.unwrap();
	assert_eq!(inc.len(), 2);
	assert_eq!(inc[0].path, "a.json");
	assert!(inc[0].namespace.is_none());
	assert_eq!(inc[1].path, "b.json");
	assert!(inc[1].namespace.is_none());
}

#[test]
fn parse_include_object_form_with_namespace() {
	let json = r#"{
        "$schema": "x",
        "includes": [
            { "path": "a.json", "namespace": "child" },
            { "path": "b.json" }
        ],
        "targets": { "root": { "commands": ["echo root"] } }
    }"#;
	let rf = parse_runfile(json).unwrap();
	let inc = rf.includes.unwrap();
	assert_eq!(inc[0].path, "a.json");
	assert_eq!(inc[0].namespace.as_deref(), Some("child"));
	assert_eq!(inc[1].path, "b.json");
	assert!(inc[1].namespace.is_none(), "missing namespace = no prefix");
}

#[test]
fn parse_include_blank_namespace_treated_as_none() {
	let json = r#"{
        "$schema": "x",
        "includes": [{ "path": "a.json", "namespace": "" }],
        "targets": { "root": { "commands": ["echo root"] } }
    }"#;
	let rf = parse_runfile(json).unwrap();
	let inc = rf.includes.unwrap();
	assert!(
		inc[0].namespace.is_none(),
		"empty-string namespace must normalise to None"
	);
}

#[test]
fn include_namespace_prefixes_target_names_and_aliases() {
	let state = run_namespace_include(
		r#"{
            "$schema": "x",
            "includes": [{ "path": "child.json", "namespace": "child" }],
            "targets": { "root": { "commands": ["echo root"] } }
        }"#,
		&[(
			"child.json",
			r#"{ "$schema": "x", "targets": {
                "build": { "commands": ["echo build"], "aliases": ["b"] },
                "lint": { "commands": ["echo lint"] }
            } }"#,
		)],
	);

	assert!(state.targets.contains_key("child:build"));
	assert!(state.targets.contains_key("child:lint"));
	assert!(
		!state.targets.contains_key("build"),
		"child's `build` must be namespaced"
	);
	assert!(!state.targets.contains_key("lint"), "child's `lint` must be namespaced");

	let aliases = state.targets["child:build"].aliases.as_ref().unwrap();
	assert_eq!(aliases, &vec!["child:b".to_string()], "aliases get the same prefix");
}

#[test]
fn include_namespace_rewrites_target_calls_inside_child() {
	let state = run_namespace_include(
		r#"{
            "$schema": "x",
            "includes": [{ "path": "child.json", "namespace": "child" }],
            "targets": { "root": { "commands": ["echo root"] } }
        }"#,
		&[(
			"child.json",
			// `lint` calls `@build` — must resolve to the child's build, never the parent's.
			r#"{ "$schema": "x", "targets": {
                "build": { "commands": ["echo build"] },
                "lint":  { "commands": ["@build"] }
            } }"#,
		)],
	);

	let lint_steps = &state.targets["child:lint"].commands;
	match &lint_steps[0] {
		CommandStep::TargetCall(call) => {
			assert_eq!(
				call.target, "child:build",
				"@build inside child must be rewritten to @child:build"
			);
		}
		other => panic!("expected TargetCall after namespacing, got {other:?}"),
	}
}

#[test]
fn parent_targets_keep_unprefixed_target_calls() {
	let state = run_namespace_include(
		r#"{
            "$schema": "x",
            "includes": [{ "path": "child.json", "namespace": "child" }],
            "targets": {
                "build": { "commands": ["echo parent-build"] },
                "all":   { "commands": ["@build", "@child:build"] }
            }
        }"#,
		&[(
			"child.json",
			r#"{ "$schema": "x", "targets": { "build": { "commands": ["echo child-build"] } } }"#,
		)],
	);

	// Parent's targets are inserted by the caller (merge_runfiles_inner) — at
	// this stage `state` holds only included targets. Sanity-check the child
	// got namespaced, so the parent's literal `@child:build` will resolve at
	// runtime against the merged map.
	assert!(state.targets.contains_key("child:build"));
	assert!(!state.targets.contains_key("build"));
}

#[test]
fn nested_includes_compose_namespaces() {
	let state = run_namespace_include(
		r#"{
            "$schema": "x",
            "includes": [{ "path": "mid.json", "namespace": "outer" }],
            "targets": { "root": { "commands": ["echo root"] } }
        }"#,
		&[
			(
				"mid.json",
				// `mid` includes `inner.json` as `inner` and has its own `@build`.
				r#"{ "$schema": "x",
                     "includes": [{ "path": "inner.json", "namespace": "inner" }],
                     "targets": {
                         "build": { "commands": ["@inner:build"] }
                     } }"#,
			),
			(
				"inner.json",
				r#"{ "$schema": "x", "targets": {
                    "build": { "commands": ["echo inner-build"] }
                } }"#,
			),
		],
	);

	// Both layers fold under `outer:`.
	assert!(state.targets.contains_key("outer:build"));
	assert!(state.targets.contains_key("outer:inner:build"));

	// `mid`'s `@inner:build` reference must compose to `@outer:inner:build`.
	let outer_build = &state.targets["outer:build"].commands;
	match &outer_build[0] {
		CommandStep::TargetCall(call) => {
			assert_eq!(call.target, "outer:inner:build");
		}
		other => panic!("expected nested TargetCall, got {other:?}"),
	}
}

#[test]
fn include_without_namespace_keeps_original_names() {
	let state = run_namespace_include(
		r#"{
            "$schema": "x",
            "includes": ["child.json"],
            "targets": { "root": { "commands": ["echo root"] } }
        }"#,
		&[(
			"child.json",
			r#"{ "$schema": "x", "targets": {
                "child_build": { "commands": ["echo child"] }
            } }"#,
		)],
	);
	assert!(state.targets.contains_key("child_build"));
}

#[test]
fn include_object_form_without_namespace_keeps_original_names() {
	let state = run_namespace_include(
		r#"{
            "$schema": "x",
            "includes": [{ "path": "child.json" }],
            "targets": { "root": { "commands": ["echo root"] } }
        }"#,
		&[(
			"child.json",
			r#"{ "$schema": "x", "targets": {
                "child_build": { "commands": ["echo child"] }
            } }"#,
		)],
	);
	assert!(state.targets.contains_key("child_build"));
}

#[test]
fn include_namespace_preserves_internal_targets() {
	let state = run_namespace_include(
		r#"{
            "$schema": "x",
            "includes": [{ "path": "child.json", "namespace": "child" }],
            "targets": { "root": { "commands": ["echo root"] } }
        }"#,
		&[(
			"child.json",
			r#"{ "$schema": "x", "targets": {
                "_helper": { "commands": ["echo helper"] }
            } }"#,
		)],
	);

	assert!(state.targets.contains_key("child:_helper"));
	// Internal-ness rides along with the canonical name through namespacing.
	assert!(
		is_internal_target_name("child:_helper"),
		"namespaced internal targets must still report internal"
	);
	assert!(!is_internal_target_name("child:build"));
	assert!(is_internal_target_name("_helper"));
}

// ── Namespace tracking for `for in: "namespaces"` ──────────────────

/// Write `files` into a temp dir alongside a root Runfile, run the full
/// `merge_runfiles` pipeline, and hand back the result. Exercises the real
/// public path (which is what records the *root* file's own namespaces —
/// `resolve_includes` alone only sees the included files').
fn merge_namespace_fixture(root_json: &str, files: &[(&str, &str)]) -> (crate::MergeResult, std::path::PathBuf) {
	let dir = TempDir::new().unwrap();
	for (rel, body) in files {
		let path = dir.path().join(rel);
		if let Some(parent) = path.parent() {
			std::fs::create_dir_all(parent).unwrap();
		}
		std::fs::write(path, body).unwrap();
	}
	let root_path = dir.path().join(RUNFILE_NAME);
	std::fs::write(&root_path, root_json).unwrap();
	let local = parse_runfile(root_json).unwrap();
	let result = crate::merge_runfiles(Some((local, root_path.clone())), &[], dir.path()).unwrap();
	// Keep the TempDir alive for the caller's assertions by leaking it — these
	// are short-lived unit tests and the OS reclaims the dir.
	std::mem::forget(dir);
	(result, root_path)
}

#[test]
fn merge_records_top_level_namespace() {
	// A single namespaced include is recorded against the file that DECLARED
	// it (the root), which is what `for "in": "namespaces"` reads for targets
	// defined in that root.
	let (result, root_path) = merge_namespace_fixture(
		r#"{
			"$schema": "x",
			"includes": [{ "path": "child.json", "namespace": "child" }],
			"targets": { "root": { "commands": ["echo root"] } }
		}"#,
		&[(
			"child.json",
			r#"{ "$schema": "x", "targets": { "build": { "commands": ["echo build"] } } }"#,
		)],
	);
	assert_eq!(
		result.namespaces_by_source.get(&root_path),
		Some(&vec!["child".to_string()])
	);
	assert_eq!(result.namespaces_for_target("root"), ["child".to_string()]);
	// The included file declares no includes of its own, so its targets see none.
	assert!(result.namespaces_for_target("child:build").is_empty());
}

#[test]
fn merge_records_no_namespaces_for_unnamespaced_includes() {
	// String-form (no namespace) and object-form-without-namespace contribute
	// nothing to any namespace list.
	let (result, _) = merge_namespace_fixture(
		r#"{
			"$schema": "x",
			"includes": ["plain.json", { "path": "obj.json" }],
			"targets": { "root": { "commands": ["echo root"] } }
		}"#,
		&[
			(
				"plain.json",
				r#"{ "$schema": "x", "targets": { "p": { "commands": ["echo p"] } } }"#,
			),
			(
				"obj.json",
				r#"{ "$schema": "x", "targets": { "o": { "commands": ["echo o"] } } }"#,
			),
		],
	);
	assert!(
		result.namespaces_by_source.is_empty(),
		"unnamespaced includes contribute nothing: {:?}",
		result.namespaces_by_source
	);
	assert!(result.namespaces_for_target("root").is_empty());
}

#[test]
fn merge_namespaces_are_per_file_and_unprefixed() {
	// Nested includes do NOT compose into one flat list. Each file reports
	// only what its own `includes` declare, bare: the root sees `outer`, and
	// `mid.json` sees `inner` (not `outer:inner`). Composition is handled by
	// the `@target` rewrite, so a dynamic `@{{ VAR.ns }}:build` inside
	// `mid.json` still resolves to `outer:inner:build`.
	let (result, root_path) = merge_namespace_fixture(
		r#"{
			"$schema": "x",
			"includes": [{ "path": "mid.json", "namespace": "outer" }],
			"targets": { "root": { "commands": ["echo root"] } }
		}"#,
		&[
			(
				"mid.json",
				r#"{ "$schema": "x",
				     "includes": [{ "path": "inner.json", "namespace": "inner" }],
				     "targets": { "build": { "commands": ["echo build"] } } }"#,
			),
			(
				"inner.json",
				r#"{ "$schema": "x", "targets": { "build": { "commands": ["echo inner-build"] } } }"#,
			),
		],
	);

	// Root's own list: just its direct include's namespace.
	assert_eq!(
		result.namespaces_by_source.get(&root_path),
		Some(&vec!["outer".to_string()])
	);
	assert_eq!(result.namespaces_for_target("root"), ["outer".to_string()]);

	// A target defined in mid.json sees mid.json's own namespace, unprefixed.
	assert_eq!(result.namespaces_for_target("outer:build"), ["inner".to_string()]);

	// The deepest file declares nothing.
	assert!(result.namespaces_for_target("outer:inner:build").is_empty());

	// The flat reporting union still carries every namespace, for consumers
	// (task-descriptors) that need to recognise namespace roots.
	assert_eq!(
		result.runfile.namespaces,
		vec!["inner".to_string(), "outer".to_string()]
	);
}

#[test]
fn merge_namespaces_dedup_in_final_runfile() {
	// `merge_runfiles` sorts and dedupes, so the same namespace appearing
	// under multiple roots yields a single entry. Tested via the public
	// `merge_runfiles` API — `MergeState` itself just accumulates.
	use crate::merge_runfiles;
	let dir = TempDir::new().unwrap();

	// Two siblings with the same namespace `"shared"`.
	std::fs::write(
		dir.path().join("a.json"),
		r#"{ "$schema": "x", "targets": { "build": { "commands": ["a"] } } }"#,
	)
	.unwrap();
	std::fs::write(
		dir.path().join("b.json"),
		r#"{ "$schema": "x", "targets": { "deploy": { "commands": ["b"] } } }"#,
	)
	.unwrap();

	let local_path = dir.path().join(RUNFILE_NAME);
	let local = parse_runfile(
		r#"{
			"$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
			"includes": [
				{ "path": "a.json", "namespace": "shared" },
				{ "path": "b.json", "namespace": "shared" }
			],
			"targets": { "root": { "commands": ["echo root"] } }
		}"#,
	)
	.unwrap();
	std::fs::write(&local_path, "{}").unwrap();

	let result = merge_runfiles(Some((local, local_path)), &[], dir.path()).unwrap();
	assert_eq!(
		result.runfile.namespaces,
		vec!["shared".to_string()],
		"duplicate namespaces from sibling includes are deduplicated"
	);
}

#[test]
fn rewrites_target_calls_inside_control_flow() {
	let state = run_namespace_include(
		r#"{
            "$schema": "x",
            "includes": [{ "path": "child.json", "namespace": "ns" }],
            "targets": { "root": { "commands": ["echo root"] } }
        }"#,
		&[(
			"child.json",
			r#"{ "$schema": "x", "targets": {
                "build": { "commands": ["echo build"] },
                "lint":  { "commands": ["echo lint"] },
                "all": {
                    "commands": [
                        { "if": "{{ RUN.os }} == windows",
                          "then": ["@build"],
                          "else": ["@lint"] },
                        { "for": "x", "in": ["a"], "do": ["@build"] }
                    ]
                }
            } }"#,
		)],
	);

	let all = &state.targets["ns:all"].commands;

	// First step: an `if` block — both branches should be rewritten.
	match &all[0] {
		CommandStep::If(i) => {
			match &i.then[0] {
				CommandStep::TargetCall(c) => assert_eq!(c.target, "ns:build"),
				other => panic!("expected target call in `then`, got {other:?}"),
			}
			let else_branch = i.r#else.as_ref().expect("else");
			match &else_branch[0] {
				CommandStep::TargetCall(c) => assert_eq!(c.target, "ns:lint"),
				other => panic!("expected target call in `else`, got {other:?}"),
			}
		}
		other => panic!("expected If, got {other:?}"),
	}

	// Second step: a `for` block — body should be rewritten too.
	match &all[1] {
		CommandStep::For(f) => match &f.body[0] {
			CommandStep::TargetCall(c) => assert_eq!(c.target, "ns:build"),
			other => panic!("expected target call in `for/do`, got {other:?}"),
		},
		other => panic!("expected For, got {other:?}"),
	}
}

#[test]
fn invalid_namespace_rejected() {
	for bad in &[":foo", "foo:bar", "_foo", "@foo", "foo bar", "", "ns?bad", "?leading"] {
		let dir = TempDir::new().unwrap();
		std::fs::write(
			dir.path().join("child.json"),
			r#"{ "$schema": "x", "targets": { "build": { "commands": ["echo"] } } }"#,
		)
		.unwrap();
		let root_path = dir.path().join(RUNFILE_NAME);
		// Empty-string namespace round-trips through the deserializer's
		// "blank = no namespace" rule, so we test it via the merge layer too —
		// but here we expect it to *not* error (it's normalised away).
		let body = if bad.is_empty() {
			r#"{ "$schema": "x", "includes": [{ "path": "child.json", "namespace": "" }],
                  "targets": { "root": { "commands": ["echo"] } } }"#
				.to_string()
		} else {
			format!(
				r#"{{ "$schema": "x", "includes": [{{ "path": "child.json", "namespace": "{}" }}],
                       "targets": {{ "root": {{ "commands": ["echo"] }} }} }}"#,
				bad.replace('"', "\\\"")
			)
		};
		std::fs::write(&root_path, body).unwrap();
		let runfile = parse_runfile_from_path(&root_path).unwrap();
		let mut state = crate::merge::MergeState::new();
		let canonical = std::fs::canonicalize(&root_path).unwrap();
		let mut visited = std::collections::HashSet::new();
		visited.insert(canonical.clone());
		let result = crate::merge::resolve_includes(
			&runfile,
			&canonical,
			&mut state,
			&mut visited,
			crate::SourceKind::Included,
		);
		if bad.is_empty() {
			assert!(result.is_ok(), "blank namespace must round-trip as no-namespace");
		} else {
			let err = result.expect_err(&format!("namespace \"{bad}\" must be rejected"));
			let msg = err.to_string();
			assert!(
				msg.contains("Invalid include namespace"),
				"error must mention namespace ({bad}): {msg}"
			);
		}
	}
}

#[test]
fn same_file_included_twice_with_different_namespaces_yields_independent_copies() {
	let state = run_namespace_include(
		r#"{
            "$schema": "x",
            "includes": [
                { "path": "tmpl.json", "namespace": "a" },
                { "path": "tmpl.json", "namespace": "b" }
            ],
            "targets": { "root": { "commands": ["echo root"] } }
        }"#,
		&[(
			"tmpl.json",
			r#"{ "$schema": "x", "targets": {
                "build": { "commands": ["echo build"] }
            } }"#,
		)],
	);

	assert!(state.targets.contains_key("a:build"));
	assert!(state.targets.contains_key("b:build"));
}

#[test]
fn diamond_include_no_namespace_no_cycle_error() {
	// A includes B and C; C also includes B. Without per-call-stack cycle
	// detection this would (incorrectly) fail as a cycle. With the chain-style
	// `visited`, B re-loads cleanly via the second path and merge_runfiles
	// detects the duplicate target as a conflict, not a cycle.
	let dir = TempDir::new().unwrap();
	std::fs::write(
		dir.path().join("b.json"),
		r#"{ "$schema": "x", "targets": { "leaf": { "commands": ["echo leaf"] } } }"#,
	)
	.unwrap();
	std::fs::write(
		dir.path().join("c.json"),
		r#"{ "$schema": "x", "includes": ["b.json"], "targets": {} }"#,
	)
	.unwrap();
	let root_path = dir.path().join(RUNFILE_NAME);
	std::fs::write(
		&root_path,
		r#"{ "$schema": "x", "includes": ["b.json", "c.json"],
              "targets": { "root": { "commands": ["echo root"] } } }"#,
	)
	.unwrap();

	let runfile = parse_runfile_from_path(&root_path).unwrap();
	let mut state = crate::merge::MergeState::new();
	let canonical = std::fs::canonicalize(&root_path).unwrap();
	let mut visited = std::collections::HashSet::new();
	visited.insert(canonical.clone());
	let result = crate::merge::resolve_includes(
		&runfile,
		&canonical,
		&mut state,
		&mut visited,
		crate::SourceKind::Included,
	);
	assert!(
		result.is_ok(),
		"diamond include should not be reported as a cycle: {:?}",
		result
	);
}
