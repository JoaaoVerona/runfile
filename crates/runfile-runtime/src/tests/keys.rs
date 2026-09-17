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
fn keys_are_loaded_once_per_run_however_many_targets_decrypt() {
	// `a` decrypts the file, runs `b`, and `b` decrypts it again -- from the same
	// pool. This used to be asserted with a pool that could decrypt nothing, so
	// `a` failed before it ever ran `b`, and the second load it would have made
	// went unseen: every target got a pool of its own.
	let env = fixture_env();
	let (loads, d) = fixture_loads_during(
		&[
			("runfiles/a.run", ".env-file = \".env\"\n\nrun b\n"),
			(
				"runfiles/b.run",
				".env-file = \".env\"\n\n$ printf '%s' \"$RUNFILE_T_SECRET\" > b.txt\n",
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
			("runfiles/caller.run", ".env-file = \".env\"\n\nrun _child\n"),
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
			("runfiles/_shared.run", ".env-file = \".env\"\n"),
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
	let reads = ".env-file = \".env\"\n\n$ true\n";
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
fn the_watch_probe_and_the_run_after_it_ask_for_keys_once() {
	// The CLI reads a target's header before every run that is not `--dry-run`,
	// to learn whether it declares `.watch`, and then runs it on the same host.
	// A header value that reads what an encrypted file holds decrypts in the
	// probe, and the run after it has what the probe loaded.
	let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
	let env = fixture_env();
	let d = project(&[
		(
			"runfiles/e.run",
			".env-file = \".env\"\n.env.RUNFILE_T_COPY = ENV.RUNFILE_T_SECRET\n\n$ printf '%s' \"$RUNFILE_T_COPY\" > copy.txt\n",
		),
		(".env", &env),
	]);
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let h = fixture_host(&cat);
	h.header_props(cat.resolve("e").expect("the target"), &[]).unwrap();
	assert_eq!(LOADS.load(Ordering::SeqCst), 1, "the probe decrypted");
	h.run("e", &[]).unwrap();
	assert_eq!(LOADS.load(Ordering::SeqCst), 1, "and the run asked nothing more");
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
			".env-file = \".env\"\n\n$ printf '%s' \"$RUNFILE_T_SECRET\" > secret.txt\n",
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
