//! Asking on stdin. Prompts go to stderr so a target's stdout stays pipeable,
//! and carry the runner's prefix: a question appearing in the middle of a
//! target's own output should say who is asking.

use std::io::{IsTerminal, Write};

pub fn confirmer() -> impl Fn(&str) -> bool + Sync {
	|msg: &str| {
		if !std::io::stdin().is_terminal() {
			return false;
		}
		eprint!("{} {msg} [y/N] ", runfile_runtime::exec::tag());
		let _ = std::io::stderr().flush();
		let mut line = String::new();
		if std::io::stdin().read_line(&mut line).is_err() {
			return false;
		}
		matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
	}
}

/// Asked for a value a target needs but was not given, under `--stdin-args`.
pub fn ask_value(kind: &str, name: &str) -> Option<String> {
	if !std::io::stdin().is_terminal() {
		return None;
	}
	eprint!("{} {kind} {name} (required): ", runfile_runtime::exec::tag());
	let _ = std::io::stderr().flush();
	let mut line = String::new();
	std::io::stdin().read_line(&mut line).ok()?;
	let v = line.trim().to_string();
	(!v.is_empty()).then_some(v)
}
