# Agent `parsers` -- shell checker (`shell-checker`)

Scope: crates/runfile-shell/{Cargo.toml, src/lib.rs, src/rules.rs, src/script.rs, src/syntax.rs, src/tests.rs,
src/walk.rs, src/words.rs, tests/rules_doc.rs}. Commit a65dba1 (v1.8.2). Binary: target-linux/debug/run.
Harness: $SCRATCH/g.sh (env -i, empty HOME/XDG, DBUS nowhere, `ulimit -v 4000000 -s 8192`, timeout -s KILL).

## Where the shell checker runs

`runfile_shell::check` runs in `runfile_runtime::Host::load` (before EVERY `run`, including `run --dry-run`),
in `run :lint` (via runfile_lsp::document::diagnostics), and in the language server's `analysis::diagnose` on
every document open/edit. It does NOT run in `run :list`/`:complete`/`--help` (those only parse). The LSP is
single-threaded on the main thread (no thread::spawn in runfile-lsp/runfile-cli), with no catch_unwind.

## Experiments and results

1. Existing finding re-verified -- exponential-time walk from nested loops. `walk.rs` walks each `for`/`while`/
   `until`/`loop`/`retry` body TWICE for flow-sensitivity (walk.rs:325-369), which is multiplicative across
   nesting: N nested loops = 2^N block walks. Measured through `run :lint --check` with N nested
   `for i in [1,2]` around one `$` line: depth 15 = 0.70 s, 18 = 5.64 s, 20 = 21.9 s, 22 killed at 60 s --
   clean 2x-per-level doubling. The existing medium rating HOLDS. New evidence for the open question: the LSP
   is single-threaded, so a depth-20 file blocks the one thread that answers every request for ~22 s (depth
   22+ effectively forever), and each edit re-triggers it -- the whole server is unresponsive, not just slow.
   Left unchanged.

2. NEW -- unbounded recursion in the word reader (stack overflow, exit 134). `words::parameter` loops over a
   `${...}` body and, on an inner `$`, calls `words::dollar` which calls `parameter` again (words.rs:278-281,
   363-412) with no depth counter. The depth-64 cap in `syntax::command` (syntax.rs:456-464) guards only
   `$(...)`/`<(...)` (which recurse through command()), NOT word-level `${...}`. Measured through
   `run :lint --check`: `$ echo ${x+${x+ ... }}` nested ~1000 deep = exit 0 (ok), ~3000 deep = abort (134); a
   ~15 KB line. The language parser treats the `$` line body as opaque, so the overflow is purely the shell
   checker's. A second vector: the bash `[[ ... ]]` conditional grammar (`Cond::or`/`and` right-recursion,
   syntax.rs:1063-1080) aborts on `$ [[ ` + 60000 `x || ` (a ~300 KB line). `$(...)` nesting confirmed safely
   capped (5000 deep = clean exit 0, no overflow). Filed low (same profile as the lang parser recursion:
   aborts Host::load/:lint/LSP before any run; recovers when the file is fixed).

## Checked and clean (with why)

- `$(...)` / `<(...)` nesting: capped at depth 64 by `command()` (syntax.rs:458); 5000-deep gives up cleanly
  (Stop::Lost, exit 0). Clean.
- `$"..."` nesting: does not recurse unboundedly (loops in `double`); 20000-deep = exit 0. Clean.
- Heredoc / backtick / `$[...]` / arithmetic readers (words.rs): iterative loops over the character stream,
  not mutually recursive without a cap, except through `command()` which is capped. No overflow found.
- script.rs placement: `place`/`build` are linear over the body; `skip_interp` is iterative. Clean.

## Notes on existing findings

- shell-checker.jsonl #1 (exponential loop walk, medium): re-measured and confirms the quoted timings; the
  medium rating holds. Added the LSP single-thread observation above (the open question in the task). Not
  modified.

## Open questions

- Release build behaviour of the `${...}` overflow not separately reproduced (no release build; project rule
  forbids building); recursion depth is frame-count-driven, so a release build aborts at marginally deeper
  nesting on a slightly larger (still small) line.
- The `[[ ]]` cond recursion needs a very large single line (~60000 operators); recorded as a secondary
  vector of the same finding rather than its own.

## FILES READ:
CLAUDE.md
security-audit-2026-10-03/00-project-map.md
crates/runfile-shell/src/lib.rs
crates/runfile-shell/src/walk.rs
crates/runfile-shell/src/syntax.rs
crates/runfile-shell/src/script.rs
crates/runfile-shell/src/words.rs
crates/runfile-runtime/src/dispatch.rs
crates/runfile-cli/src/list.rs
