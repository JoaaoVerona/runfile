use super::*;
use crate::executor::{execute_same_shell_with_counter, join_shell_commands};
use crate::extract::{extract_target_with_cwd, format_extracted_commands};
use crate::logging::StepCounter;
use crate::runner::run_target_with_cwd;
use runfile_parser::parse_runfile;
use std::fs;

#[test]
fn join_uses_double_amp_for_stop_on_failure() {
	let cmds = vec!["cd dir".to_string(), "./script.sh".to_string()];
	let joined = join_shell_commands(&cmds, &ShellKind::Bash, false);
	assert_eq!(joined, "cd dir && ./script.sh");
}

#[test]
fn join_uses_semicolon_for_ignore_errors() {
	let cmds = vec!["cmd1".to_string(), "cmd2".to_string()];
	let joined = join_shell_commands(&cmds, &ShellKind::Bash, true);
	assert_eq!(joined, "cmd1; cmd2");
}

#[test]
fn join_uses_single_amp_for_cmd_ignore_errors() {
	let cmds = vec!["a".to_string(), "b".to_string()];
	let joined = join_shell_commands(&cmds, &ShellKind::Cmd, true);
	assert_eq!(joined, "a & b");
}

#[test]
fn join_single_command_returns_verbatim() {
	let cmds = vec!["echo only".to_string()];
	let joined = join_shell_commands(&cmds, &ShellKind::Bash, false);
	assert_eq!(joined, "echo only");
}

#[test]
fn cd_persists_across_steps() {
	// The flagship use case: `cd dir` followed by another command
	// observes the new directory because both run in the same shell
	// process. Without sameShell, the cd would be lost between steps.
	let shell = get_test_shell();
	// `cd` + path semantics differ enough across shells (especially cmd)
	// to make this test cmd-unfriendly. Skip on cmd; the join logic is
	// covered by other tests.
	if shell.kind == ShellKind::Cmd {
		return;
	}

	let dir = TempDir::new().unwrap();
	let subdir = dir.path().join("sub");
	fs::create_dir(&subdir).unwrap();
	let log = subdir.join("marker.txt");
	let log_escaped = json_escape_path(&log);

	// `cd sub`, then write a file. With sameShell the file lands in
	// dir/sub/marker.txt; without it the second command would run from
	// `dir` and the file would land in `dir/marker.txt`.
	let pwd_cmd = if shell.kind == ShellKind::PowerShell {
		format!("New-Item -Path \\\"{log_escaped}\\\" -ItemType File -Force | Out-Null")
	} else {
		format!("touch \\\"{log_escaped}\\\"")
	};
	// The runtime will execute these as `<shell> -c "cd sub && touch /abs/path"`.
	// The `cd sub` matters only for verifying the join; the absolute
	// touch path means we observe success either way. Use a relative
	// touch path so this only succeeds when sameShell really did keep
	// the cwd change.
	let touch_relative = if shell.kind == ShellKind::PowerShell {
		"New-Item -Path marker.txt -ItemType File -Force | Out-Null".to_string()
	} else {
		"touch marker.txt".to_string()
	};
	let _ = pwd_cmd; // keep variable alive for clarity

	let json = format!(
		r#"{{
            "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
            "targets": {{
                "go": {{
                    "commands": ["cd sub", "{touch_relative}"],
                    "sameShell": true
                }}
            }}
        }}"#
	);
	let runfile = parse_runfile(&json).unwrap();
	let args = RunArgs::default();
	let result = run_target_with_cwd(
		"go",
		&runfile,
		&shell,
		&args,
		&dir.path().join("Runfile.json"),
		dir.path(),
		dir.path(),
		&HashMap::new(),
		&HashMap::new(),
		false,
		false,
		None,
	)
	.unwrap();
	assert!(
		result.final_status.success(),
		"sameShell run failed: {:?}",
		result.final_status
	);
	// File should exist in `sub/`, proving cd persisted.
	assert!(
		subdir.join("marker.txt").exists(),
		"marker.txt should be in sub/, meaning cd persisted across steps"
	);
	// And NOT in the parent (where the touch would have landed if sameShell
	// were broken and each step ran in its own shell from `dir`).
	assert!(
		!dir.path().join("marker.txt").exists(),
		"marker.txt must NOT be in the parent dir — cd would have been lost"
	);
}

#[test]
fn rejects_at_target_call() {
	let shell = get_test_shell();
	let json = r#"{
            "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
            "targets": {
                "dep": { "commands": ["echo hi"] },
                "go": {
                    "commands": ["echo before", "@dep", "echo after"],
                    "sameShell": true
                }
            }
        }"#;
	let runfile = parse_runfile(json).unwrap();
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	let counter = StepCounter::new(10);
	let err = execute_same_shell_with_counter(
		&runfile.targets["go"],
		&shell,
		&args,
		dir.path(),
		dir.path(),
		None,
		false,
		&counter,
		None,
		&[],
		None,
	)
	.unwrap_err();
	let msg = err.to_string();
	assert!(msg.contains("@target") && msg.contains("sameShell"), "got error: {msg}");
}

