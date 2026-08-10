//! Which caller-supplied arguments a set of command templates references.
//!
//! Two very different consumers need the same answer: the executor validates
//! what the user actually passed against what the target can accept, and the
//! MCP server turns it into a JSON schema describing the target's inputs. They
//! used to scan independently and drifted — the MCP copy recognised only a
//! substitution whose *entire* body was `ARGS`, so a target consuming its
//! positional through `{{ one_of(ARGS, 'major', 'minor') }}` was advertised as
//! taking no input at all. Keeping the scan here, once, is what stops that
//! happening again.

use std::collections::HashSet;

/// What a set of command templates references from the caller's arguments.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArgUsage {
	/// A bare `ARGS` token appears somewhere, so the target consumes positionals.
	pub uses_positional: bool,
	/// Keys from `ARG.<key>` — string-valued named arguments.
	pub arg_keys: HashSet<String>,
	/// Keys from `FLAG.<key>` — boolean flags.
	pub flag_keys: HashSet<String>,
	/// The `arg_keys` that appeared at least once with no `?` fallback in their
	/// substitution, and which the caller therefore has to supply.
	pub required_keys: HashSet<String>,
}

impl ArgUsage {
	/// Every named key, `ARG.` and `FLAG.` alike — the view argument *validation*
	/// takes, which only asks whether `--name` is referenced at all.
	pub fn named_keys(&self) -> HashSet<String> {
		self.arg_keys.union(&self.flag_keys).cloned().collect()
	}
}

/// Scan already-walked command templates for `ARGS` / `ARG.<key>` / `FLAG.<key>`
/// references inside `{{ ... }}` substitutions.
///
/// Callers holding a `Vec<CommandStep>` should flatten it first with
/// [`crate::walk_step_templates`], and add any other strings that get
/// substituted (a target's `env` values, say).
pub fn scan_arg_usage(templates: &[String]) -> ArgUsage {
	let mut usage = ArgUsage::default();
	for template in templates {
		scan_one_template(template, &mut usage);
	}
	usage
}

/// Walk one template, handing each `{{ ... }}` body to [`collect_from_substitution`].
///
/// This loop may step a byte at a time: `i` indexes `bytes` (never panics), and
/// every `template[..]` slice below is taken at an offset derived from an ASCII
/// `{{` / `}}` anchor, which is always a character boundary. A multi-byte
/// character matches none of the branches and gets walked over. Keep it that
/// way — slicing `template` at a raw `i` here would introduce a panic on any
/// Runfile whose commands carry non-ASCII text.
fn scan_one_template(template: &str, usage: &mut ArgUsage) {
	let bytes = template.as_bytes();
	let mut i = 0;
	while i < bytes.len() {
		let b = bytes[i];
		// Skip escape sequences so a literal `\{{` doesn't register as a
		// substitution when scanning for arg usage.
		if b == b'\\' && bytes.get(i + 1) == Some(&b'{') && bytes.get(i + 2) == Some(&b'{') {
			i += 3;
			continue;
		}
		if b == b'\\' && bytes.get(i + 1) == Some(&b'}') && bytes.get(i + 2) == Some(&b'}') {
			i += 3;
			continue;
		}
		if b == b'{' && bytes.get(i + 1) == Some(&b'{') {
			let body_start = i + 2;
			if let Some(rel_close) = template[body_start..].find("}}") {
				let body = template[body_start..body_start + rel_close].trim();
				// A `?` anywhere in the body means the chain has a fallback, so
				// every key it references is optional.
				collect_from_substitution(body, body.contains('?'), usage);
				i = body_start + rel_close + 2;
				continue;
			}
			// Unterminated — give up scanning the rest (real validation surfaces
			// the error elsewhere).
			break;
		}
		i += 1;
	}
}

/// Collect every `ARG.<key>` / `FLAG.<key>` / bare `ARGS` in one substitution
/// body. A chain may hold several (`{{ ARG.a ? ARG.b }}`), and a bare `ARGS`
/// counts wherever it appears — on its own (`{{ ARGS }}`), inside a chain, or as
/// a function argument (`{{ one_of(ARGS, ...) }}`) — because all of them consume
/// the caller's positionals.
fn collect_from_substitution(inner: &str, has_default: bool, usage: &mut ArgUsage) {
	let bytes = inner.as_bytes();
	let mut i = 0;
	while i < bytes.len() {
		// `i` advances a whole character at a time, or past an all-ASCII
		// `ARG.<key>` / `FLAG.<key>` / `ARGS` token — so it always lands on a
		// character boundary and this slice cannot panic. Stepping one *byte*
		// would blow up on the first multi-byte character, and substitutions
		// carry prose in practice: `{{ error('… — …') }}`.
		let s = &inner[i..];
		if let Some(rest) = s.strip_prefix("ARG.") {
			let key = leading_key(rest);
			if !key.is_empty() {
				let advance = "ARG.".len() + key.len();
				if !has_default {
					usage.required_keys.insert(key.clone());
				}
				usage.arg_keys.insert(key);
				// Skip past `ARG.<key>` so the bare-ARGS scan below doesn't
				// double-count the same token.
				i += advance;
				continue;
			}
		} else if let Some(rest) = s.strip_prefix("FLAG.") {
			let key = leading_key(rest);
			if !key.is_empty() {
				let advance = "FLAG.".len() + key.len();
				usage.flag_keys.insert(key);
				i += advance;
				continue;
			}
		}
		// Bare `ARGS` (no trailing `.`). Bound the match with non-identifier
		// characters on both sides so it doesn't fire on `ARGS_FOO` or `MYARGS`;
		// a trailing `.` means it is really `ARG.<key>`, handled above.
		if s.starts_with("ARGS") {
			let prev = if i == 0 { None } else { bytes.get(i - 1).copied() };
			let next = bytes.get(i + 4).copied();
			let prev_is_word = matches!(prev, Some(c) if c.is_ascii_alphanumeric() || c == b'_');
			let next_is_continuation = matches!(next, Some(c) if c.is_ascii_alphanumeric() || c == b'_' || c == b'.');
			if !prev_is_word && !next_is_continuation {
				usage.uses_positional = true;
				i += 4;
				continue;
			}
		}
		// Nothing matched here — step over one whole character, keeping `i` on a
		// boundary. (`s` is non-empty while `i < bytes.len()`, so the default is
		// unreachable; it just keeps the expression total.)
		i += s.chars().next().map_or(1, char::len_utf8);
	}
}

/// The identifier that starts `rest` — the `<key>` of `ARG.<key>` / `FLAG.<key>`.
fn leading_key(rest: &str) -> String {
	rest.chars()
		.take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
		.collect()
}
