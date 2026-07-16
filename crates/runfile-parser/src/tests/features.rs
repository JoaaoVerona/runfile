use super::*;

// ── extendStdio tests ─────────────────────────────────────────────

#[test]
fn parse_extend_stdio() {
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": {
                "commands": ["npm run build"],
                "extendStdio": [
                    { "fromFile": "build.log", "stream": "stdout" },
                    { "fromFile": "errors.log", "stream": "stderr" }
                ]
            }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	let ext = rf.targets["build"].extend_stdio.as_ref().unwrap();
	assert_eq!(ext.len(), 2);
	assert_eq!(ext[0].from_file, "build.log");
	assert_eq!(ext[0].stream, StdioStream::Stdout);
	assert_eq!(ext[1].from_file, "errors.log");
	assert_eq!(ext[1].stream, StdioStream::Stderr);
}

#[test]
fn parse_extend_stdio_rejects_unknown_stream() {
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": {
                "commands": ["echo"],
                "extendStdio": [
                    { "fromFile": "x.log", "stream": "stdin" }
                ]
            }
        }
    }"#;
	assert!(parse_runfile(json).is_err());
}

#[test]
fn parse_extend_stdio_rejects_missing_fields() {
	// Missing "stream"
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": {
                "commands": ["echo"],
                "extendStdio": [{ "fromFile": "x.log" }]
            }
        }
    }"#;
	assert!(parse_runfile(json).is_err());

	// Missing "fromFile"
	let json2 = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": {
                "commands": ["echo"],
                "extendStdio": [{ "stream": "stdout" }]
            }
        }
    }"#;
	assert!(parse_runfile(json2).is_err());
}

// ── forceKillOnSigInt tests ───────────────────────────────────────

#[test]
fn parse_force_kill_on_sig_int() {
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "unity": {
                "commands": ["unity -batchmode"],
                "forceKillOnSigInt": true
            }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert_eq!(rf.targets["unity"].force_kill_on_sig_int, Some(true));
}