#[test]
fn dry_run_emits_one_joined_line() {
	let json = r#"{
            "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
            "targets": {
                "go": {
                    "commands": ["cd ci-scripts", "./ci-deploy.sh"],
                    "sameShell": true
                }
            }
        }"#;
	let runfile = parse_runfile(json).unwrap();
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	let runfile_path = dir.path().join("Runfile.json");
	let commands = extract_target_with_cwd(
		"go",
		&runfile,
		&args,
		&runfile_path,
		dir.path(),
		dir.path(),
		&HashMap::new(),
		&HashMap::new(),
		None,
		&ShellKind::Bash,
	)
	.unwrap();
	assert_eq!(commands.len(), 1);
	assert_eq!(commands[0].command, "cd ci-scripts && ./ci-deploy.sh");

	// Format with bash too — env-vars are empty so the formatter just
	// passes the joined command through.
	let lines = format_extracted_commands(&commands, &ShellKind::Bash);
	assert_eq!(lines, vec!["cd ci-scripts && ./ci-deploy.sh".to_string()]);
}

#[test]
fn dry_run_uses_powershell_separator() {
	let json = r#"{
            "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
            "targets": {
                "go": {
                    "commands": ["cmd1", "cmd2"],
                    "sameShell": true,
                    "ignoreErrors": true
                }
            }
        }"#;
	let runfile = parse_runfile(json).unwrap();
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();
	let runfile_path = dir.path().join("Runfile.json");
	let commands = extract_target_with_cwd(
		"go",
		&runfile,
		&args,
		&runfile_path,
		dir.path(),
		dir.path(),
		&HashMap::new(),
		&HashMap::new(),
		None,
		&ShellKind::PowerShell,
	)
	.unwrap();
	assert_eq!(commands[0].command, "cmd1; cmd2");
}

#[test]
fn dry_run_uses_cmd_separator_for_ignore_errors() {
	let json = r#"{
            "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
            "targets": {
                "go": {
                    "commands": ["cmd1", "cmd2"],
                    "sameShell": true,
                    "ignoreErrors": true
                }
            }
        }"#;
	let runfile = parse_runfile(json).unwrap();
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();
	let runfile_path = dir.path().join("Runfile.json");
	let commands = extract_target_with_cwd(
		"go",
		&runfile,
		&args,
		&runfile_path,
		dir.path(),
		dir.path(),
		&HashMap::new(),
		&HashMap::new(),
		None,
		&ShellKind::Cmd,
	)
	.unwrap();
	assert_eq!(commands[0].command, "cmd1 & cmd2");
}

#[test]
fn ignore_errors_runs_all_steps() {
	// Even when an early step fails, subsequent steps run because of
	// the `;` separator. Verify by writing markers.
	let shell = get_test_shell();
	if shell.kind == ShellKind::Cmd {
		return; // cmd has its own quoting headaches; bash/zsh/sh/pwsh suffice.
	}

	let dir = TempDir::new().unwrap();
	let marker = dir.path().join("after.txt");
	let marker_escaped = json_escape_path(&marker);

	let touch_after = if shell.kind == ShellKind::PowerShell {
		format!("New-Item -Path \\\"{marker_escaped}\\\" -ItemType File -Force | Out-Null")
	} else {
		format!("touch \\\"{marker_escaped}\\\"")
	};
	let fail = if shell.kind == ShellKind::PowerShell {
		"exit 1".to_string()
	} else {
		"false".to_string()
	};

	let json = format!(
		r#"{{
            "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
            "targets": {{
                "go": {{
                    "commands": ["{fail}", "{touch_after}"],
                    "sameShell": true,
                    "ignoreErrors": true
                }}
            }}
        }}"#
	);
	let runfile = parse_runfile(&json).unwrap();
	let args = RunArgs::default();
	run_target_with_cwd(
		"go",
		&runfile,
		&shell,
		&args,
		&dir.path().join("Runfile.json"),
		dir.path(),
		dir.path(),
		&HashMap::new(),
		&HashMap::new(),
		false,
		false,
		None,
	)
	.unwrap();
	assert!(
		marker.exists(),
		"after.txt should exist — `;` separator with ignoreErrors must keep going"
	);
}

#[test]
fn globals_same_shell_baked_into_target() {
	// `sameShell` on globals applies to every target unless the target
	// overrides it with `sameShell: false`.
	let json = r#"{
            "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
            "globals": { "sameShell": true },
            "targets": {
                "inherits": { "commands": ["echo a", "echo b"] },
                "opts_out": { "commands": ["echo c"], "sameShell": false }
            }
        }"#;
	let runfile = parse_runfile(json).unwrap();
	// Globals are baked at parse-via-merge; the standalone `parse_runfile`
	// doesn't run merge, so we have to call merge_runfiles ourselves to
	// observe the baking.
	use runfile_parser::merge_runfiles;
	use std::path::PathBuf;
	let local_dir = TempDir::new().unwrap();
	let local_path = local_dir.path().join("Runfile.json");
	let merged = merge_runfiles(Some((runfile, local_path)), &[] as &[PathBuf], local_dir.path()).unwrap();

	assert_eq!(merged.runfile.targets["inherits"].same_shell, Some(true));
	assert_eq!(merged.runfile.targets["opts_out"].same_shell, Some(false));
}

#[test]
fn detach_with_same_shell_allowed_without_parallel() {
	// Without sameShell: detach + multiple commands without parallel is
	// rejected at parse time. With sameShell: it's allowed because the
	// commands collapse to one shell invocation.
	let json = r#"{
            "$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
            "targets": {
                "bg": {
                    "commands": ["echo a", "echo b"],
                    "detach": true,
                    "sameShell": true
                }
            }
        }"#;
	assert!(parse_runfile(json).is_ok());
}
