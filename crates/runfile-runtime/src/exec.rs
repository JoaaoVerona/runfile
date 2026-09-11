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
	/// `cmd` carries its own quoting: it is a command someone wrote when the
	/// runner knows which one, and a phrase when it does not.
	#[error("{cmd} exited with status {code}")]
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
/// Whether this command line names a shell, and so whether its body is shell
/// text. `None` is the default shell, which always is one.
///
/// Public because `render` has to ask before it interpolates: a shell body is
/// shell-quoted, and anybody else's is not.
pub fn body_is_shell(command: Option<&str>) -> bool {
	match command {
		None => true,
		Some(cmd) => split_command(cmd)
			.first()
			.is_some_and(|program| is_shell(Path::new(program))),
	}
}

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

/// The program to start, and every argument it takes ahead of the script.
///
/// A shell's `-e` goes *after* the words it was named with. In front of them it
/// is an argument to the wrong thing: `busybox sh` names its applet first, so
/// `busybox -e sh` asked for an applet called `-e` and ran nothing, and bash
/// reads its long options only ahead of the short ones, so `bash -e --posix`
/// was refused outright. A one-word shell -- almost every `.shell`, and the
/// default always -- has nothing to go after and comes out as it always did.
fn program_and_args(command: Option<&str>) -> Result<(PathBuf, Vec<String>), ExecError> {
	let (program, mut args): (PathBuf, Vec<String>) = match command {
		Some(cmd) => {
			let parts = split_command(cmd);
			let (head, rest) = parts.split_first().ok_or(ExecError::NoShell)?;
			(PathBuf::from(head), rest.to_vec())
		}
		None => (crate::shell::default_shell().ok_or(ExecError::NoShell)?, Vec::new()),
	};
	if is_shell(&program) {
		args.push("-e".into());
	}
	Ok((program, args))
}

/// Hand a shell its script as the argument after `-c`.
///
/// Its own function because Windows needs one script quoted that the standard
/// library leaves bare; everything else takes the path it always took.
fn push_script(c: &mut Command, body: &str) {
	#[cfg(windows)]
	if body.contains('\n') && !body.contains([' ', '\t']) {
		use std::os::windows::process::CommandExt;
		c.arg("-c").raw_arg(windows_quoted(body));
		return;
	}
	c.arg("-c").arg(body);
}

/// One Windows command-line argument, quoted.
///
/// The standard library quotes an argument that holds a space or a tab, and
/// nothing else -- a newline does not count. So a block whose every line is a
/// bare word (`true`, then `false`) reached the command line unquoted, and the
/// shell's own parser, which does treat a newline as a separator, saw several
/// arguments and ran only the first: everything below line one was dropped and
/// a block that should have failed succeeded. A block with a space anywhere in
/// it -- almost every real one, which is why this went unnoticed -- was quoted
/// by the standard library and worked, so only the scripts that are already
/// broken take this path.
///
/// The rules are `CommandLineToArgvW`'s, which is what the standard library
/// implements too: a run of backslashes is doubled only where a quote follows
/// it or ends the argument.
#[cfg(any(windows, test))]
fn windows_quoted(arg: &str) -> String {
	let mut out = String::with_capacity(arg.len() + 2);
	out.push('"');
	let mut backslashes = 0usize;
	for c in arg.chars() {
		match c {
			'\\' => backslashes += 1,
			'"' => {
				for _ in 0..=backslashes {
					out.push('\\');
				}
				backslashes = 0;
			}
			_ => backslashes = 0,
		}
		out.push(c);
	}
	for _ in 0..backslashes {
		out.push('\\');
	}
	out.push('"');
	out
}

