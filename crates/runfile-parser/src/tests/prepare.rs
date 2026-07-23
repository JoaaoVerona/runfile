use super::*;
use std::path::{Path, PathBuf};

/// Parse + merge a single in-memory Runfile (no includes) so `required_prepares`
/// is baked. The path need not exist — merge only reads it for include
/// resolution (a no-op here) and source-dir derivation.
fn merge_json(json: &str) -> Runfile {
	let rf = parse_runfile(json).unwrap();
	let path = PathBuf::from("/tmp/Runfile.json");
	merge_runfiles(Some((rf, path)), &[], Path::new("/tmp"))
		.unwrap()
		.runfile
}

/// Parse + merge a root Runfile plus a set of relative include files written to
/// a temp dir, exercising the full include/namespace/bake pipeline.
fn merge_from_files(root: &str, files: &[(&str, &str)]) -> Runfile {
	let dir = TempDir::new().unwrap();
	for (rel, body) in files {
		let p = dir.path().join(rel);
		if let Some(parent) = p.parent() {
			std::fs::create_dir_all(parent).unwrap();
		}
		std::fs::write(p, body).unwrap();
	}
	let root_path = dir.path().join("Runfile.json");
	std::fs::write(&root_path, root).unwrap();
	let rf = parse_runfile_from_path(&root_path).unwrap();
	merge_runfiles(Some((rf, root_path)), &[], dir.path()).unwrap().runfile
}

// ── Value parsing / format validation ─────────────────────────────

#[test]
fn parse_prepare_value_strips_at_and_splits_args() {
	assert_eq!(parse_prepare_value("@setup"), Ok(("setup".to_string(), String::new())));
	assert_eq!(
		parse_prepare_value("@setup --prod"),
		Ok(("setup".to_string(), "--prod".to_string()))
	);
}

#[test]
fn parse_prepare_value_rejects_bare_name() {
	assert!(parse_prepare_value("setup").is_err());
}

#[test]
fn parse_prepare_value_rejects_optional_form() {
	assert!(parse_prepare_value("@?setup").is_err());
}

#[test]
fn prepare_invocation_normalizes_whitespace() {
	assert_eq!(prepare_invocation("setup", ""), "setup");
	assert_eq!(prepare_invocation("setup", "  --prod   --fast "), "setup --prod --fast");
	assert_eq!(prepare_invocation_target("setup --prod"), "setup");
	assert_eq!(prepare_invocation_target("setup"), "setup");
}

#[test]
fn globals_prepare_rejects_bare_at_parse_time() {
	let json = r#"{ "$schema": "x", "globals": { "prepare": "setup" },
		"targets": { "setup": { "commands": "echo hi" } } }"#;
	let err = parse_runfile(json).unwrap_err().to_string();
	assert!(err.contains("prepare") && err.contains("'@'"), "{err}");
}

#[test]
fn target_prepare_rejects_optional_at_parse_time() {
	let json = r#"{ "$schema": "x",
		"targets": { "build": { "commands": "echo b", "prepare": "@?setup" } } }"#;
	let err = parse_runfile(json).unwrap_err().to_string();
	assert!(err.contains("prepare") && err.contains("@?"), "{err}");
}

// ── required_prepares baking (additive union) ─────────────────────

#[test]
fn global_prepare_baked_into_every_target() {
	let rf = merge_json(
		r#"{ "$schema": "x", "globals": { "prepare": "@setup" },
			"targets": {
				"setup": { "commands": "echo s" },
				"build": { "commands": "echo b" }
			} }"#,
	);
	assert_eq!(rf.targets["build"].required_prepares, vec!["setup".to_string()]);
	// The prepare target itself also carries the requirement — the runner
	// exempts it from the gate by name, not by clearing the field.
	assert_eq!(rf.targets["setup"].required_prepares, vec!["setup".to_string()]);
}

#[test]
fn target_prepare_is_additive_with_global() {
	let rf = merge_json(
		r#"{ "$schema": "x", "globals": { "prepare": "@setup" },
			"targets": {
				"setup": { "commands": "echo s" },
				"setup-tests": { "commands": "echo st" },
				"test": { "commands": "echo t", "prepare": "@setup-tests" }
			} }"#,
	);
	// Global first, then the target's own — union, order preserved.
	assert_eq!(
		rf.targets["test"].required_prepares,
		vec!["setup".to_string(), "setup-tests".to_string()]
	);
}

