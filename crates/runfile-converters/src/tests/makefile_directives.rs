use super::*;

// ── New Makefile tests: directives, logging, ignoreErrors, $(MAKE) ──

#[test]
fn convert_makefile_skips_ifeq_directives() {
	let makefile = "\
.PHONY: build

ifeq ($(OS),Windows_NT)
RM = del /Q
else
RM = rm -f
endif

build:
\t$(RM) output.txt
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert!(result.targets.contains_key("build"));
	// Last assignment wins (rm -f)
	assert_eq!(result.targets["build"].commands, vec!["rm -f output.txt"]);
}

#[test]
fn convert_makefile_skips_ifdef_directives() {
	let makefile = "\
.PHONY: build

ifdef DEBUG
CFLAGS = -g
endif

CFLAGS ?= -O2

build:
\tgcc $(CFLAGS) main.c
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert!(result.targets.contains_key("build"));
}

#[test]
fn convert_makefile_skips_include_directive() {
	let makefile = "\
include config.mk
-include optional.mk

.PHONY: build

build:
\tcargo build
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert!(result.targets.contains_key("build"));
	assert_eq!(result.targets["build"].commands, vec!["cargo build"]);
}

#[test]
fn convert_makefile_skips_define_endef_block() {
	let makefile = "\
define HELP_TEXT
Usage: make build
  build  - Build the project
  test   - Run tests
endef

.PHONY: build

build:
\tcargo build
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets.len(), 1);
	assert!(result.targets.contains_key("build"));
}

#[test]
fn convert_makefile_all_silent_sets_logging_false() {
	let makefile = "\
.PHONY: quiet

quiet:
\t@echo step1
\t@echo step2
\t@echo step3
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let spec = &result.targets["quiet"];
	assert_eq!(spec.logging, Some(false));
}

#[test]
fn convert_makefile_mixed_silent_no_logging_change() {
	let makefile = "\
.PHONY: mixed

mixed:
\t@echo silent
\techo loud
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let spec = &result.targets["mixed"];
	assert_eq!(spec.logging, None);
}

#[test]
fn convert_makefile_all_ignore_error_sets_flag() {
	let makefile = "\
.PHONY: cleanup

cleanup:
\t-rm -rf build/
\t-rm -rf dist/
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let spec = &result.targets["cleanup"];
	assert_eq!(spec.ignore_errors, Some(true));
}

#[test]
fn convert_makefile_mixed_ignore_no_flag() {
	let makefile = "\
.PHONY: mixed

mixed:
\t-rm -rf build/
\tcargo build
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let spec = &result.targets["mixed"];
	assert_eq!(spec.ignore_errors, None);
}

#[test]
fn convert_makefile_at_dash_combined_prefix() {
	let makefile = "\
.PHONY: quiet

quiet:
\t@-rm -rf temp/
\t@-rm -rf cache/
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let spec = &result.targets["quiet"];
	assert_eq!(spec.commands, vec!["rm -rf temp/", "rm -rf cache/"]);
	assert_eq!(spec.logging, Some(false));
	assert_eq!(spec.ignore_errors, Some(true));
}

#[test]
fn convert_makefile_make_recursive_call() {
	let makefile = "\
.PHONY: all clean build

all:
\t$(MAKE) clean
\t$(MAKE) build

clean:
\trm -rf target/

build:
\tcargo build
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let spec = &result.targets["all"];
	assert_eq!(spec.commands, vec!["run clean", "run build"]);
}

#[test]
fn convert_makefile_make_recursive_with_curly_braces() {
	let makefile = "\
.PHONY: all test

all:
\t${MAKE} test

test:
\tcargo test
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets["all"].commands, vec!["run test"]);
}

#[test]
fn convert_makefile_make_word_recursive_call() {
	let makefile = "\
.PHONY: all build

all:
\tmake build

build:
\tcargo build
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets["all"].commands, vec!["run build"]);
}

#[test]
fn convert_makefile_export_variable_in_non_recipe() {
	let makefile = "\
export CC = gcc

.PHONY: build

build:
\t$(CC) main.c
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets["build"].commands, vec!["gcc main.c"]);
}

// ══════════════════════════════════════════════════════════════════════
// Additional Package JSON converter coverage
// ══════════════════════════════════════════════════════════════════════

#[test]
fn convert_npm_non_string_script_skipped() {
	let json: serde_json::Value = serde_json::from_str(r#"{"build": "tsc", "broken": 42}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	assert!(result.targets.contains_key("build"));
	assert!(!result.targets.contains_key("broken"));
}

#[test]
fn convert_npm_cross_env_extraction() {
	let json: serde_json::Value =
		serde_json::from_str(r#"{"dev": "cross-env NODE_ENV=development node server.js"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let spec = &result.targets["dev"];
	assert_eq!(spec.commands, vec!["node server.js"]);
	let env = spec.env.as_ref().unwrap();
	assert!(env.contains_key("NODE_ENV"));
}

#[test]
fn convert_npm_windows_only_script() {
	let json: serde_json::Value = serde_json::from_str(r#"{"win": "set NODE_ENV=prod && node app.js"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let spec = &result.targets["win"];
	// Windows-only scripts now wrap their commands in `if "{{ RUN.os }} == windows"`.
	assert!(matches!(&spec.commands[0], runfile_parser::CommandStep::If(_)));
}

#[test]
fn convert_npm_empty_scripts() {
	let json: serde_json::Value = serde_json::from_str(r#"{}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	assert!(result.targets.is_empty());
	assert!(result.skipped.is_empty());
}

#[test]
fn convert_npm_single_command() {
	let json: serde_json::Value = serde_json::from_str(r#"{"start": "node index.js"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let spec = &result.targets["start"];
	assert_eq!(spec.commands, vec!["node index.js"]);
	assert!(spec.env.is_none());
}

#[test]
fn split_chained_commands_respects_single_quotes() {
	let parts = crate::package_json::split_chained_commands("echo 'a && b' && echo c");
	assert_eq!(parts.len(), 2);
	assert_eq!(parts[0].trim(), "echo 'a && b'");
	assert_eq!(parts[1].trim(), "echo c");
}

#[test]
fn split_chained_commands_respects_double_quotes() {
	let parts = crate::package_json::split_chained_commands(r#"echo "a && b" && echo c"#);
	assert_eq!(parts.len(), 2);
	assert_eq!(parts[0].trim(), r#"echo "a && b""#);
	assert_eq!(parts[1].trim(), "echo c");
}

#[test]
fn split_chained_commands_single_ampersand_not_split() {
	let parts = crate::package_json::split_chained_commands("cmd & other");
	assert_eq!(parts.len(), 1);
	assert_eq!(parts[0], "cmd & other");
}

#[test]
fn shell_tokenize_handles_quoted_spaces() {
	let tokens = crate::package_json::shell_tokenize(r#"echo "hello world" foo"#);
	assert_eq!(tokens, vec!["echo", "\"hello world\"", "foo"]);
}

#[test]
fn shell_tokenize_handles_single_quoted() {
	let tokens = crate::package_json::shell_tokenize("echo 'hello world' foo");
	assert_eq!(tokens, vec!["echo", "'hello world'", "foo"]);
}

#[test]
fn shell_tokenize_empty_input() {
	let tokens = crate::package_json::shell_tokenize("");
	assert!(tokens.is_empty());
}

#[test]
fn shell_tokenize_multiple_spaces() {
	let tokens = crate::package_json::shell_tokenize("a   b   c");
	assert_eq!(tokens, vec!["a", "b", "c"]);
}
