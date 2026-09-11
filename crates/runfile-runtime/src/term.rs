//! The terminal a person is reading, and how wide text is on it.
//!
//! Here rather than in the CLI because this crate already makes the platform
//! calls -- `libc` for Ctrl+C on Unix, the console API on Windows -- so asking
//! a terminal how wide it is was one more of them rather than a new dependency.

use std::borrow::Cow;
use std::io::IsTerminal;

/// How many columns the terminal on stdout has, or `None` when stdout is not
/// one: a pipe has no width, and whatever reads it wants whole lines --
/// `run :list | grep deploy` must not find half a description.
///
/// `COLUMNS` comes first when it holds a positive number, the way `ls` and
/// `git` read it: set, it is someone saying how wide to be. The terminal is
/// asked otherwise. Neither answering leaves the output as it was.
pub fn columns() -> Option<usize> {
	if !std::io::stdout().is_terminal() {
		return None;
	}
	std::env::var("COLUMNS")
		.ok()
		.and_then(|c| c.trim().parse::<usize>().ok())
		.filter(|&c| c > 0)
		.or_else(queried)
}

#[cfg(unix)]
fn queried() -> Option<usize> {
	// SAFETY: a zeroed `winsize` is a valid value of it, and `TIOCGWINSZ`
	// writes one through the pointer, which is live and writable for the call.
	let mut size: libc::winsize = unsafe { std::mem::zeroed() };
	let answered = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) } == 0;
	(answered && size.ws_col > 0).then_some(usize::from(size.ws_col))
}

#[cfg(windows)]
fn queried() -> Option<usize> {
	use windows_sys::Win32::System::Console::{
		CONSOLE_SCREEN_BUFFER_INFO, GetConsoleScreenBufferInfo, GetStdHandle, STD_OUTPUT_HANDLE,
	};
	// SAFETY: a zeroed `CONSOLE_SCREEN_BUFFER_INFO` is a valid value of it,
	// and the call only writes one through the pointer. A stdout that is not a
	// console -- mintty hands a native program a pipe -- makes it fail, which
	// is an answer rather than a fault.
	let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
	let answered = unsafe { GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), &mut info) } != 0;
	// The visible window, not the buffer: the buffer can be far wider than
	// what is on screen, and wrapping is about what is on screen.
	let width = i32::from(info.srWindow.Right) - i32::from(info.srWindow.Left) + 1;
	(answered && width > 0).then(|| width as usize)
}

#[cfg(not(any(unix, windows)))]
fn queried() -> Option<usize> {
	None
}

/// How many columns `s` takes on a terminal.
///
/// A character is one column, except for the two kinds that are not: the wide
/// characters of East Asian scripts and emoji, which take two, and the
/// combining marks, joiners and variation selectors that sit on the character
/// before them, which take none. Counting characters instead would let a
/// description holding either run past the edge and wrap -- the one thing a
/// listing of one line per target must not do. The ranges are the classic
/// `wcwidth` table, trimmed to what a comment in a runfile plausibly holds.
pub fn width_of(s: &str) -> usize {
	s.chars().map(char_width).sum()
}

/// Code points that sit on the character before them: combining marks,
/// zero-width spaces and joiners, and variation selectors.
const ZERO: &[(u32, u32)] = &[
	(0x0300, 0x036F),
	(0x200B, 0x200F),
	(0x20D0, 0x20FF),
	(0xFE00, 0xFE0F),
	(0xFE20, 0xFE2F),
];

/// Code points a terminal draws two columns wide: the East Asian wide and
/// fullwidth ranges, the emoji blocks above U+1F000 whole, and the emoji
/// scattered through the older symbol blocks below it that are wide too --
/// `✅`, `❌`, `⚡`, `✨`, the ones a description is likely to hold.
const WIDE: &[(u32, u32)] = &[
	(0x1100, 0x115F),
	(0x231A, 0x231B),
	(0x23E9, 0x23EC),
	(0x23F0, 0x23F0),
	(0x23F3, 0x23F3),
	(0x2614, 0x2615),
	(0x26A1, 0x26A1),
	(0x26D4, 0x26D4),
	(0x2705, 0x2705),
	(0x270A, 0x270B),
	(0x2728, 0x2728),
	(0x274C, 0x274C),
	(0x274E, 0x274E),
	(0x2753, 0x2755),
	(0x2757, 0x2757),
	(0x2795, 0x2797),
	(0x2B50, 0x2B50),
	(0x2B55, 0x2B55),
	(0x2E80, 0x303E),
	(0x3041, 0x33FF),
	(0x3400, 0x4DBF),
	(0x4E00, 0x9FFF),
	(0xA000, 0xA4CF),
	(0xAC00, 0xD7A3),
	(0xF900, 0xFAFF),
	(0xFE30, 0xFE4F),
	(0xFF00, 0xFF60),
	(0xFFE0, 0xFFE6),
	(0x1F300, 0x1F64F),
	(0x1F680, 0x1F6FF),
	(0x1F900, 0x1F9FF),
	(0x1FA70, 0x1FAFF),
	(0x20000, 0x3FFFD),
];

fn char_width(c: char) -> usize {
	let within = |ranges: &[(u32, u32)]| ranges.iter().any(|&(lo, hi)| (lo..=hi).contains(&u32::from(c)));
	if within(ZERO) {
		0
	} else if within(WIDE) {
		2
	} else {
		1
	}
}

/// `s` cut to at most `cols` columns, ending in `…` where something was cut.
///
/// The ellipsis is what says a line was cut rather than written that short.
/// It sits against the last word kept, not after the space that followed it,
/// and a wide character that would straddle the limit is left out whole
/// rather than half-drawn.
pub fn fit(s: &str, cols: usize) -> Cow<'_, str> {
	if width_of(s) <= cols {
		return Cow::Borrowed(s);
	}
	let Some(room) = cols.checked_sub(1) else {
		return Cow::Borrowed("");
	};
	let mut kept = String::new();
	let mut used = 0;
	for c in s.chars() {
		let w = char_width(c);
		if used + w > room {
			break;
		}
		kept.push(c);
		used += w;
	}
	Cow::Owned(format!("{}…", kept.trim_end()))
}