#[test]
fn parse_force_kill_on_sig_int_in_globals() {
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "globals": {
            "forceKillOnSigInt": true
        },
        "targets": {
            "unity": {
                "commands": ["unity -batchmode"]
            }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert_eq!(rf.globals.unwrap().force_kill_on_sig_int, Some(true));
}

// ── Internal targets (names starting with "_") ─────────────────────

#[test]
fn parse_accepts_internal_target_name() {
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "_setup": { "commands": ["echo internal"] },
            "build":  { "commands": ["cargo build"] }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert!(rf.targets.contains_key("_setup"));
	assert!(is_internal_target_name("_setup"));
	assert!(!is_internal_target_name("build"));
}

#[test]
fn is_internal_resolves_through_aliases() {
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "_setup": {
                "commands": ["echo internal"],
                "aliases": ["bootstrap"]
            },
            "build": { "commands": ["cargo build"] }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	// Canonical and alias for an internal target both report as internal.
	assert!(rf.is_internal("_setup"));
	assert!(rf.is_internal("bootstrap"));
	// Public target is not internal.
	assert!(!rf.is_internal("build"));
	// Unknown name is not internal.
	assert!(!rf.is_internal("nope"));
}

#[test]
fn public_target_names_excludes_internal_targets_and_their_aliases() {
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "_setup": {
                "commands": ["echo internal"],
                "aliases": ["bootstrap"]
            },
            "build": {
                "commands": ["cargo build"],
                "aliases": ["b"]
            }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();

	let all = rf.all_target_names();
	assert!(all.contains(&"_setup"));
	assert!(all.contains(&"bootstrap"));
	assert!(all.contains(&"build"));
	assert!(all.contains(&"b"));

	let public = rf.public_target_names();
	assert!(!public.contains(&"_setup"));
	assert!(!public.contains(&"bootstrap"));
	assert!(public.contains(&"build"));
	assert!(public.contains(&"b"));
}

#[test]
fn internal_target_can_be_referenced_via_at_call() {
	// `@` invocations to internal targets (`_name`) are valid — internal-only
	// means "not directly invocable from the CLI", not "uncallable".
	let json = r#"{
        "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "_setup": { "commands": ["echo setup"] },
            "build": {
                "commands": ["@_setup", "cargo build"]
            }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	match &rf.targets["build"].commands[0] {
		CommandStep::TargetCall(call) => assert_eq!(call.target, "_setup"),
		_ => panic!("expected TargetCall"),
	}
}

// ──────────────────────────────────────────────────────────────────
// Control flow: if / for blocks
// ──────────────────────────────────────────────────────────────────

#[test]
fn parse_if_block_with_string_then() {
	let json = r#"{
		"$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"deploy": {
				"commands": [
					{ "if": "{{ ARG.env }} == production", "then": ["./deploy-prod.sh"] }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let cmd0 = &rf.targets["deploy"].commands[0];
	match cmd0 {
		CommandStep::If(if_step) => {
			assert_eq!(if_step.condition, "{{ ARG.env }} == production");
			assert_eq!(if_step.then.len(), 1);
			assert!(if_step.r#else.is_none());
		}
		_ => panic!("expected If block"),
	}
}

#[test]
fn parse_if_block_with_else() {
	let json = r#"{
		"$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"deploy": {
				"commands": [
					{ "if": "{{ ARG.dry-run }}", "then": ["echo would deploy"], "else": ["./deploy.sh"] }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let cmd0 = &rf.targets["deploy"].commands[0];
	match cmd0 {
		CommandStep::If(if_step) => {
			assert!(if_step.r#else.is_some());
			let else_branch = if_step.r#else.as_ref().unwrap();
			assert_eq!(else_branch.len(), 1);
		}
		_ => panic!("expected If block"),
	}
}

#[test]
fn parse_if_block_with_ignore_errors() {
	let json = r#"{
		"$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"clean": {
				"commands": [
					{ "if": "{{ FLAG.force }} == true", "then": ["rm -rf target"], "ignoreErrors": true }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::If(if_step) = &rf.targets["clean"].commands[0] {
		assert_eq!(if_step.ignore_errors, Some(true));
	} else {
		panic!("expected If block");
	}
}

#[test]
fn parse_if_block_then_as_string_shorthand() {
	let json = r#"{
		"$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"deploy": {
				"commands": [
					{ "if": "{{ ARG.env }} == production", "then": "./deploy-prod.sh" }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::If(if_step) = &rf.targets["deploy"].commands[0] {
		assert_eq!(if_step.then.len(), 1);
		assert_eq!(if_step.then[0], "./deploy-prod.sh");
		assert!(if_step.r#else.is_none());
	} else {
		panic!("expected If block");
	}
}

#[test]
fn parse_if_block_else_as_string_shorthand() {
	let json = r#"{
		"$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"deploy": {
				"commands": [
					{ "if": "{{ ARG.dry-run }}", "then": "echo would deploy", "else": "./deploy.sh" }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::If(if_step) = &rf.targets["deploy"].commands[0] {
		assert_eq!(if_step.then.len(), 1);
		assert_eq!(if_step.then[0], "echo would deploy");
		let else_branch = if_step.r#else.as_ref().unwrap();
		assert_eq!(else_branch.len(), 1);
		assert_eq!(else_branch[0], "./deploy.sh");
	} else {
		panic!("expected If block");
	}
}

#[test]
fn parse_if_block_mixed_string_then_array_else() {
	// String `then` + array `else` and vice versa should both work side by side.
	let json = r#"{
		"$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"t": {
				"commands": [
					{ "if": "{{ ARG.x }}", "then": "echo a", "else": ["echo b", "echo c"] },
					{ "if": "{{ ARG.y }}", "then": ["echo d", "echo e"], "else": "echo f" }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let cmds = &rf.targets["t"].commands;
	if let CommandStep::If(s) = &cmds[0] {
		assert_eq!(s.then.len(), 1);
		assert_eq!(s.r#else.as_ref().unwrap().len(), 2);
	} else {
		panic!("expected If");
	}
	if let CommandStep::If(s) = &cmds[1] {
		assert_eq!(s.then.len(), 2);
		assert_eq!(s.r#else.as_ref().unwrap().len(), 1);
	} else {
		panic!("expected If");
	}
}

// ── `commands` as string shorthand ────────────────────────

#[test]
fn parse_target_commands_as_string_shorthand() {
	// A bare string in place of the `commands` array should be treated as a
	// one-element array containing a single shell step.
	let json = r#"{
		"$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"build": { "commands": "cargo build" }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let cmds = &rf.targets["build"].commands;
	assert_eq!(cmds.len(), 1);
	assert_eq!(cmds[0], "cargo build");
}

#[test]
fn parse_target_commands_string_shorthand_target_call() {
	// The `@target` shorthand applies even when `commands` is a string.
	let json = r#"{
		"$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"a": { "commands": "@b --release" },
			"b": { "commands": ["cargo build --release"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let cmds = &rf.targets["a"].commands;
	assert_eq!(cmds.len(), 1);
	if let CommandStep::TargetCall(call) = &cmds[0] {
		assert_eq!(call.target, "b");
		assert_eq!(call.args_template, "--release");
	} else {
		panic!("expected TargetCall, got {:?}", cmds[0]);
	}
}

#[test]
fn parse_when_step_commands_as_string_shorthand() {
	// `when:` blocks accept the same string shorthand for `commands`.
	let json = r#"{
		"$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"t": {
				"commands": [
					"./run-tests.sh",
					{ "when": "failure", "commands": "./report.sh" }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let cmds = &rf.targets["t"].commands;
	assert_eq!(cmds.len(), 2);
	if let CommandStep::When(w) = &cmds[1] {
		assert_eq!(w.when, WhenCondition::Failure);
		assert_eq!(w.commands.len(), 1);
		assert_eq!(w.commands[0], "./report.sh");
	} else {
		panic!("expected When, got {:?}", cmds[1]);
	}
}

#[test]
fn parse_target_commands_as_array_still_works() {
	// Adding the string shorthand must not break the existing array form.
	let json = r#"{
		"$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"build": { "commands": ["cargo build", "echo done"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let cmds = &rf.targets["build"].commands;
	assert_eq!(cmds.len(), 2);
	assert_eq!(cmds[0], "cargo build");
	assert_eq!(cmds[1], "echo done");
}

// ── `when:` block parsing ─────────────────────────────────

#[test]
fn parse_when_block_default_success() {
	let json = r#"{
		"$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"t": { "commands": [{ "commands": ["echo hi"] }] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::When(w) = &rf.targets["t"].commands[0] {
		assert_eq!(w.when, WhenCondition::Success);
		assert_eq!(w.commands.len(), 1);
	} else {
		panic!("expected When");
	}
}

#[test]
fn parse_when_block_failure() {
	let json = r#"{
		"$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"t": { "commands": [{ "when": "failure", "commands": ["./report.sh"] }] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::When(w) = &rf.targets["t"].commands[0] {
		assert_eq!(w.when, WhenCondition::Failure);
	} else {
		panic!("expected When");
	}
}

#[test]
fn parse_when_block_always() {
	let json = r#"{
		"$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"t": { "commands": [{ "when": "always", "commands": ["./cleanup.sh"] }] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::When(w) = &rf.targets["t"].commands[0] {
		assert_eq!(w.when, WhenCondition::Always);
	} else {
		panic!("expected When");
	}
}

#[test]
fn parse_when_on_if_block() {
	let json = r#"{
		"$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"t": { "commands": [
				{ "when": "always", "if": "{{ RUN.os }} == windows", "then": "rm -rf tmp" }
			] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::If(if_step) = &rf.targets["t"].commands[0] {
		assert_eq!(if_step.when, Some(WhenCondition::Always));
	} else {
		panic!("expected If");
	}
}

#[test]
fn parse_when_on_for_block() {
	let json = r#"{
		"$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"t": { "commands": [
				{ "when": "failure", "for": "f", "glob": "logs/*", "do": ["cat {{ VAR.f }}"] }
			] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::For(for_step) = &rf.targets["t"].commands[0] {
		assert_eq!(for_step.when, Some(WhenCondition::Failure));
	} else {
		panic!("expected For");
	}
}

#[test]
fn parse_when_block_with_ignore_errors() {
	let json = r#"{
		"$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"t": { "commands": [
				{ "when": "always", "commands": ["./cleanup.sh"], "ignoreErrors": true }
			] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::When(w) = &rf.targets["t"].commands[0] {
		assert_eq!(w.ignore_errors, Some(true));
	} else {
		panic!("expected When");
	}
}

#[test]
fn parse_when_block_rejects_empty_commands() {
	let json = r#"{
		"$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"t": { "commands": [{ "when": "always", "commands": [] }] }
		}
	}"#;
	assert!(parse_runfile(json).is_err());
}

#[test]
fn parse_when_block_rejects_unknown_when_value() {
	let json = r#"{
		"$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"t": { "commands": [{ "when": "sometimes", "commands": ["echo"] }] }
		}
	}"#;
	assert!(parse_runfile(json).is_err());
}
