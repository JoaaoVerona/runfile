//! Spawning an `exec` block.
//!
//! Every block is exactly one process. The body is written to the command's
//! stdin, so `exec sudo tee f`, `exec node` and `exec ssh host bash` all work
//! by the same mechanism -- the command is arbitrary, not a fixed interpreter
//! list.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, thiserror::Error)]
pub enum ExecError {
	#[error("no shell found: install bash, or Git for Windows on Windows")]
	NoShell,
	#[error("could not start `{cmd}`: {source}")]
	Spawn { cmd: String, source: std::io::Error },
	#[error("`{cmd}` exited with status {code}")]
	Status { cmd: String, code: i32 },
	#[error("`{cmd}` produced output that is not UTF-8")]
	NotUtf8 { cmd: String },
}

pub struct Spawn<'a> {
	/// `None` means the default shell, which is what `$` resolves to.
	pub command: Option<&'a str>,
	pub body: &'a str,
	pub cwd: &'a Path,
	pub env: &'a [(String, String)],
	pub capture: bool,
	/// Print what would run instead of running it. An `exec` body is literal
	/// text, so dry-run shows exactly what the command would receive -- more
	/// faithful than it could ever be over an opaque script file.
	pub dry_run: bool,
	/// Prefix every output line with this, for a `.parallel` fan-out where
	/// several children write at once. `None` inherits the terminal, which is
	/// what a sequential run wants: no prefix, no extra pipe, colours intact.
	pub label: Option<&'a str>,
	/// Start it and do not wait. For a fire-and-forget command whose whole
	/// point is to outlive the run: a dev server, a log tailer.
	pub detach: bool,
	/// Announce the command before running it, on stderr. The runner does this
	/// natively, which is why there is no `logging` property to turn it on.
	pub announce: bool,
}

/// Split a command line into program and arguments, respecting quotes so
/// `exec docker run -i --rm python:3 python` reaches the right program.
fn split_command(cmd: &str) -> Vec<String> {
	let (mut out, mut cur, mut quote) = (Vec::new(), String::new(), None::<char>);
	for c in cmd.chars() {
		match (quote, c) {
			(Some(q), _) if c == q => quote = None,
			(Some(_), _) => cur.push(c),
			(None, '\'' | '"') => quote = Some(c),
			(None, c) if c.is_whitespace() => {
				if !cur.is_empty() {
					out.push(std::mem::take(&mut cur));
				}
			}
			(None, _) => cur.push(c),
		}
	}
	if !cur.is_empty() {
		out.push(cur);
	}
	out
}

/// Shells that take `-e` and mean stop-on-failure by it.
///
/// `brush` is here because it is a bash-compatible shell someone may name in
/// `.shell`, and without it a `brush` target would silently lose the
/// stop-on-failure every other shell gets. It is not a *default* candidate:
/// the default has to supply the POSIX toolbox as well as the language, which
/// a shell alone does not.
///
/// `$ x` must behave exactly like `exec sh` with `x` as its body, so the
/// runner's stop-on-failure default has to reach an explicitly named shell
/// too. Detection is on the *first* word only, which is what keeps
/// `exec docker run -i alpine sh` and `exec ssh host bash` out of it: there
/// the program is docker and ssh, and the inner shell is not ours to flag.
fn is_shell(program: &Path) -> bool {
	let Some(name) = program.file_name().and_then(|n| n.to_str()) else {
		return false;
	};
	let name = name.strip_suffix(".exe").unwrap_or(name);
	matches!(
		name,
		"sh" | "bash" | "dash" | "ash" | "zsh" | "ksh" | "busybox" | "brush"
	)
}

