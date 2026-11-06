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
		("runfiles/e.run", "import_env_file(\".env\")\n$ true\n"),
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
		("runfiles/e.run", "import_env_file(\".env\")\n$ true\n"),
		(".env", "RUNFILE_ENCRYPTION_PUBLIC_KEY=aa\nSECRET=encrypted:Zm9vYmFy\n"),
	];
	let (loads, out) = loads_during(files, "e");
	assert_eq!(loads, 1, "an encrypted value must ask, exactly once");
	// The empty pool cannot match, so this fails -- the point is that it asked.
	assert!(out.is_err(), "no key can decrypt it, so the run must fail");
}

#[test]
fn keys_are_loaded_once_per_run_however_many_targets_decrypt() {
	// `a` decrypts the file, runs `b`, and `b` decrypts it again -- from the same
	// pool. This used to be asserted with a pool that could decrypt nothing, so
	// `a` failed before it ever ran `b`, and the second load it would have made
	// went unseen: every target got a pool of its own.
	let env = fixture_env();
	let (loads, d) = fixture_loads_during(
		&[
			("runfiles/a.run", "import_env_file(\".env\")\n\nrun b\n"),
			(
				"runfiles/b.run",
				"import_env_file(\".env\")\n\n$ printf '%s' \"$RUNFILE_T_SECRET\" > b.txt\n",
			),
			(".env", &env),
		],
		"a",
	);
	assert_eq!(read(&d, "b.txt"), "the-secret", "`b` ran, and decrypted");
	assert_eq!(loads, 1, "an unlock prompt must appear once in a run");
}

/// A key made for these tests and nothing else, with its public half and one
/// value encrypted under it -- `the-secret`. This crate does not depend on
/// `runfile-crypto`, so they are written out rather than made per run.
const FIXTURE_KEY: &str = "82ef9a80942a3659106b044e364d70969dcde5a0a214b390e5d889197bac6305";
const FIXTURE_PUBLIC: &str = "e4fcb22d81fff6ffc0beb94a2d57f0ce1baaa68f2db7dabaa02619614f93a150";
const FIXTURE_SECRET: &str = "encrypted:ib2NmXKlKX0km3oAx+lxEg5QZiOtoeFpaN0jEo067ltvHzGaceE=";

/// A `.env` holding one value encrypted under the fixture key, as
/// `RUNFILE_T_SECRET`.
fn fixture_env() -> String {
	format!("RUNFILE_ENCRYPTION_PUBLIC_KEY={FIXTURE_PUBLIC}\nRUNFILE_T_SECRET={FIXTURE_SECRET}\n")
}

/// [`counting_loader`], with the fixture key in the pool.
fn counting_fixture_loader() -> Vec<String> {
	LOADS.fetch_add(1, Ordering::SeqCst);
	vec![FIXTURE_KEY.to_string()]
}

/// A host for `cat` whose key pool holds the fixture key and counts its loads,
/// with the counter at zero.
fn fixture_host(cat: &runfile_discovery::Catalog) -> crate::dispatch::Host<'_> {
	let mut h = crate::dispatch::Host::new(cat);
	h.assume_yes = true;
	h.keys = counting_fixture_loader;
	LOADS.store(0, Ordering::SeqCst);
	h
}

/// Loads during one run of `target` that the fixture key can decrypt, and the
/// project it ran in, so a test can look at what the run left behind.
fn fixture_loads_during(files: &[(&str, &str)], target: &str) -> (usize, tempfile::TempDir) {
	let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
	let d = project(files);
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let h = fixture_host(&cat);
	h.run(target, &[]).unwrap();
	(LOADS.load(Ordering::SeqCst), d)
}

