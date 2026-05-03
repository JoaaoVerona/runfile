use crate::args::{check_env_case_duplicates, validate_args, LoopScope, RunArgs, SubstitutionError};
use crate::env::{build_env, EnvFileError};
use runfile_parser::{
	walk_spec_aux_templates, walk_step_templates, CommandSpec, CommandStep, ForStep, IfStep, Runfile, WhenStep,
	WORKING_DIRECTORY_CWD,
};
use runfile_shell::ShellKind;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExtractError {
	#[error("Dependency cycle detected: {0}")]
	CycleDetected(String),

	#[error("Unknown target \"{0}\" referenced in lifecycle")]
	UnknownTarget(String),

	#[error("{0}")]
	Substitution(#[from] SubstitutionError),

	#[error("{0}")]
	EnvFile(#[from] EnvFileError),
}

/// A single extracted command line, ready to be printed.
#[derive(Debug, Clone)]
pub struct ExtractedCommand {
	/// The command string with env vars already substituted.
	pub command: String,
	/// Non-system env vars that should be set for this command.
	pub env_vars: Vec<(String, String)>,
}

/// A group of extracted commands belonging to a single target.
/// Used by dry-run mode to show commands grouped by target in dependency order.
#[derive(Debug, Clone)]
pub struct ExtractedTarget {
	/// The target name.
	pub target_name: String,
	/// The commands that would be executed for this target.
	pub commands: Vec<ExtractedCommand>,
}

/// Extract all commands that would be executed for a target, including dependencies.
/// Returns the commands in execution order, with env vars inlined.
pub fn extract_target(
	target_name: &str,
	runfile: &Runfile,
	args: &RunArgs,
	working_dir: &Path,
) -> Result<Vec<ExtractedCommand>, ExtractError> {
	extract_target_with_cwd(target_name, runfile, args, working_dir, working_dir, &HashMap::new())
}

/// Extract all commands for a target with separate runfile dir and caller CWD.
/// `source_dirs` maps target names to their source Runfile's parent directory.
pub fn extract_target_with_cwd(
	target_name: &str,
	runfile: &Runfile,
	args: &RunArgs,
	runfile_dir: &Path,
	caller_cwd: &Path,
	source_dirs: &HashMap<String, PathBuf>,
) -> Result<Vec<ExtractedCommand>, ExtractError> {
	let all_commands = collect_all_extract_commands(target_name, runfile)?;
	validate_args(args, &all_commands)?;

	let mut ctx = ExtractContext {
		runfile,
		args,
		runfile_dir,
		caller_cwd,
		source_dirs,
		completed: HashSet::new(),
		in_progress: HashSet::new(),
	};
	extract_recursive(&mut ctx, target_name)
}

/// Extract all commands grouped by target, in dependency execution order.
/// Used by dry-run mode to show which commands belong to which target.
pub fn extract_target_grouped(
	target_name: &str,
	runfile: &Runfile,
	args: &RunArgs,
	runfile_dir: &Path,
	caller_cwd: &Path,
	source_dirs: &HashMap<String, PathBuf>,
) -> Result<Vec<ExtractedTarget>, ExtractError> {
	let all_commands = collect_all_extract_commands(target_name, runfile)?;
	validate_args(args, &all_commands)?;

	let mut ctx = ExtractContext {
		runfile,
		args,
		runfile_dir,
		caller_cwd,
		source_dirs,
		completed: HashSet::new(),
		in_progress: HashSet::new(),
	};
	extract_recursive_grouped(&mut ctx, target_name)
}

fn extract_recursive_grouped(
	ctx: &mut ExtractContext<'_>,
	target_name: &str,
) -> Result<Vec<ExtractedTarget>, ExtractError> {
	if ctx.completed.contains(target_name) {
		return Ok(Vec::new());
	}

	if !ctx.in_progress.insert(target_name.to_string()) {
		return Err(ExtractError::CycleDetected(target_name.to_string()));
	}

	let spec = ctx
		.runfile
		.targets
		.get(target_name)
		.ok_or_else(|| ExtractError::UnknownTarget(target_name.to_string()))?;

	let mut all_targets = Vec::new();

	let target_runfile_dir = ctx.target_dir(target_name);
	// Extract is a static-analysis path; we compare the *unsubstituted* string
	// to detect the cwd mode. Fully resolving substitutions would require
	// runtime state we don't have here. Anything that doesn't textually equal
	// "cwd" falls back to the runfile parent.
	let effective_working_dir = match spec.working_directory.as_deref() {
		Some(s) if s == WORKING_DIRECTORY_CWD => ctx.caller_cwd,
		_ => target_runfile_dir,
	};

	let cmds = extract_commands(spec, ctx.args, effective_working_dir)?;

	all_targets.push(ExtractedTarget {
		target_name: target_name.to_string(),
		commands: cmds,
	});

	ctx.completed.insert(target_name.to_string());
	ctx.in_progress.remove(target_name);

	Ok(all_targets)
}

struct ExtractContext<'a> {
	runfile: &'a Runfile,
	args: &'a RunArgs,
	runfile_dir: &'a Path,
	caller_cwd: &'a Path,
	source_dirs: &'a HashMap<String, PathBuf>,
	completed: HashSet<String>,
	in_progress: HashSet<String>,
}

impl ExtractContext<'_> {
	fn target_dir(&self, target_name: &str) -> &Path {
		self.source_dirs
			.get(target_name)
			.map(|p| p.as_path())
			.unwrap_or(self.runfile_dir)
	}
}

