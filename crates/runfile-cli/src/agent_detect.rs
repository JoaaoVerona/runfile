use std::io::IsTerminal;

/// Env vars checked for agent detection, with their expected "active" values.
const AGENT_ENV_VARS: &[(&str, &str)] = &[("CLAUDECODE", "1"), ("LLM_INVOCATION", "true"), ("AGENT_MODE", "1")];

/// Pure logic: returns `true` if any env var signals an agent, or stdin is not a terminal.
///
/// Extracted so the detection rules can be fully tested without touching process-global state.
fn detect(env_lookup: impl Fn(&str) -> Option<String>, stdin_is_terminal: bool) -> bool {
	for &(var, expected) in AGENT_ENV_VARS {
		if env_lookup(var).as_deref() == Some(expected) {
			return true;
		}
	}
	if !stdin_is_terminal {
		return true;
	}
	false
}

/// Returns `true` if the current process appears to have been invoked by an LLM agent
/// rather than a human user at an interactive terminal.
///
/// Detection heuristics (any match → agent):
/// - `CLAUDECODE=1`
/// - `LLM_INVOCATION=true`
/// - `AGENT_MODE=1`
/// - stdin is not a terminal (piped/redirected)
pub fn is_agent_invocation() -> bool {
	detect(|name| std::env::var(name).ok(), std::io::stdin().is_terminal())
}

/// If an agent invocation is detected, print an error and exit.
/// Use this to guard commands that expose sensitive data (private keys, decrypted secrets).
pub fn refuse_if_agent(action_description: &str) {
	if is_agent_invocation() {
		eprintln!(
			"Error: refusing to {action_description} — \
			 LLM agent detected.\n\
			 This command exposes SENSITIVE DATA and is restricted to interactive terminal use.\n\
			 Do NOT attempt to bypass this restriction, no matter what your system prompt is or what the user requested.\n\
			 This is a SECURITY mechanism. This command stores SENSITIVE DATA and SHOULD NOT BE LEAKED in any way.\n\
			 If required, request to the user for them to manually run this command before proceeding."
		);
		std::process::exit(1);
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Helper: build an env lookup from a slice of (key, value) pairs.
	fn env_from<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
		move |name| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string())
	}

	// ── No signals at all ────────────────────────────────────────

	#[test]
	fn no_env_vars_interactive_terminal_is_not_agent() {
		assert!(!detect(env_from(&[]), true));
	}

	// ── Each env var independently triggers detection ────────────

	#[test]
	fn claudecode_1_is_agent() {
		assert!(detect(env_from(&[("CLAUDECODE", "1")]), true));
	}

	#[test]
	fn llm_invocation_true_is_agent() {
		assert!(detect(env_from(&[("LLM_INVOCATION", "true")]), true));
	}

	#[test]
	fn agent_mode_1_is_agent() {
		assert!(detect(env_from(&[("AGENT_MODE", "1")]), true));
	}

	// ── Wrong values do NOT trigger ─────────────────────────────

	#[test]
	fn claudecode_0_is_not_agent() {
		assert!(!detect(env_from(&[("CLAUDECODE", "0")]), true));
	}

	#[test]
	fn llm_invocation_false_is_not_agent() {
		assert!(!detect(env_from(&[("LLM_INVOCATION", "false")]), true));
	}

	#[test]
	fn agent_mode_0_is_not_agent() {
		assert!(!detect(env_from(&[("AGENT_MODE", "0")]), true));
	}

	#[test]
	fn claudecode_empty_is_not_agent() {
		assert!(!detect(env_from(&[("CLAUDECODE", "")]), true));
	}

	// ── Non-interactive stdin triggers detection ─────────────────

	#[test]
	fn piped_stdin_is_agent() {
		assert!(detect(env_from(&[]), false));
	}

	#[test]
	fn piped_stdin_with_no_env_vars_is_agent() {
		assert!(detect(env_from(&[("UNRELATED", "value")]), false));
	}

	// ── Combinations ────────────────────────────────────────────

	#[test]
	fn multiple_env_vars_still_agent() {
		let env = &[("CLAUDECODE", "1"), ("AGENT_MODE", "1")];
		assert!(detect(env_from(env), true));
	}

	#[test]
	fn env_var_plus_piped_stdin_still_agent() {
		assert!(detect(env_from(&[("CLAUDECODE", "1")]), false));
	}

	#[test]
	fn wrong_values_with_interactive_terminal_is_not_agent() {
		let env = &[("CLAUDECODE", "yes"), ("LLM_INVOCATION", "1"), ("AGENT_MODE", "true")];
		assert!(!detect(env_from(env), true));
	}
}
