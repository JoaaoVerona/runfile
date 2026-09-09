//! Asking on stdin. Prompts go to stderr so a target's stdout stays pipeable,
//! and carry the runner's prefix: a question appearing in the middle of a
//! target's own output should say who is asking.

use std::io::{IsTerminal, Write};

/// Asked by `confirm(…)`. A plain function rather than a closure, so it can be
/// a pointer in `Scope` the way `ask_value` is.
pub fn confirm(question: &str) -> bool {
	if !std::io::stdin().is_terminal() {
		return false;
	}
	eprint!("{} {question} [y/N] ", runfile_runtime::exec::tag());
	let _ = std::io::stderr().flush();
	let mut line = String::new();
	if std::io::stdin().read_line(&mut line).is_err() {
		return false;
	}
	matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
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

/// Ask for one input up front, showing what it falls back to.
///
/// An empty answer keeps the fallback -- which is the difference between this
/// and the old lazy prompt: a value with a default is worth offering, and
/// pressing Enter has to mean "leave it alone".
pub fn ask_input(label: &str, default: Option<&str>, required: bool) -> Option<String> {
	if !std::io::stdin().is_terminal() {
		return None;
	}
	let tail = match (default, required) {
		(Some(""), _) => " [empty]".to_string(),
		(Some(d), _) => format!(" [{d}]"),
		(None, true) => " (required)".to_string(),
		(None, false) => String::new(),
	};
	eprint!("{} {label}{tail}: ", runfile_runtime::exec::tag());
	let _ = std::io::stderr().flush();
	let mut line = String::new();
	std::io::stdin().read_line(&mut line).ok()?;
	let v = line.trim().to_string();
	(!v.is_empty()).then_some(v)
}

/// Ask whether to pass a flag. Off unless the answer is yes, which is what an
/// absent flag means anyway.
pub fn ask_flag(label: &str) -> bool {
	if !std::io::stdin().is_terminal() {
		return false;
	}
	eprint!("{} pass {label}? (y/N) ", runfile_runtime::exec::tag());
	let _ = std::io::stderr().flush();
	let mut line = String::new();
	if std::io::stdin().read_line(&mut line).is_err() {
		return false;
	}
	matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}
