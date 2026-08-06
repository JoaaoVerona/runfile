use super::*;

// ── JSON5 parsing tests ───────────────────────────────────────────

#[test]
fn json5_trailing_comma_in_object() {
	let input = r#"{"a": 1, "b": 2,}"#;
	let val: serde_json::Value = from_json_str(input).unwrap();
	assert_eq!(val["a"], 1);
	assert_eq!(val["b"], 2);
}

#[test]
fn json5_trailing_comma_in_array() {
	let input = r#"[1, 2, 3,]"#;
	let val: serde_json::Value = from_json_str(input).unwrap();
	assert_eq!(val[0], 1);
	assert_eq!(val[2], 3);
}

#[test]
fn json5_single_line_comments() {
	let input = r#"{
		// This is a comment
		"a": 1
	}"#;
	let val: serde_json::Value = from_json_str(input).unwrap();
	assert_eq!(val["a"], 1);
}

#[test]
fn json5_block_comments() {
	let input = r#"{
		/* block comment */
		"a": 1
	}"#;
	let val: serde_json::Value = from_json_str(input).unwrap();
	assert_eq!(val["a"], 1);
}

#[test]
fn json5_unquoted_keys() {
	let input = r#"{a: 1, b: 2}"#;
	let val: serde_json::Value = from_json_str(input).unwrap();
	assert_eq!(val["a"], 1);
	assert_eq!(val["b"], 2);
}

#[test]
fn json5_single_quoted_strings() {
	let input = r#"{'a': 'hello'}"#;
	let val: serde_json::Value = from_json_str(input).unwrap();
	assert_eq!(val["a"], "hello");
}

#[test]
fn json5_plain_json_still_works() {
	let input = r#"{"a": 1}"#;
	let val: serde_json::Value = from_json_str(input).unwrap();
	assert_eq!(val["a"], 1);
}

#[test]
fn json5_real_error_propagated() {
	let input = r#"{"a": }"#;
	assert!(from_json_str::<serde_json::Value>(input).is_err());
}

#[test]
fn json5_runfile_with_comments() {
	let input = r#"{
		"$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
		// Build targets
		"targets": {
			"build": {
				"commands": ["cargo build"], // main build command
			}
		}
	}"#;
	let rf = parse_runfile(input).unwrap();
	assert_eq!(rf.targets["build"].commands, vec!["cargo build"]);
}

// ── Merge tests ───────────────────────────────────────────────────

#[test]
fn merge_local_only_no_global_files() {
	let runfile = Runfile {
		schema: "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json".into(),
		includes: None,
		targets: {
			let mut t = HashMap::new();
			t.insert("build".into(), CommandSpec::new(vec!["cargo build".into()]));
			t
		},
		globals: None,
		namespaces: Vec::new(),
	};
	let dir = TempDir::new().unwrap();
	let path = dir.path().join(RUNFILE_NAME);
	let result = merge_runfiles(Some((runfile, path)), &[], dir.path()).unwrap();
	assert_eq!(result.runfile.targets.len(), 1);
	assert!(result.runfile.targets.contains_key("build"));
}

#[test]
fn merge_global_only_no_local() {
	let dir = TempDir::new().unwrap();
	let global_path = dir.path().join("global.json");
	std::fs::write(
		&global_path,
		r#"{ "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json", "targets": { "lint": { "commands": ["cargo clippy"] } } }"#,
	)
	.unwrap();

	let result = merge_runfiles(None, &[global_path], dir.path()).unwrap();
	assert_eq!(result.runfile.targets.len(), 1);
	assert!(result.runfile.targets.contains_key("lint"));
}

