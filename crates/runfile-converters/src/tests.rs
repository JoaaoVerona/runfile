use std::collections::HashSet;

// ── Package JSON tests ─────────────────────────────────────────────

#[test]
fn convert_simple_npm_scripts() {
	let json: serde_json::Value =
		serde_json::from_str(r#"{"build": "tsc", "test": "jest", "lint": "eslint ."}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	assert_eq!(result.targets.len(), 3);
	assert!(result.targets.contains_key("build"));
	assert!(result.targets.contains_key("test"));
	assert!(result.targets.contains_key("lint"));
	assert_eq!(result.targets["build"].commands, vec!["tsc"]);
}

#[test]
fn convert_npm_with_env_extraction() {
	let json: serde_json::Value = serde_json::from_str(r#"{"dev": "NODE_ENV=development node server.js"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let spec = &result.targets["dev"];
	assert_eq!(spec.commands, vec!["node server.js"]);
	let env = spec.env.as_ref().unwrap();
	assert!(env.contains_key("NODE_ENV"));
}

#[test]
fn convert_npm_skips_prepare() {
	let json: serde_json::Value = serde_json::from_str(r#"{"prepare": "husky install", "build": "tsc"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	assert_eq!(result.targets.len(), 1);
	assert!(!result.targets.contains_key("prepare"));
}

#[test]
fn convert_npm_skips_on_collision() {
	let mut existing = HashSet::new();
	existing.insert("build".to_string());

	let json: serde_json::Value = serde_json::from_str(r#"{"build": "tsc", "test": "jest"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &existing);
	assert!(!result.targets.contains_key("build"));
	assert!(result.targets.contains_key("test"));
	assert_eq!(result.skipped, vec!["build"]);
}

#[test]
fn convert_npm_chained_commands() {
	let json: serde_json::Value =
		serde_json::from_str(r#"{"ci": "npm run lint && npm run test && npm run build"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let spec = &result.targets["ci"];
	assert_eq!(spec.commands.len(), 3);
}

// ── Makefile tests ─────────────────────────────────────────────────

#[test]
fn convert_simple_makefile() {
	let makefile = "\
.PHONY: build test clean

build:
\tcargo build --release

test:
\tcargo test

clean:
\trm -rf target/
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets.len(), 3);
	assert!(result.targets.contains_key("build"));
	assert!(result.targets.contains_key("test"));
	assert!(result.targets.contains_key("clean"));
	assert_eq!(result.targets["build"].commands, vec!["cargo build --release"]);
}

#[test]
fn convert_makefile_with_deps() {
	let makefile = "\
.PHONY: all build test

all: build test
\techo done

build:
\tcargo build

test:
\tcargo test
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let all = &result.targets["all"];
	// Make deps now appear as `@target` invocations at the start of `commands`.
	let mut targets: Vec<&str> = Vec::new();
	for cmd in &all.commands {
		if let runfile_parser::CommandStep::TargetCall(call) = cmd {
			targets.push(&call.target);
		}
	}
	assert_eq!(targets, &["build", "test"]);
}

#[test]
fn convert_makefile_strips_silent_prefix() {
	let makefile = "\
.PHONY: quiet

quiet:
\t@echo hello
\t@echo world
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets["quiet"].commands, vec!["echo hello", "echo world"]);
}

#[test]
fn convert_makefile_skips_special_targets() {
	let makefile = "\
.PHONY: build
.SUFFIXES:
.DEFAULT:
\techo default

build:
\tcargo build
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets.len(), 1);
	assert!(result.targets.contains_key("build"));
}

#[test]
fn convert_makefile_expands_variables() {
	let makefile = "\
CC = gcc
CFLAGS = -Wall -O2

.PHONY: build

build:
\t$(CC) $(CFLAGS) main.c -o main
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets["build"].commands, vec!["gcc -Wall -O2 main.c -o main"]);
}

#[test]
fn convert_makefile_skips_on_collision() {
	let mut existing = HashSet::new();
	existing.insert("build".to_string());

	let makefile = "\
.PHONY: build test

build:
\tcargo build

test:
\tcargo test
";
	let result = crate::convert_makefile(makefile, &existing);
	assert!(!result.targets.contains_key("build"));
	assert!(result.targets.contains_key("test"));
	assert_eq!(result.skipped, vec!["build"]);
}

#[test]
fn convert_makefile_multi_command_target() {
	let makefile = "\
.PHONY: deploy

deploy:
\techo Building...
\tcargo build --release
\techo Deploying...
\t./deploy.sh
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets["deploy"].commands.len(), 4);
}

