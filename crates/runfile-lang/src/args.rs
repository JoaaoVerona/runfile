//! A command line, classified against what the target reads.
//!
//! `--key=value` is unambiguous and always was. A bare `--key` is not: with
//! nothing declaring which names take a value, `--target aarch64` could be an
//! argument and its value, or a flag and a positional. It was read as the
//! second, always -- so `run build --target aarch64` handed `cargo` a target
//! triple with no `--target` in front of it, and the only way to say what was
//! meant was `run build -- --target aarch64`.
//!
//! Nothing has to be declared, because `inputs::of` already walks the tree and
//! knows: a name read as `ARG.key` takes a value, a name read as `FLAG.key`
//! does not. That is the same list `--help` prints and `--stdin-args` asks
//! from, so a target cannot document one shape and parse another.
//!
//! **No command line that works today changes meaning.** A `--key=value` is
//! untouched; a bare `--key` read as `FLAG.key` is still a flag; and a bare
//! `--key` that is *not* read as a flag is, today, a hard error -- so every
//! word this newly claims came from a command line that already failed. That
//! is what makes the rule safe to change rather than a break to argue about.
//!
//! A word this classifies under no name is `Unknown`. Whether that is an error
//! or a positional is the caller's, since only the caller knows whether the
//! target reads `ARGS`.

use crate::Inputs;

/// One word of a target's command line, classified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arg {
	/// `--key=value`, or `--key value` for a name read as `ARG.key`.
	Arg { key: String, value: String },
	/// `--key` for a name read as `FLAG.key`.
	Flag(String),
	/// A positional, in the order it was written. Everything after a bare
	/// `--` is one of these, exactly as typed.
	Positional(String),
	/// A `--key` or `--key=value` the target reads under no name. `token` is
	/// the word as typed, so a caller that forwards it forwards what was
	/// written rather than a reconstruction of it.
	Unknown { token: String, key: String },
}

/// `--key` names something read as `ARG.key`, with no value to put in it.
///
/// `next` is the word that was there and could not be the value, so whoever
/// reports this can say why it was not taken rather than claiming there was
/// nothing after it. The sentence a person reads is the runner's, beside the
/// one for an input no name reads -- the two refusals are the same kind of
/// thing and are worded in one place.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`--{key}` needs a value")]
pub struct MissingValue {
	pub key: String,
	pub next: Option<String>,
}

/// Classify `argv` against what the target reads.
///
/// A bare `--key` for a name read as `ARG.key` takes the next word as its
/// value -- unless that word begins with `-`, which is refused rather than
/// swallowed: `run build --target --release` would otherwise set the triple to
/// `--release` and fail three seconds later inside `cargo`, where the mistake
/// is no longer visible. `--key=--release` is how to mean it.
///
/// A name read as **both** `ARG.key` and `FLAG.key` is a flag in its bare
/// form. That is the one case where a currently working command line exists --
/// `run x --verbose hello` is a flag and a positional today -- and preferring
/// the flag is what keeps this from changing it.
pub fn parse(argv: &[String], reads: &Inputs) -> Result<Vec<Arg>, MissingValue> {
	let mut out = Vec::with_capacity(argv.len());
	let mut i = 0;
	while i < argv.len() {
		let word = &argv[i];
		i += 1;
		// A bare `--` ends parsing: everything after it is a positional
		// exactly as typed, flags included. That is how a wrapper forwards a
		// word this would otherwise claim for itself.
		if word == "--" {
			out.extend(argv[i..].iter().cloned().map(Arg::Positional));
			break;
		}
		let Some(rest) = word.strip_prefix("--") else {
			out.push(Arg::Positional(word.clone()));
			continue;
		};
		if let Some((key, value)) = rest.split_once('=') {
			out.push(if reads.args.contains_key(key) {
				Arg::Arg {
					key: key.to_string(),
					value: value.to_string(),
				}
			} else {
				unknown(word, key)
			});
			continue;
		}
		if reads.flags.contains(rest) {
			out.push(Arg::Flag(rest.to_string()));
		} else if reads.args.contains_key(rest) {
			let next = argv.get(i);
			match next.filter(|v| !v.starts_with('-')) {
				Some(v) => {
					out.push(Arg::Arg {
						key: rest.to_string(),
						value: v.clone(),
					});
					i += 1;
				}
				None => {
					return Err(MissingValue {
						key: rest.to_string(),
						next: next.cloned(),
					});
				}
			}
		} else {
			out.push(unknown(word, rest));
		}
	}
	Ok(out)
}

/// `--=value` and `--` with nothing after the dashes name nothing, so there is
/// no name to report as unread. They are words like any other.
fn unknown(word: &str, key: &str) -> Arg {
	if key.is_empty() {
		return Arg::Positional(word.to_string());
	}
	Arg::Unknown {
		token: word.to_string(),
		key: key.to_string(),
	}
}
