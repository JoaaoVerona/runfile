//! Every relative path follows the working directory in effect where it is
//! written: a command, the file functions, `.env-file`, `.add-path`, `.watch`
//! and `.workdir` itself.
//!
//! The file functions and the properties used to resolve against the anchor
//! whatever `.workdir` said, so `$ ls` and `glob("*")` on one line listed two
//! different directories. Each test here puts a different `marker` in each
//! directory, so a path that resolved against the wrong one reads the wrong
//! word rather than the same one.

use super::{host_run, project};

/// A project whose anchor, `sub` and `sub/deeper` each hold a `marker` naming
/// the directory it is in.
fn nested(target: &str) -> tempfile::TempDir {
	let d = project(&[("runfiles/t.run", target)]);
	std::fs::create_dir_all(d.path().join("sub/deeper")).unwrap();
	std::fs::write(d.path().join("marker"), "anchor").unwrap();
	std::fs::write(d.path().join("sub/marker"), "sub").unwrap();
	std::fs::write(d.path().join("sub/deeper/marker"), "deeper").unwrap();
	d
}

#[test]
fn the_file_functions_read_and_write_where_the_commands_run() {
	let d = nested(
		".workdir = \"sub\"\n\n\
		 $ test {{ read_file(\"marker\") }} = sub\n\
		 $ test {{ file_exists(\"deeper/marker\") }} = true\n\
		 $ test {{ directory_exists(\"deeper\") }} = true\n\
		 $ test {{ join(\",\", glob(\"*/marker\")) }} = deeper/marker\n\
		 write_file(\"made\", \"x\")\n\
		 $ test -f made\n",
	);
	host_run(&d, "t").expect("every function looked in `sub`, where the commands ran");
	assert!(d.path().join("sub/made").is_file());
	assert!(!d.path().join("made").exists(), "nothing was written to the anchor");
}

#[test]
fn a_header_value_reads_from_the_workdir_above_it_and_not_one_below() {
	let d = nested(
		".env.BEFORE = read_file(\"marker\")\n\
		 .workdir = \"sub\"\n\
		 .env.AFTER = read_file(\"marker\")\n\n\
		 $ test \"$BEFORE\" = anchor\n\
		 $ test \"$AFTER\" = sub\n",
	);
	host_run(&d, "t").expect("each value read from the directory in effect at its own line");
}

#[test]
fn a_property_below_a_statement_moves_reads_from_its_line_down() {
	let d = nested(
		"$ test {{ read_file(\"marker\") }} = anchor\n\
		 .workdir = \"sub\"\n\
		 $ test {{ read_file(\"marker\") }} = sub\n\
		 $ test \"$(cat marker)\" = sub\n",
	);
	host_run(&d, "t").expect("the line above the property read the anchor, the lines below it `sub`");
}

#[test]
fn a_nested_workdir_is_relative_to_the_one_around_it_and_ends_with_its_block() {
	let d = nested(
		".workdir = \"sub\"\n\n\
		 do\n\
		 \t.workdir = \"deeper\"\n\n\
		 \t$ test {{ read_file(\"marker\") }} = deeper\n\
		 \t$ test \"$(cat marker)\" = deeper\n\
		 end\n\n\
		 $ test {{ read_file(\"marker\") }} = sub\n\n\
		 do\n\
		 \t.workdir = RUN.parent\n\n\
		 \t$ test {{ read_file(\"marker\") }} = anchor\n\
		 end\n",
	);
	host_run(&d, "t").expect("`deeper` under `sub` is `sub/deeper`, an absolute one replaces, and both end at `end`");
}

#[test]
fn a_loop_asks_its_condition_where_its_body_runs() {
	// The runner asks a loop's condition under the body's properties, as it
	// does for the environment. Only `sub` holds `deeper/marker`, so a
	// condition read from the anchor would never let the body run.
	let d = nested(
		"let passes = 0\n\n\
		 while file_exists(\"deeper/marker\") && passes == 0\n\
		 \t.workdir = \"sub\"\n\n\
		 \tpasses = passes + 1\n\
		 end\n\n\
		 $ test {{ passes }} = 1\n",
	);
	host_run(&d, "t").expect("the condition looked in `sub`");
}

#[test]
fn env_files_resolve_where_they_are_written() {
	// Two `.env-file = ".env"` lines name two files: one above the `.workdir`,
	// one below it.
	let d = nested(
		".env-file = \".env\"\n\
		 .workdir = \"sub\"\n\
		 .env-file = \".env\"\n\n\
		 $ test \"$FROM_ANCHOR\" = yes\n\
		 $ test \"$FROM_SUB\" = yes\n\n\
		 do\n\
		 \t.workdir = \"deeper\"\n\
		 \t.env-file = \".env\"\n\n\
		 \t$ test \"$FROM_DEEPER\" = yes\n\
		 end\n",
	);
	std::fs::write(d.path().join(".env"), "FROM_ANCHOR=yes\n").unwrap();
	std::fs::write(d.path().join("sub/.env"), "FROM_SUB=yes\n").unwrap();
	std::fs::write(d.path().join("sub/deeper/.env"), "FROM_DEEPER=yes\n").unwrap();
	host_run(&d, "t").expect("each `.env-file` was read from the directory in effect at its line");
}