fn extract_recursive(ctx: &mut ExtractContext<'_>, target_name: &str) -> Result<Vec<ExtractedCommand>, ExtractError> {
	if ctx.completed.contains(target_name) {
		return Ok(Vec::new());
	}

	if !ctx.in_progress.insert(target_name.to_string()) {
		return Err(ExtractError::CycleDetected(target_name.to_string()));
	}

	let spec = ctx
		.runfile
		.targets
		.get(target_name)
		.ok_or_else(|| ExtractError::UnknownTarget(target_name.to_string()))?;

	let mut all_commands = Vec::new();

	let target_runfile_dir = ctx.target_dir(target_name);
	// Extract is a static-analysis path; we compare the *unsubstituted* string
	// to detect the cwd mode. Fully resolving substitutions would require
	// runtime state we don't have here. Anything that doesn't textually equal
	// "cwd" falls back to the runfile parent.
	let effective_working_dir = match spec.working_directory.as_deref() {
		Some(s) if s == WORKING_DIRECTORY_CWD => ctx.caller_cwd,
		_ => target_runfile_dir,
	};

	let mut cmds = extract_commands(spec, ctx.args, effective_working_dir)?;
	all_commands.append(&mut cmds);

	ctx.completed.insert(target_name.to_string());
	ctx.in_progress.remove(target_name);

	Ok(all_commands)
}

/// Extract commands from a single target spec (no dependency resolution).
///
/// Static analysis, not a runtime preview: `if` blocks emit both branches,
/// `for in` blocks expand each literal iteration with `$(LOOP.var)` resolved,
/// and `for glob` / `for shell` blocks emit the body once with the loop
/// variable bound to a `<var>` placeholder (we don't touch the filesystem
/// or run iterator commands during extract).
fn extract_commands(
	spec: &CommandSpec,
	args: &RunArgs,
	working_dir: &Path,
) -> Result<Vec<ExtractedCommand>, ExtractError> {
	let env = build_env(spec, working_dir, args, None)?;
	check_env_case_duplicates(&env)?;

	// Show only the spec-defined env keys (not envFiles or system env), but
	// pull the resolved values from the fully-built env so `$(FLAGS.x)`,
	// `$(ARGS.x)`, `$(ENV.x)`, etc. references are substituted instead of
	// printed literally.
	let extra_env: Vec<(String, String)> = if let Some(spec_env) = &spec.env {
		let mut pairs: Vec<(String, String)> = spec_env
			.keys()
			.filter_map(|k| env.get(k).map(|v| (k.clone(), v.clone())))
			.collect();
		pairs.sort_by(|a, b| a.0.cmp(&b.0));
		pairs
	} else {
		Vec::new()
	};

	let mut leaf_commands: Vec<String> = Vec::new();
	let mut loop_scope = LoopScope::new();
	walk_extract_steps(&spec.commands, args, &env, &mut loop_scope, &mut leaf_commands)?;

	Ok(leaf_commands
		.into_iter()
		.map(|cmd| ExtractedCommand {
			command: cmd,
			env_vars: extra_env.clone(),
		})
		.collect())
}

