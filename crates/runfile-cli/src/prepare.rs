//! The prepare gate.
//!
//! A target named `setup` is the gate for its directory: every other target in
//! that directory requires it to have been run, and to have been run against
//! its current definition. 87 of the corpus's 88 `prepare` values were already
//! `@setup`, so naming it is enough and nothing has to be declared.
//!
//! The hash covers the setup file's own text, so editing what setup *does*
//! re-triggers the requirement while runtime values do not.
//!
//! None of it applies in CI -- see [`ci`].

use runfile_discovery::{Catalog, Origin, Target};
use runfile_state::PrepareState;

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

/// CI has no gate and keeps no record of one.
///
/// A runner is built from scratch and thrown away; there is no earlier session
/// whose `setup` this one could be relying on, so asking whether one happened
/// is asking about a machine that did not exist. The consequence worth spelling
/// out is that [`record`] consults this too: a runner that never *reads*
/// `state.json` has no business *writing* one, and the file used to be created
/// on every CI run purely to be deleted by a cleanup step afterwards.
///
/// `RUNFILE_SKIP_PREPARE` is deliberately not the same thing. It turns the gate
/// off on a machine whose state is still worth keeping, so a `setup` run under
/// it is still recorded -- otherwise unsetting the variable would report a
/// setup that plainly did run as never having run.
fn ci() -> bool {
	crate::ci_detect::is_ci()
}

pub fn enforce(cat: &Catalog, t: &Target) -> Result<(), String> {
	if ci() || std::env::var_os("RUNFILE_SKIP_PREPARE").is_some_and(|v| !v.is_empty()) {
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
	if ci() {
		return; // nothing reads it there, so nothing writes it
	}
	if gate_for(cat, t).is_some() {
		return; // not a gate itself
	}
	let Some(want) = digest(t) else { return };
	let mut state = PrepareState::load().unwrap_or_default();
	state.record(&t.path, want);
	let _ = state.save();
}