/// Keeps this process's own standard handles out of a detached child.
///
/// `Stdio::null()` says what a child *uses*, not what it *holds*. Windows
/// spawns with `bInheritHandles: TRUE`, so every inheritable handle here is
/// duplicated into the child -- including the pipes our own stdout and stderr
/// were handed on. A detached command then holds them open after the run is
/// over, and whoever is reading them waits for exactly the command that was
/// meant to outlive it: `.detach` returned at once and the caller still sat
/// there for the full thirty seconds.
///
/// Unix needs none of this: everything but the three descriptors a child is
/// given is close-on-exec.
///
/// Clearing the flag is safe for the children spawned meanwhile, `.parallel`
/// included, because `Stdio::inherit()` does not rely on it -- the standard
/// library duplicates the handle it passes with `bInheritHandle` set, whatever
/// the original says. It is restored on drop all the same.
#[cfg(windows)]
struct KeepHandles([(windows_sys::Win32::Foundation::HANDLE, u32); 3]);

#[cfg(windows)]
impl KeepHandles {
	fn clear() -> Self {
		use windows_sys::Win32::Foundation::{GetHandleInformation, HANDLE_FLAG_INHERIT, SetHandleInformation};
		use windows_sys::Win32::System::Console::{
			GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
		};
		let mut saved = [(std::ptr::null_mut(), 0u32); 3];
		for (slot, id) in saved
			.iter_mut()
			.zip([STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE])
		{
			// SAFETY: `GetStdHandle` answers with a handle this process owns or
			// with null, and both calls only read or set one flag on it.
			unsafe {
				let h = GetStdHandle(id);
				let mut flags = 0u32;
				// A handle we cannot ask about is one we must not change: a
				// redirected-to-nothing stream has none to restore.
				if !h.is_null() && GetHandleInformation(h, &mut flags) != 0 {
					SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0);
					*slot = (h, flags & HANDLE_FLAG_INHERIT);
				}
			}
		}
		Self(saved)
	}
}

#[cfg(windows)]
impl Drop for KeepHandles {
	fn drop(&mut self) {
		use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
		for (h, flag) in self.0 {
			if !h.is_null() {
				// SAFETY: the handle came from `GetStdHandle` above and is put
				// back exactly as it was found.
				unsafe { SetHandleInformation(h, HANDLE_FLAG_INHERIT, flag) };
			}
		}
	}
}

/// Run, and report the exit status rather than failing on it.
///
/// A command that exits non-zero is an *answer* here, not a failure -- that is
/// what `if $ cmd` and `code_of($ cmd)` are for. A command that could not be
/// started at all still fails: a missing shell is an environment problem, not
/// something a target asked about.
pub fn spawn_code(s: Spawn<'_>) -> Result<i32, ExecError> {
	match spawn(s) {
		Ok(_) => Ok(0),
		Err(ExecError::Status { code, .. }) => Ok(code),
		Err(e) => Err(e),
	}
}

