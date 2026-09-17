//! A build a calling target hands its environment to: `base_env` is what the
//! caller exported, and `defaults` is what only its `.env-file`s supplied.

use super::*;
use crate::PrivateKeyProvider;

const SEP: &str = if cfg!(windows) { ";" } else { ":" };

/// A build of `files` and `own` over what a caller handed over, with no
/// `.add-path` and no keys.
fn called(
	dir: &TempDir,
	base: &HashMap<String, String>,
	defaults: &HashMap<String, String>,
	files: &[String],
	own: &HashMap<String, String>,
) -> HashMap<String, String> {
	let params = EnvBuildParams {
		env_files: Some(files),
		env: Some(own),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: Some(base),
		defaults: Some(defaults),
	};
	build_env(&params, &no_substitute).unwrap()
}

fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
	pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// PATH after prepending `add` onto a build called with `path`.
fn path_after(dir: &TempDir, path: &str, add: &[String]) -> String {
	let base = map(&[("PATH", path)]);
	let params = EnvBuildParams {
		env_files: None,
		env: None,
		add_to_path: Some(add),
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: Some(&base),
		defaults: None,
	};
	get_path_value(&build_env(&params, &no_substitute).unwrap()).to_string()
}

#[test]
fn what_a_build_is_called_with_stands_in_for_the_process_environment() {
	// Wholly: a variable only this process has is not brought back, and the
	// process's PATH does not undo a caller's. Laid over it instead, a called
	// target would lose every PATH its caller assigned.
	let dir = TempDir::new().unwrap();
	let (only_here, _) = std::env::vars()
		.find(|(k, _)| !k.eq_ignore_ascii_case("PATH"))
		.expect("a process with no environment variable but PATH");
	let base = map(&[("PATH", "/called/with")]);
	let env = called(&dir, &base, &HashMap::new(), &[], &HashMap::new());
	assert_eq!(get_path_value(&env), "/called/with");
	assert!(
		!env.contains_key(&only_here),
		"`{only_here}` came back from the process"
	);
}

#[test]
fn what_a_caller_exported_beats_the_called_targets_env_file() {
	// The dotenv rule, one level down: a file is a default, and what the target
	// was called with was exported.
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "KEY=from_file\n").unwrap();
	let env = called(
		&dir,
		&map(&[("KEY", "exported")]),
		&HashMap::new(),
		&[".env".to_string()],
		&HashMap::new(),
	);
	assert_eq!(env["KEY"], "exported");
}

#[test]
fn a_default_gives_way_to_the_called_targets_own_env_file() {
	// What stops a caller's `.env` from clobbering the called target's
	// `.env.test`: both are files, and the called target's is the nearer one.
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env.test"), "DB=test\n").unwrap();
	let env = called(
		&dir,
		&HashMap::new(),
		&map(&[("DB", "dev")]),
		&[".env.test".to_string()],
		&HashMap::new(),
	);
	assert_eq!(env["DB"], "test");
}

#[test]
fn a_default_nothing_else_sets_is_kept() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "OTHER=x\n").unwrap();
	let defaults = map(&[("DB", "dev")]);
	let without_files = called(&dir, &HashMap::new(), &defaults, &[], &HashMap::new());
	assert_eq!(without_files["DB"], "dev");
	let beside_a_file = called(&dir, &HashMap::new(), &defaults, &[".env".to_string()], &HashMap::new());
	assert_eq!(
		beside_a_file["DB"], "dev",
		"a file that does not name it leaves it alone"
	);
	assert_eq!(beside_a_file["OTHER"], "x");
}

#[test]
fn a_missing_env_file_leaves_a_default_where_it_was() {
	let dir = TempDir::new().unwrap();
	let env = called(
		&dir,
		&HashMap::new(),
		&map(&[("DB", "dev")]),
		&[".env.not-there".to_string()],
		&HashMap::new(),
	);
	assert_eq!(env["DB"], "dev");
}

#[test]
fn the_called_targets_own_assignment_beats_a_default_and_an_export() {
	let dir = TempDir::new().unwrap();
	let env = called(
		&dir,
		&map(&[("EXPORTED", "caller")]),
		&map(&[("DEFAULT", "caller-file")]),
		&[],
		&map(&[("EXPORTED", "own"), ("DEFAULT", "own")]),
	);
	assert_eq!(env["EXPORTED"], "own");
	assert_eq!(env["DEFAULT"], "own");
}

#[test]
fn an_export_wins_where_it_and_a_default_name_the_same_key() {
	// A caller hands over the two halves of one environment, so this never
	// happens; if it did, the exported value is the one a command had.
	let dir = TempDir::new().unwrap();
	let env = called(
		&dir,
		&map(&[("KEY", "exported")]),
		&map(&[("KEY", "default")]),
		&[],
		&HashMap::new(),
	);
	assert_eq!(env["KEY"], "exported");
}