#[test]
fn merge_local_and_global_conflict() {
	let dir = TempDir::new().unwrap();
	let global_path = dir.path().join("global.json");
	std::fs::write(
		&global_path,
		r#"{ "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json", "targets": { "build": { "commands": ["global build"] }, "deploy": { "commands": ["deploy"] } } }"#,
	)
	.unwrap();

	let local = Runfile {
		schema: "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json".into(),
		includes: None,
		targets: {
			let mut t = HashMap::new();
			t.insert("build".into(), {
				let mut s = CommandSpec::new(vec!["local build".into()]);
				s.description = Some("local".into());
				s
			});
			t.insert("test".into(), {
				let mut s = CommandSpec::new(vec!["local test".into()]);
				s.description = Some("local test".into());
				s
			});
			t
		},
		globals: None,
		namespaces: Vec::new(),
	};
	let local_path = dir.path().join(RUNFILE_NAME);

	let result = merge_runfiles(
		Some((local, local_path.clone())),
		std::slice::from_ref(&global_path),
		dir.path(),
	)
	.unwrap();

	// "build" is defined in both local and global — should be a conflict
	assert!(result.conflicts.contains_key("build"), "build should be a conflict");
	assert!(
		!result.runfile.targets.contains_key("build"),
		"build should not be in runnable targets"
	);

	// Conflict should list both sources
	let build_sources = &result.conflicts["build"];
	assert_eq!(build_sources.len(), 2);

	// "test" is only in local — should be runnable
	assert!(result.runfile.targets.contains_key("test"));

	// "deploy" is only in global — should be runnable
	assert!(result.runfile.targets.contains_key("deploy"));
}

#[test]
fn merge_only_in_directories_filters() {
	let base = TempDir::new().unwrap();
	let allowed = base.path().join("allowed");
	std::fs::create_dir(&allowed).unwrap();

	let global_path = base.path().join("global.json");
	std::fs::write(
        &global_path,
        r#"{ "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json", "targets": { "lint": { "commands": ["lint"] } }, "globals": { "onlyInDirectories": ["allowed"] } }"#,
    )
    .unwrap();

	// CWD is under allowed — should include
	let result = merge_runfiles(None, std::slice::from_ref(&global_path), &allowed).unwrap();
	assert!(result.runfile.targets.contains_key("lint"));

	// CWD is base (not under allowed) — should exclude
	let result = merge_runfiles(None, &[global_path], base.path());
	assert!(result.is_err()); // No targets
}

#[test]
fn merge_missing_global_file_skipped() {
	let dir = TempDir::new().unwrap();
	let missing = dir.path().join("nonexistent.json");

	let local = Runfile {
		schema: "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json".into(),
		includes: None,
		targets: {
			let mut t = HashMap::new();
			t.insert("build".into(), CommandSpec::new(vec!["echo".into()]));
			t
		},
		globals: None,
		namespaces: Vec::new(),
	};
	let local_path = dir.path().join(RUNFILE_NAME);

	let result = merge_runfiles(Some((local, local_path)), &[missing], dir.path()).unwrap();
	assert_eq!(result.runfile.targets.len(), 1);
}

#[test]
fn merge_globals_baked_into_targets() {
	let dir = TempDir::new().unwrap();
	let global_path = dir.path().join("global.json");
	std::fs::write(
        &global_path,
        r#"{ "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json", "targets": { "build": { "commands": ["build"] } }, "globals": { "logging": true, "env": { "FOO": "bar" } } }"#,
    )
    .unwrap();

	let result = merge_runfiles(None, &[global_path], dir.path()).unwrap();
	let spec = &result.runfile.targets["build"];
	assert_eq!(spec.logging, Some(true));
	assert!(spec.env.is_some());
	assert!(result.runfile.globals.is_none());
}

#[test]
fn merge_globals_vars_baked_and_overridden() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("global.json");
	std::fs::write(
		&path,
		r#"{ "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
            "targets": { "build": { "commands": ["build"], "vars": { "shared": "target", "own": "o" } } },
            "globals": { "vars": { "g": "global-g", "shared": "global-shared" } } }"#,
	)
	.unwrap();

	let result = merge_runfiles(None, &[path], dir.path()).unwrap();
	let vars = result.runfile.targets["build"].vars.as_ref().unwrap();
	// Global-only key carries through; target key wins on conflict; target-only key kept.
	assert_eq!(vars["g"], EnvValue::String("global-g".into()));
	assert_eq!(vars["shared"], EnvValue::String("target".into()));
	assert_eq!(vars["own"], EnvValue::String("o".into()));
	assert!(result.runfile.globals.is_none());
}