#[test]
fn convert_makefile_skips_file_targets() {
	let makefile = "\
.PHONY: build

build:
\tcargo build

target/release/myapp: src/main.rs
\tcargo build --release
";
	// target/release/myapp has a dot and slash, non-phony with file deps — should still be included
	// since it has commands. But its dep (src/main.rs) should not become dependsOn since it has a slash.
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert!(result.targets.contains_key("build"));
}

#[test]
fn convert_makefile_skips_comments_and_blanks() {
	let makefile = "\
# This is a comment
.PHONY: build

# Another comment
build:
\t# inline comment gets kept (it's a valid shell comment)
\tcargo build
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets["build"].commands.len(), 2);
}

#[test]
fn convert_makefile_export_as_env() {
	let makefile = "\
.PHONY: serve

serve:
\texport PORT=3000
\tnode server.js
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let spec = &result.targets["serve"];
	assert!(spec.env.as_ref().unwrap().contains_key("PORT"));
	assert_eq!(spec.commands, vec!["node server.js"]);
}

#[test]
fn convert_makefile_line_continuation() {
	let makefile = ".PHONY: build\n\nbuild:\n\tgcc -Wall \\\n\t\t-O2 \\\n\t\t-o main main.c\n";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let spec = &result.targets["build"];
	assert_eq!(spec.commands.len(), 1);
	assert_eq!(spec.commands[0], "gcc -Wall  \t\t-O2  \t\t-o main main.c");
}

#[test]
fn convert_makefile_line_continuation_in_variable() {
	let makefile = "\
SOURCES = foo.c \\\n  bar.c \\\n  baz.c

.PHONY: build

build:
\tgcc $(SOURCES) -o app
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let spec = &result.targets["build"];
	// The continuation joins lines with a space, preserving the indentation from the original
	assert!(spec.commands[0].contains("gcc"));
	assert!(spec.commands[0].contains("foo.c"));
	assert!(spec.commands[0].contains("bar.c"));
	assert!(spec.commands[0].contains("baz.c"));
	assert!(spec.commands[0].contains("-o app"));
	assert_eq!(spec.commands.len(), 1);
}

#[test]
fn convert_makefile_multiname_target() {
	let makefile = "\
.PHONY: kill-docker cleanup

kill-docker cleanup:
\tdocker kill foo
\tdocker rm bar
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let spec = &result.targets["kill-docker"];
	assert_eq!(spec.commands, vec!["docker kill foo", "docker rm bar"]);
	assert_eq!(spec.aliases.as_ref().unwrap(), &["cleanup"]);
}

#[test]
fn convert_makefile_multiname_does_not_leak_recipes() {
	let makefile = "\
.PHONY: aaa bbb ccc

aaa:
\techo aaa

bbb ccc:
\techo bbb

";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets["aaa"].commands, vec!["echo aaa"]);
	assert_eq!(result.targets["bbb"].commands, vec!["echo bbb"]);
	assert!(result.targets["bbb"]
		.aliases
		.as_ref()
		.unwrap()
		.contains(&"ccc".to_string()));
}

#[test]
fn convert_makefile_inline_env_not_extracted() {
	// Bare VAR=value at start of command is shell inline-env, not an export
	let makefile = "\
.PHONY: test

test:
\tENV=test NODE_ENV=test node app.js
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let spec = &result.targets["test"];
	assert!(
		spec.env.is_none(),
		"inline env vars should stay as command, not be extracted"
	);
	assert_eq!(spec.commands.len(), 1);
	assert!(spec.commands[0].contains("ENV=test"));
	assert!(spec.commands[0].contains("node app.js"));
}

#[test]
fn convert_makefile_continuation_with_inline_env() {
	// Real-world pattern: inline env vars with continuation lines
	let makefile = "\
.PHONY: func-test

func-test:
\t@ENV=test \\\n\tNODE_ENV=test \\\n\tnpx cucumber-js \\\n\t--backtrace --exit
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let spec = &result.targets["func-test"];
	assert_eq!(spec.commands.len(), 1);
	assert!(spec.commands[0].contains("ENV=test"));
	assert!(spec.commands[0].contains("NODE_ENV=test"));
	assert!(spec.commands[0].contains("npx cucumber-js"));
	assert!(spec.commands[0].contains("--backtrace --exit"));
	assert!(spec.env.is_none());
}

// ══════════════════════════════════════════════════════════════════════
// Additional Makefile converter coverage
// ══════════════════════════════════════════════════════════════════════

#[test]
fn convert_makefile_pattern_rule_skipped() {
	let makefile = "\
.PHONY: build

build:
\tgcc -o app main.c

%.o: %.c
\tgcc -c $< -o $@
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets.len(), 1);
	assert!(result.targets.contains_key("build"));
}

