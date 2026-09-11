//! The credential store must stay untouched unless something decrypts.
//!
//! A locked keyring blocks on an interactive unlock prompt, so an eager load
//! turns every `run <target>` into a hang. That is not reproducible on a
//! developer machine with an unlocked session, which is exactly why it needs a
//! test that counts the loads instead of observing the symptom.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::project;

static LOADS: AtomicUsize = AtomicUsize::new(0);

/// A stand-in for `keyring_keys::all_private_keys`, counting each call.
fn counting_loader() -> Vec<String> {
	LOADS.fetch_add(1, Ordering::SeqCst);
	Vec::new()
}

/// Serialises every test that reads the counter, since it is process-global.
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn loads_during(files: &[(&str, &str)], target: &str) -> (usize, Result<(), crate::RunError>) {
	let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
	let d = project(files);
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let mut h = crate::dispatch::Host::new(&cat);
	h.assume_yes = true;
	h.keys = counting_loader;
	LOADS.store(0, Ordering::SeqCst);
	let out = h.run(target, &[]);
	(LOADS.load(Ordering::SeqCst), out)
}

#[test]
fn a_target_that_decrypts_nothing_never_asks_for_keys() {
	let (loads, out) = loads_during(&[("runfiles/plain.run", "$ true\n")], "plain");
	out.unwrap();
	assert_eq!(loads, 0, "a plain target must not touch the credential store");
}

#[test]
fn a_plain_env_file_never_asks_for_keys() {
	// Env building has a key provider wired in; it must stay deferred when the
	// file holds nothing encrypted.
	let files = &[
		("runfiles/e.run", ".env-file = \".env\"\n$ true\n"),
		(".env", "GREETING=hi\n"),
	];
	let (loads, out) = loads_during(files, "e");
	out.unwrap();
	assert_eq!(loads, 0, "an unencrypted env file must not touch the credential store");
}

#[test]
fn an_encrypted_env_value_does_ask_for_keys() {
	// The counterpart: deferral must not mean "never", or decryption silently
	// stops working -- which is how this was wired before.
	let files = &[
		("runfiles/e.run", ".env-file = \".env\"\n$ true\n"),
		(".env", "RUNFILE_ENCRYPTION_PUBLIC_KEY=aa\nSECRET=encrypted:Zm9vYmFy\n"),
	];
	let (loads, out) = loads_during(files, "e");
	assert_eq!(loads, 1, "an encrypted value must ask, exactly once");
	// The empty pool cannot match, so this fails -- the point is that it asked.
	assert!(out.is_err(), "no key can decrypt it, so the run must fail");
}

#[test]
fn keys_are_loaded_at_most_once_per_run() {
	let files = &[
		("runfiles/a.run", ".env-file = \".env\"\nrun b\n"),
		("runfiles/b.run", ".env-file = \".env\"\n$ true\n"),
		(".env", "RUNFILE_ENCRYPTION_PUBLIC_KEY=aa\nS=encrypted:Zm9vYmFy\n"),
	];
	let (loads, _) = loads_during(files, "a");
	assert!(
		loads <= 1,
		"asked {loads} times; an unlock prompt must appear at most once"
	);
}

/// Loads while the watch probe reads a target's header -- which the CLI does
/// before **every** run that is not `--dry-run`, to learn whether it declares
/// `.watch`. Anything a header costs, it costs twice.
fn loads_probing(files: &[(&str, &str)], target: &str) -> usize {
	let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
	let d = project(files);
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let mut h = crate::dispatch::Host::new(&cat);
	h.keys = counting_loader;
	LOADS.store(0, Ordering::SeqCst);
	let _ = h.header_props(cat.resolve(target).expect("the target"), &[]);
	LOADS.load(Ordering::SeqCst)
}

const ENCRYPTED: &str = "RUNFILE_ENCRYPTION_PUBLIC_KEY=aa\nSECRET=encrypted:Zm9vYmFy\n";

#[test]
fn the_watch_probe_builds_no_environment_for_a_header_that_never_reads_it() {
	// A header value reads the environment as it stands at its own line, which
	// can mean building it part-way -- reading and decrypting every file. Done
	// eagerly, that would unlock the keyring in the probe as well as in the
	// run, on every run of a target with an encrypted file. So it is built
	// only for a value that is about to look.
	let files = &[
		(
			"runfiles/e.run",
			".env-file = \".env\"\n.env.MODE = \"dev\"\n.shell = \"sh\"\n$ true\n",
		),
		(".env", ENCRYPTED),
	];
	assert_eq!(loads_probing(files, "e"), 0, "nothing in the header reads `ENV`");
}

#[test]
fn a_header_value_that_reads_the_environment_is_the_one_case_that_builds_it() {
	// The counterpart: when a value below the file does read `ENV`, it has to
	// see what the file loaded, and that is only knowable by reading it.
	let files = &[
		(
			"runfiles/e.run",
			".env-file = \".env\"\n.env.COPY = ENV.SECRET ? \"none\"\n$ true\n",
		),
		(".env", ENCRYPTED),
	];
	assert_eq!(loads_probing(files, "e"), 1, "the value reads what the file holds");
}

