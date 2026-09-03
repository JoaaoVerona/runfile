//! Execution tests. These spawn real processes -- the point is the process
//! model, so a mock shell would test nothing.

mod exec;
mod walk;

use crate::run::{Dispatch, RunError, Runner};
use runfile_lang::Value;
use runfile_lang::eval::Scope;

/// Records dispatched targets instead of running them.
#[derive(Default)]
pub struct Recorder {
	pub calls: Vec<String>,
}

impl Dispatch for Recorder {
	fn run(&mut self, target: &str, args: &[String]) -> Result<(), RunError> {
		self.calls.push(if args.is_empty() {
			target.to_string()
		} else {
			format!("{target} {}", args.join(" "))
		});
		Ok(())
	}
}

pub fn run_src(src: &str, d: &mut dyn Dispatch) -> Result<Vec<String>, RunError> {
	let target = runfile_lang::parse(src).expect("parse");
	let mut scope = Scope::new();
	scope.run.insert("os".into(), Value::Str("linux".into()));
	let dir = std::env::temp_dir();
	let mut r = Runner {
		scope,
		anchor: dir,
		dispatch: d,
		assume_yes: true,
		trace: Vec::new(),
	};
	crate::run::run_target(&target, &mut r)?;
	Ok(r.trace)
}
