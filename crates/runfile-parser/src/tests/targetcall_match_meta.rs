use super::*;

// ── @target invocation parsing ─────────────────────────────

#[test]
fn parse_target_call_no_args() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"build": { "commands": ["echo build"] },
			"ci": { "commands": ["@build"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::TargetCall(call) = &rf.targets["ci"].commands[0] {
		assert_eq!(call.target, "build");
		assert_eq!(call.args_template, "");
	} else {
		panic!("expected TargetCall");
	}
}

#[test]
fn parse_target_call_with_args() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"build": { "commands": ["echo build"] },
			"ci": { "commands": ["@build --release --features foo"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::TargetCall(call) = &rf.targets["ci"].commands[0] {
		assert_eq!(call.target, "build");
		assert_eq!(call.args_template, "--release --features foo");
	} else {
		panic!("expected TargetCall");
	}
}

#[test]
fn parse_target_call_with_args_substitution_template() {
	// {{ ARGS }} and {{ RUN.os }} are preserved in the args_template — substitution
	// happens at runtime.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"build": { "commands": ["echo build"] },
			"ci": { "commands": ["@build {{ ARGS }} --os={{ RUN.os }}"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::TargetCall(call) = &rf.targets["ci"].commands[0] {
		assert_eq!(call.target, "build");
		assert_eq!(call.args_template, "{{ ARGS }} --os={{ RUN.os }}");
	}
}

#[test]
fn parse_target_call_inside_if_branches() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"prod-deploy": { "commands": ["echo prod"] },
			"dev-deploy": { "commands": ["echo dev"] },
			"deploy": {
				"commands": [
					{ "if": "{{ ARG.env }} == production", "then": "@prod-deploy", "else": "@dev-deploy" }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::If(if_step) = &rf.targets["deploy"].commands[0] {
		assert!(matches!(&if_step.then[0], CommandStep::TargetCall(c) if c.target == "prod-deploy"));
		let else_branch = if_step.r#else.as_ref().unwrap();
		assert!(matches!(&else_branch[0], CommandStep::TargetCall(c) if c.target == "dev-deploy"));
	} else {
		panic!("expected If");
	}
}

#[test]
fn parse_target_call_inside_for_body() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"build": { "commands": ["echo build"] },
			"matrix": {
				"commands": [
					{ "for": "v", "in": ["1", "2"], "do": ["@build --version {{ VAR.v }}"] }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::For(for_step) = &rf.targets["matrix"].commands[0] {
		assert!(
			matches!(&for_step.body[0], CommandStep::TargetCall(c) if c.target == "build" && c.args_template == "--version {{ VAR.v }}")
		);
	} else {
		panic!("expected For");
	}
}

#[test]
fn parse_target_call_with_quoted_args() {
	// Quoted args are kept verbatim — shlex-splitting happens at execute time.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"echo": { "commands": ["echo {{ ARGS }}"] },
			"t": { "commands": ["@echo \"hello world\" foo"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::TargetCall(call) = &rf.targets["t"].commands[0] {
		assert_eq!(call.target, "echo");
		assert_eq!(call.args_template, "\"hello world\" foo");
	}
}

#[test]
fn parse_target_call_rejects_empty_target_name() {
	// `@` alone or `@ args` is rejected at parse time.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"bad": { "commands": ["@ foo"] }
		}
	}"#;
	let err = parse_runfile(json).unwrap_err().to_string();
	assert!(
		err.contains("@") || err.contains("target name"),
		"unexpected error: {err}"
	);
}

#[test]
fn parse_target_call_serializes_back_to_string() {
	// Round-trip: TargetCall serializes as the original `@target args` form.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"build": { "commands": ["echo build"] },
			"t": { "commands": ["@build --release"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let serialized = serde_json::to_string(&rf.targets["t"].commands).unwrap();
	assert!(
		serialized.contains("\"@build --release\""),
		"expected @build --release in {serialized}"
	);
}

#[test]
fn parse_plain_string_with_at_inside_is_shell_command() {
	// `email@host` (no leading @) is a plain shell command.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"t": { "commands": ["echo email@host"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	assert!(matches!(&rf.targets["t"].commands[0], CommandStep::Shell(s) if s == "echo email@host"));
}

#[test]
fn parse_if_block_string_then_rejects_object() {
	// A non-array, non-string `then` should still be a parse error.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"bad": { "commands": [
				{ "if": "{{ ARG.x }}", "then": { "if": "true", "then": [] } }
			] }
		}
	}"#;
	assert!(parse_runfile(json).is_err());
}

#[test]
fn parse_if_block_empty_then_allowed() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"noop": { "commands": [
				{ "if": "{{ ARG.x }}", "then": [] }
			] }
		}
	}"#;
	parse_runfile(json).unwrap();
}

#[test]
fn parse_if_rejects_empty_condition() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"bad": { "commands": [
				{ "if": "", "then": [] }
			] }
		}
	}"#;
	let err = parse_runfile(json).unwrap_err();
	let msg = err.to_string();
	assert!(msg.contains("Invalid condition") || msg.contains("Empty"), "got: {msg}");
}