#[test]
fn a_file_substitution_sees_the_defaults_and_the_exports_while_files_load() {
	// The substitute hook is handed the map as it stands, and both halves are in
	// it from the start -- so a path or value built from one resolves.
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env.staging"), "WHERE=staging\n").unwrap();
	let seen = std::sync::Mutex::new(Vec::new());
	let substitute = |input: &str, env: &HashMap<String, String>| -> Result<String, String> {
		seen.lock()
			.unwrap()
			.push((env.get("STAGE").cloned(), env.get("REGION").cloned()));
		Ok(input.replace("{{ STAGE }}", env.get("STAGE").map(String::as_str).unwrap_or("?")))
	};
	let base = map(&[("REGION", "eu")]);
	let defaults = map(&[("STAGE", "staging")]);
	let files = vec![".env.{{ STAGE }}".to_string()];
	let params = EnvBuildParams {
		env_files: Some(&files),
		env: None,
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: Some(&base),
		defaults: Some(&defaults),
	};
	let env = build_env(&params, &substitute).unwrap();
	assert_eq!(env["WHERE"], "staging");
	assert_eq!(
		seen.lock().unwrap()[0],
		(Some("staging".to_string()), Some("eu".to_string()))
	);
}

#[test]
fn an_add_path_goes_in_front_of_the_path_a_target_was_called_with() {
	// A caller's `.add-path` arrives as part of PATH, so a called target's own
	// entries are found first -- a subproject's `node_modules/.bin` ahead of the
	// root's that called it.
	let dir = TempDir::new().unwrap();
	let path = path_after(&dir, &format!("/caller/bin{SEP}/usr/bin"), &["own".to_string()]);
	let own = dir.path().join("own").to_string_lossy().into_owned();
	assert_eq!(path, format!("{own}{SEP}/caller/bin{SEP}/usr/bin"));
}

#[test]
fn an_add_path_entry_already_on_path_moves_to_the_front_rather_than_repeating() {
	let dir = TempDir::new().unwrap();
	let own = dir.path().join("bin").to_string_lossy().into_owned();
	let path = path_after(&dir, &format!("/usr/bin{SEP}{own}{SEP}/bin"), &["bin".to_string()]);
	assert_eq!(path, format!("{own}{SEP}/usr/bin{SEP}/bin"));
}

#[test]
fn the_same_add_path_at_every_level_of_a_call_stays_one_entry() {
	// A `_shared.run` read by a target and by the target it calls adds the same
	// directory at both levels. Copies would stack up once per level.
	let dir = TempDir::new().unwrap();
	let add = ["node_modules/.bin".to_string()];
	let first = path_after(&dir, "/usr/bin", &add);
	let second = path_after(&dir, &first, &add);
	let third = path_after(&dir, &second, &add);
	assert_eq!(second, first);
	assert_eq!(third, first);
}

#[test]
fn a_path_holding_none_of_the_entries_comes_back_exactly_as_it_was() {
	// Split on the separator and joined on it again, so an empty segment (the
	// working directory, to a POSIX shell) and a directory already listed twice
	// are not this function's to tidy.
	let dir = TempDir::new().unwrap();
	let held = format!("/a{SEP}{SEP}/b{SEP}/a{SEP}");
	let path = path_after(&dir, &held, &["c".to_string()]);
	let c = dir.path().join("c").to_string_lossy().into_owned();
	assert_eq!(path, format!("{c}{SEP}{held}"));
}

#[test]
fn a_public_key_a_callers_file_supplied_decrypts_the_called_targets_file() {
	// An encrypted file may leave its public key to a file loaded before it. One
	// target loading both finds it; so does a called target whose caller loaded
	// the first.
	let dir = TempDir::new().unwrap();
	let key_hex = runfile_crypto::generate_key();
	let public_key = runfile_crypto::derive_public_key(&key_hex).unwrap();
	let encrypted = runfile_crypto::encrypt("the-secret", &key_hex).unwrap();
	std::fs::write(dir.path().join(".env.secrets"), format!("SECRET={encrypted}\n")).unwrap();
	let private_keys = vec![key_hex];
	let files = vec![".env.secrets".to_string()];
	for (base, defaults) in [
		(
			HashMap::new(),
			map(&[(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR, public_key.as_str())]),
		),
		(
			map(&[(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR, public_key.as_str())]),
			HashMap::new(),
		),
	] {
		let params = EnvBuildParams {
			env_files: Some(&files),
			env: None,
			add_to_path: None,
			working_dir: dir.path(),
			env_files_base_dir: dir.path(),
			available_private_keys: Some(&private_keys),
			base_env: Some(&base),
			defaults: Some(&defaults),
		};
		let env = build_env(&params, &no_substitute).unwrap();
		assert_eq!(env["SECRET"], "the-secret");
	}
}

/// A key pool that fails the test if anything asks it.
struct NeverAsked;

impl PrivateKeyProvider for NeverAsked {
	fn keys(&self) -> &[String] {
		panic!("the key pool was asked for, with nothing encrypted to decrypt");
	}
}

#[test]
fn what_a_caller_handed_over_is_already_plain_and_asks_for_no_keys() {
	// A caller decrypted its values before handing them over. Asking the key
	// pool again would be a second unlock prompt for a value already in hand.
	let dir = TempDir::new().unwrap();
	let pool = NeverAsked;
	let base = map(&[(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR, "aa"), ("TOKEN", "plain")]);
	let defaults = map(&[("SECRET", "decrypted-by-the-caller")]);
	let params = EnvBuildParams {
		env_files: None,
		env: None,
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: Some(&pool),
		base_env: Some(&base),
		defaults: Some(&defaults),
	};
	let env = build_env(&params, &no_substitute).unwrap();
	assert_eq!(env["SECRET"], "decrypted-by-the-caller");
}