#[test]
fn convert_makefile_variable_conditional_assignment() {
	let makefile = "\
CC ?= gcc

.PHONY: build

build:
\t$(CC) main.c
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets["build"].commands, vec!["gcc main.c"]);
}

#[test]
fn convert_makefile_variable_append_assignment() {
	let makefile = "\
FLAGS = -Wall
FLAGS += -O2

.PHONY: build

build:
\tgcc $(FLAGS) main.c
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	// Note: += just overwrites in the simple parser
	let cmd = &result.targets["build"].commands[0];
	assert!(cmd.contains("gcc"));
	assert!(cmd.contains("main.c"));
}

#[test]
fn convert_makefile_curly_brace_variable() {
	let makefile = "\
CC = gcc

.PHONY: build

build:
\t${CC} main.c
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets["build"].commands, vec!["gcc main.c"]);
}

#[test]
fn convert_makefile_unknown_variable_kept_as_is() {
	let makefile = "\
.PHONY: build

build:
\techo $(UNKNOWN_VAR)
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets["build"].commands, vec!["echo $(UNKNOWN_VAR)"]);
}

#[test]
fn convert_makefile_function_call_kept_as_is() {
	let makefile = "\
.PHONY: build

build:
\techo $(shell uname -s)
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets["build"].commands, vec!["echo $(shell uname -s)"]);
}

#[test]
fn convert_makefile_no_targets() {
	let makefile = "# just a comment\n\n";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert!(result.targets.is_empty());
}

#[test]
fn convert_makefile_target_with_file_deps_filtered() {
	let makefile = "\
.PHONY: build

build: src/main.rs Cargo.toml
\tcargo build
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let spec = &result.targets["build"];
	// File-like deps (with / or .) should be filtered out — no leading `@target`.
	assert!(
		!matches!(&spec.commands[0], runfile_parser::CommandStep::TargetCall(_)),
		"File deps should not become @target invocations"
	);
}

#[test]
fn convert_makefile_target_with_phony_deps_kept() {
	let makefile = "\
.PHONY: all build test

all: build test
\techo done

build:
\tcargo build

test:
\tcargo test
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let spec = &result.targets["all"];
	let mut targets: Vec<&str> = Vec::new();
	for cmd in &spec.commands {
		if let runfile_parser::CommandStep::TargetCall(call) = cmd {
			targets.push(&call.target);
		}
	}
	assert_eq!(targets, &["build", "test"]);
}

#[test]
fn convert_makefile_export_with_quoted_value() {
	let makefile = "\
.PHONY: test

test:
\texport PORT=\"3000\"
\tnode server.js
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let spec = &result.targets["test"];
	assert!(spec.env.as_ref().unwrap().contains_key("PORT"));
	assert_eq!(spec.commands, vec!["node server.js"]);
}

#[test]
fn convert_makefile_non_phony_with_commands_included() {
	// Non-phony targets with commands should still be included
	let makefile = "\
myapp:
\tgcc main.c -o myapp
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert!(result.targets.contains_key("myapp"));
}

#[test]
fn convert_makefile_non_phony_empty_commands_skipped() {
	// Non-phony targets with no commands should be skipped (file targets)
	let makefile = "\
data.json: input.csv
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert!(result.targets.is_empty());
}

#[test]
fn convert_makefile_strips_both_at_and_dash_prefixes() {
	let makefile = "\
.PHONY: test

test:
\t@echo silent
\t-echo ignore_error
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(
		result.targets["test"].commands,
		vec!["echo silent", "echo ignore_error"]
	);
}

#[test]
fn convert_makefile_colon_in_first_position_skipped() {
	// A line starting with : is not a valid target
	let makefile = "\
.PHONY: build

:weird:
\techo weird

build:
\techo build
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert!(result.targets.contains_key("build"));
	assert!(!result.targets.contains_key(""));
}

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
	// Windows-only scripts now wrap their commands in `if "$(RUN.os) == windows"`.
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

// ── New Package JSON tests: lifecycle scripts, hooks, npm run, tools ──

#[test]
fn convert_npm_skips_all_lifecycle_scripts() {
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"preinstall": "echo pre",
			"install": "echo install",
			"postinstall": "node scripts/setup.js",
			"prepublishOnly": "npm run build",
			"prepare": "husky install",
			"build": "tsc"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	assert_eq!(result.targets.len(), 1);
	assert!(result.targets.contains_key("build"));
	assert!(!result.targets.contains_key("install"));
	assert!(!result.targets.contains_key("preinstall"));
	assert!(!result.targets.contains_key("postinstall"));
	assert!(!result.targets.contains_key("prepublishOnly"));
	assert!(!result.targets.contains_key("prepare"));
}