#[cfg(unix)]
#[test]
fn add_paths_resolve_where_they_are_written() {
	use std::os::unix::fs::PermissionsExt;
	// The shape `react-extended-maps` uses: `.add-path` above `.workdir`, so the
	// path names the subproject from the anchor and must not be moved under it.
	let d = nested(
		".add-path = \"sub/bin\"\n\
		 .workdir = \"sub\"\n\
		 .add-path = \"deeper/bin\"\n\n\
		 $ from-sub\n\
		 $ from-deeper\n",
	);
	for (dir, name) in [("sub/bin", "from-sub"), ("sub/deeper/bin", "from-deeper")] {
		let bin = d.path().join(dir);
		std::fs::create_dir_all(&bin).unwrap();
		std::fs::write(bin.join(name), "#!/bin/sh\nexit 0\n").unwrap();
		std::fs::set_permissions(bin.join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
	}
	host_run(&d, "t").expect("both directories were put on PATH where they were written");
}

#[test]
fn a_shared_workdir_is_where_its_targets_read_from() {
	// The machine-wide directory's `.workdir = RUN.cwd` is the case this is for:
	// a `_shared.run` that moves its targets moves what they read, too.
	let d = nested("$ test {{ read_file(\"marker\") }} = sub\n$ test {{ shared }} = sub\n");
	std::fs::write(
		d.path().join("runfiles/_shared.run"),
		".workdir = \"sub\"\n\nlet shared = read_file(\"marker\")\n",
	)
	.unwrap();
	host_run(&d, "t").expect("the target and the shared `let` both read from `sub`");
}

#[test]
fn a_watch_pattern_is_matched_beneath_the_workdir_it_was_written_under() {
	let d = nested(
		".watch = \"top/**\"\n\
		 .workdir = \"sub\"\n\
		 .watch = \"src/**\"\n\n\
		 $ true\n",
	);
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let host = crate::dispatch::Host::new(&cat);
	let props = host.header_props(cat.resolve("t").unwrap(), &[]).unwrap();
	let got: Vec<(std::path::PathBuf, String)> = props.watch.into_iter().map(|w| (w.dir, w.pattern)).collect();
	let anchor = cat.resolve("t").unwrap().anchor.clone();
	assert_eq!(
		got,
		vec![
			(anchor.clone(), "top/**".to_string()),
			(anchor.join("sub"), "src/**".to_string()),
		]
	);
}

#[test]
fn a_working_directory_that_is_not_there_is_named_rather_than_the_shell() {
	// Starting a process reports a missing directory exactly as a missing
	// program, so every form that runs one used to say `could not start bash`.
	for line in [
		"$ true",
		"exec sh\n\ttrue\nend",
		"let out = $ echo hi",
		"detach $ true",
		"if $ true\n\tprint(\"ran\")\nend",
		"$ true\n$ true",
	] {
		let d = nested(&format!(".workdir = \"nope\"\n\n{line}\n"));
		let e = host_run(&d, "t").expect_err(line).to_string();
		assert!(
			e.contains("working directory") && e.contains("nope") && e.contains("does not exist"),
			"{line:?}: {e}"
		);
		assert!(!e.contains("could not start"), "{line:?} blamed the program: {e}");
	}
	let d = nested(".workdir = \"marker\"\n\n$ true\n");
	let e = host_run(&d, "t").unwrap_err().to_string();
	assert!(e.contains("is not a directory"), "{e}");
}

#[test]
fn a_working_directory_is_judged_when_a_command_runs_not_when_it_is_named() {
	// An earlier line may be what makes it, so naming one that is not there
	// yet is not a mistake on its own.
	let d = nested("$ mkdir made\n\ndo\n\t.workdir = \"made\"\n\n\t$ touch here\nend\n");
	host_run(&d, "t").expect("the directory existed by the time a command ran in it");
	assert!(d.path().join("made/here").is_file());
}

#[test]
fn a_missing_program_is_still_reported_as_one() {
	let d = nested("exec runfile-no-such-program-anywhere\n\tx\nend\n");
	let e = host_run(&d, "t").unwrap_err().to_string();
	assert!(e.contains("could not start"), "{e}");
	assert!(!e.contains("working directory"), "{e}");
}
