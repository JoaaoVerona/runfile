//! Behavioral integration tests for the generate/`:init`/`:convert` subcommands, driving the actual
//! `run` binary as a subprocess.
//!
//! These cover the CLI wiring that the generators-crate unit tests can't reach: that
//! `:generate task-descriptors` prints the editor-agnostic descriptor JSON to stdout (writing
//! nothing to disk), that the on-disk editor writers produce the right files and honor
//! `.editorconfig`, and that the `--include-*` flags gate which targets reach those files. Each
//! test runs in an isolated temp dir with `HOME` / `XDG_CONFIG_HOME` / `APPDATA` pointed at an empty
//! dir so user settings and global Runfiles can't leak targets into the output.

use std::path::Path;
use std::process::{Command, Output};

/// Run the compiled `run` binary in `dir` with a hermetic environment.
fn run_in(dir: &Path, args: &[&str]) -> Output {
	let home = dir.join("_home");
	std::fs::create_dir_all(&home).unwrap();
	Command::new(env!("CARGO_BIN_EXE_run"))
		.args(args)
		.current_dir(dir)
		.env("HOME", &home)
		.env("XDG_CONFIG_HOME", home.join(".config"))
		.env("APPDATA", home.join("AppData"))
		// `dirs::config_dir()` ignores `%APPDATA%` on Windows (it reads the Known
		// Folder API), so the vars above don't isolate settings there. This one
		// does, cross-platform — without it, real machine-registered global files
		// leak into `--include-globals` output on Windows CI.
		.env("RUNFILE_CONFIG_DIR", home.join("runfile"))
		.env_remove("RUNFILE_TARGET")
		.env_remove("RUNFILE_ENV_FILE_TARGET")
		.output()
		.expect("run binary executes")
}

fn write(path: &Path, contents: &str) {
	if let Some(parent) = path.parent() {
		std::fs::create_dir_all(parent).unwrap();
	}
	std::fs::write(path, contents).unwrap();
}

fn stdout_of(out: &Output) -> String {
	String::from_utf8(out.stdout.clone()).expect("stdout is UTF-8")
}

const RUNFILE_TWO_TARGETS: &str = r#"{ "$schema": "x", "targets": { "build": { "commands": ["echo build"] }, "test": { "commands": ["echo test"] } } }"#;
const RUNFILE_ONE_TARGET: &str = r#"{ "$schema": "x", "targets": { "build": { "commands": ["echo build"] } } }"#;

const EDITORCONFIG_2SPACE_FINAL_NL: &str =
	"root = true\n[*]\nindent_style = space\nindent_size = 2\ninsert_final_newline = true\n";

// ── :generate task-descriptors: editor-agnostic stdout contract ──────────

/// Run `:generate task-descriptors` in `root` and parse its stdout as JSON.
fn task_descriptors(root: &Path) -> serde_json::Value {
	let out = run_in(root, &[":generate", "task-descriptors"]);
	assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
	serde_json::from_str(&stdout_of(&out)).expect("task-descriptors emits valid JSON")
}

/// The first source group whose `kind` matches, or panic.
fn source_of<'a>(doc: &'a serde_json::Value, kind: &str) -> &'a serde_json::Value {
	doc["sources"]
		.as_array()
		.expect("sources array")
		.iter()
		.find(|s| s["kind"] == kind)
		.unwrap_or_else(|| panic!("no source of kind {kind}: {doc}"))
}

/// The target named `name` across every source group, or panic.
fn target_of<'a>(doc: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
	doc["sources"]
		.as_array()
		.expect("sources array")
		.iter()
		.flat_map(|s| s["targets"].as_array().expect("targets array"))
		.find(|t| t["name"] == name)
		.unwrap_or_else(|| panic!("no target {name}: {doc}"))
}

