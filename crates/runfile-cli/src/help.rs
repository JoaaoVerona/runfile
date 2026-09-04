//! Help text, rendered plainly or with colour depending on who is reading.
//!
//! The help is data rather than a string literal, so the same rows render for a
//! terminal and for a pipe. Every subcommand's help is built the same way, so
//! they cannot drift into different shapes.

use std::io::IsTerminal;

/// Whether to emit escape codes.
///
/// A pipe gets none, `NO_COLOR` is honoured whatever its value, and `TERM=dumb`
/// means a terminal that would show them literally.
pub fn colour() -> bool {
	if std::env::var_os("NO_COLOR").is_some() {
		return false;
	}
	if std::env::var("TERM").is_ok_and(|t| t == "dumb") {
		return false;
	}
	std::io::stdout().is_terminal()
}

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const CYAN: &str = "\x1b[36m";
const RESET: &str = "\x1b[0m";

/// One `name  description` pair. An empty description makes a bare line.
pub struct Row(pub &'static str, pub &'static str);

/// A titled group of rows. An empty title omits the heading.
pub struct Section(pub &'static str, pub &'static [Row]);

/// Render sections, aligning descriptions to one column across all of them so
/// the whole page reads as a single table rather than several.
pub fn render(intro: &str, sections: &[Section]) -> String {
	let paint = colour();
	let (bold, dim, cyan, reset) = if paint {
		(BOLD, DIM, CYAN, RESET)
	} else {
		("", "", "", "")
	};
	// Rows whose name is too long get their description on the next line
	// rather than pushing every other description across the page.
	const MAX: usize = 34;
	let width = sections
		.iter()
		.flat_map(|s| s.1.iter())
		.filter(|r| !r.1.is_empty() && r.0.chars().count() <= MAX)
		.map(|r| r.0.chars().count())
		.max()
		.unwrap_or(0);

	let mut out = String::new();
	if !intro.is_empty() {
		out.push_str(&format!("{bold}{intro}{reset}\n"));
	}
	for Section(title, rows) in sections {
		out.push('\n');
		if !title.is_empty() {
			out.push_str(&format!("{bold}{title}{reset}\n"));
		}
		for Row(name, desc) in *rows {
			if desc.is_empty() {
				out.push_str(&format!("  {cyan}{name}{reset}\n"));
				continue;
			}
			let n = name.chars().count();
			if n > MAX {
				out.push_str(&format!("  {cyan}{name}{reset}\n"));
				out.push_str(&format!("  {:width$}  {dim}{desc}{reset}\n", "", width = width));
			} else {
				out.push_str(&format!(
					"  {cyan}{name}{reset}{:pad$}  {dim}{desc}{reset}\n",
					"",
					pad = width - n
				));
			}
		}
	}
	out
}

/// Whether these arguments are asking for help rather than for work.
pub fn wants_help(args: &[String]) -> bool {
	args.iter().any(|a| a == "--help" || a == "-h")
}