#[test]
fn merge_source_dirs_tracked() {
	let dir = TempDir::new().unwrap();
	let global_dir = dir.path().join("global");
	std::fs::create_dir(&global_dir).unwrap();
	let global_path = global_dir.join(RUNFILE_NAME);
	std::fs::write(
		&global_path,
		r#"{ "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json", "targets": { "lint": { "commands": ["lint"] } } }"#,
	)
	.unwrap();

	let local = Runfile {
		schema: "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json".into(),
		includes: None,
		targets: {
			let mut t = HashMap::new();
			t.insert("build".into(), CommandSpec::new(vec!["build".into()]));
			t
		},
		globals: None,
		namespaces: Vec::new(),
	};
	let local_path = dir.path().join(RUNFILE_NAME);

	let result = merge_runfiles(Some((local, local_path)), &[global_path], dir.path()).unwrap();

	// "lint" should come from global_dir, "build" from dir
	assert_eq!(result.source_dirs["lint"], global_dir);
	assert_eq!(result.source_dirs["build"], *dir.path());
}

#[test]
fn cross_file_target_refs_accepted_at_parse_time() {
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "deploy": {
                "commands": ["@build", "deploy"]
            }
        }
    }"#;
	// `@target` references to unknown targets are validated at runtime, not
	// parse time — included files may define `build` later.
	assert!(parse_runfile(json).is_ok());
	assert!(parse_runfile_partial(json).is_ok());
}

#[test]
fn partial_parse_allows_zero_targets() {
	let json = r#"{ "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json", "targets": {} }"#;
	assert!(parse_runfile(json).is_err());
	assert!(parse_runfile_partial(json).is_ok());
}

#[test]
fn reject_detach_without_parallel_multiple_commands() {
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "bg": {
                "commands": ["echo hello", "echo world"],
                "detach": true
            }
        }
    }"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(err.to_string().contains("detach"));
	assert!(err.to_string().contains("parallel"));
}

#[test]
fn reject_detach_without_parallel_multiple_commands_partial() {
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "bg": {
                "commands": ["echo hello", "echo world"],
                "detach": true
            }
        }
    }"#;
	let err = parse_runfile_partial(json).unwrap_err();
	assert!(err.to_string().contains("detach"));
}

#[test]
fn accept_detach_single_command_without_parallel() {
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "bg": {
                "commands": ["echo hello"],
                "detach": true
            }
        }
    }"#;
	assert!(parse_runfile(json).is_ok());
}

#[test]
fn accept_detach_with_parallel() {
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "bg": {
                "commands": ["echo hello", "echo world"],
                "parallel": true,
                "detach": true
            }
        }
    }"#;
	assert!(parse_runfile(json).is_ok());
}

#[test]
fn accept_parallel_without_detach() {
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "multi": {
                "commands": ["echo a", "echo b"],
                "parallel": true
            }
        }
    }"#;
	let runfile = parse_runfile(json).unwrap();
	assert_eq!(runfile.targets["multi"].parallel, Some(true));
}

// ── Env key validation tests ──────────────────────────────────────

#[test]
fn is_valid_env_key_accepts_simple_names() {
	assert!(is_valid_env_key("FOO"));
	assert!(is_valid_env_key("bar"));
	assert!(is_valid_env_key("NODE_ENV"));
	assert!(is_valid_env_key("_PRIVATE"));
	assert!(is_valid_env_key("A"));
	assert!(is_valid_env_key("_"));
	assert!(is_valid_env_key("a1b2c3"));
	assert!(is_valid_env_key("MY_VAR_123"));
}

#[test]
fn is_valid_env_key_rejects_empty() {
	assert!(!is_valid_env_key(""));
}

#[test]
fn is_valid_env_key_rejects_leading_digit() {
	assert!(!is_valid_env_key("1FOO"));
	assert!(!is_valid_env_key("0"));
	assert!(!is_valid_env_key("99_PROBLEMS"));
}

#[test]
fn is_valid_env_key_rejects_special_chars() {
	assert!(!is_valid_env_key("FOO-BAR"));
	assert!(!is_valid_env_key("FOO.BAR"));
	assert!(!is_valid_env_key("FOO BAR"));
	assert!(!is_valid_env_key("FOO;BAR"));
	assert!(!is_valid_env_key("VAR&whoami"));
	assert!(!is_valid_env_key("VAR|cat"));
	assert!(!is_valid_env_key("$env:VAR"));
	assert!(!is_valid_env_key("VAR=value"));
	assert!(!is_valid_env_key("FOO`BAR"));
	assert!(!is_valid_env_key("FOO(BAR)"));
}

