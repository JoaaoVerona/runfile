//! Preparation-target support: computing the content hash of a prepare
//! target's (recursively expanded) commands, and identifying which targets are
//! themselves preparation targets.
//!
//! A target's `prepare` requirements live on [`crate::CommandSpec::required_prepares`]
//! (baked during merge). The runner gates a target on each requirement by
//! comparing [`Runfile::prepare_command_hash`] against the hash recorded when
//! the preparation target was last run. The hash covers the *raw* (pre-
//! substitution) command definition, so it re-triggers on edits to the setup
//! script itself, not on changes to runtime argument/environment values.

use crate::schema::{CommandStep, Runfile, prepare_invocation_target};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

impl Runfile {
	/// Canonical names of every target referenced as a preparation target by any
	/// target in this Runfile (via [`crate::CommandSpec::required_prepares`]).
	/// Used to (a) exempt a preparation target from its own gate and (b) decide
	/// whether a completed direct run should record a prepare hash.
	pub fn prepare_target_names(&self) -> HashSet<String> {
		let mut names = HashSet::new();
		for spec in self.targets.values() {
			for invocation in &spec.required_prepares {
				let token = prepare_invocation_target(invocation);
				if let Some(canonical) = self.resolve_target(token) {
					names.insert(canonical.to_string());
				}
			}
		}
		names
	}

	/// A stable hex digest of a preparation target's command definition: the raw
	/// (pre-substitution) `commands` of `target`, plus the `commands` of every
	/// target it statically invokes via `@target`, walked depth-first with cycle
	/// protection. Dynamic invocations (`@{{ ... }}`) are opaque — their literal
	/// template is captured in the parent's serialization but not recursed into.
	///
	/// Returns `None` when `target` doesn't resolve to a known target. Changing
	/// any command in the transitive set changes the digest, which re-triggers
	/// the prepare requirement for every target that depends on it.
	pub fn prepare_command_hash(&self, target: &str) -> Option<String> {
		let mut visited = HashSet::new();
		let mut buf = String::new();
		self.accumulate_prepare_commands(target, &mut visited, &mut buf)?;
		Some(hex::encode(Sha256::digest(buf.as_bytes())))
	}

	fn accumulate_prepare_commands(&self, name: &str, visited: &mut HashSet<String>, buf: &mut String) -> Option<()> {
		let canonical = self.resolve_target(name)?.to_string();
		// Cycle / already-included guard: `@a → @b → @a` and diamond references
		// each contribute their commands exactly once, keeping the digest stable.
		if !visited.insert(canonical.clone()) {
			return Some(());
		}
		let spec = self.targets.get(&canonical)?;
		// Header the block with the canonical name so two different structures
		// can't collapse to the same serialization, then the raw command tree.
		buf.push_str(&canonical);
		buf.push('\n');
		buf.push_str(&serde_json::to_string(&spec.commands).unwrap_or_default());
		buf.push('\n');

		let mut refs = Vec::new();
		collect_target_calls(&spec.commands, &mut refs);
		for target_ref in refs {
			// Dynamic `@{{ ... }}` names can't be resolved statically; the literal
			// template is already in this target's serialization above.
			if target_ref.contains("{{") {
				continue;
			}
			// A `@dep` that doesn't resolve is an execution-time error, not a
			// hashing concern — skip it silently here.
			let _ = self.accumulate_prepare_commands(&target_ref, visited, buf);
		}
		Some(())
	}
}

/// Collect the static target names referenced by `@target` calls anywhere in a
/// command-step tree, in encounter order. Mirrors the traversal of
/// [`crate::merge`]'s namespace rewriter.
fn collect_target_calls(steps: &[CommandStep], out: &mut Vec<String>) {
	for step in steps {
		match step {
			CommandStep::Shell(_) => {}
			CommandStep::TargetCall(call) => out.push(call.target.clone()),
			CommandStep::When(w) => collect_target_calls(&w.commands, out),
			CommandStep::If(i) => {
				collect_target_calls(&i.then, out);
				if let Some(else_branch) = &i.r#else {
					collect_target_calls(else_branch, out);
				}
			}
			CommandStep::For(f) => collect_target_calls(&f.body, out),
			CommandStep::Match(m) => {
				for case_steps in m.cases.values() {
					collect_target_calls(case_steps, out);
				}
				if let Some(default) = &m.default {
					collect_target_calls(default, out);
				}
			}
		}
	}
}
