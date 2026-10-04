# Agent `sweep` -- files no other agent read

Commit a65dba1 (v1.8.2). Experiments used the prebuilt debug binary in isolation (env -i, empty HOME/config,
DBUS nowhere, ulimit, timeout) on fixtures in SCRATCH/sweep only; payloads only created marker files there.
Nothing in the repository was modified; no cargo, no network, no repo runfiles run.

## Verdicts

| File | Verdict |
|---|---|
| crates/runfile-shell/src/rules.rs | Live code. Two findings (sweep.jsonl #1 arithmetic context, #2 quadratic rules). Exemption locations recorded below. No panics: every slice is at a char boundary (`Script::bytes` maps char indices to byte offsets; `&t[..len-1]` and `[1..]` cut after an ASCII `]`/`=`), every index is guarded (`program()` returns `None` on an empty command, `positions` guards `j + 1 < n`, `unterminated_exec` guards `j < 2`). A fuzz of 33 non-ASCII/edge lines through `:lint` produced no panic. Recursion in `visit` is bounded by the depth-64 cap in `syntax::command`. |
| crates/runfile-lang/src/keywords.rs | Static hover data (name, syntax, doc, example). Cleared. One note for the fix: the `$` hover text (lines 24-26) says an interpolation "never needs quoting", which the HIGH and the arithmetic finding contradict; it should be updated along with README. |
| crates/runfile-shell/src/tests.rs | Hardening finding (#3): `bash_runs` (853-867) uses a predictable directory in the shared temp dir and accepts it if it already exists. `bash()` runs `bash -n` only (parse, no execution) -- safe even for the opt-in `corpus()` sweep. Pins contradicting the existing HIGH: see below. |
| crates/runfile-shell/tests/rules_doc.rs | Read-only; parses SHELL-CHECK-RULES.md and holds its examples to the checker. Cleared. It pins the doc's "not flagged" ssh / `sh -c` / echo-into-rc / message examples, so the HIGH's fix has to edit the doc too. |
| crates/runfile-lang/src/tests/check.rs | Calls every library function for real (`call_for_real`) with `base_dir` = a `tempfile::tempdir()`, `assume_yes`, an empty key pool (`Keys::default()` loads nothing -- no keyring), and removes what `temp_file`/`temp_dir` made. Cleared. |
| crates/runfile-lang/src/tests/lines.rs | Pure parse tests. Cleared. |
| crates/runfile-lang/src/tests/parser.rs | Pure parse/fingerprint tests; usefully pins that no `\r` reaches a shell from a CRLF file. Cleared. |
| crates/runfile-lang/src/tests/resolve.rs | Pure name-resolution tests. Cleared. |
| crates/runfile-lang/src/tests/spans.rs | Pure, plus a read-only walk of the repository's `.run` files. Nit: the walk follows directory symlinks with no visited set, so a symlink loop would hang it; the repo has no symlinks today. Cleared. |
| crates/runfile-lang/tests/editor_grammars.rs | Reads two grammar files, compares lists. Cleared. |
| crates/runfile-lang/tests/format.rs | Pure in-memory formatter tests; pins that shell text and `exec` command spacing are never rewritten. Cleared. |
| crates/runfile-lang/tests/golden.rs | Writes only under `UPDATE_GOLDEN`, only to `tests/golden`. Cleared. |
| crates/runfile-lang/tests/readme.rs | Read-only parse of README examples. Cleared. |
| crates/runfile-lang/tests/repo_format.rs | Read-only walk and in-memory format (no writes). Same symlink nit as spans.rs; also does not skip `target-*`. Cleared. |
| crates/runfile-lang/tests/rules_doc.rs | Read-only, as the shell one. Cleared. |
| .editorconfig | Editor settings only. Cleared. |

No test file touches the real HOME, the keyring or the network, or leaves processes behind. No fixture holds
anything that looks like a real secret.

## `quoted_interpolation`: exactly what is exempt (for the HIGH's fix)

- `rules.rs:396-401` -- `judged` is `true` only for `Pos::Program`, `Pos::Redirect`, `Pos::Plain{..}`, and
  `Pos::Test{paired: false}`. Everything else is exempt for double quotes.
- `rules.rs:402-411` -- a single-quoted or `$'…'` hole is flagged anywhere; a double-quoted one only if
  `judged`; an `Opaque` hole (backticks, `${…}` operand, arithmetic, per `syntax.rs:28-30`) never.
- `rules.rs:58-61` -- `Pos::Unknown` is documented as "every argument that may be code for something else to
  run: `sh -c`, `ssh`, `eval`, `echo` into a file".
- `rules.rs:251-256` -- `positions()` starts every word at `Pos::Unknown`; only `FILE_COMMANDS` (76-80),
  grep (266-270), `[`/`test` (271-281), find outside `-exec` (282-303) and docker/podman path options
  (304-313) get judged positions. So every argument of echo, printf, ssh, sh/bash, eval, git, curl, etc. is
  exempt.
- `rules.rs:107-108` -- assignment words (`X="…"`, `export Y="…"`) are `Unknown`.
- `rules.rs:138-139` -- `for` list words are `Unknown`; `144` the `case` subject is `Unknown`, `147` patterns
  are `Pattern` (exempt).
- `rules.rs:196-202` -- `<<<` here-strings, `<&`, `>&` targets are `Unknown`.
- `rules.rs:158` and `276` -- a test with two double-quoted holes (`paired`) is exempt.
- **Heredoc bodies are not checked at all**, whatever their quoting: `words.rs:610-635` (`read_heredocs`)
  only scans for the delimiter line, skipping holes at 617, and builds no `Word`, so no rule ever sees an
  interpolation there -- not even a single-quoted one, which is otherwise always flagged.
- Arithmetic: `words.rs:335-338` skips holes; `rules.rs:188-194` and `389` never look inside.

Confirmed on the binary: every exempt position above that I exercised (echo, assignment, `<<<`, heredoc
body in `$` lines and in `exec bash`, paired test, `"${X:-…}"`, `case`, `for`, double quotes inside
backticks or `$(…)`, `sh -c`, `eval`, echo-into-file, printf, export) ran an embedded command substitution in
the local shell and drew no finding, which matches the existing HIGH. The arithmetic positions are a further
context that HIGH does not name, filed as sweep #1.

## Tests that pin, or contradict, existing findings

- `tests.rs:85-97` (`double_quotes_around_code_for_another_shell_are_left_alone`) asserts ssh, `sh -c`,
  docker `sh -c`, echo-into-rc, a message, `find -exec sh -c` and a paired test are clean. These are injection
  sites under the lang-eval HIGH; the fix must change this test.
- `tests.rs:99-104` asserts a heredoc body interpolation is clean -- also an injection site under the HIGH.
  (Its `"$(basename {{ … }})"` case is genuinely safe.)
- `rules_doc.rs:52-77` pins SHELL-CHECK-RULES.md's "not flagged" examples, including the same patterns.
- `tests.rs:522` (`[ {{ ARG.n }} -gt 3 ]` clean) is correct: `[` does not evaluate arithmetic. No test covers
  `[[ … -gt … ]]` or `$(( ))` with an interpolation.
- No test pins the shell-checker DoS findings or sweep #2.

## Findings written (findings/sweep.jsonl)

1. medium -- interpolation in shell arithmetic contexts is evaluated as code; checker blind to it.
2. low -- quadratic `truncated_input` and `unquoted()` in rules.rs (debug: 50 KB file, 9.5 s in `:lint`).
3. hardening -- `bash_runs` test temp dir is predictable and reused.

FILES READ:
CLAUDE.md
security-audit-2026-10-03/00-project-map.md
security-audit-2026-10-03/coverage-report.md
security-audit-2026-10-03/agents/shell-checker.md
security-audit-2026-10-03/findings/lang-eval.jsonl
crates/runfile-shell/src/rules.rs
crates/runfile-shell/src/tests.rs
crates/runfile-shell/tests/rules_doc.rs
crates/runfile-shell/src/lib.rs
crates/runfile-shell/src/words.rs
crates/runfile-shell/src/syntax.rs
crates/runfile-shell/src/script.rs
crates/runfile-shell/Cargo.toml
crates/runfile-lang/src/keywords.rs
crates/runfile-lang/src/tests/check.rs
crates/runfile-lang/src/tests/lines.rs
crates/runfile-lang/src/tests/parser.rs
crates/runfile-lang/src/tests/resolve.rs
crates/runfile-lang/src/tests/spans.rs
crates/runfile-lang/tests/editor_grammars.rs
crates/runfile-lang/tests/format.rs
crates/runfile-lang/tests/golden.rs
crates/runfile-lang/tests/readme.rs
crates/runfile-lang/tests/repo_format.rs
crates/runfile-lang/tests/rules_doc.rs
crates/runfile-lang/src/value.rs
crates/runfile-lang/src/eval.rs
crates/runfile-lang/src/functions.rs
README.md
SHELL-CHECK-RULES.md
.editorconfig
