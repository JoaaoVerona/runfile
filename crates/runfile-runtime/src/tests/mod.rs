//! Execution tests. These spawn real processes -- the point is the process
//! model, so a mock shell would test nothing.

mod args;
mod exec;
mod interrupt;
mod keys;
mod loops;
mod parallel;
mod term;
mod walk;

use crate::run::{Dispatch, RunError, Runner};
use runfile_lang::Value;
use runfile_lang::eval::Scope;

/// Records dispatched targets instead of running them. Behind a Mutex because
/// Dispatch is Sync -- a `.parallel` block calls it from several threads.
#[derive(Default)]
pub struct Recorder {
	pub calls: std::sync::Mutex<Vec<String>>,
}

impl Recorder {
	pub fn calls(&self) -> Vec<String> {
		self.calls.lock().expect("calls").clone()
	}
}

impl Dispatch for Recorder {
	fn run(
		&self,
		target: &str,
		args: &[String],
		_chain: &[String],
		_label: Option<&str>,
	) -> Result<Vec<String>, RunError> {
		self.calls.lock().expect("calls").push(if args.is_empty() {
			target.to_string()
		} else {
			format!("{target} {}", args.join(" "))
		});
		// A recorder runs nothing, so it has no trace to contribute.
		Ok(Vec::new())
	}
}

pub fn run_src(src: &str, d: &dyn Dispatch) -> Result<Vec<String>, RunError> {
	let target = runfile_lang::parse(src).expect("parse");
	let mut scope = Scope::new();
	scope.run.insert("os".into(), Value::Str("linux".into()));
	let dir = std::env::temp_dir();
	let mut scope = scope;
	scope.assume_yes = true;
	let mut r = Runner {
		scope,
		chain: Vec::new(),
		env: Vec::new(),
		anchor: dir,
		dispatch: d,
		interrupted: None,
		label: None,
		dry_run: false,
		trace: Vec::new(),
	};
	crate::run::run_target(&target, &mut r)?;
	Ok(r.trace)
}

pub fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
	let d = tempfile::TempDir::new().unwrap();
	for (p, body) in files {
		let full = d.path().join(p);
		std::fs::create_dir_all(full.parent().unwrap()).unwrap();
		std::fs::write(full, body).unwrap();
	}
	d
}

pub fn host_run(d: &tempfile::TempDir, target: &str) -> Result<Vec<String>, crate::RunError> {
	let cat = runfile_discovery::discover(d.path(), None).unwrap();
	let mut h = crate::dispatch::Host::new(&cat);
	h.assume_yes = true;
	h.run(target, &[])?;
	let t = h.trace.lock().expect("trace").clone();
	Ok(t)
}

/// Run `f` with `HOME` pointed at `home`, restoring it afterwards.
///
/// `discovery::is_machine_wide` reads the environment, and tests share one
/// process and run on several threads -- so the swap is serialized and the
/// restore happens on the way out however `f` ended.
pub fn with_home<T>(home: &std::path::Path, f: impl FnOnce() -> T) -> T {
	static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
	let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
	let prev = std::env::var_os("HOME");
	// SAFETY: the lock makes this the only thread touching HOME, and every
	// reader of it in this crate runs inside `f`.
	unsafe { std::env::set_var("HOME", home) };
	let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
	match prev {
		Some(v) => unsafe { std::env::set_var("HOME", v) },
		None => unsafe { std::env::remove_var("HOME") },
	}
	match out {
		Ok(v) => v,
		Err(p) => std::panic::resume_unwind(p),
	}
}