#[test]
fn parse_rejects_invalid_env_key_in_target() {
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": {
                "commands": ["echo test"],
                "env": { "VALID_KEY": "ok", "bad;key": "injected" }
            }
        }
    }"#;
	let result = parse_runfile(json);
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(err.contains("bad;key"), "error should mention the bad key: {err}");
	assert!(
		err.contains("Invalid environment variable name"),
		"should be env key error: {err}"
	);
}

#[test]
fn parse_rejects_env_key_with_ampersand() {
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "test": {
                "commands": ["echo test"],
                "env": { "VAR&whoami": "pwned" }
            }
        }
    }"#;
	let result = parse_runfile(json);
	assert!(result.is_err());
}

#[test]
fn parse_rejects_env_key_with_dollar_sign() {
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "test": {
                "commands": ["echo test"],
                "env": { "$env:SECRET": "value" }
            }
        }
    }"#;
	let result = parse_runfile(json);
	assert!(result.is_err());
}

#[test]
fn parse_rejects_env_key_starting_with_digit() {
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "test": {
                "commands": ["echo test"],
                "env": { "1KEY": "value" }
            }
        }
    }"#;
	let result = parse_runfile(json);
	assert!(result.is_err());
}

#[test]
fn parse_rejects_invalid_env_key_in_globals() {
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["echo test"] }
        },
        "globals": {
            "env": { "OK_KEY": "fine", "bad key": "spaces" }
        }
    }"#;
	let result = parse_runfile(json);
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(err.contains("bad key"), "error should mention the bad key: {err}");
}

#[test]
fn parse_accepts_valid_env_keys() {
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": {
                "commands": ["echo test"],
                "env": {
                    "SIMPLE": "ok",
                    "_UNDERSCORE": "ok",
                    "camelCase": "ok",
                    "MIX_123_abc": "ok",
                    "A": "ok"
                }
            }
        },
        "globals": {
            "env": { "GLOBAL_VAR": "value" }
        }
    }"#;
	let result = parse_runfile(json);
	assert!(result.is_ok(), "valid env keys should be accepted: {:?}", result.err());
}

#[test]
fn parse_partial_also_rejects_invalid_env_keys() {
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": {
                "commands": ["echo test"],
                "env": { "key;injection": "value" }
            }
        }
    }"#;
	let result = parse_runfile_partial(json);
	assert!(result.is_err());
}

// ── Includes declared inside registered *global* Runfiles ─────────────

/// Build a global Runfile at `<dir>/global/Runfile.json` that includes
/// `sub/Runfile.json`, optionally under a namespace. Returns (tempdir, path).
fn global_with_include(namespace: Option<&str>) -> (TempDir, std::path::PathBuf) {
	let dir = TempDir::new().unwrap();
	let gdir = dir.path().join("global");
	std::fs::create_dir_all(gdir.join("sub")).unwrap();
	std::fs::write(
		gdir.join("sub").join("Runfile.json"),
		r#"{ "$schema": "x", "targets": { "subtarget": { "commands": ["echo sub"] } } }"#,
	)
	.unwrap();
	let entry = match namespace {
		Some(ns) => format!(r#"{{ "path": "sub/Runfile.json", "namespace": "{ns}" }}"#),
		None => r#""sub/Runfile.json""#.to_string(),
	};
	let gpath = gdir.join("Runfile.json");
	std::fs::write(
		&gpath,
		format!(
			r#"{{ "$schema": "x", "includes": [{entry}],
			      "targets": {{ "gtarget": {{ "commands": ["echo g"] }} }} }}"#
		),
	)
	.unwrap();
	(dir, gpath)
}

#[test]
fn global_file_namespaced_includes_are_resolved() {
	// Regression: a registered global Runfile's `includes` used to be parsed
	// and then silently ignored — only the local Runfile's includes were ever
	// resolved. Namespaced or not, they now merge in.
	let (dir, gpath) = global_with_include(Some("gns"));
	let result = merge_runfiles(None, &[gpath], dir.path()).unwrap();

	assert!(
		result.runfile.targets.contains_key("gns:subtarget"),
		"namespaced include of a global file should merge in: {:?}",
		result.runfile.targets.keys().collect::<Vec<_>>()
	);
	assert!(result.runfile.targets.contains_key("gtarget"));
}

