//! The prepare gate.
//!
//! A target named `setup` is the gate for its directory: every other target in
//! that directory requires it to have been run, and to have been run against
//! its current definition. 87 of the corpus's 88 `prepare` values were already
//! `@setup`, so naming it is enough and nothing has to be declared.
//!
//! The hash covers the setup file's own text, so editing what setup *does*
//! re-triggers the requirement while runtime values do not.

use runfile_discovery::{Catalog, Origin, Target};
use runfile_settings::PrepareState;

/// The `setup` target that governs a given target, if there is one.
fn gate_for<'a>(cat: &'a Catalog, t: &Target) -> Option<&'a Target> {
	let name = match t.name.rsplit_once(':') {
		Some((ns, _)) if t.origin == Origin::Included => format!("{ns}:setup"),
		_ => "setup".to_string(),
	};
	if t.name == name {
		return None; // the gate does not gate itself
	}
	cat.resolve(&name)
}

fn digest(t: &Target) -> Option<String> {
	let src = std::fs::read_to_string(&t.path).ok()?;
	// The parsed tree, not the text: reflowing a comment in a setup target is
	// not a change to what it does, and used to re-trigger the gate.
	let ast = runfile_lang::parse(&src).ok()?;
	Some(format!("{:x}", runfile_lang::fingerprint(&ast)))
}

pub fn enforce(cat: &Catalog, t: &Target) -> Result<(), String> {
	if std::env::var_os("RUNFILE_SKIP_PREPARE").is_some_and(|v| !v.is_empty()) || crate::ci_detect::is_ci() {
		return Ok(());
	}
	let Some(gate) = gate_for(cat, t) else { return Ok(()) };
	let Some(want) = digest(gate) else { return Ok(()) };
	let state = PrepareState::load().unwrap_or_default();
	match state.recorded_hash(&gate.path) {
		Some(have) if have == want => Ok(()),
		Some(_) => Err(format!(
			"`{}` has changed since you last ran it\n\n    run {}",
			gate.name, gate.name
		)),
		None => Err(format!("`{}` has never been run\n\n    run {}", gate.name, gate.name)),
	}
}

/// Called after a gate target succeeds, so the next run passes.
pub fn record(cat: &Catalog, t: &Target) {
	if gate_for(cat, t).is_some() {
		return; // not a gate itself
	}
	let Some(want) = digest(t) else { return };
	let mut state = PrepareState::load().unwrap_or_default();
	state.record(&t.path, want);
	let _ = state.save();
}
