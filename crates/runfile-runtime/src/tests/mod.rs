//! Execution tests. These spawn real processes -- the point is the process
//! model, so a mock shell would test nothing.

mod args;
mod exec;
mod interrupt;
mod keys;
mod parallel;
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
