use runfile_parser::{is_internal_target_name, CommandSpec, EnvValue, Runfile};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// A serializable tool definition for --inspect output.
/// This is our own type, decoupled from rmcp's Tool struct.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
	pub name: String,
	pub description: String,
	#[serde(rename = "inputSchema")]
	pub input_schema: serde_json::Value,
}

/// Result of scanning a target's commands for argument patterns.
struct ArgScan {
	/// True if any command uses `$(ARGS)` (bare positional).
	uses_positional: bool,
	/// Keys from `$(ARGS.key)` patterns (string-valued named arguments).
	arg_keys: HashSet<String>,
	/// Keys from `$(FLAGS.key)` patterns (boolean flags).
	flag_keys: HashSet<String>,
	/// Keys that appear without a `?` default (required arguments).
	required_keys: HashSet<String>,
}

/// Collect all strings from a CommandSpec that could contain argument placeholders.
/// Walks `commands` (including nested if/for/when/@target) and env value strings.
fn collect_scannable_strings(spec: &CommandSpec) -> Vec<String> {
	let mut strings = Vec::new();
	runfile_parser::walk_step_templates(&spec.commands, &mut |t| strings.push(t.to_string()));
	collect_env_strings(&spec.env, &mut strings);
	strings
}

/// Collect string values from an optional env map.
fn collect_env_strings(env: &Option<std::collections::HashMap<String, EnvValue>>, out: &mut Vec<String>) {
	if let Some(env) = env {
		for val in env.values() {
			if let EnvValue::String(s) = val {
				out.push(s.clone());
			}
		}
	}
}

/// Scan strings for `$(ARGS)`, `$(ARGS.key)`, and `$(FLAGS.key)` patterns.
fn scan_arg_patterns(strings: &[String]) -> ArgScan {
	let mut scan = ArgScan {
		uses_positional: false,
		arg_keys: HashSet::new(),
		flag_keys: HashSet::new(),
		required_keys: HashSet::new(),
	};

	for s in strings {
		let mut chars = s.chars().peekable();
		while let Some(ch) = chars.next() {
			if ch == '$' && chars.peek() == Some(&'(') {
				chars.next(); // consume '('
				let mut expr = String::new();
				let mut depth = 1;
				for c in chars.by_ref() {
					if c == '(' {
						depth += 1;
					} else if c == ')' {
						depth -= 1;
						if depth == 0 {
							break;
						}
					}
					expr.push(c);
				}

				let trimmed = expr.trim();
				if trimmed == "ARGS" {
					scan.uses_positional = true;
				} else if let Some(rest) = trimmed.strip_prefix("ARGS.") {
					let has_default = rest.contains('?');
					let key = rest.split('?').next().unwrap_or("").trim();
					if !key.is_empty() {
						scan.arg_keys.insert(key.to_string());
						if !has_default {
							scan.required_keys.insert(key.to_string());
						}
					}
				} else if let Some(rest) = trimmed.strip_prefix("FLAGS.") {
					let key = rest.split('?').next().unwrap_or("").trim();
					if !key.is_empty() {
						scan.flag_keys.insert(key.to_string());
					}
				}
			}
		}
	}

	scan
}

/// Build tool definitions for all targets in a Runfile.
///
/// Security: env_files, env, and other sensitive fields are intentionally
/// excluded from the output.
pub fn build_tool_defs(runfile: &Runfile) -> Vec<ToolDef> {
	let mut target_names: Vec<&String> = runfile.targets.keys().filter(|n| !is_internal_target_name(n)).collect();
	target_names.sort();

	target_names
		.iter()
		.map(|name| {
			let spec = &runfile.targets[*name];

			let description = spec
				.description
				.clone()
				.unwrap_or_else(|| format!("Run the \"{name}\" target"));

			let strings = collect_scannable_strings(spec);
			let scan = scan_arg_patterns(&strings);
			let has_any_args = scan.uses_positional || !scan.arg_keys.is_empty() || !scan.flag_keys.is_empty();

			let input_schema = if !has_any_args {
				serde_json::json!({
					"type": "object",
					"properties": {}
				})
			} else {
				let mut properties = serde_json::Map::new();
				let mut required: Vec<String> = Vec::new();

				// Named string arguments from $(ARGS.key) patterns
				let mut sorted_args: Vec<&String> = scan.arg_keys.iter().collect();
				sorted_args.sort();
				for key in sorted_args {
					properties.insert(
						key.clone(),
						serde_json::json!({
							"type": "string",
							"description": format!("Value for the --{key} argument")
						}),
					);
					if scan.required_keys.contains(key) {
						required.push(key.clone());
					}
				}

				// Boolean flags from $(FLAGS.key) patterns (skip if already in arg_keys)
				let mut sorted_flags: Vec<&String> =
					scan.flag_keys.iter().filter(|k| !scan.arg_keys.contains(*k)).collect();
				sorted_flags.sort();
				for key in sorted_flags {
					properties.insert(
						key.clone(),
						serde_json::json!({
							"type": "boolean",
							"description": format!("Enable the --{key} flag")
						}),
					);
				}

				// Positional args array for $(ARGS) usage
				if scan.uses_positional {
					properties.insert(
						"args".to_string(),
						serde_json::json!({
							"type": "array",
							"items": { "type": "string" },
							"description": "Additional positional arguments"
						}),
					);
				}

				let mut schema = serde_json::json!({
					"type": "object",
					"properties": serde_json::Value::Object(properties)
				});

				if !required.is_empty() {
					required.sort();
					schema
						.as_object_mut()
						.unwrap()
						.insert("required".to_string(), serde_json::json!(required));
				}

				schema
			};

			ToolDef {
				name: name.to_string(),
				description,
				input_schema,
			}
		})
		.collect()
}

/// Serialize tool definitions as pretty JSON for --inspect output.
pub fn inspect_json(runfile: &Runfile) -> String {
	let tools = build_tool_defs(runfile);
	serde_json::to_string_pretty(&tools).expect("tool defs are always serializable")
}