#[test]
fn convert_npm_pre_hook_prepends_to_commands() {
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"pretest": "eslint .",
			"test": "jest"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	assert_eq!(result.targets.len(), 1, "pretest should not be a standalone target");
	assert!(!result.targets.contains_key("pretest"));
	let test_spec = &result.targets["test"];
	// First command should be the pre hook.
	assert_eq!(test_spec.commands[0], "eslint .");
}

#[test]
fn convert_npm_post_hook_appends_to_commands() {
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"build": "tsc",
			"postbuild": "cp -r dist/ output/"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	assert_eq!(result.targets.len(), 1, "postbuild should not be a standalone target");
	assert!(!result.targets.contains_key("postbuild"));
	let build_spec = &result.targets["build"];
	// Last command should be the post hook.
	assert_eq!(
		build_spec.commands.last().unwrap(),
		&runfile_parser::CommandStep::Shell("cp -r dist/ output/".into())
	);
}

#[test]
fn convert_npm_pre_and_post_hooks_together() {
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"prebuild": "rimraf dist/",
			"build": "tsc",
			"postbuild": "node scripts/copy-assets.js"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	assert_eq!(result.targets.len(), 1);
	let spec = &result.targets["build"];
	// Pre/post hooks now flank the main command in `commands`.
	assert_eq!(
		spec.commands.first().unwrap(),
		&runfile_parser::CommandStep::Shell("rimraf dist/".into())
	);
	assert_eq!(
		spec.commands.last().unwrap(),
		&runfile_parser::CommandStep::Shell("node scripts/copy-assets.js".into())
	);
}

#[test]
fn convert_npm_pre_hook_without_base_stays_as_target() {
	// "preflight" is not pre + "flight" because "flight" doesn't exist
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"preflight": "eslint . && tsc --noEmit"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	assert_eq!(result.targets.len(), 1);
	assert!(result.targets.contains_key("preflight"));
}

#[test]
fn convert_npm_run_references_replaced() {
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"lint": "eslint .",
			"test": "jest",
			"ci": "npm run lint && npm run test"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let ci = &result.targets["ci"];
	assert_eq!(ci.commands, vec!["run lint", "run test"]);
}

