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

/// Serialised, because the counter is process-global.
fn loads_during(files: &[(&str, &str)], target: &str) -> (usize, Result<(), crate::RunError>) {
	static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
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
