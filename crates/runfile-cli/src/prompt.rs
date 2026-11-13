//! Asking on stdin. Prompts go to stderr so a target's stdout stays pipeable,
//! and carry the runner's prefix: a question appearing in the middle of a
//! target's own output should say who is asking.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};

/// Set once stdin has reached its end: nobody is left to answer, so nothing
/// more is asked. A prompt per input with no answer to any of them would only
/// bury the error the run is about to report.
static EXHAUSTED: AtomicBool = AtomicBool::new(false);

/// Asked by `confirm(…)`. A plain function rather than a closure, so it can be
/// a pointer in `Scope` the way `ask_value` is.
///
/// Only a terminal is asked: nothing has said that anyone answers on stdin,
/// and a pipe that happens to hold a `y` is not somebody agreeing.
///
/// Interrupted while it waits, it declines -- a `Cancelled`, which nothing
/// inside the run forgives, and which `main` reports as the 130 an interrupt
/// is because the flag is set.
pub fn confirm(question: &str) -> bool {
	std::io::stdin().is_terminal() && yes(question)
}

/// `confirm(…)` under `--stdin-args`, which says somebody answers on stdin --
/// so it is asked wherever every other question is. The VS Code extension's
/// task terminal is a pipe, and a target that asked before a restore was
/// declined there without a word reaching the person sitting at it.
pub fn confirm_on_stdin(question: &str) -> bool {
	answerable() && yes(question)
}

fn yes(question: &str) -> bool {
	eprint!("{} {question} [y/N] ", runfile_runtime::exec::tag());
	let _ = std::io::stderr().flush();
	answer().is_some_and(|a| matches!(a.to_ascii_lowercase().as_str(), "y" | "yes"))
}

/// Whether `--stdin-args` may wait on stdin for an answer.
///
/// The flag is the caller saying that someone will answer there, terminal or
/// not. Requiring a terminal on top of it left the flag's main caller with no
/// questions at all: every task the VS Code extension makes passes the flag,
/// and its task terminal hands `run` a pipe and writes each line typed into
/// it. A pipe also lets answers be scripted,
/// `printf 'minor\n' | run --stdin-args release`.
///
/// In CI only a terminal is asked, which is all CI ever was asked: nobody is
/// there to answer, and a runner that leaves stdin open would wait on it until
/// the job timed out.
fn answerable() -> bool {
	!EXHAUSTED.load(Ordering::Relaxed)
		&& !runfile_runtime::interrupt::interrupted()
		&& (std::io::stdin().is_terminal() || !crate::ci_detect::is_ci())
}

/// One line typed in answer, trimmed, or `None` at the end of input or when
/// the run was interrupted while it waited.
fn answer() -> Option<String> {
	match read_line() {
		Some(line) => Some(line.trim().to_string()),
		None => {
			EXHAUSTED.store(true, Ordering::Relaxed);
			// End the prompt's line: nothing was typed to end it, so the error
			// that follows -- or the shell's prompt, after a Ctrl+C -- would be
			// printed on the end of the question.
			eprintln!();
			None
		}
	}
}

/// A line from stdin, or `None` at its end -- or as soon as a signal says the
/// run is over, whether or not anything was typed.
///
/// The read is on a thread of its own, and this one waits for it a
/// `interrupt::POLL` at a time, looking at the flag in between. Read here, a
/// Ctrl+C set the flag and the read carried on: the standard library retries a
/// read a signal interrupts, so the prompt sat there until Enter, and whatever
/// had been typed by then was taken as the answer to a run that was already
/// stopping. SIGTERM -- the VS Code extension closing its task terminal --
/// waited the same way, until the SIGKILL that follows it two seconds later,
/// which leaves every temp file the run had made where it was.
///
/// A signal wins over a line that arrives with it. An answered read's thread
/// has been joined by the time the answer is returned, so a prompt leaves no
/// thread behind -- `stdin_args::collect` sets the environment from answers,
/// which is only sound while nothing else runs. A read given up on is left
/// blocked, holding stdin: nothing is asked again after an interrupt, and the
/// process is on its way out.
fn read_line() -> Option<String> {
	use runfile_runtime::interrupt::{POLL, interrupted};
	use std::sync::mpsc::{RecvTimeoutError, channel};

	if interrupted() {
		return None;
	}
	let (tx, rx) = channel();
	let reader = std::thread::spawn(move || {
		let mut line = String::new();
		let got = match std::io::stdin().read_line(&mut line) {
			Ok(0) | Err(_) => None,
			Ok(_) => Some(line),
		};
		let _ = tx.send(got);
	});
	loop {
		match rx.recv_timeout(POLL) {
			Ok(got) => {
				let _ = reader.join();
				return got.filter(|_| !interrupted());
			}
			Err(RecvTimeoutError::Timeout) if interrupted() => return None,
			Err(RecvTimeoutError::Timeout) => {}
			// The reader panicked, which reading a line does not do.
			Err(RecvTimeoutError::Disconnected) => return None,
		}
	}
}

/// Asked for a value a target needs but was not given, under `--stdin-args`.
///
/// `kind` carries its own separator -- `argument --`, `environment ` -- so the
/// name is joined to it as it is: a blank between the two printed `-- port`.
pub fn ask_value(kind: &str, name: &str) -> Option<String> {
	if !answerable() {
		return None;
	}
	eprint!("{} {kind}{name} (required): ", runfile_runtime::exec::tag());
	let _ = std::io::stderr().flush();
	answer().filter(|v| !v.is_empty())
}

/// Ask for one input up front, showing what it falls back to and what it may
/// be: `--env (one of staging, production) [staging]: `.
///
/// An empty answer keeps the fallback -- which is the difference between this
/// and the old lazy prompt: a value with a default is worth offering, and
/// pressing Enter has to mean "leave it alone". The options are shown, not
/// enforced: the target's own `one_of` says what is wrong with an answer, and
/// a check in a branch the run never reaches refuses nothing.
pub fn ask_input(label: &str, u: &runfile_lang::inputs::Use) -> Option<String> {
	if !answerable() {
		return None;
	}
	eprint!("{} {label}{}: ", runfile_runtime::exec::tag(), tail(u));
	let _ = std::io::stderr().flush();
	answer().filter(|v| !v.is_empty())
}

/// What follows an input's name in its prompt: a note in parentheses, and the
/// value Enter keeps in brackets.
fn tail(u: &runfile_lang::inputs::Use) -> String {
	let mut notes = Vec::new();
	// A default says more than "required": Enter is answered either way.
	if u.required && u.default.is_none() {
		notes.push("required".to_string());
	}
	notes.extend(crate::target_help::one_of(u.choices.as_deref()));
	let mut out = String::new();
	if !notes.is_empty() {
		out.push_str(&format!(" ({})", notes.join("; ")));
	}
	match u.default.as_deref() {
		Some("") => out.push_str(" [empty]"),
		Some(d) => out.push_str(&format!(" [{}]", crate::target_help::sanitize(d))),
		None => {}
	}
	out
}

/// Ask whether to pass a flag. Off unless the answer is yes, which is what an
/// absent flag means anyway.
pub fn ask_flag(label: &str) -> bool {
	if !answerable() {
		return false;
	}
	eprint!("{} pass {label}? (y/N) ", runfile_runtime::exec::tag());
	let _ = std::io::stderr().flush();
	answer().is_some_and(|a| matches!(a.to_ascii_lowercase().as_str(), "y" | "yes"))
}