// ---- temp files
//
// They live here because, like the key pool, what matters is the mechanism
// around them rather than the value they return.

#[test]
fn a_temp_file_is_created_with_its_content_and_removed_after_the_run() {
	let d = project(&[(
		"runfiles/t.run",
		"let f = temp_file(\"secret\", \"json\")\n$ cp {{ f }} copied.txt\n$ echo {{ f }} > path.txt\n",
	)]);
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let mut h = crate::dispatch::Host::new(&cat);
	h.assume_yes = true;
	h.run("t", &[]).unwrap();

	let path = std::fs::read_to_string(d.path().join("path.txt"))
		.unwrap()
		.trim()
		.to_string();
	assert!(path.ends_with(".json"), "the extension is honoured: {path}");
	assert_eq!(
		std::fs::read_to_string(d.path().join("copied.txt")).unwrap(),
		"secret",
		"the content was written"
	);
	assert!(
		std::path::Path::new(&path).exists(),
		"still there until the caller cleans up"
	);
	h.cleanup_temps();
	assert!(!std::path::Path::new(&path).exists(), "and gone afterwards");
}

#[test]
fn a_temp_file_is_removed_even_when_the_target_fails() {
	// The case the whole registry exists for: a half-way failure must not leave
	// a decoded credential in the temp directory.
	let files = &[(
		"runfiles/t.run",
		"let f = temp_file(\"secret\")\n$ echo {{ f }} > path.txt\n$ false\n",
	)];
	let d = project(files);
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let mut h = crate::dispatch::Host::new(&cat);
	h.assume_yes = true;
	assert!(h.run("t", &[]).is_err(), "the target fails");

	let path = std::fs::read_to_string(d.path().join("path.txt"))
		.unwrap()
		.trim()
		.to_string();
	assert!(std::path::Path::new(&path).exists());
	h.cleanup_temps();
	assert!(!std::path::Path::new(&path).exists(), "cleaned up despite the failure");
}

#[test]
fn a_temp_dir_is_removed_with_what_is_inside_it() {
	let d = project(&[(
		"runfiles/t.run",
		"let dir = temp_dir()\n$ echo {{ dir }} > path.txt\n$ touch {{ dir }}/inside\n",
	)]);
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let mut h = crate::dispatch::Host::new(&cat);
	h.assume_yes = true;
	h.run("t", &[]).unwrap();

	let path = std::fs::read_to_string(d.path().join("path.txt"))
		.unwrap()
		.trim()
		.to_string();
	assert!(std::path::Path::new(&path).join("inside").exists());
	h.cleanup_temps();
	assert!(!std::path::Path::new(&path).exists(), "removed recursively");
}

#[test]
fn a_preview_creates_no_temp_file() {
	let d = project(&[(
		"runfiles/t.run",
		"let f = temp_file(\"x\")\n$ echo {{ f }} > path.txt\n",
	)]);
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let mut h = crate::dispatch::Host::new(&cat);
	h.assume_yes = true;
	h.dry_run = true;
	h.run("t", &[]).unwrap();
	assert!(h.temps.take().is_empty(), "a preview must not touch the filesystem");
}

#[test]
fn two_temp_files_in_one_run_are_different() {
	let d = project(&[(
		"runfiles/t.run",
		"let a = temp_file()\nlet b = temp_file()\n$ echo {{ a }} {{ b }} > paths.txt\n",
	)]);
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let mut h = crate::dispatch::Host::new(&cat);
	h.assume_yes = true;
	h.run("t", &[]).unwrap();
	let line = std::fs::read_to_string(d.path().join("paths.txt")).unwrap();
	let paths: Vec<&str> = line.split_whitespace().collect();
	assert_eq!(paths.len(), 2);
	assert_ne!(paths[0], paths[1]);
	h.cleanup_temps();
}

#[test]
fn a_writing_function_in_a_property_runs_once_per_run() {
	// The declaration region is evaluated twice: once to read `.watch` before
	// the run, once during it. A side effect there must not happen twice --
	// the second file would be an orphan nothing referenced.
	let d = project(&[(
		"runfiles/t.run",
		".watch = \"src/**\"\n.env.CREDS = temp_file(\"secret\")\n$ true\n",
	)]);
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let mut h = crate::dispatch::Host::new(&cat);
	h.assume_yes = true;
	let target = cat.resolve("t").unwrap();

	// Exactly what the CLI does before running a target that declares `.watch`.
	let props = h.header_props(target, &[]).unwrap();
	assert_eq!(props.watch, vec!["src/**".to_string()], "the probe still reads it");
	assert!(h.temps.take().is_empty(), "and creates nothing");

	h.run("t", &[]).unwrap();
	let made = h.temps.take();
	assert_eq!(made.len(), 1, "one call, one file: {made:?}");
	for p in made {
		let _ = std::fs::remove_file(p);
	}
}