pub fn spawn(s: Spawn<'_>) -> Result<String, ExecError> {
	let (program, mut args): (PathBuf, Vec<String>) = match s.command {
		Some(cmd) => {
			let parts = split_command(cmd);
			let (head, rest) = parts.split_first().ok_or(ExecError::NoShell)?;
			(PathBuf::from(head), rest.to_vec())
		}
		None => (crate::shell::default_shell().ok_or(ExecError::NoShell)?, Vec::new()),
	};
	if is_shell(&program) {
		args.insert(0, "-e".into());
	}
	let label = s
		.command
		.map(str::to_string)
		.unwrap_or_else(|| program.display().to_string());

	if s.dry_run {
		// A capture still has to yield something; an empty string keeps the
		// rest of the target evaluable so dry-run reaches every statement.
		return Ok(String::new());
	}
	let mut c = Command::new(&program);
	c.args(&args).current_dir(s.cwd).stdin(Stdio::piped());
	for (k, v) in s.env {
		c.env(k, v);
	}
	let labelled = s.label.is_some() && !s.capture;
	if s.detach {
		// Its streams have to go nowhere. Inherited, they would keep the
		// runner's own stdout and stderr open after it exits, so whoever is
		// reading them waits for a command that was meant to outlive the run.
		c.stdout(Stdio::null());
		c.stderr(Stdio::null());
	} else if s.capture {
		c.stdout(Stdio::piped());
	} else if labelled {
		c.stdout(Stdio::piped());
		c.stderr(Stdio::piped());
	}
	if s.announce {
		// stderr, so a pipeline reading `run`'s output is unaffected. Bold
		// cyan when a terminal is watching, plain when it is not.
		announce(&label, s.body);
	}
	let mut child = c.spawn().map_err(|e| ExecError::Spawn {
		cmd: label.clone(),
		source: e,
	})?;
	{
		let mut stdin = child.stdin.take().expect("stdin was piped");
		stdin.write_all(s.body.as_bytes()).map_err(|e| ExecError::Spawn {
			cmd: label.clone(),
			source: e,
		})?;
	}
	if s.detach {
		// Nothing to wait for, and nothing to report: its exit status arrives
		// long after this run is over.
		return Ok(String::new());
	}
	if labelled {
		let prefix = s.label.unwrap_or_default().to_string();
		let (out, err) = (child.stdout.take(), child.stderr.take());
		// One thread for the other stream, so neither can block the other by
		// filling its pipe while we read only from this one.
		let with = prefix.clone();
		let pump_err = std::thread::spawn(move || relay(err, &with, true));
		relay(out, &prefix, false);
		let _ = pump_err.join();
	}
	let out = child.wait_with_output().map_err(|e| ExecError::Spawn {
		cmd: label.clone(),
		source: e,
	})?;
	if !out.status.success() {
		return Err(ExecError::Status {
			cmd: label,
			code: out.status.code().unwrap_or(-1),
		});
	}
	if !s.capture {
		return Ok(String::new());
	}
	let mut text = String::from_utf8(out.stdout).map_err(|_| ExecError::NotUtf8 { cmd: label })?;
	// One trailing newline is stripped, matching what a shell substitution does.
	if text.ends_with('\n') {
		text.pop();
		if text.ends_with('\r') {
			text.pop();
		}
	}
	Ok(text)
}

/// Copy one of a child's streams out, a line at a time, behind its label.
///
/// Whole lines, because `println!` locks once per call: two branches writing at
/// the same moment interleave by line rather than mid-word. Invalid UTF-8 is
/// replaced rather than dropped -- output is for a person to read, and a
/// mangled byte should not lose the line around it.
fn relay(stream: Option<impl std::io::Read>, label: &str, is_err: bool) {
	let Some(stream) = stream else { return };
	let reader = std::io::BufReader::new(stream);
	let mut buf = Vec::new();
	let mut r = reader;
	loop {
		buf.clear();
		match std::io::BufRead::read_until(&mut r, b'\n', &mut buf) {
			Ok(0) | Err(_) => return,
			Ok(_) => {}
		}
		while matches!(buf.last(), Some(b'\n' | b'\r')) {
			buf.pop();
		}
		let line = String::from_utf8_lossy(&buf);
		if is_err {
			eprintln!("{label} | {line}");
		} else {
			println!("{label} | {line}");
		}
	}
}

/// Print the command about to run.
///
/// The body rather than the program, because `sh` is what almost every block
/// is and `cargo build` is what the reader wants to see. A multi-line body is
/// shown whole: it is one process, and half of it would be a lie.
fn announce(program: &str, body: &str) {
	let colour = std::io::IsTerminal::is_terminal(&std::io::stderr());
	let (tag, bold, reset) = if colour {
		("\x1b[1m\x1b[36m[runfile]\x1b[0m", "\x1b[1m", "\x1b[0m")
	} else {
		("[runfile]", "", "")
	};
	let text = if body.trim().is_empty() {
		program
	} else {
		body.trim_end()
	};
	for line in text.lines() {
		eprintln!("{tag} {bold}{line}{reset}");
	}
}

#[cfg(test)]
mod tests {
	use super::is_shell;
	use std::path::Path;

	#[test]
	fn a_bash_compatible_shell_is_recognised_as_one() {
		// Being on this list is what inserts `-e`. A shell missing from it
		// runs fine and silently stops stopping on failure, which is the
		// worst way for this to be wrong.
		for name in ["sh", "bash", "dash", "ash", "zsh", "ksh", "busybox", "brush"] {
			assert!(is_shell(Path::new(name)), "{name}");
			assert!(is_shell(Path::new(&format!("/usr/bin/{name}"))), "{name} by path");
			assert!(is_shell(Path::new(&format!("{name}.exe"))), "{name}.exe");
		}
	}

	#[test]
	fn only_the_first_word_counts_as_the_program() {
		// `exec docker run -i alpine sh` runs docker; the inner shell is not
		// ours to flag, and `-e` would go to the wrong program.
		assert!(!is_shell(Path::new("docker")));
		assert!(!is_shell(Path::new("python3")));
		assert!(!is_shell(Path::new("brushfoo")));
	}
}