#[test]
fn parse_if_accepts_arbitrary_condition_text() {
	// Under the new if-evaluation model the parser does NOT pre-parse the
	// condition as DSL — it's just a substitution template. Syntax errors
	// surface at runtime when the substitution machinery actually evaluates
	// the body. So the parser accepts any non-empty string here.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"bad": { "commands": [
				{ "if": "a && b || c", "then": [] }
			] }
		}
	}"#;
	let rf = parse_runfile(json).expect("parser should accept arbitrary if text");
	let cmd = &rf.targets["bad"].commands[0];
	match cmd {
		CommandStep::If(if_step) => {
			assert_eq!(if_step.condition, "a && b || c");
		}
		_ => panic!("expected If step"),
	}
}

#[test]
fn parse_for_in_block() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"build_each": {
				"commands": [
					{ "for": "service", "in": ["api", "web"], "do": ["echo {{ VAR.service }}"] }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::For(for_step) = &rf.targets["build_each"].commands[0] {
		assert_eq!(for_step.var, "service");
		assert_eq!(
			for_step.r#in.as_ref().unwrap(),
			&crate::ForInValue::Literal(vec!["api".to_string(), "web".to_string()])
		);
		assert_eq!(for_step.body.len(), 1);
	} else {
		panic!("expected For block");
	}
}

#[test]
fn parse_for_do_accepts_single_string() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"each": {
				"commands": [
					{ "for": "x", "in": ["a", "b"], "do": "echo {{ VAR.x }}" }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::For(for_step) = &rf.targets["each"].commands[0] {
		assert_eq!(for_step.body.len(), 1);
		assert_eq!(for_step.body[0], "echo {{ VAR.x }}");
	} else {
		panic!("expected For block");
	}
}

#[test]
fn parse_for_in_namespaces_magic_string() {
	// `"in": "namespaces"` is the only string form accepted — anything else
	// errors. Used to iterate over namespace prefixes from `includes`.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"build_all": {
				"commands": [
					{ "for": "ns", "in": "namespaces", "do": "@{{ VAR.ns }}:build" }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::For(for_step) = &rf.targets["build_all"].commands[0] {
		assert_eq!(for_step.var, "ns");
		assert_eq!(for_step.r#in.as_ref().unwrap(), &crate::ForInValue::Namespaces);
		// Body's "@{{ VAR.ns }}:build" string starts with @, so it parses as a target call
		// with an empty target (the namespace is filled in at runtime).
		assert_eq!(for_step.body.len(), 1);
	} else {
		panic!("expected For block");
	}
}

#[test]
fn parse_for_in_array_still_works_alongside_magic_string() {
	// Sanity: existing `in: [array]` form is unaffected by the new magic-string
	// path through `ForInValue`.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"each": {
				"commands": [
					{ "for": "x", "in": ["a", "b", "c"], "do": "echo {{ VAR.x }}" }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::For(for_step) = &rf.targets["each"].commands[0] {
		assert_eq!(
			for_step.r#in.as_ref().unwrap(),
			&crate::ForInValue::Literal(vec!["a".into(), "b".into(), "c".into()])
		);
	} else {
		panic!("expected For block");
	}
}

#[test]
fn parse_for_in_string_other_than_namespaces_errors() {
	// Only `"namespaces"` is a recognised string form — anything else is a
	// hard error to catch typos like `"namespace"` (singular).
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"bad": {
				"commands": [
					{ "for": "ns", "in": "namespace", "do": ["echo"] }
				]
			}
		}
	}"#;
	let err = parse_runfile(json).unwrap_err();
	let msg = err.to_string();
	assert!(
		msg.contains("namespaces") && msg.contains("namespace"),
		"error should call out the typo and the accepted keyword: {msg}"
	);
}

#[test]
fn parse_for_in_object_form_errors() {
	// Defensive: rejecting non-array/non-string `in` values with a clear message.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"bad": {
				"commands": [
					{ "for": "x", "in": { "a": 1 }, "do": [] }
				]
			}
		}
	}"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(err.to_string().contains("namespaces") || err.to_string().contains("array"));
}