fn read(d: &tempfile::TempDir, name: &str) -> String {
	std::fs::read_to_string(d.path().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn a_called_target_is_handed_its_callers_decrypted_values_without_asking_for_keys_again() {
	// The caller decrypted the file before it ran anything, and hands the plain
	// value over. Reading the file again in the called target would be a second
	// unlock prompt for a value already in hand.
	let env = fixture_env();
	let (loads, d) = fixture_loads_during(
		&[
			("runfiles/caller.run", "import_env_file(\".env\")\n\nrun _child\n"),
			(
				"runfiles/_child.run",
				"$ printf '%s' \"$RUNFILE_T_SECRET\" > secret.txt\n",
			),
			(".env", &env),
		],
		"caller",
	);
	assert_eq!(loads, 1, "asked once, by the caller");
	assert_eq!(read(&d, "secret.txt"), "the-secret");
}

#[test]
fn a_shared_encrypted_env_file_asks_once_for_every_target_that_reads_it() {
	// The usual shape of the case above: neither target names the file, the
	// `_shared.run` above both of them does, so each loads and decrypts it.
	let env = fixture_env();
	let (loads, _d) = fixture_loads_during(
		&[
			("runfiles/_shared.run", "import_env_file(\".env\")\n"),
			("runfiles/caller.run", "run _child\n"),
			("runfiles/_child.run", "$ true\n"),
			(".env", &env),
		],
		"caller",
	);
	assert_eq!(loads, 1);
}

#[test]
fn parallel_branches_that_run_decrypting_targets_ask_once_between_them() {
	// Only the targets the branches run read the file, so all three reach for
	// the pool at once: the first loads it, and the others wait for its answer.
	let env = fixture_env();
	let reads = "import_env_file(\".env\")\n\n$ true\n";
	let (loads, _d) = fixture_loads_during(
		&[
			(
				"runfiles/all.run",
				"parallel do\n\trun _one\n\trun _two\n\trun _three\nend\n",
			),
			("runfiles/_one.run", reads),
			("runfiles/_two.run", reads),
			("runfiles/_three.run", reads),
			(".env", &env),
		],
		"all",
	);
	assert_eq!(loads, 1);
}

#[test]
fn a_run_whose_lines_read_an_encrypted_file_asks_for_keys_once() {
	// A run used to be preceded by a probe of the target's header, to learn
	// whether it declared `.watch`, and a value there that read an encrypted
	// file decrypted twice. The run itself says what it watches now.
	let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
	let env = fixture_env();
	let d = project(&[
		(
			"runfiles/e.run",
			"import_env_file(\".env\")\nENV.RUNFILE_T_COPY = ENV.RUNFILE_T_SECRET\n\n$ printf '%s' \"$RUNFILE_T_COPY\" > copy.txt\n",
		),
		(".env", &env),
	]);
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let h = fixture_host(&cat);
	h.run("e", &[]).unwrap();
	assert_eq!(LOADS.load(Ordering::SeqCst), 1, "one load for the run");
	assert_eq!(read(&d, "copy.txt"), "the-secret");
}

/// Whether [`sometimes_loader`] finds the fixture key.
static KEY_ADDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// [`counting_fixture_loader`], for a key that is only there once
/// [`KEY_ADDED`] says so -- a keyring still locked, or a key not yet added.
fn sometimes_loader() -> Vec<String> {
	LOADS.fetch_add(1, Ordering::SeqCst);
	if KEY_ADDED.load(Ordering::SeqCst) {
		vec![FIXTURE_KEY.to_string()]
	} else {
		Vec::new()
	}
}

#[test]
fn each_run_on_a_host_asks_again_so_the_next_run_finds_a_key_this_one_could_not() {
	// Watch mode runs a target again on the same host after every save, and the
	// next save is meant to be the retry. A pool that outlived its run would
	// remember that there was no key until the session restarted -- and a pool
	// dropped only when a run succeeded would remember it exactly when it
	// failed, so the run that fails is the one asked about here.
	let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
	let env = fixture_env();
	let d = project(&[
		(
			"runfiles/e.run",
			"import_env_file(\".env\")\n\n$ printf '%s' \"$RUNFILE_T_SECRET\" > secret.txt\n",
		),
		(".env", &env),
	]);
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let mut h = fixture_host(&cat);
	h.keys = sometimes_loader;
	KEY_ADDED.store(false, Ordering::SeqCst);
	assert!(h.run("e", &[]).is_err(), "no key to decrypt with yet");

	KEY_ADDED.store(true, Ordering::SeqCst);
	h.run("e", &[]).expect("the key is there now, and this run asks for it");
	assert_eq!(LOADS.load(Ordering::SeqCst), 2, "one load for each run");
	assert_eq!(read(&d, "secret.txt"), "the-secret");
}

/// Loads during one run of `target`, however it ends.
fn loads_running(files: &[(&str, &str)], target: &str) -> usize {
	let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
	let d = project(files);
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let mut h = crate::dispatch::Host::new(&cat);
	h.keys = counting_loader;
	LOADS.store(0, Ordering::SeqCst);
	let _ = h.run(target, &[]);
	LOADS.load(Ordering::SeqCst)
}

const ENCRYPTED: &str = "RUNFILE_ENCRYPTION_PUBLIC_KEY=aa\nSECRET=encrypted:Zm9vYmFy\n";

#[test]
fn a_file_with_nothing_encrypted_in_it_asks_for_no_keys() {
	let files = &[
		(
			"runfiles/e.run",
			"import_env_file(\".env\")\nENV.COPY = ENV.PLAIN\n$ true\n",
		),
		(".env", "PLAIN=1\n"),
	];
	assert_eq!(loads_running(files, "e"), 0, "nothing to decrypt");
}

#[test]
fn a_file_with_an_encrypted_value_asks_once_whether_or_not_anything_reads_it() {
	// A file is decrypted as it is imported, which is what keeps the rule one
	// sentence: what a file holds is in the environment from that line down.
	let files = &[
		("runfiles/e.run", "import_env_file(\".env\")\n$ true\n"),
		(".env", ENCRYPTED),
	];
	assert_eq!(loads_running(files, "e"), 1);
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
fn the_temp_guard_cleans_up_when_it_is_dropped() {
	// What protects a decrypted temp file from a panic: nothing catches an
	// unwind, so cleanup has to happen on the way out, not only on return.
	let d = project(&[(
		"runfiles/t.run",
		"let f = temp_file(\"secret\")\n$ echo {{ f }} > path.txt\n",
	)]);
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let mut h = crate::dispatch::Host::new(&cat);
	h.assume_yes = true;
	let path = {
		let _guard = h.temp_guard();
		h.run("t", &[]).unwrap();
		std::fs::read_to_string(d.path().join("path.txt"))
			.unwrap()
			.trim()
			.to_string()
	};
	assert!(!std::path::Path::new(&path).exists(), "the guard removed it on drop");
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
fn a_writing_function_above_the_first_line_runs_once_per_run() {
	// A header used to be evaluated twice: once by a probe to read `.watch`
	// before the run, once during it, so a side effect there needed guarding
	// against happening twice. The run itself says what it watches now.
	let d = project(&[(
		"runfiles/t.run",
		".watch = \"src/**\"\nENV.CREDS = temp_file(\"secret\")\n$ true\n",
	)]);
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let seen = std::sync::Mutex::new(Vec::new());
	let hook = |p: &crate::props::Props| -> Result<(), String> {
		seen.lock().unwrap().extend(p.watch.iter().map(|w| w.pattern.clone()));
		Ok(())
	};
	let mut h = crate::dispatch::Host::new(&cat);
	h.assume_yes = true;
	h.on_header = Some(&hook);
	h.run("t", &[]).unwrap();
	assert_eq!(*seen.lock().unwrap(), vec!["src/**"], "the run says what it watches");
	let made = h.temps.take();
	assert_eq!(made.len(), 1, "one call, one file: {made:?}");
	for p in made {
		let _ = std::fs::remove_file(p);
	}
}