pub fn spawn(s: Spawn<'_>) -> Result<String, ExecError> {
	let (program, args) = program_and_args(s.command)?;
	let shell = is_shell(&program);
	let label = s
		.command
		.map(str::to_string)
		.unwrap_or_else(|| program.display().to_string());

	if s.dry_run {
		// A capture still has to yield something; an empty string keeps the
		// rest of the target evaluable so dry-run reaches every statement.
		return Ok(String::new());
	}
	// Announced from inside the script where that is safe, so each command is
	// named as it runs rather than all of them before any of them do.
	let script = if s.announce && shell { traced(s.body) } else { None };
	if s.announce && script.is_none() {
		// stderr, so a pipeline reading `run`'s output is unaffected. Bold
		// cyan when a terminal is watching, plain when it is not.
		announce(&label, s.body);
	}

	let mut c = Command::new(&program);
	c.args(&args).current_dir(s.cwd);
	if shell {
		// A shell gets its script as an argument, so **stdin stays the
		// terminal**. Handing the script over on stdin instead left every
		// interactive command inside it -- `ssh`, `vim`, a REPL, anything
		// asking for a password -- reading a pipe that was already at EOF.
		push_script(&mut c, script.as_deref().unwrap_or(s.body));
		// Except when detached: a background process must not hold the
		// terminal's input after the run that started it is over.
		c.stdin(if s.detach { Stdio::null() } else { Stdio::inherit() });
	} else {
		// `exec <command>` hands the body to that command's stdin; that is
		// what `exec tee file` and `exec python3` are for.
		c.stdin(Stdio::piped());
	}
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
	// Held across the spawn only: see `KeepHandles`.
	#[cfg(windows)]
	let _keep = s.detach.then(KeepHandles::clear);
	let mut child = c.spawn().map_err(|e| ExecError::Spawn {
		cmd: label.clone(),
		source: e,
	})?;
	if let Some(mut stdin) = child.stdin.take() {
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
			cmd: failed_label(&program, s.command, s.body, script.is_some()),
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
/// The prefix every message from the runner carries.
///
/// Exported so a message written anywhere -- an error, a warning, a prompt --
/// is marked as the runner's rather than the target's. A person reading a
/// terminal is watching two things talk at once, and the whole point of the
/// prefix is telling them apart.
pub fn tag() -> &'static str {
	tags().0
}

/// The tag every announcement carries, bold cyan when a terminal is watching.
fn tags() -> (&'static str, &'static str, &'static str) {
	if std::io::IsTerminal::is_terminal(&std::io::stderr()) {
		("\x1b[1m\x1b[36m[runfile]\x1b[0m", "\x1b[1m", "\x1b[0m")
	} else {
		("[runfile]", "", "")
	}
}

/// A script that announces each of its own commands as it reaches it.
///
/// A block of `$` lines is one process -- that is what makes `cd` persist --
/// so the runner cannot see when each command starts; it hands the whole
/// script over at once and could only ever print all of them up front. Putting
/// the announcement *inside* the script is how each command is named as it
/// runs, and how a failure lands under the line that caused it.
///
/// `None` when that would be unsafe, and the caller announces the block as a
/// whole instead. A `for` or an `if` spread over several `$` lines is one
/// command to the shell, and a line inserted into the middle of it would cut
/// it in half; so would one inserted inside a quote or a heredoc that spans
/// lines. The test is deliberately blunt: anything it is unsure of falls back.
fn traced(body: &str) -> Option<String> {
	let lines: Vec<&str> = body.lines().collect();
	// One command already announces itself in the right place.
	if lines.len() < 2 {
		return None;
	}
	let (tag, bold, reset) = tags();
	let mut out = String::new();
	for l in &lines {
		let t = l.trim();
		if t.is_empty() {
			out.push('\n');
			continue;
		}
		if !standalone(t) {
			return None;
		}
		let said = runfile_lang::Value::Str(format!("{tag} {bold}{t}{reset}")).to_shell();
		out.push_str(&format!("printf '%s\\n' {said} >&2\n"));
		out.push_str(l);
		out.push('\n');
	}
	Some(out)
}

/// Whether a line is a whole command, so an announcement may precede it.
fn standalone(line: &str) -> bool {
	// A heredoc's body is the following lines; a quote may open here and close
	// somewhere below. Neither can have anything put between.
	if line.contains("<<") || line.matches('\'').count() % 2 == 1 || line.matches('"').count() % 2 == 1 {
		return false;
	}
	const CONTINUES: &[&str] = &["\\", "&&", "||", "|", "do", "then", "else", "in", "{", "(", ";"];
	if CONTINUES.iter().any(|o| line.ends_with(o)) {
		return false;
	}
	const CLOSES: &[&str] = &["fi", "done", "esac", "else", "elif", "}", ")", ";;"];
	!CLOSES.contains(&line.split_whitespace().next().unwrap_or(""))
}

/// What to name when a command fails.
///
/// Never the shell. A person wrote `$ docker compose up -d`, not bash, and
/// being told that `/usr/bin/bash` exited with status 1 names an
/// implementation detail and nothing they can act on -- the same reason a
/// `.parallel` branch is never labelled `bash`.
///
/// Several `$` lines share one shell and `-e` stops at the one that failed,
/// which the runner cannot see. Where the script announces each command as it
/// runs, it does not have to: the last one shown is the one that stopped.
/// Where it does not -- a `for` or a heredoc spread over several lines, which
/// is announced whole -- there is no such line to point at, and the block is
/// named instead. Pointing at output that says something else would be worse
/// than saying less.
fn failed_label(program: &Path, command: Option<&str>, body: &str, traced: bool) -> String {
	if !is_shell(program) {
		let named = command.map_or_else(|| program.display().to_string(), str::to_string);
		return format!("`{named}`");
	}
	let mut lines = body.lines().map(str::trim).filter(|l| !l.is_empty());
	match (lines.next(), lines.next()) {
		(Some(only), None) => format!("`{only}`"),
		(Some(_), Some(_)) if traced => "the command above".to_string(),
		(Some(first), Some(_)) => format!("a command in `{first} …`"),
		_ => format!("`{}`", program.display()),
	}
}

fn announce(program: &str, body: &str) {
	let (tag, bold, reset) = tags();
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
	use super::{Spawn, is_shell, program_and_args, spawn_code, windows_quoted};
	use std::path::Path;

	#[test]
	fn a_script_is_quoted_the_way_windows_reads_one_back() {
		// `CommandLineToArgvW`'s rules, which the standard library implements
		// too: a run of backslashes is doubled only where a quote follows it
		// or ends the argument. Tested on every platform, because the encoding
		// is a pure question about text and the platform that needs it is the
		// one this cannot run on.
		assert_eq!(windows_quoted("true\nfalse"), "\"true\nfalse\"");
		assert_eq!(windows_quoted(""), "\"\"");
		// A lone backslash is literal: nothing follows it to escape.
		assert_eq!(windows_quoted("a\\b"), "\"a\\b\"");
		// Before a quote it doubles, and the quote itself takes one more.
		assert_eq!(windows_quoted("a\\\"b"), "\"a\\\\\\\"b\"");
		assert_eq!(windows_quoted("a\"b"), "\"a\\\"b\"");
		// And at the end, where the closing quote would otherwise be escaped.
		assert_eq!(windows_quoted("a\\"), "\"a\\\\\"");
		assert_eq!(windows_quoted("a\\\\"), "\"a\\\\\\\\\"");
	}

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

	#[test]
	fn stop_on_failure_goes_after_the_words_the_shell_is_named_with() {
		// The tests above say *whether* `-e` is added; this is where it lands,
		// which is the other half of getting it right. In front of the words it
		// went to the wrong thing: busybox took `-e` for the name of an applet,
		// and bash refuses a long option once it has read a short one.
		let argv = |cmd: &str| {
			let (program, args) = program_and_args(Some(cmd)).expect("a program");
			let mut argv = vec![program.display().to_string()];
			argv.extend(args);
			argv
		};
		assert_eq!(argv("bash"), ["bash", "-e"]);
		assert_eq!(argv("busybox sh"), ["busybox", "sh", "-e"]);
		assert_eq!(argv("bash --posix"), ["bash", "--posix", "-e"]);
		// Not a shell, so nothing is added at either end.
		assert_eq!(argv("docker run -i alpine sh"), ["docker", "run", "-i", "alpine", "sh"]);
	}

	#[test]
	fn busybox_named_with_its_applet_runs_and_stops_on_failure() {
		// The order above is only worth pinning if it is the one busybox reads,
		// and `-e` only worth moving if it still means stop-on-failure where it
		// went. Skipped where busybox is not installed, as shellcheck's are.
		if which::which("busybox").is_err() {
			eprintln!("skipped: busybox is not installed");
			return;
		}
		let dir = std::env::temp_dir();
		let status = |body: &str| {
			spawn_code(Spawn {
				command: Some("busybox sh"),
				body,
				cwd: &dir,
				env: &[],
				capture: false,
				dry_run: false,
				label: None,
				detach: false,
				announce: false,
			})
			.expect("busybox starts")
		};
		// `busybox -e sh` answered 127 here: there is no applet called `-e`.
		assert_eq!(status("true"), 0, "busybox ran nothing");
		// Without `-e` a script's status is its last command's, which is 0.
		assert_eq!(status("false\ntrue"), 1, "busybox did not stop at `false`");
	}
}