#[test]
fn for_in_namespaces_roundtrips_through_serde() {
	// Serialize → deserialize must preserve the magic value (string form),
	// not collapse it into an array.
	let original = crate::ForInValue::Namespaces;
	let json = serde_json::to_value(&original).unwrap();
	assert_eq!(json, serde_json::json!("namespaces"));
	let parsed: crate::ForInValue = serde_json::from_value(json).unwrap();
	assert_eq!(parsed, original);

	// Literal also roundtrips cleanly.
	let literal = crate::ForInValue::Literal(vec!["a".into(), "b".into()]);
	let json = serde_json::to_value(&literal).unwrap();
	assert_eq!(json, serde_json::json!(["a", "b"]));
	let parsed: crate::ForInValue = serde_json::from_value(json).unwrap();
	assert_eq!(parsed, literal);
}

#[test]
fn parse_for_glob_block() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"fmt": {
				"commands": [
					{ "for": "f", "glob": "src/**/*.rs", "do": ["rustfmt {{ VAR.f }}"] }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::For(for_step) = &rf.targets["fmt"].commands[0] {
		assert_eq!(for_step.glob.as_deref(), Some("src/**/*.rs"));
	} else {
		panic!("expected For block");
	}
}

#[test]
fn parse_for_shell_block() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"check": {
				"commands": [
					{ "for": "f", "shell": "git diff --name-only", "do": ["echo {{ VAR.f }}"] }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::For(for_step) = &rf.targets["check"].commands[0] {
		assert_eq!(for_step.shell.as_deref(), Some("git diff --name-only"));
	} else {
		panic!("expected For block");
	}
}

#[test]
fn parse_for_rejects_no_iterator() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"bad": { "commands": [
				{ "for": "x", "do": ["echo {{ VAR.x }}"] }
			] }
		}
	}"#;
	let err = parse_runfile(json).unwrap_err();
	let msg = err.to_string();
	assert!(msg.contains("for") && msg.contains("none"), "got: {msg}");
}

#[test]
fn parse_for_rejects_multiple_iterators() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"bad": { "commands": [
				{ "for": "x", "in": ["a"], "glob": "*.rs", "do": [] }
			] }
		}
	}"#;
	let err = parse_runfile(json).unwrap_err();
	let msg = err.to_string();
	assert!(msg.contains("for") && (msg.contains("in") || msg.contains("glob")));
}

#[test]
fn parse_for_rejects_invalid_var_name() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"bad": { "commands": [
				{ "for": "1abc", "in": ["x"], "do": [] }
			] }
		}
	}"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(err.to_string().contains("loop variable"));
}

#[test]
fn parse_for_with_parallel_flag() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"par": {
				"commands": [
					{ "for": "x", "in": ["1","2","3"], "parallel": true, "do": ["sleep {{ VAR.x }}"] }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::For(for_step) = &rf.targets["par"].commands[0] {
		assert_eq!(for_step.parallel, Some(true));
	} else {
		panic!("expected For block");
	}
}

#[test]
fn parse_nested_control_flow() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"complex": {
				"commands": [
					{ "for": "svc", "in": ["api","web"], "do": [
						{ "if": "{{ VAR.svc }} == api", "then": [
							"echo building api",
							{ "for": "stage", "in": ["lint","test","build"], "do": ["echo api {{ VAR.stage }}"] }
						], "else": [
							"echo building {{ VAR.svc }}"
						] }
					] }
				]
			}
		}
	}"#;
	parse_runfile(json).unwrap();
}

#[test]
fn parse_unknown_control_flow_field_rejected() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"bad": { "commands": [
				{ "if": "{{ ARG.x }}", "then": [], "extraField": 1 }
			] }
		}
	}"#;
	assert!(parse_runfile(json).is_err());
}

