//! What a target `run` from another one is handed.
//!
//! The environment itself lives in the scope, changed line by line -- by
//! `ENV.X = …`, `add_path()` and `import_env_file()` -- and read by `ENV.X` and
//! by every command; see `runfile_lang::state`. All that is left here is how it
//! crosses a `run`.

use runfile_lang::Scope;
use std::collections::HashMap;

/// The environment a target is `run` with, as the target that ran it hands it
/// over: what the caller's commands have at that line, in two halves.
///
/// A value that only a `.env` file supplied is a **default**, and stays one in
/// the called target, so its own `import_env_file()` still replaces it.
/// Everything else is **exported** -- the caller's shell, its `ENV.X = …`, its
/// `add_path()`, and whatever it was run with itself -- and stands where a
/// shell's variables stand: no file of the called target's replaces it, and
/// its own `ENV.X = …` does.
///
/// Handing the whole of it over as exported is what `$ run x` does, since a new
/// process is given nothing but variables. It is what would let a caller's
/// `.env` beat the called target's `.env.test`: `run test` from `ci` would test
/// against the development database.
#[derive(Clone, Default, PartialEq)]
pub struct Inherited {
	pub exported: HashMap<String, String>,
	pub defaults: HashMap<String, String>,
	/// The working directory of the line that ran it -- `None` from the command
	/// line, which is the process's own.
	///
	/// A target of the project starts at its anchor whatever this says, since
	/// it works on the project it belongs to. A **package** starts here: it was
	/// fetched to work on whatever called it, so `web-docs:i18n-sync` running
	/// j18n has it translate `web-docs/`, and a `set_cwd()` above the line moves
	/// where it starts as it moves every command.
	pub workdir: Option<std::path::PathBuf>,
}

impl Inherited {
	/// What a target run with this has before it sets anything itself, which
	/// is what `ENV.X` reads until one of its lines changes it.
	pub fn environment(&self) -> HashMap<String, String> {
		let mut env = self.defaults.clone();
		env.extend(self.exported.iter().map(|(k, v)| (k.clone(), v.clone())));
		env
	}

	/// Start `sc` from this: its environment, and which of its values a file
	/// supplied.
	pub fn install(&self, sc: &mut Scope) {
		sc.env = self.environment();
		sc.from_files = self
			.defaults
			.keys()
			.filter(|k| !self.exported.contains_key(*k))
			.cloned()
			.collect();
	}
}

impl std::fmt::Debug for Inherited {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		// Names only: a value may well be a decrypted secret.
		fn names(half: &HashMap<String, String>) -> Vec<&String> {
			let mut keys: Vec<&String> = half.keys().collect();
			keys.sort();
			keys
		}
		f.debug_struct("Inherited")
			.field("exported", &names(&self.exported))
			.field("defaults", &names(&self.defaults))
			.field("workdir", &self.workdir)
			.finish()
	}
}

/// What a `run` at this line hands the target it runs: the environment a
/// command here would be given, split into its two halves.
///
/// A value is a default when a file supplied it and nothing since has replaced
/// it. PATH is always exported: most of what a target does to PATH is its
/// `add_path()`, and a called target's own entries belong in front of that.
pub fn handed_over(sc: &Scope) -> Inherited {
	let mut out = Inherited::default();
	for (k, v) in &sc.env {
		let half = match sc.from_files.contains(k) && !k.eq_ignore_ascii_case("PATH") {
			true => &mut out.defaults,
			false => &mut out.exported,
		};
		half.insert(k.clone(), v.clone());
	}
	out.workdir = Some(sc.workdir.clone());
	out
}