/// Recursive walker that produces extract output with loop-scope awareness.
///
/// Iterator source templates (the `in` array elements, `glob` pattern, `shell`
/// command) are NOT emitted as commands — they're metadata. Only shell-leaf
/// command strings and target-call arg templates contribute to the output.
fn walk_extract_steps(
	steps: &[CommandStep],
	args: &RunArgs,
	env: &HashMap<String, String>,
	loop_scope: &mut LoopScope,
	out: &mut Vec<String>,
) -> Result<(), ExtractError> {
	for step in steps {
		match step {
			CommandStep::Shell(template) => {
				out.push(args.substitute_with_loop(template, env, loop_scope)?);
			}
			CommandStep::TargetCall(call) => {
				if !call.args_template.is_empty() {
					out.push(args.substitute_with_loop(&call.args_template, env, loop_scope)?);
				}
			}
			CommandStep::When(WhenStep { commands, .. }) => {
				walk_extract_steps(commands, args, env, loop_scope, out)?;
			}
			CommandStep::If(IfStep { then, r#else, .. }) => {
				walk_extract_steps(then, args, env, loop_scope, out)?;
				if let Some(else_steps) = r#else {
					walk_extract_steps(else_steps, args, env, loop_scope, out)?;
				}
			}
			CommandStep::For(ForStep { var, r#in, body, .. }) => {
				if let Some(items) = r#in {
					for item in items {
						let value = args.substitute_with_loop(item, env, loop_scope)?;
						loop_scope.push(var.as_str(), value);
						let r = walk_extract_steps(body, args, env, loop_scope, out);
						loop_scope.pop();
						r?;
					}
				} else {
					// `for glob` / `for shell` — bind a placeholder so
					// `$(LOOP.var)` references resolve without touching the
					// filesystem or running side-effecting iterator commands.
					loop_scope.push(var.as_str(), format!("<{var}>"));
					let r = walk_extract_steps(body, args, env, loop_scope, out);
					loop_scope.pop();
					r?;
				}
			}
		}
	}
	Ok(())
}

/// Format extracted commands as shell-native lines ready to execute.
pub fn format_extracted_commands(commands: &[ExtractedCommand], shell_kind: &ShellKind) -> Vec<String> {
	commands
		.iter()
		.map(|cmd| format_single_command(cmd, shell_kind))
		.collect()
}

fn format_single_command(cmd: &ExtractedCommand, shell_kind: &ShellKind) -> String {
	if cmd.env_vars.is_empty() {
		return cmd.command.clone();
	}

	match shell_kind {
		ShellKind::Bash | ShellKind::Zsh | ShellKind::Sh => {
			let env_prefix: String = cmd
				.env_vars
				.iter()
				.map(|(k, v)| format_bash_env_assignment(k, v))
				.collect::<Vec<_>>()
				.join(" ");
			format!("{} {}", env_prefix, cmd.command)
		}
		ShellKind::Fish => {
			let env_prefix: String = cmd
				.env_vars
				.iter()
				.map(|(k, v)| format!("{}={}", k, shell_quote_fish(v)))
				.collect::<Vec<_>>()
				.join(" ");
			format!("env {} {}", env_prefix, cmd.command)
		}
		ShellKind::PowerShell => {
			let env_stmts: String = cmd
				.env_vars
				.iter()
				.map(|(k, v)| format!("$env:{}={}", k, shell_quote_powershell(v)))
				.collect::<Vec<_>>()
				.join("; ");
			format!("{}; {}", env_stmts, cmd.command)
		}
		ShellKind::Cmd => {
			let env_stmts: String = cmd
				.env_vars
				.iter()
				.map(|(k, v)| format!("set \"{}={}\"", k, v))
				.collect::<Vec<_>>()
				.join(" && ");
			format!("{} && {}", env_stmts, cmd.command)
		}
	}
}

fn format_bash_env_assignment(key: &str, value: &str) -> String {
	if needs_quoting(value) {
		format!("{}='{}'", key, value.replace('\'', "'\\''"))
	} else {
		format!("{}={}", key, value)
	}
}

fn shell_quote_fish(value: &str) -> String {
	if needs_quoting(value) {
		format!("'{}'", value.replace('\'', "'\\''"))
	} else {
		value.to_string()
	}
}

fn shell_quote_powershell(value: &str) -> String {
	format!("'{}'", value.replace('\'', "''"))
}

fn needs_quoting(value: &str) -> bool {
	value.is_empty() || value.chars().any(|c| " \t\n\"'\\$`!#&|;(){}[]<>?*~".contains(c))
}

fn collect_all_extract_commands(target_name: &str, runfile: &Runfile) -> Result<Vec<String>, ExtractError> {
	let mut commands = Vec::new();
	let mut completed = HashSet::new();
	let mut in_progress = HashSet::new();
	collect_extract_commands_recursive(target_name, runfile, &mut commands, &mut completed, &mut in_progress)?;
	Ok(commands)
}

fn collect_extract_commands_recursive(
	target_name: &str,
	runfile: &Runfile,
	commands: &mut Vec<String>,
	completed: &mut HashSet<String>,
	in_progress: &mut HashSet<String>,
) -> Result<(), ExtractError> {
	if completed.contains(target_name) {
		return Ok(());
	}
	if !in_progress.insert(target_name.to_string()) {
		return Err(ExtractError::CycleDetected(target_name.to_string()));
	}

	let spec = runfile
		.targets
		.get(target_name)
		.ok_or_else(|| ExtractError::UnknownTarget(target_name.to_string()))?;

	walk_step_templates(&spec.commands, &mut |t| commands.push(t.to_string()));
	walk_spec_aux_templates(spec, &mut |t| commands.push(t.to_string()));

	completed.insert(target_name.to_string());
	in_progress.remove(target_name);

	Ok(())
}