#[test]
fn task_descriptors_emits_local_targets_and_writes_nothing() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write(
		&root.join("Runfile.json"),
		r#"{ "$schema": "x", "targets": {
			"build": { "description": "Build it", "commands": ["cargo build"] },
			"test": { "commands": ["cargo test"] }
		} }"#,
	);

	let doc = task_descriptors(root);
	assert_eq!(doc["formatVersion"], 1);

	let local = source_of(&doc, "local");
	assert!(
		local["filePath"].as_str().unwrap().ends_with("Runfile.json"),
		"local filePath should point at the source Runfile: {local}"
	);

	let build = target_of(&doc, "build");
	assert_eq!(build["description"], "Build it");
	assert!(
		build.get("namespace").is_none(),
		"un-namespaced target must omit the namespace key"
	);

	let test = target_of(&doc, "test");
	assert!(
		test.get("description").is_none(),
		"a target without a description omits the key"
	);

	// Pure stdout command — nothing lands on disk.
	assert!(!root.join(".vscode").exists(), "task-descriptors must not write files");
}

#[test]
fn task_descriptors_groups_by_kind_with_namespaces_and_globals_always_on() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write_namespaced_project(root);

	// Register a global file — task-descriptors always merges globals (no flag).
	let global = root.join("global/Runfile.json");
	write(
		&global,
		r#"{ "$schema": "x", "targets": { "deploy": { "commands": ["echo deploy"] } } }"#,
	);
	let add = run_in(root, &[":config", "global-files", "add", global.to_str().unwrap()]);
	assert!(add.status.success(), "stderr: {}", String::from_utf8_lossy(&add.stderr));

	let doc = task_descriptors(root);

	// Local: the root's own build, un-namespaced.
	assert_eq!(source_of(&doc, "local")["kind"], "local");
	assert!(target_of(&doc, "build").get("namespace").is_none());

	// Included + namespaced: `api:deploy` carries namespace "api"; the plain
	// include's `clean` is pulled in but stays un-namespaced. Both come from
	// `included` sources (always resolved, no --include-namespaces needed).
	assert_eq!(target_of(&doc, "api:deploy")["namespace"], "api");
	assert!(
		target_of(&doc, "clean").get("namespace").is_none(),
		"plain include stays un-namespaced"
	);
	assert_eq!(source_of(&doc, "included")["kind"], "included");

	// Global: always merged in and tagged kind "global".
	let global_src = source_of(&doc, "global");
	assert!(
		global_src["targets"]
			.as_array()
			.unwrap()
			.iter()
			.any(|t| t["name"] == "deploy"),
		"global source should carry the registered global target: {global_src}"
	);
}

#[test]
fn task_descriptors_colon_named_local_is_not_a_namespace() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	// A local target whose *name* contains a colon (like `all:package`) is NOT a
	// namespace; only the real `api` include is.
	write(
		&root.join("Runfile.json"),
		r#"{
			"$schema": "x",
			"includes": [ { "path": "api/Runfile.json", "namespace": "api" } ],
			"targets": { "all:package": { "commands": ["echo all"] } }
		}"#,
	);
	write(
		&root.join("api/Runfile.json"),
		r#"{ "$schema": "x", "targets": { "deploy": { "commands": ["echo deploy"] } } }"#,
	);

	let doc = task_descriptors(root);
	assert!(
		target_of(&doc, "all:package").get("namespace").is_none(),
		"a colon in the name is not a namespace"
	);
	assert_eq!(
		target_of(&doc, "api:deploy")["namespace"],
		"api",
		"a real include namespace is reported"
	);
}

// ── :generate --include-namespaces pulls in included/namespaced targets ──

/// A root Runfile that includes a sub-Runfile under the `api` namespace, plus a
/// plain (un-namespaced) include. Written into `dir` alongside the two includes.
fn write_namespaced_project(root: &Path) {
	write(
		&root.join("Runfile.json"),
		r#"{
			"$schema": "x",
			"includes": [
				{ "path": "api/Runfile.json", "namespace": "api" },
				"shared/Runfile.json"
			],
			"targets": { "build": { "commands": ["echo root build"] } }
		}"#,
	);
	write(
		&root.join("api/Runfile.json"),
		r#"{ "$schema": "x", "targets": { "deploy": { "commands": ["echo api deploy"] } } }"#,
	);
	write(
		&root.join("shared/Runfile.json"),
		r#"{ "$schema": "x", "targets": { "clean": { "commands": ["echo shared clean"] } } }"#,
	);
}

