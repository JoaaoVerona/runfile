use crate::parallel_output::*;
use std::sync::{Arc, Mutex};

#[test]
fn strip_keeps_sgr_color() {
	let input = b"\x1b[31mred\x1b[0m";
	assert_eq!(strip_cursor_escapes(input), input);
}

#[test]
fn strip_removes_cursor_up() {
	let input = b"hello\x1b[2Aworld";
	assert_eq!(strip_cursor_escapes(input), b"helloworld");
}

#[test]
fn strip_removes_erase_line() {
	let input = b"foo\x1b[Kbar";
	assert_eq!(strip_cursor_escapes(input), b"foobar");
}

#[test]
fn strip_removes_osc_title() {
	let input = b"a\x1b]0;title\x07b";
	assert_eq!(strip_cursor_escapes(input), b"ab");
}

#[test]
fn strip_handles_truncated_csi() {
	// Should not panic on a half-finished escape sequence.
	let input = b"foo\x1b[12";
	let _ = strip_cursor_escapes(input);
}

fn drive(data: &[u8]) -> Vec<String> {
	let mut state = LineState::default();
	let mut out: Vec<String> = Vec::new();
	let mut emit = |line: &[u8]| out.push(String::from_utf8_lossy(line).to_string());
	process_chunk(data, &mut state, &mut emit);
	if !state.line.is_empty() {
		out.push(String::from_utf8_lossy(&state.line).to_string());
	}
	out
}

#[test]
fn process_chunk_splits_on_lf() {
	assert_eq!(drive(b"a\nb\nc"), vec!["a", "b", "c"]);
}

#[test]
fn process_chunk_splits_on_cr() {
	// Bare CR (progress-bar redraw style) becomes a line break.
	assert_eq!(drive(b"frame1\rframe2\rframe3"), vec!["frame1", "frame2", "frame3"]);
}

#[test]
fn process_chunk_collapses_crlf() {
	// `\r\n` must emit exactly once, not "line\r" + "" from the LF.
	assert_eq!(drive(b"line1\r\nline2\r\n"), vec!["line1", "line2"]);
}

#[test]
fn process_chunk_preserves_blank_lf_lines() {
	// Genuine blank lines (LF-only) are preserved.
	assert_eq!(drive(b"a\n\nb\n"), vec!["a", "", "b"]);
}

#[test]
fn process_chunk_handles_mixed_terminators() {
	assert_eq!(
		drive(b"line1\nline2\rline3\r\nline4"),
		vec!["line1", "line2", "line3", "line4"]
	);
}

#[test]
fn prefix_contains_label() {
	// Always contains the label, regardless of NO_COLOR setting.
	let p = format_parallel_prefix(7, "@build");
	assert!(p.contains("[@build]"));
	assert!(p.ends_with(' '));
}

// ── Label derivation ──────────────────────────────────────────────

fn label_of(cmd: &str) -> String {
	shell_label_parts(cmd).0
}

fn context_of(cmd: &str) -> Option<String> {
	shell_label_parts(cmd).1
}

/// Resolve labels for a batch of shell commands, trimming the alignment
/// padding so assertions stay readable.
fn labels_for(cmds: &[&str]) -> Vec<String> {
	let candidates: Vec<LabelCandidate> = cmds.iter().map(|c| LabelCandidate::shell(c)).collect();
	resolve_parallel_labels(&candidates)
		.into_iter()
		.map(|l| l.trim_end().to_string())
		.collect()
}

#[test]
fn shell_label_uses_first_nonempty_line_trimmed() {
	assert_eq!(label_of("\n   \n  npm run build  \n"), "npm run build");
}

