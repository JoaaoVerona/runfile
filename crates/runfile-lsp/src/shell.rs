//! Shellcheck delegation for `$` runs and `exec sh` / `exec bash` bodies.
//!
//! The shell inside a `.run` file is real shell, and shellcheck is far better
//! at judging it than anything this project would write. What is needed here is
//! only the two things shellcheck cannot do: hand it a script it can parse, and
//! map its findings back to the lines the author actually wrote.

use std::io::Write;
use std::process::{Command, Stdio};

use runfile_lang::{Block, InterpPart, Statement};

use crate::analysis::{Diagnostic, Range, Severity};

/// Shells whose bodies are worth checking. `exec python` is not shell, and
/// handing it to shellcheck would produce nothing but noise.
/// Commands whose `exec` body is shell, so shellcheck has something to say
/// about it. `brush` is bash-compatible, so bash's checker applies.
const SHELLS: &[&str] = &["sh", "bash", "dash", "ash", "ksh", "brush"];

/// One shell script pulled out of a document, with the line each of its lines
/// came from.
#[derive(Debug, PartialEq, Eq)]
pub struct Chunk {
	pub shell: String,
	pub script: String,
	/// `source_lines[i]` is the 1-based source line of script line `i`.
	pub source_lines: Vec<usize>,
}

/// An interpolation becomes exactly one shell word, so it is replaced by one
/// quoted placeholder. Anything else -- dropping it, or leaving the braces --
/// makes shellcheck report on a command the author never wrote.
const PLACEHOLDER: &str = "'{{}}'";

/// Flatten the interpolated parts of one line into checkable shell text.
fn render(parts: &[InterpPart]) -> String {
	parts
		.iter()
		.map(|p| match p {
			InterpPart::Literal(s) => s.as_str(),
			InterpPart::Expr(_) => PLACEHOLDER,
		})
		.collect()
}

/// Pull every checkable shell chunk out of a document, innermost blocks
/// included.
pub fn chunks(block: &Block, default_shell: &str) -> Vec<Chunk> {
	let mut out = Vec::new();
	collect(block, default_shell, &mut out);
	out
}

fn collect(block: &Block, default_shell: &str, out: &mut Vec<Chunk>) {
	for st in &block.statements {
		if let Statement::Exec {
			command, body, lines, ..
		} = st
		{
			// No command is the `$` shorthand, which runs the default shell.
			let shell = match command {
				None => default_shell.to_string(),
				Some(parts) => first_word(&render(parts)),
			};
			if SHELLS.contains(&shell.as_str()) && !body.is_empty() {
				out.push(Chunk {
					shell,
					script: body.iter().map(|l| render(l)).collect::<Vec<_>>().join("\n"),
					source_lines: lines.clone(),
				});
			}
		}
		for inner in sub_blocks(st) {
			collect(inner, default_shell, out);
		}
	}
}

fn first_word(s: &str) -> String {
	s.split_whitespace().next().unwrap_or_default().to_string()
}

fn sub_blocks(st: &Statement) -> Vec<&Block> {
	match st {
		Statement::If { then, otherwise, .. } => {
			let mut v = vec![then];
			v.extend(otherwise.iter());
			v
		}
		Statement::For { body, .. } | Statement::Loop { body, .. } | Statement::Do { body, .. } => vec![body],
		Statement::Retry { body, otherwise, .. } => {
			let mut v = vec![body];
			v.extend(otherwise.iter());
			v
		}
		Statement::Match { cases, default, .. } => {
			let mut v: Vec<&Block> = cases.iter().map(|c| &c.body).collect();
			v.extend(default.iter());
			v
		}
		_ => Vec::new(),
	}
}

/// One shellcheck finding, before it is mapped back to the document.
#[derive(Debug, PartialEq, Eq)]
pub struct Finding {
	pub line: usize,
	pub column: usize,
	pub end_column: usize,
	pub level: String,
	pub code: u64,
	pub message: String,
}