#[test]
fn generate_vscode_without_flag_excludes_namespaced_targets() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write_namespaced_project(root);

	let out = run_in(root, &[":generate", "vscode-tasks"]);
	assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

	let tasks = std::fs::read_to_string(root.join(".vscode/tasks.json")).expect(".vscode/tasks.json written");
	assert!(
		tasks.contains("\"label\": \"run build\""),
		"expected root build:\n{tasks}"
	);
	assert!(
		!tasks.contains("run api:deploy"),
		"namespaced target must be absent without the flag:\n{tasks}"
	);
	assert!(
		!tasks.contains("run clean"),
		"included target must be absent without the flag:\n{tasks}"
	);
}

#[test]
fn generate_vscode_writes_file_with_include_namespaces() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write_namespaced_project(root);

	let out = run_in(root, &[":generate", "vscode-tasks", "--include-namespaces"]);
	assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

	let tasks = std::fs::read_to_string(root.join(".vscode/tasks.json")).expect(".vscode/tasks.json written");
	assert!(
		tasks.contains("\"label\": \"run build\""),
		"expected root build:\n{tasks}"
	);
	assert!(
		tasks.contains("\"label\": \"run api:deploy\""),
		"expected namespaced api:deploy task:\n{tasks}"
	);
	assert!(
		tasks.contains("\"label\": \"run clean\""),
		"expected plain-include clean task:\n{tasks}"
	);
}

#[test]
fn generate_jetbrains_include_namespaces_sanitizes_colon_in_filename() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write_namespaced_project(root);

	let out = run_in(
		root,
		&[":generate", "jetbrains-run-configurations", "--include-namespaces"],
	);
	assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

	// The colon in `api:deploy` is sanitized to `_` in the filename, but the run
	// invocation keeps the prefixed name.
	let cfg = std::fs::read_to_string(root.join(".run/Runfile_api_deploy.run.xml"))
		.expect(".run/Runfile_api_deploy.run.xml written");
	assert!(
		cfg.contains("value=\"run --stdin-args api:deploy\""),
		"expected namespaced invocation:\n{cfg}"
	);
	assert!(
		root.join(".run/Runfile_build.run.xml").is_file(),
		"root target config should also exist"
	);
}

// ── :generate --include-globals pulls in registered global-file targets ──

#[test]
fn generate_vscode_include_globals_adds_global_targets_with_detail() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write(&root.join("Runfile.json"), RUNFILE_ONE_TARGET);

	// A global Runfile registered via `run :config global-files add`. Its target
	// carries a description so we can assert it surfaces as the task `detail`.
	let global = root.join("global/Runfile.json");
	write(
		&global,
		r#"{ "$schema": "x", "targets": { "deploy": { "description": "Ship it", "commands": ["echo deploy"] } } }"#,
	);
	let add = run_in(root, &[":config", "global-files", "add", global.to_str().unwrap()]);
	assert!(add.status.success(), "stderr: {}", String::from_utf8_lossy(&add.stderr));

	// Without the flag: only the local target — the global one stays out.
	let out = run_in(root, &[":generate", "vscode-tasks"]);
	assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
	let plain = std::fs::read_to_string(root.join(".vscode/tasks.json")).expect(".vscode/tasks.json written");
	assert!(plain.contains("\"run build\""), "expected local build:\n{plain}");
	assert!(
		!plain.contains("run deploy"),
		"global target must be absent without the flag:\n{plain}"
	);

	// With the flag: local + global targets, and the global's description as `detail`.
	let out = run_in(root, &[":generate", "vscode-tasks", "--include-globals"]);
	assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
	let tasks = std::fs::read_to_string(root.join(".vscode/tasks.json")).expect(".vscode/tasks.json written");
	assert!(
		tasks.contains("\"label\": \"run build\""),
		"expected local build:\n{tasks}"
	);
	assert!(
		tasks.contains("\"label\": \"run deploy\""),
		"expected global deploy:\n{tasks}"
	);
	assert!(
		tasks.contains("\"detail\": \"Ship it\""),
		"expected global description surfaced as detail:\n{tasks}"
	);
}