#[test]
fn shell_label_truncates_at_max_width() {
	// 24 scalars, counted in chars rather than bytes.
	assert_eq!(
		labels_for(&["echo aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]),
		["echo aaaaaaaaaaaaaaaaaaa"]
	);
	assert_eq!(
		labels_for(&["echo ãããããããããããããããããããããããã"]),
		["echo ããããããããããããããããããã"]
	);
}

#[test]
fn shell_label_skips_leading_cd() {
	// The reported case: a shared `cd <path> &&` prefix used to render both
	// leaves as `cd /Users/jo`.
	assert_eq!(
		labels_for(&[
			"cd /Users/joao.verona/Workspace/battle-tanks-api && run api",
			"cd /Users/joao.verona/Workspace/battle-tanks && run game-room --delay",
		]),
		["run api", "run game-room --delay"]
	);
}

#[test]
fn shell_label_skips_env_assignments_and_other_setup() {
	assert_eq!(label_of("FOO=bar BAZ=1 npm run dev"), "npm run dev");
	assert_eq!(label_of("export FOO=bar && npm run dev"), "npm run dev");
	assert_eq!(label_of("source .venv/bin/activate; python app.py"), "python app.py");
	assert_eq!(label_of("FOO=bar; npm run dev"), "npm run dev");
}

#[test]
fn shell_label_takes_first_meaningful_segment() {
	// The first real command identifies the branch; later steps don't.
	assert_eq!(
		label_of("cd api && docker compose up -d && echo ready"),
		"docker compose up -d"
	);
}

#[test]
fn shell_label_falls_back_to_whole_line_when_all_setup() {
	assert_eq!(label_of("cd /tmp"), "cd /tmp");
	assert_eq!(label_of("FOO=bar"), "FOO=bar");
}

#[test]
fn shell_label_does_not_split_inside_quotes_or_subshells() {
	assert_eq!(label_of("echo 'a && b'"), "echo 'a && b'");
	assert_eq!(label_of("echo \"x; y\""), "echo \"x; y\"");
	assert_eq!(label_of("echo $(a && b)"), "echo $(a && b)");
	// A pipeline is one logical command; a lone `&` is not a separator.
	assert_eq!(label_of("cd api && cat f | grep x"), "cat f | grep x");
}

#[test]
fn cd_context_is_last_cd_basename() {
	assert_eq!(
		context_of("cd /a/b/battle-tanks-api && run api").as_deref(),
		Some("battle-tanks-api")
	);
	assert_eq!(context_of("cd /a/ && cd b/c/ && run x").as_deref(), Some("c"));
	assert_eq!(context_of("cd 'my dir' && run x").as_deref(), Some("my dir"));
	// Relative hops carry no identity.
	assert_eq!(context_of("cd .. && run x"), None);
	assert_eq!(context_of("npm run dev"), None);
}

// ── Batch-wide uniqueness ─────────────────────────────────────────

#[test]
fn colliding_labels_widen_until_distinct() {
	// Identical for the first 24 chars — the group widens rather than
	// collapsing to two identical prefixes.
	let labels = labels_for(&[
		"pnpm --filter @acme/web-frontend dev",
		"pnpm --filter @acme/web-backend dev",
	]);
	assert_eq!(
		labels,
		["pnpm --filter @acme/web-frontend", "pnpm --filter @acme/web-backend"]
	);
}

#[test]
fn identical_commands_fall_back_to_cd_context() {
	let labels = labels_for(&["cd api && npm run dev", "cd web && npm run dev"]);
	assert_eq!(labels, ["api", "web"]);
}

#[test]
fn truly_identical_leaves_get_ordinal_suffixes() {
	let labels = labels_for(&["npm run dev", "npm run dev"]);
	assert_eq!(labels, ["npm run dev #1", "npm run dev #2"]);
}

#[test]
fn only_colliding_leaves_are_disambiguated() {
	// The distinct leaf keeps its plain label; only the pair is touched.
	let labels = labels_for(&["cd api && npm run dev", "cd web && npm run dev", "docker compose up"]);
	assert_eq!(labels, ["api", "web", "docker compose up"]);
}

#[test]
fn disambiguation_does_not_create_a_new_collision() {
	// The `cd` fallback would produce a second `api`, so the group must
	// escalate to ordinals instead of silently colliding again.
	let labels = labels_for(&["cd api && npm run dev", "cd api && npm run dev", "api"]);
	assert_eq!(labels, ["npm run dev #1", "npm run dev #2", "api"]);
}

#[test]
fn target_call_labels_participate_in_uniqueness() {
	let candidates = vec![
		LabelCandidate::target("@build".to_string()),
		LabelCandidate::target("@build".to_string()),
	];
	let labels: Vec<String> = resolve_parallel_labels(&candidates)
		.into_iter()
		.map(|l| l.trim_end().to_string())
		.collect();
	assert_eq!(labels, ["@build #1", "@build #2"]);
}

// ── Alignment ─────────────────────────────────────────────────────

#[test]
fn labels_are_padded_to_a_common_width() {
	let candidates: Vec<LabelCandidate> = ["run api", "run game-room"]
		.iter()
		.map(|c| LabelCandidate::shell(c))
		.collect();
	let labels = resolve_parallel_labels(&candidates);
	assert_eq!(labels, ["run api      ", "run game-room"]);
	assert!(labels.iter().all(|l| l.chars().count() == 13));
}

#[test]
fn padding_is_capped_so_one_outlier_does_not_bloat_the_rest() {
	// A target-call label is never truncated, so it can exceed the 24-char
	// cap. It sticks out on its own rather than padding every other leaf to
	// its width.
	let long = "@web-user:build:infrastructure".to_string();
	assert!(long.chars().count() > 24);
	let candidates = vec![
		LabelCandidate::shell("a"),
		LabelCandidate::shell("b"),
		LabelCandidate::target(long.clone()),
	];
	let labels = resolve_parallel_labels(&candidates);
	assert_eq!(labels[0].chars().count(), 24);
	assert_eq!(labels[1].chars().count(), 24);
	assert_eq!(labels[2], long);
}

#[test]
fn truncation_does_not_leave_a_trailing_space() {
	// Cutting at the cap can land on a space; the label must not carry it
	// into the alignment pass.
	assert_eq!(
		labels_for(&["echo starting game-room --delay"]),
		["echo starting game-room"]
	);
}

#[test]
fn single_leaf_gets_no_padding() {
	assert_eq!(
		resolve_parallel_labels(&[LabelCandidate::shell("run api")]),
		["run api"]
	);
}

// ── End-to-end pump tests via OutputStream::Capture ───────────────

use std::io::Cursor;

fn run_pump(input: &[u8], prefix: &str) -> String {
	let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
	let stream = OutputStream::Capture(buf.clone());
	let h = spawn_line_pump(Cursor::new(input.to_vec()), prefix.to_string(), stream);
	h.join().unwrap();
	let bytes: Vec<u8> = buf.lock().unwrap().clone();
	// Normalize line endings so tests work on both Windows (where the
	// pump emits CRLF for terminal compatibility) and Unix (LF only).
	String::from_utf8(bytes).unwrap().replace("\r\n", "\n")
}

#[test]
fn pump_prefixes_each_line() {
	let out = run_pump(b"line1\nline2\nline3\n", "[1] ");
	assert_eq!(out, "[1] line1\n[1] line2\n[1] line3\n");
}

#[test]
fn pump_flattens_cr_progress_redraw() {
	// `\r`-driven progress redraws become append-only chronological lines.
	let out = run_pump(b"frame1\rframe2\rframe3\n", "[X] ");
	assert_eq!(out, "[X] frame1\n[X] frame2\n[X] frame3\n");
}

#[test]
fn pump_preserves_sgr_colors() {
	let out = run_pump(b"\x1b[31mred\x1b[0m text\n", "[2] ");
	assert_eq!(out, "[2] \x1b[31mred\x1b[0m text\n");
}

#[test]
fn pump_treats_cursor_movement_escapes_as_line_boundaries() {
	// `\x1b[1A` (cursor up) and `\x1b[K` (erase line) split the stream
	// into separate prefixed lines — each represents a "virtual row" of
	// a TTY-mode redraw frame.
	let out = run_pump(b"a\x1b[1Ab\x1b[Kc\n", "[7] ");
	assert_eq!(out, "[7] a\n[7] b\n[7] c\n");
}

#[test]
fn pump_handles_crlf_without_blank_lines() {
	// `\r\n` must not emit a spurious empty prefixed line for the LF.
	let out = run_pump(b"line1\r\nline2\r\n", "[1] ");
	assert_eq!(out, "[1] line1\n[1] line2\n");
}

#[test]
fn pump_flushes_trailing_partial_line() {
	// A child exiting without a final newline must still surface its
	// output (otherwise short-lived commands could go silent).
	let out = run_pump(b"no-newline-at-end", "[9] ");
	assert_eq!(out, "[9] no-newline-at-end\n");
}

#[test]
fn pump_drops_blank_lines() {
	// Pure-whitespace / empty lines are dropped — they're almost always
	// redraw-frame artifacts and add visual noise in parallel output.
	let out = run_pump(b"a\n\nb\n", "[1] ");
	assert_eq!(out, "[1] a\n[1] b\n");
}

#[test]
fn pump_trims_leading_and_trailing_whitespace() {
	// Layout-padding whitespace (left over after stripping cursor
	// positioning escapes) is trimmed from each line.
	let out = run_pump(b"      Image foo Pulling          \n", "[2] ");
	assert_eq!(out, "[2] Image foo Pulling\n");
}

#[test]
fn pump_drops_pure_whitespace_lines() {
	// A line that's only spaces / tabs (after escape stripping) is
	// dropped entirely — common in Docker Compose-style progress UIs
	// where redraw frames leave behind whitespace-only "rows".
	let out = run_pump(b"real line\n          \n\t\t\nanother\n", "[3] ");
	assert_eq!(out, "[3] real line\n[3] another\n");
}

#[test]
fn pump_splits_tty_frame_at_cursor_position() {
	// TTY-mode tools emit a single "frame" with cursor-position escapes
	// between virtual rows of content, no `\n` between rows. Each row
	// should emerge as its own prefixed line.
	let frame = b"\x1b[1;1HRow A\x1b[2;1HRow B\x1b[3;1HRow C\n";
	let out = run_pump(frame, "[1] ");
	assert_eq!(out, "[1] Row A\n[1] Row B\n[1] Row C\n");
}

#[test]
fn pump_splits_at_cursor_up_then_text() {
	// Docker Compose's redraw pattern: cursor up + content (overwrites
	// previous row). Each content piece should become its own line.
	let frame = b"first\x1b[1Asecond\x1b[1Athird\n";
	let out = run_pump(frame, "[2] ");
	assert_eq!(out, "[2] first\n[2] second\n[2] third\n");
}

#[test]
fn pump_splits_at_erase_line() {
	// `\x1b[K` (erase line) typically marks a redraw boundary too.
	let frame = b"old content\x1b[Knew content\n";
	let out = run_pump(frame, "[3] ");
	assert_eq!(out, "[3] old content\n[3] new content\n");
}

#[test]
fn pump_does_not_split_at_sgr() {
	// SGR (color/style) escapes must NOT split lines — they stay inline
	// within the segment so colored text keeps its color.
	let line = b"\x1b[31mred\x1b[0m and \x1b[32mgreen\x1b[0m\n";
	let out = run_pump(line, "[4] ");
	assert_eq!(out, "[4] \x1b[31mred\x1b[0m and \x1b[32mgreen\x1b[0m\n");
}