#[test]
fn parse_object_without_if_or_for_rejected() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"bad": { "commands": [
				{ "foo": "bar" }
			] }
		}
	}"#;
	assert!(parse_runfile(json).is_err());
}

#[test]
fn parse_control_flow_inside_when_block() {
	// Lifecycle hooks were replaced with `when`-guarded blocks. A `before`
	// step's previous "run inline commands first" role is now just
	// prepending to `commands`; the always/failure-only cases use `when`.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"deploy": {
				"commands": [
					{ "if": "{{ ARG.skip-tests }}", "then": ["echo skipping"], "else": ["./test.sh"] },
					"echo deploying"
				]
			}
		}
	}"#;
	parse_runfile(json).unwrap();
}

#[test]
fn parse_backwards_compat_string_only_commands_still_works() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"build": { "commands": ["cargo build", "cargo test"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let cmds = &rf.targets["build"].commands;
	assert_eq!(cmds.len(), 2);
	assert!(matches!(cmds[0], CommandStep::Shell(ref s) if s == "cargo build"));
	assert!(matches!(cmds[1], CommandStep::Shell(ref s) if s == "cargo test"));
}

#[test]
fn walk_step_templates_visits_all_string_payloads() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"x": {
				"commands": [
					"echo top",
					{ "if": "{{ ARG.flag }}", "then": ["echo then1", "echo then2"], "else": ["echo else1"] },
					{ "for": "x", "in": ["a","b"], "do": ["echo {{ VAR.x }}"] },
					{ "for": "f", "glob": "*.rs", "do": ["rustfmt {{ VAR.f }}"] }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let mut seen: Vec<String> = Vec::new();
	walk_step_templates(&rf.targets["x"].commands, &mut |t| seen.push(t.to_string()));

	assert!(seen.contains(&"echo top".to_string()));
	assert!(seen.contains(&"{{ ARG.flag }}".to_string()));
	assert!(seen.contains(&"echo then1".to_string()));
	assert!(seen.contains(&"echo then2".to_string()));
	assert!(seen.contains(&"echo else1".to_string()));
	assert!(seen.contains(&"a".to_string()));
	assert!(seen.contains(&"b".to_string()));
	assert!(seen.contains(&"echo {{ VAR.x }}".to_string()));
	assert!(seen.contains(&"*.rs".to_string()));
	assert!(seen.contains(&"rustfmt {{ VAR.f }}".to_string()));
}

#[test]
fn walk_spec_aux_templates_visits_all_substitutable_fields() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"x": {
				"commands": ["echo go"],
				"env": {
					"A": "{{ ARG.a }}",
					"B": "{{ FLAG.b }}",
					"N": 42,
					"BL": true
				},
				"vars": {
					"V": "{{ ARG.vararg }}"
				},
				"envFiles": [".env.{{ RUN.os }}", ".env"],
				"forceShell": "{{ ARG.shell ? 'bash' }}",
				"addToPath": ["bin/{{ ARG.profile }}"],
				"workingDirectory": "{{ ARG.dir ? RUN.parent }}",
				"confirm": "Run with {{ ARG.env }}?",
				"extendStdio": [{ "fromFile": "logs/{{ RUN.os }}.log", "stream": "stdout" }]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let mut seen: Vec<String> = Vec::new();
	walk_spec_aux_templates(&rf.targets["x"], &mut |t| seen.push(t.to_string()));

	// commands array is intentionally NOT covered by this walker.
	assert!(!seen.iter().any(|s| s == "echo go"));

	// env values: only string variants are visited (numbers/bools have no templates).
	assert!(seen.iter().any(|s| s == "{{ ARG.a }}"));
	assert!(seen.iter().any(|s| s == "{{ FLAG.b }}"));
	assert!(!seen.iter().any(|s| s == "42"));
	assert!(!seen.iter().any(|s| s == "true"));

	// vars values are visited too, so arg-usage scanning sees their references.
	assert!(seen.iter().any(|s| s == "{{ ARG.vararg }}"));

	assert!(seen.iter().any(|s| s == ".env.{{ RUN.os }}"));
	assert!(seen.iter().any(|s| s == ".env"));
	assert!(seen.iter().any(|s| s == "{{ ARG.shell ? 'bash' }}"));
	assert!(seen.iter().any(|s| s == "bin/{{ ARG.profile }}"));
	assert!(seen.iter().any(|s| s == "{{ ARG.dir ? RUN.parent }}"));
	assert!(seen.iter().any(|s| s == "Run with {{ ARG.env }}?"));
	assert!(seen.iter().any(|s| s == "logs/{{ RUN.os }}.log"));
}

#[test]
fn parse_dsl_features_all_supported() {
	let conditions = [
		"{{ ARG.x }}",
		"{{ ARG.x }} == y",
		"{{ ARG.x }} != y",
		"a == b && c == d",
		"a == b || c == d",
		"!a",
		"!!a",
		"!(a == b)",
		"(a && b) || c",
		"a || (b && c)",
		"{{ ARG.x ? 'default' }} == foo",
		"{{ ENV.HOME }} != \"\"",
	];
	for c in conditions {
		let escaped = c.replace('\\', "\\\\").replace('"', "\\\"");
		let json = format!(
			r#"{{ "$schema": "x", "targets": {{ "t": {{ "commands": [
				{{ "if": "{escaped}", "then": [] }}
			] }} }} }}"#
		);
		parse_runfile(&json).unwrap_or_else(|e| panic!("Failed to parse condition `{c}`: {e}"));
	}
}

