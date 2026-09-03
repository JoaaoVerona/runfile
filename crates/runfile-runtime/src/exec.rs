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

pub fn spawn(s: Spawn<'_>) -> Result<String, ExecError> {
	let (program, args): (PathBuf, Vec<String>) = match s.command {
		Some(cmd) => {
			let parts = split_command(cmd);
			let (head, rest) = parts.split_first().ok_or(ExecError::NoShell)?;
			(PathBuf::from(head), rest.to_vec())
		}
		// `$` gets the runner's defaults, including stop-on-failure.
		None => (
			crate::shell::default_shell().ok_or(ExecError::NoShell)?,
			vec!["-e".into()],
		),
	};
	let label = s
		.command
		.map(str::to_string)
		.unwrap_or_else(|| program.display().to_string());

	let mut c = Command::new(&program);
	c.args(&args).current_dir(s.cwd).stdin(Stdio::piped());
	for (k, v) in s.env {
		c.env(k, v);
	}
	if s.capture {
		c.stdout(Stdio::piped());
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
