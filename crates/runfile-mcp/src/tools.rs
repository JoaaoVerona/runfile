use runfile_parser::{CommandSpec, EnvValue, Runfile, is_internal_target_name};
use serde::{Deserialize, Serialize};

/// A serializable tool definition for --inspect output.
/// This is our own type, decoupled from rmcp's Tool struct.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
	pub name: String,
	pub description: String,
	#[serde(rename = "inputSchema")]
	pub input_schema: serde_json::Value,
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
			// Shared with the executor's argument validation (runfile-parser), so a
			// target this schema describes as taking no input is one the CLI would
			// also refuse arguments for.
			let scan = runfile_parser::scan_arg_usage(&strings);
			let has_any_args = scan.uses_positional || !scan.arg_keys.is_empty() || !scan.flag_keys.is_empty();

			let input_schema = if !has_any_args {
				serde_json::json!({
					"type": "object",
					"properties": {}
				})
			} else {
				let mut properties = serde_json::Map::new();
				let mut required: Vec<String> = Vec::new();

				// Named string arguments from {{ ARG.key }} patterns
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

				// Boolean flags from {{ FLAG.key }} patterns (skip if already in arg_keys)
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

				// Positional args array for {{ ARGS }} usage
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