#[test]
fn generate_vscode_include_globals_works_without_a_local_runfile() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	// No Runfile.json at `root` (or above it) — only a registered global file,
	// tucked in a subdir so auto-discovery from `root` can't find it as "local".
	let global = root.join("global/Runfile.json");
	write(
		&global,
		r#"{ "$schema": "x", "targets": { "deploy": { "commands": ["echo deploy"] } } }"#,
	);
	let add = run_in(root, &[":config", "global-files", "add", global.to_str().unwrap()]);
	assert!(add.status.success(), "stderr: {}", String::from_utf8_lossy(&add.stderr));

	// Without the flag and no local Runfile: the historical hard error stands.
	let bare = run_in(root, &[":generate", "vscode-tasks"]);
	assert!(
		!bare.status.success(),
		"expected failure with no local Runfile and no flag"
	);
	assert!(
		!root.join(".vscode").exists(),
		"no file should be written on the error path"
	);

	// With --include-globals: the global target generates even though there is no
	// local Runfile to anchor to.
	let out = run_in(root, &[":generate", "vscode-tasks", "--include-globals"]);
	assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
	let tasks = std::fs::read_to_string(root.join(".vscode/tasks.json")).expect(".vscode/tasks.json written");
	assert!(
		tasks.contains("\"label\": \"run deploy\""),
		"expected global deploy with no local Runfile:\n{tasks}"
	);
}

// ── :generate writes files, honoring .editorconfig ───────────────────────

#[test]
fn generate_vscode_writes_file_with_editorconfig_formatting() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write(&root.join("Runfile.json"), RUNFILE_TWO_TARGETS);
	write(&root.join(".editorconfig"), EDITORCONFIG_2SPACE_FINAL_NL);

	let out = run_in(root, &[":generate", "vscode-tasks"]);
	assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

	let tasks = std::fs::read_to_string(root.join(".vscode/tasks.json")).expect(".vscode/tasks.json written");
	assert!(tasks.contains("\n  \"version\""), "expected 2-space indent:\n{tasks}");
	assert!(tasks.ends_with('\n'), "expected final newline");
	assert!(tasks.contains("\"label\": \"run build\""));
}

// ── :init honors .editorconfig ───────────────────────────────────────────

#[test]
fn init_default_uses_tab_indent_and_lf() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();

	let out = run_in(root, &[":init"]);
	assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

	let bytes = std::fs::read(root.join("Runfile.json")).expect("Runfile.json written");
	let text = String::from_utf8(bytes).unwrap();
	// Historical default: tab indentation, LF, trailing newline — unchanged when no .editorconfig.
	assert!(
		text.contains("\n\t\"$schema\""),
		"expected tab indent by default:\n{text}"
	);
	assert!(!text.contains("\r\n"), "expected LF by default");
	assert!(text.ends_with('\n'));
}

#[test]
fn init_respects_editorconfig_spaces_and_crlf() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write(
		&root.join(".editorconfig"),
		"root = true\n[*]\nindent_style = space\nindent_size = 2\nend_of_line = crlf\ninsert_final_newline = true\n",
	);

	let out = run_in(root, &[":init"]);
	assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

	let bytes = std::fs::read(root.join("Runfile.json")).expect("Runfile.json written");
	let text = String::from_utf8(bytes).unwrap();
	assert!(text.contains("\r\n"), "expected CRLF line endings:\n{text:?}");
	assert!(
		text.contains("\r\n  \"$schema\""),
		"expected 2-space indent with CRLF:\n{text:?}"
	);
	assert!(
		!text.contains('\t'),
		"tabs should have been converted to spaces:\n{text:?}"
	);
	assert!(text.ends_with("\r\n"));
}

// ── :convert honors .editorconfig ────────────────────────────────────────

#[test]
fn convert_package_json_respects_editorconfig() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write(
		&root.join("package.json"),
		r#"{ "scripts": { "build": "webpack", "lint": "eslint ." } }"#,
	);
	write(
		&root.join(".editorconfig"),
		"root = true\n[*]\nindent_style = space\nindent_size = 2\nend_of_line = crlf\ninsert_final_newline = true\n",
	);

	let out = run_in(root, &[":convert", "package-json"]);
	assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

	let bytes = std::fs::read(root.join("Runfile.json")).expect("Runfile.json written");
	let text = String::from_utf8(bytes).unwrap();
	assert!(text.contains("\r\n"), "expected CRLF line endings:\n{text:?}");
	assert!(text.contains("\r\n  \"$schema\""), "expected 2-space indent:\n{text:?}");
	assert!(text.ends_with("\r\n"), "expected final newline");
	// The converted targets are present.
	assert!(text.contains("\"build\""));
	assert!(text.contains("\"lint\""));
}