#[test]
fn duplicate_global_and_target_prepare_deduped() {
	let rf = merge_json(
		r#"{ "$schema": "x", "globals": { "prepare": "@setup" },
			"targets": {
				"setup": { "commands": "echo s" },
				"build": { "commands": "echo b", "prepare": "@setup" }
			} }"#,
	);
	assert_eq!(rf.targets["build"].required_prepares, vec!["setup".to_string()]);
}

#[test]
fn prepare_with_args_keeps_args_in_invocation() {
	let rf = merge_json(
		r#"{ "$schema": "x",
			"targets": {
				"setup": { "commands": "echo s" },
				"deploy": { "commands": "echo d", "prepare": "@setup --prod" }
			} }"#,
	);
	assert_eq!(rf.targets["deploy"].required_prepares, vec!["setup --prod".to_string()]);
}

#[test]
fn no_prepare_means_empty_required() {
	let rf = merge_json(r#"{ "$schema": "x", "targets": { "build": { "commands": "echo b" } } }"#);
	assert!(rf.targets["build"].required_prepares.is_empty());
}

// ── Namespaced includes rewrite the prepare target ────────────────

#[test]
fn namespaced_include_prefixes_required_prepares() {
	let rf = merge_from_files(
		r#"{ "$schema": "x", "includes": [{ "path": "api.json", "namespace": "api" }],
			"targets": { "root": { "commands": "echo r" } } }"#,
		&[(
			"api.json",
			r#"{ "$schema": "x", "globals": { "prepare": "@setup" },
				"targets": {
					"setup": { "commands": "echo s" },
					"build": { "commands": "echo b" }
				} }"#,
		)],
	);
	assert_eq!(rf.targets["api:build"].required_prepares, vec!["api:setup".to_string()]);
	// And the prepare target itself is discoverable under its namespaced name.
	assert!(rf.prepare_target_names().contains("api:setup"));
}

// ── prepare_command_hash: stability, recursion, sensitivity ───────

#[test]
fn prepare_hash_is_stable_across_calls() {
	let rf = merge_json(r#"{ "$schema": "x", "targets": { "setup": { "commands": ["echo a", "echo b"] } } }"#);
	assert_eq!(rf.prepare_command_hash("setup"), rf.prepare_command_hash("setup"));
	assert!(rf.prepare_command_hash("setup").is_some());
}

#[test]
fn prepare_hash_changes_when_commands_change() {
	let a = merge_json(r#"{ "$schema": "x", "targets": { "setup": { "commands": "echo v1" } } }"#);
	let b = merge_json(r#"{ "$schema": "x", "targets": { "setup": { "commands": "echo v2" } } }"#);
	assert_ne!(a.prepare_command_hash("setup"), b.prepare_command_hash("setup"));
}

#[test]
fn prepare_hash_covers_transitive_target_calls() {
	let a = merge_json(
		r#"{ "$schema": "x", "targets": {
			"setup": { "commands": ["echo s", "@dep"] },
			"dep": { "commands": "echo DEP-V1" }
		} }"#,
	);
	let b = merge_json(
		r#"{ "$schema": "x", "targets": {
			"setup": { "commands": ["echo s", "@dep"] },
			"dep": { "commands": "echo DEP-V2" }
		} }"#,
	);
	// `setup`'s own commands are identical; only `@dep`'s body changed.
	assert_ne!(a.prepare_command_hash("setup"), b.prepare_command_hash("setup"));
}

#[test]
fn prepare_hash_handles_recursive_cycles() {
	// a → b → a; the cycle guard must terminate.
	let rf = merge_json(
		r#"{ "$schema": "x", "targets": {
			"a": { "commands": ["echo a", "@b"] },
			"b": { "commands": ["echo b", "@a"] }
		} }"#,
	);
	assert!(rf.prepare_command_hash("a").is_some());
}

#[test]
fn prepare_hash_none_for_unknown_target() {
	let rf = merge_json(r#"{ "$schema": "x", "targets": { "build": { "commands": "echo b" } } }"#);
	assert!(rf.prepare_command_hash("nope").is_none());
}

#[test]
fn prepare_target_names_collects_all_prepares() {
	let rf = merge_json(
		r#"{ "$schema": "x", "globals": { "prepare": "@setup" },
			"targets": {
				"setup": { "commands": "echo s" },
				"setup-tests": { "commands": "echo st" },
				"test": { "commands": "echo t", "prepare": "@setup-tests" }
			} }"#,
	);
	let names = rf.prepare_target_names();
	assert!(names.contains("setup"));
	assert!(names.contains("setup-tests"));
	assert_eq!(names.len(), 2);
}
