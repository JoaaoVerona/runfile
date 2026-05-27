use super::*;

// ── Reserved target name tests ─────────────────────────────────────

#[test]
fn reject_target_name_starting_with_colon() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            ":build": { "commands": ["echo"] }
        }
    }"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(matches!(err, ParseError::ReservedTargetName(_)));
}

#[test]
fn reject_target_name_colon_list() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            ":list": { "commands": ["echo"] }
        }
    }"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(matches!(err, ParseError::ReservedTargetName(_)));
}

#[test]
fn accept_target_names_with_colon_not_at_start() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "ci:build": { "commands": ["echo"] },
            "test:unit": { "commands": ["echo"] }
        }
    }"#;
	assert!(parse_runfile(json).is_ok());
}

#[test]
fn accept_previously_reserved_target_names() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "list": { "commands": ["echo"] },
            "config": { "commands": ["echo"] },
            "utilities": { "commands": ["echo"] }
        }
    }"#;
	assert!(parse_runfile(json).is_ok());
}

// ── Alias tests ───────────────────────────────────────────────────

#[test]
fn parse_target_with_aliases() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "stop-dev": { "commands": ["./stop.sh"], "aliases": ["stop", "sd"] }
        }
    }"#;
	let runfile = parse_runfile(json).unwrap();
	let spec = &runfile.targets["stop-dev"];
	assert_eq!(
		spec.aliases.as_ref().unwrap(),
		&vec!["stop".to_string(), "sd".to_string()]
	);
}

#[test]
fn resolve_target_by_name() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["cargo build"], "aliases": ["b"] }
        }
    }"#;
	let runfile = parse_runfile(json).unwrap();
	assert_eq!(runfile.resolve_target("build"), Some("build"));
}

#[test]
fn resolve_target_by_alias() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["cargo build"], "aliases": ["b"] }
        }
    }"#;
	let runfile = parse_runfile(json).unwrap();
	assert_eq!(runfile.resolve_target("b"), Some("build"));
}

#[test]
fn resolve_target_unknown_returns_none() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["cargo build"] }
        }
    }"#;
	let runfile = parse_runfile(json).unwrap();
	assert_eq!(runfile.resolve_target("unknown"), None);
}

#[test]
fn all_target_names_includes_aliases() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["cargo build"], "aliases": ["b"] },
            "test": { "commands": ["cargo test"] }
        }
    }"#;
	let runfile = parse_runfile(json).unwrap();
	let names = runfile.all_target_names();
	assert!(names.contains(&"build"));
	assert!(names.contains(&"b"));
	assert!(names.contains(&"test"));
}

#[test]
fn reject_alias_conflicts_with_target() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["cargo build"], "aliases": ["test"] },
            "test": { "commands": ["cargo test"] }
        }
    }"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(matches!(err, ParseError::AliasConflictsWithTarget(_, _)));
}

#[test]
fn reject_duplicate_alias_across_targets() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["cargo build"], "aliases": ["x"] },
            "test": { "commands": ["cargo test"], "aliases": ["x"] }
        }
    }"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(matches!(err, ParseError::DuplicateAlias(_, _, _)));
}

#[test]
fn reject_alias_starting_with_colon() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["cargo build"], "aliases": [":b"] }
        }
    }"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(matches!(err, ParseError::ReservedAlias(_, _)));
}

#[test]
fn accept_alias_with_colon_not_at_start() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["cargo build"], "aliases": ["ci:b"] }
        }
    }"#;
	assert!(parse_runfile(json).is_ok());
}

#[test]
fn reject_alias_same_as_target() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["cargo build"], "aliases": ["build"] }
        }
    }"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(matches!(err, ParseError::AliasSameAsTarget(_, _)));
}

// ── workingDirectory tests ──────────────────────────────────────────

#[test]
fn parse_working_directory_substitution_on_target() {
	// `workingDirectory` is a free-form path that supports `{{ ... }}`
	// substitution.
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": {
                "commands": ["echo"],
                "workingDirectory": "{{ RUN.cwd }}"
            }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert_eq!(rf.targets["build"].working_directory.as_deref(), Some("{{ RUN.cwd }}"));
}

#[test]
fn parse_working_directory_relative_path_on_target() {
	// Plain relative paths are accepted; the runner resolves them against
	// the target's source Runfile directory.
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": {
                "commands": ["echo"],
                "workingDirectory": "subdir/build"
            }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert_eq!(rf.targets["build"].working_directory.as_deref(), Some("subdir/build"));
}

#[test]
fn parse_working_directory_absolute_path_on_target() {
	// Absolute paths pass through untouched.
	#[cfg(windows)]
	let abs = r"C:\\Users\\dev\\project";
	#[cfg(not(windows))]
	let abs = "/home/dev/project";
	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "build": {{
                "commands": ["echo"],
                "workingDirectory": "{abs}"
            }}
        }}
    }}"#
	);
	let rf = parse_runfile(&json).unwrap();
	assert!(rf.targets["build"].working_directory.is_some());
}

#[test]
fn parse_working_directory_on_globals() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["echo"] }
        },
        "globals": {
            "workingDirectory": "{{ RUN.cwd }}"
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert_eq!(rf.globals.unwrap().working_directory.as_deref(), Some("{{ RUN.cwd }}"));
}

#[test]
fn parse_working_directory_absent_is_none() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["echo"] }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert!(rf.targets["build"].working_directory.is_none());
}