#[test]
fn global_file_plain_includes_are_resolved() {
	// The un-namespaced (string) form was equally ignored.
	let (dir, gpath) = global_with_include(None);
	let result = merge_runfiles(None, &[gpath], dir.path()).unwrap();
	assert!(result.runfile.targets.contains_key("subtarget"));
}

#[test]
fn global_file_included_targets_are_kind_global() {
	// Provenance: the user registered the *outer* file machine-wide, so
	// everything it drags in is machine-wide too. This keeps them grouped
	// under `global` in `:list` / filed under Globals by the VS Code
	// extension, and — critically — out of the `SourceKind::Included` filter
	// that `--include-globals` (without `--include-namespaces`) applies.
	let (dir, gpath) = global_with_include(Some("gns"));
	let result = merge_runfiles(None, &[gpath], dir.path()).unwrap();

	let (_, kind) = result.target_sources.get("gns:subtarget").expect("target recorded");
	assert_eq!(*kind, crate::SourceKind::Global);
}

#[test]
fn global_file_broken_include_warns_and_skips_rest_of_merge() {
	// A broken include inside ONE registered global must not take down every
	// `run` invocation on the machine — the surrounding loop already treats a
	// missing / unparsable global file as warn-and-continue, and includes now
	// match that. The other global's targets still merge.
	let dir = TempDir::new().unwrap();
	let broken = dir.path().join("broken.json");
	std::fs::write(
		&broken,
		r#"{ "$schema": "x", "includes": ["does-not-exist.json"],
		     "targets": { "from-broken": { "commands": ["echo b"] } } }"#,
	)
	.unwrap();
	let good = dir.path().join("good.json");
	std::fs::write(
		&good,
		r#"{ "$schema": "x", "targets": { "from-good": { "commands": ["echo g"] } } }"#,
	)
	.unwrap();

	// `merge_runfiles_silent` takes the same path minus the stderr warning.
	let result = crate::merge_runfiles_silent(None, &[broken, good], dir.path()).unwrap();
	assert!(
		result.runfile.targets.contains_key("from-good"),
		"a sibling global must still merge after a broken include"
	);
	assert!(
		result.runfile.targets.contains_key("from-broken"),
		"the broken file's own targets are unaffected — only its include is skipped"
	);
}

#[test]
fn namespaces_are_scoped_per_source_file_across_local_and_global() {
	// `for "in": "namespaces"` must not leak across files: a project target
	// iterates only its own Runfile's namespaces, and a global target only
	// its own — regardless of what else took part in the merge.
	let dir = TempDir::new().unwrap();

	// Global declares namespace `gns`.
	let (gdir_holder, gpath) = global_with_include(Some("gns"));

	// Local declares namespace `api`.
	std::fs::create_dir_all(dir.path().join("api")).unwrap();
	std::fs::write(
		dir.path().join("api").join("Runfile.json"),
		r#"{ "$schema": "x", "targets": { "dev": { "commands": ["echo api-dev"] } } }"#,
	)
	.unwrap();
	let local_json = r#"{
		"$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
		"includes": [{ "path": "api/Runfile.json", "namespace": "api" }],
		"targets": { "root": { "commands": ["echo root"] } }
	}"#;
	let local_path = dir.path().join(RUNFILE_NAME);
	std::fs::write(&local_path, local_json).unwrap();
	let local = parse_runfile(local_json).unwrap();

	let result = merge_runfiles(Some((local, local_path)), &[gpath], dir.path()).unwrap();

	assert_eq!(
		result.namespaces_for_target("root"),
		["api".to_string()],
		"a local target sees only its own file's namespaces"
	);
	assert_eq!(
		result.namespaces_for_target("gtarget"),
		["gns".to_string()],
		"a global target sees only its own file's namespaces"
	);
	// The flat reporting union still spans both, for namespace-root detection.
	assert_eq!(result.runfile.namespaces, vec!["api".to_string(), "gns".to_string()]);
	drop(gdir_holder);
}