/// Read shellcheck's `--format=json1` output.
///
/// Parsed by hand from the fields actually used: adding a JSON dependency to
/// read six scalars out of one array would not earn its keep.
pub fn parse_findings(json: &str) -> Vec<Finding> {
	let value: serde_json::Value = match serde_json::from_str(json) {
		Ok(v) => v,
		Err(_) => return Vec::new(),
	};
	let Some(items) = value["comments"].as_array() else {
		return Vec::new();
	};
	items
		.iter()
		.map(|c| Finding {
			line: c["line"].as_u64().unwrap_or(1) as usize,
			column: c["column"].as_u64().unwrap_or(1) as usize,
			end_column: c["endColumn"].as_u64().unwrap_or(0) as usize,
			level: c["level"].as_str().unwrap_or("warning").to_string(),
			code: c["code"].as_u64().unwrap_or(0),
			message: c["message"].as_str().unwrap_or_default().to_string(),
		})
		.collect()
}

/// Map one finding onto the document it came from.
///
/// Columns survive only when the line had no interpolation: a placeholder is a
/// different width from the text it stands for, so past one the column would
/// point at the wrong character. Reporting the whole line is honest; a
/// confidently wrong column is not.
pub fn place(f: &Finding, chunk: &Chunk, src: &str, exact_columns: bool) -> Option<Diagnostic> {
	let line = *chunk.source_lines.get(f.line.saturating_sub(1))?;
	let idx = line.saturating_sub(1);
	let width = src.lines().nth(idx).map(str::len).unwrap_or(0);
	let (start_col, end_col) = if exact_columns {
		let start = f.column.saturating_sub(1);
		let end = if f.end_column > f.column {
			f.end_column - 1
		} else {
			width
		};
		(start.min(width), end.min(width))
	} else {
		(0, width)
	};
	Some(Diagnostic {
		range: Range {
			start_line: idx,
			start_col,
			end_line: idx,
			end_col,
		},
		message: format!("{} [SC{}]", f.message, f.code),
		// Shellcheck's four levels map onto LSP's four; flattening them would
		// put a style note and a real error in the same bucket.
		severity: match f.level.as_str() {
			"error" => Severity::Error,
			"warning" => Severity::Warning,
			"style" => Severity::Hint,
			_ => Severity::Information,
		},
	})
}

/// Run shellcheck over one chunk. `None` when shellcheck is not installed --
/// it is an enhancement, so its absence must be silent.
pub fn check(chunk: &Chunk, program: &str) -> Option<Vec<Finding>> {
	let mut child = Command::new(program)
		.args(["--format=json1", "--shell", &chunk.shell, "-"])
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::null())
		.spawn()
		.ok()?;
	child.stdin.take()?.write_all(chunk.script.as_bytes()).ok()?;
	let out = child.wait_with_output().ok()?;
	Some(parse_findings(&String::from_utf8_lossy(&out.stdout)))
}