#[test]
fn convert_npm_run_unknown_script_kept() {
	// "npm run unknown" where "unknown" is not in the scripts map stays unchanged
	let json: serde_json::Value = serde_json::from_str(r#"{"ci": "npm run unknown-thing"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let ci = &result.targets["ci"];
	assert_eq!(ci.commands, vec!["npm run unknown-thing"]);
}

#[test]
fn convert_npm_yarn_run_replaced() {
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"build": "tsc",
			"ci": "yarn run build"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	assert_eq!(result.targets["ci"].commands, vec!["run build"]);
}

#[test]
fn convert_npm_yarn_shorthand_replaced() {
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"build": "tsc",
			"ci": "yarn build"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	assert_eq!(result.targets["ci"].commands, vec!["run build"]);
}

#[test]
fn convert_npm_npx_prefix_stripped() {
	let json: serde_json::Value = serde_json::from_str(r#"{"test": "npx jest --coverage"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	assert_eq!(result.targets["test"].commands, vec!["jest --coverage"]);
}

#[test]
fn convert_npm_node_modules_bin_stripped() {
	let json: serde_json::Value = serde_json::from_str(r#"{"test": "./node_modules/.bin/jest --verbose"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	assert_eq!(result.targets["test"].commands, vec!["jest --verbose"]);
}

#[test]
fn convert_npm_run_s_sequential() {
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"lint": "eslint .",
			"test": "jest",
			"build": "tsc",
			"ci": "run-s lint test build"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let ci = &result.targets["ci"];
	assert_eq!(ci.commands, vec!["run lint", "run test", "run build"]);
	assert!(ci.parallel.is_none());
}

#[test]
fn convert_npm_run_p_parallel() {
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"watch:css": "postcss --watch",
			"watch:js": "tsc --watch",
			"dev": "run-p watch:css watch:js"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let dev = &result.targets["dev"];
	assert_eq!(dev.commands, vec!["run watch:css", "run watch:js"]);
	assert_eq!(dev.parallel, Some(true));
}

#[test]
fn convert_npm_run_all_parallel_flag() {
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"lint": "eslint .",
			"test": "jest",
			"check": "npm-run-all --parallel lint test"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let check = &result.targets["check"];
	assert_eq!(check.commands, vec!["run lint", "run test"]);
	assert_eq!(check.parallel, Some(true));
}

#[test]
fn convert_npm_concurrently() {
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"watch:css": "postcss --watch",
			"watch:js": "tsc --watch",
			"dev": "concurrently \"npm run watch:css\" \"npm run watch:js\""
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let dev = &result.targets["dev"];
	assert_eq!(dev.commands, vec!["run watch:css", "run watch:js"]);
	assert_eq!(dev.parallel, Some(true));
}

#[test]
fn convert_npm_concurrently_with_flags() {
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"serve": "node server.js",
			"watch": "tsc --watch",
			"dev": "concurrently --kill-others \"npm run serve\" \"npm run watch\""
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let dev = &result.targets["dev"];
	assert_eq!(dev.commands, vec!["run serve", "run watch"]);
	assert_eq!(dev.parallel, Some(true));
}

#[test]
fn convert_npm_pre_hook_cleans_npm_run() {
	// Pre-hook commands should also get npm run → run replacement
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"lint": "eslint .",
			"prebuild": "npm run lint",
			"build": "tsc"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let build = &result.targets["build"];
	// Pre-hook now lives at the start of `commands`, not in a separate before list.
	assert_eq!(
		build.commands.first().unwrap(),
		&runfile_parser::CommandStep::Shell("run lint".into())
	);
}

// ── dotenvx tests ───────────────────────────────────────────────────

#[test]
fn convert_npm_dotenvx_basic() {
	let json: serde_json::Value = serde_json::from_str(r#"{"dev": "dotenvx run -- node server.js"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let spec = &result.targets["dev"];
	assert_eq!(spec.commands, vec!["node server.js"]);
	assert!(spec.env_files.is_none(), "no -f flag means no explicit envFiles");
}

#[test]
fn convert_npm_dotenvx_with_env_file() {
	let json: serde_json::Value =
		serde_json::from_str(r#"{"dev": "dotenvx run -f .env.local -- node server.js"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let spec = &result.targets["dev"];
	assert_eq!(spec.commands, vec!["node server.js"]);
	let env_files = spec.env_files.as_ref().unwrap();
	assert_eq!(env_files, &[".env.local"]);
}

#[test]
fn convert_npm_dotenvx_multiple_env_files() {
	let json: serde_json::Value =
		serde_json::from_str(r#"{"dev": "dotenvx run -f .env -f .env.local -- node server.js"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let spec = &result.targets["dev"];
	assert_eq!(spec.commands, vec!["node server.js"]);
	let env_files = spec.env_files.as_ref().unwrap();
	assert_eq!(env_files, &[".env", ".env.local"]);
}

#[test]
fn convert_npm_dotenvx_no_separator() {
	// dotenvx run without -- separator
	let json: serde_json::Value = serde_json::from_str(r#"{"dev": "dotenvx run node server.js"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let spec = &result.targets["dev"];
	assert_eq!(spec.commands, vec!["node server.js"]);
}

#[test]
fn convert_npm_dotenvx_with_npm_run() {
	// dotenvx wrapping an npm run command that references a known script
	let json: serde_json::Value = serde_json::from_str(
		r#"{
			"serve": "node server.js",
			"dev": "dotenvx run -f .env.dev -- npm run serve"
		}"#,
	)
	.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let dev = &result.targets["dev"];
	assert_eq!(dev.commands, vec!["run serve"]);
	assert_eq!(dev.env_files.as_ref().unwrap(), &[".env.dev"]);
}

#[test]
fn convert_npm_dotenvx_in_chain() {
	// dotenvx in a && chain — env files are collected
	let json: serde_json::Value =
		serde_json::from_str(r#"{"ci": "dotenvx run -f .env.test -- jest && dotenvx run -f .env.test -- eslint ."}"#)
			.unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let spec = &result.targets["ci"];
	assert_eq!(spec.commands, vec!["jest", "eslint ."]);
	// Both parts referenced the same file — should be deduplicated
	assert_eq!(spec.env_files.as_ref().unwrap(), &[".env.test"]);
}

#[test]
fn convert_npm_dotenvx_env_file_long_form() {
	let json: serde_json::Value =
		serde_json::from_str(r#"{"dev": "dotenvx run --env-file=.env.prod -- node app.js"}"#).unwrap();
	let scripts = json.as_object().unwrap();

	let result = crate::convert_package_json_scripts(scripts, &HashSet::new());
	let spec = &result.targets["dev"];
	assert_eq!(spec.commands, vec!["node app.js"]);
	assert_eq!(spec.env_files.as_ref().unwrap(), &[".env.prod"]);
}