#[test]
fn parse_optional_target_call_marker() {
	// `@?target` parses with optional = true; the `?` is stripped from the
	// in-memory target name.
	let json = r#"{
		"$schema": "x",
		"targets": {
			"a": { "commands": ["@?b --release"] },
			"b": { "commands": ["echo b"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::TargetCall(call) = &rf.targets["a"].commands[0] {
		assert_eq!(call.target, "b");
		assert_eq!(call.args_template, "--release");
		assert!(call.optional);
	} else {
		panic!("expected TargetCall");
	}
}

#[test]
fn parse_optional_target_call_with_dynamic_name() {
	// `@?{{ VAR.ns }}:build` is the canonical use case — combine optional with
	// runtime substitution. The `?` is stripped, leaving the substitutable name.
	let json = r#"{
		"$schema": "x",
		"targets": {
			"a": {
				"commands": [
					{ "for": "ns", "in": "namespaces", "do": "@?{{ VAR.ns }}:build" }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::For(for_step) = &rf.targets["a"].commands[0] {
		if let CommandStep::TargetCall(call) = &for_step.body[0] {
			assert_eq!(call.target, "{{ VAR.ns }}:build");
			assert!(call.optional);
		} else {
			panic!("expected TargetCall in for body");
		}
	} else {
		panic!("expected For");
	}
}

#[test]
fn parse_optional_target_call_no_args() {
	let json = r#"{
		"$schema": "x",
		"targets": {
			"a": { "commands": ["@?b"] },
			"b": { "commands": ["echo b"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::TargetCall(call) = &rf.targets["a"].commands[0] {
		assert_eq!(call.target, "b");
		assert_eq!(call.args_template, "");
		assert!(call.optional);
	} else {
		panic!("expected TargetCall");
	}
}

#[test]
fn parse_optional_target_call_empty_name_errors() {
	let json = r#"{
		"$schema": "x",
		"targets": {
			"a": { "commands": ["@?"] }
		}
	}"#;
	let err = parse_runfile(json).unwrap_err().to_string();
	assert!(err.contains("@?") || err.contains("target name"), "got: {err}");
}

#[test]
fn parse_non_optional_target_call_has_optional_false() {
	// Plain `@target` should leave optional = false (not the new opt-in).
	let json = r#"{
		"$schema": "x",
		"targets": {
			"a": { "commands": ["@b"] },
			"b": { "commands": ["echo"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::TargetCall(call) = &rf.targets["a"].commands[0] {
		assert!(!call.optional);
	} else {
		panic!("expected TargetCall");
	}
}

#[test]
fn parse_optional_target_call_serializes_back_with_question_mark() {
	// Round-trip: the `@?` marker must survive a serialize → deserialize cycle.
	let json = r#"{
		"$schema": "x",
		"targets": {
			"a": { "commands": ["@?b --opt"] },
			"b": { "commands": ["echo"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let serialized = serde_json::to_string(&rf).unwrap();
	assert!(serialized.contains("@?b --opt"), "serialized: {serialized}");
	// And re-parsing produces the same step.
	let rf2 = parse_runfile(&serialized).unwrap();
	if let CommandStep::TargetCall(call) = &rf2.targets["a"].commands[0] {
		assert_eq!(call.target, "b");
		assert_eq!(call.args_template, "--opt");
		assert!(call.optional);
	} else {
		panic!("expected TargetCall");
	}
}

#[test]
fn target_name_with_question_mark_rejected() {
	// `?` is reserved for the `@?target` optional-call marker, so a declared
	// target name containing `?` must be rejected.
	let json = r#"{
		"$schema": "x",
		"targets": {
			"foo?bar": { "commands": ["echo"] }
		}
	}"#;
	let err = parse_runfile(json).unwrap_err().to_string();
	assert!(err.contains("?"), "got: {err}");
}

#[test]
fn alias_with_question_mark_rejected() {
	let json = r#"{
		"$schema": "x",
		"targets": {
			"a": { "commands": ["echo"], "aliases": ["a?b"] }
		}
	}"#;
	let err = parse_runfile(json).unwrap_err().to_string();
	assert!(err.contains("?"), "got: {err}");
}

#[test]
fn target_call_with_question_mark_in_name_rejected() {
	// `@foo?bar` is parsed as `@<foo?bar>` (no leading `?`), and `foo?bar`
	// then fails validation because the target name contains `?`.
	let json = r#"{
		"$schema": "x",
		"targets": {
			"a": { "commands": ["@foo?bar"] }
		}
	}"#;
	let err = parse_runfile(json).unwrap_err().to_string();
	assert!(err.contains("?"), "got: {err}");
}

// ── Match step tests ──────────────────────────────────────────────

#[test]
fn parse_match_block() {
	let json = r#"{
		"$schema": "x",
		"targets": {
			"emulate": {
				"commands": [
					{
						"match": "{{ ARG.tier }}",
						"cases": {
							"1": "echo tier 1",
							"2": ["echo tier 2", "echo two"]
						}
					}
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::Match(m) = &rf.targets["emulate"].commands[0] {
		assert_eq!(m.r#match, "{{ ARG.tier }}");
		assert_eq!(m.cases.len(), 2);
		assert_eq!(m.cases["1"], vec![CommandStep::shell("echo tier 1")]);
		assert_eq!(
			m.cases["2"],
			vec![CommandStep::shell("echo tier 2"), CommandStep::shell("echo two")]
		);
		assert!(m.default.is_none());
	} else {
		panic!("expected Match block");
	}
}

#[test]
fn parse_match_with_default_and_target_call() {
	let json = r#"{
		"$schema": "x",
		"targets": {
			"a": { "commands": ["echo a"] },
			"dispatch": {
				"commands": [
					{
						"match": "{{ ARG.mode ? 'prod' }}",
						"cases": {
							"prod": "@a",
							"dev": ["echo dev"]
						},
						"default": "echo unknown"
					}
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::Match(m) = &rf.targets["dispatch"].commands[0] {
		assert_eq!(m.r#match, "{{ ARG.mode ? 'prod' }}");
		// String case "prod" parsed as `@a` → TargetCall.
		assert!(matches!(&m.cases["prod"][0], CommandStep::TargetCall(c) if c.target == "a"));
		let default = m.default.as_ref().expect("default should be set");
		assert_eq!(default, &vec![CommandStep::shell("echo unknown")]);
	} else {
		panic!("expected Match block");
	}
}

#[test]
fn parse_match_when_and_ignore_errors() {
	let json = r#"{
		"$schema": "x",
		"targets": {
			"t": {
				"commands": [
					{
						"match": "{{ ARG.x }}",
						"cases": { "a": "echo a" },
						"when": "always",
						"ignoreErrors": true
					}
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::Match(m) = &rf.targets["t"].commands[0] {
		assert_eq!(m.when, Some(WhenCondition::Always));
		assert_eq!(m.ignore_errors, Some(true));
	} else {
		panic!("expected Match block");
	}
}

#[test]
fn parse_match_empty_match_expression_rejected() {
	let json = r#"{
		"$schema": "x",
		"targets": {
			"t": {
				"commands": [
					{ "match": "", "cases": { "a": "echo a" } }
				]
			}
		}
	}"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(matches!(err, ParseError::EmptyMatchExpression(_)), "got: {err:?}");
}

#[test]
fn parse_match_no_cases_no_default_rejected() {
	let json = r#"{
		"$schema": "x",
		"targets": {
			"t": {
				"commands": [
					{ "match": "{{ ARG.x }}", "cases": {} }
				]
			}
		}
	}"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(matches!(err, ParseError::EmptyMatchCases(_)), "got: {err:?}");
}

#[test]
fn parse_match_default_only_is_allowed() {
	// Edge case: an empty `cases` map paired with a `default` is essentially
	// "always run default" — silly but not invalid.
	let json = r#"{
		"$schema": "x",
		"targets": {
			"t": {
				"commands": [
					{ "match": "{{ ARG.x ? 'y' }}", "cases": {}, "default": "echo y" }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::Match(m) = &rf.targets["t"].commands[0] {
		assert!(m.cases.is_empty());
		assert!(m.default.is_some());
	} else {
		panic!("expected Match block");
	}
}

#[test]
fn parse_match_unknown_field_rejected() {
	// deny_unknown_fields applies — typos should fail loudly.
	let json = r#"{
		"$schema": "x",
		"targets": {
			"t": {
				"commands": [
					{ "match": "{{ ARG.x }}", "cases": { "a": "echo a" }, "extra": true }
				]
			}
		}
	}"#;
	assert!(parse_runfile(json).is_err());
}

#[test]
fn parse_match_round_trips_through_serde() {
	// Parse → serialize → parse must produce the same tree.
	let json = r#"{
		"$schema": "x",
		"targets": {
			"t": {
				"commands": [
					{
						"match": "{{ ARG.x }}",
						"cases": { "a": "echo a", "b": ["echo b1", "echo b2"] },
						"default": "echo other"
					}
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let serialized = serde_json::to_string(&rf).unwrap();
	assert!(
		serialized.contains("\"match\":\"{{ ARG.x }}\""),
		"serialized: {serialized}"
	);
	let rf2 = parse_runfile(&serialized).unwrap();
	assert_eq!(rf.targets["t"].commands, rf2.targets["t"].commands);
}

#[test]
fn match_walks_templates_inside_cases_and_default() {
	// `walk_step_templates` should visit the match template, every case body,
	// and the default body so static analysis (arg-usage scanning) sees
	// `{{ ARG.* }}` references inside them.
	let json = r#"{
		"$schema": "x",
		"targets": {
			"t": {
				"commands": [
					{
						"match": "{{ ARG.tier }}",
						"cases": { "1": "echo {{ ARG.foo }}" },
						"default": "echo {{ ARG.bar }}"
					}
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let mut seen: Vec<String> = Vec::new();
	walk_step_templates(&rf.targets["t"].commands, &mut |t| seen.push(t.to_string()));
	assert!(seen.iter().any(|s| s == "{{ ARG.tier }}"), "saw: {seen:?}");
	assert!(seen.iter().any(|s| s == "echo {{ ARG.foo }}"), "saw: {seen:?}");
	assert!(seen.iter().any(|s| s == "echo {{ ARG.bar }}"), "saw: {seen:?}");
}

// ── Metadata field tests ──────────────────────────────────────────

#[test]
fn parse_target_with_metadata() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"build": {
				"commands": ["cargo build"],
				"metadata": { "excludeFromGenerateCommand": true }
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let spec = &rf.targets["build"];
	let meta = spec.metadata.as_ref().expect("metadata present");
	assert_eq!(meta.exclude_from_generate_command, Some(true));
	assert!(spec.is_excluded_from_generate());
}

#[test]
fn parse_globals_with_metadata() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": { "x": { "commands": ["echo"] } },
		"globals": { "metadata": { "excludeFromGenerateCommand": false } }
	}"#;
	let rf = parse_runfile(json).unwrap();
	let globals = rf.globals.as_ref().unwrap();
	let meta = globals.metadata.as_ref().unwrap();
	assert_eq!(meta.exclude_from_generate_command, Some(false));
}

#[test]
fn metadata_preserves_unknown_keys() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"build": {
				"commands": ["cargo build"],
				"metadata": { "owner": "team-platform", "tags": ["ci", "fast"] }
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let meta = rf.targets["build"].metadata.as_ref().unwrap();
	assert_eq!(meta.exclude_from_generate_command, None);
	assert_eq!(meta.extra.get("owner"), Some(&serde_json::json!("team-platform")));
	assert_eq!(meta.extra.get("tags"), Some(&serde_json::json!(["ci", "fast"])));
}

#[test]
fn metadata_accepts_any_property_with_any_value_type() {
	// Metadata is a fully open object — any key, any JSON value type
	// (including deeply nested objects and mixed-type arrays) round-trips
	// untouched. Editor extensions, CI scripts, and other tooling can stash
	// arbitrary fields here without parser errors.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"build": {
				"commands": ["cargo build"],
				"metadata": {
					"string": "hello",
					"number": 42,
					"float": 1.5,
					"boolean": true,
					"null_value": null,
					"array": [1, "two", false, null],
					"nested": { "deep": { "deeper": { "value": 1 } } },
					"mixed_array": [ { "k": "v" }, [1, 2, 3] ]
				}
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let meta = rf.targets["build"].metadata.as_ref().unwrap();
	assert_eq!(meta.exclude_from_generate_command, None);
	assert_eq!(meta.extra.get("string"), Some(&serde_json::json!("hello")));
	assert_eq!(meta.extra.get("number"), Some(&serde_json::json!(42)));
	assert_eq!(meta.extra.get("float"), Some(&serde_json::json!(1.5)));
	assert_eq!(meta.extra.get("boolean"), Some(&serde_json::json!(true)));
	assert_eq!(meta.extra.get("null_value"), Some(&serde_json::json!(null)));
	assert_eq!(
		meta.extra.get("nested"),
		Some(&serde_json::json!({ "deep": { "deeper": { "value": 1 } } }))
	);
	assert_eq!(
		meta.extra.get("mixed_array"),
		Some(&serde_json::json!([ { "k": "v" }, [1, 2, 3] ]))
	);
}

#[test]
fn target_default_not_excluded_from_generate() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": { "build": { "commands": ["cargo build"] } }
	}"#;
	let rf = parse_runfile(json).unwrap();
	assert!(!rf.targets["build"].is_excluded_from_generate());
}

#[test]
fn merge_metadata_globals_into_target_target_wins() {
	// Globals say excludeFromGenerateCommand=true; target overrides to false.
	let dir = TempDir::new().unwrap();
	let path = dir.path().join(RUNFILE_NAME);
	std::fs::write(
		&path,
		r#"{
			"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
			"globals": {
				"metadata": { "excludeFromGenerateCommand": true, "owner": "team-A" }
			},
			"targets": {
				"build": {
					"commands": ["cargo build"],
					"metadata": { "excludeFromGenerateCommand": false, "owner": "team-B" }
				},
				"test": {
					"commands": ["cargo test"]
				}
			}
		}"#,
	)
	.unwrap();
	let runfile = parse_runfile_from_path(&path).unwrap();
	let result = merge_runfiles(Some((runfile, path)), &[], dir.path()).unwrap();

	let build = result.runfile.targets["build"].metadata.as_ref().unwrap();
	assert_eq!(
		build.exclude_from_generate_command,
		Some(false),
		"target wins over globals"
	);
	assert_eq!(build.extra.get("owner"), Some(&serde_json::json!("team-B")));
	assert!(!result.runfile.targets["build"].is_excluded_from_generate());

	let test = result.runfile.targets["test"].metadata.as_ref().unwrap();
	assert_eq!(
		test.exclude_from_generate_command,
		Some(true),
		"global value reaches target with no own metadata"
	);
	assert_eq!(test.extra.get("owner"), Some(&serde_json::json!("team-A")));
	assert!(result.runfile.targets["test"].is_excluded_from_generate());
}

#[test]
fn merge_metadata_no_globals_keeps_target_value() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join(RUNFILE_NAME);
	std::fs::write(
		&path,
		r#"{
			"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
			"targets": {
				"build": {
					"commands": ["cargo build"],
					"metadata": { "excludeFromGenerateCommand": true }
				}
			}
		}"#,
	)
	.unwrap();
	let runfile = parse_runfile_from_path(&path).unwrap();
	let result = merge_runfiles(Some((runfile, path)), &[], dir.path()).unwrap();

	let build = result.runfile.targets["build"].metadata.as_ref().unwrap();
	assert_eq!(build.exclude_from_generate_command, Some(true));
}