/// Every shellcheck finding in a document, as diagnostics.
pub fn diagnose(src: &str, default_shell: &str, program: &str) -> Vec<Diagnostic> {
	let Ok(ast) = runfile_lang::parse(src) else {
		return Vec::new();
	};
	let mut out = Vec::new();
	for chunk in chunks(&ast.body, default_shell) {
		let exact = !chunk.script.contains(PLACEHOLDER);
		let Some(findings) = check(&chunk, program) else {
			return Vec::new();
		};
		out.extend(findings.iter().filter_map(|f| place(f, &chunk, src, exact)));
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	fn parsed(src: &str) -> Block {
		runfile_lang::parse(src).expect("parses").body
	}

	#[test]
	fn a_shell_run_becomes_one_chunk() {
		let c = chunks(&parsed("$ echo a\n$ echo b\n"), "bash");
		assert_eq!(c.len(), 1, "consecutive `$` lines are one process");
		assert_eq!(c[0].script, "echo a\necho b");
		assert_eq!(c[0].source_lines, vec![1, 2]);
		assert_eq!(c[0].shell, "bash");
	}

	#[test]
	fn blank_and_comment_lines_do_not_shift_the_mapping() {
		// They are transparent to the run, so line 3 of the file is line 2 of
		// the script -- exactly the case a naive offset gets wrong.
		let c = chunks(&parsed("$ echo a\n\n# note\n$ echo b\n"), "sh");
		assert_eq!(c[0].script, "echo a\necho b");
		assert_eq!(c[0].source_lines, vec![1, 4]);
	}

	#[test]
	fn an_exec_body_is_dedented_and_mapped() {
		let c = chunks(&parsed("exec bash\n\techo a\n\techo b\nend\n"), "sh");
		assert_eq!(c[0].script, "echo a\necho b");
		assert_eq!(c[0].source_lines, vec![2, 3]);
	}

	#[test]
	fn a_non_shell_command_is_not_checked() {
		assert!(chunks(&parsed("exec python3\nprint(1)\nend\n"), "sh").is_empty());
	}

	#[test]
	fn shell_inside_a_block_is_found() {
		let c = chunks(&parsed("if FLAG.x\n\t$ echo a\nend\n"), "sh");
		assert_eq!(c.len(), 1);
		assert_eq!(c[0].source_lines, vec![2]);
	}

	#[test]
	fn an_interpolation_becomes_one_quoted_word() {
		// It resolves to exactly one shell argument, so anything else would
		// have shellcheck reporting on a command nobody wrote.
		let c = chunks(&parsed("$ cp {{ ARG.src }} /tmp\n"), "sh");
		assert_eq!(c[0].script, "cp '{{}}' /tmp");
	}

	#[test]
	fn findings_are_read_from_shellchecks_json() {
		let json = r#"{"comments":[{"file":"-","line":2,"endLine":2,"column":5,"endColumn":9,
			"level":"warning","code":2086,"message":"Double quote to prevent globbing."}]}"#;
		let f = parse_findings(json);
		assert_eq!(f.len(), 1);
		assert_eq!(f[0].line, 2);
		assert_eq!(f[0].code, 2086);
	}

	#[test]
	fn unparseable_output_yields_nothing_rather_than_failing() {
		assert!(parse_findings("not json").is_empty());
		assert!(parse_findings("{}").is_empty());
	}

	#[test]
	fn a_finding_lands_on_the_line_the_author_wrote() {
		let src = "$ echo a\n\n# note\n$ echo b\n";
		let chunk = &chunks(&parsed(src), "sh")[0];
		let f = Finding {
			line: 2,
			column: 1,
			end_column: 3,
			level: "warning".into(),
			code: 2086,
			message: "m".into(),
		};
		let d = place(&f, chunk, src, true).expect("placed");
		assert_eq!(d.range.start_line, 3, "script line 2 is file line 4");
	}

	#[test]
	fn a_line_with_an_interpolation_is_reported_whole() {
		// The placeholder is a different width from what it stands for, so a
		// column would point at the wrong character.
		let src = "$ cp {{ ARG.src }} /tmp\n";
		let chunk = &chunks(&parsed(src), "sh")[0];
		let f = Finding {
			line: 1,
			column: 4,
			end_column: 6,
			level: "warning".into(),
			code: 2086,
			message: "m".into(),
		};
		let d = place(&f, chunk, src, false).expect("placed");
		assert_eq!((d.range.start_col, d.range.end_col), (0, src.trim_end().len()));
	}

	#[test]
	fn a_finding_past_the_end_of_the_chunk_is_dropped() {
		let src = "$ echo a\n";
		let chunk = &chunks(&parsed(src), "sh")[0];
		let f = Finding {
			line: 99,
			column: 1,
			end_column: 2,
			level: "error".into(),
			code: 1,
			message: "m".into(),
		};
		assert!(place(&f, chunk, src, true).is_none());
	}

	#[test]
	fn a_missing_shellcheck_is_silent() {
		let chunk = Chunk {
			shell: "sh".into(),
			script: "echo $x".into(),
			source_lines: vec![1],
		};
		assert!(check(&chunk, "definitely-not-a-real-program").is_none());
	}
}
