# Agent `parsers` -- language front end (`lang-parser`)

Scope: crates/runfile-lang/src/{ast,check,format,keywords,lexer,parser,resolve,types}.rs, the unit tests
src/tests/{check,lines,parser,resolve,spans}.rs and integration tests tests/{editor_grammars,format,golden,
readme,repo_format,rules_doc}.rs. Commit a65dba1 (v1.8.2). Binary: target-linux/debug/run (debug build, overflow
checks on). Harness: $SCRATCH/g.sh runs the binary under `env -i`, an empty HOME/XDG/config, DBUS pointed nowhere,
`ulimit -v 4000000; ulimit -s 8192`, `timeout -s KILL`, and prints exit status + wall time.

## What reaches the parser, and what runs after it

`list::facts()` (crates/runfile-cli/src/list.rs:25) calls `runfile_lang::parse` for EVERY target file, only to
read `hidden`/`description`/`uses_args`. So `run :list`, `run :list --json` (the VS Code extension runs this on
startup), `run :list --names` and `run :complete` (every `run <Tab>`) parse every `.run` file in the discovered
project. `run <t> --help`, `run :lint`, `run :generate`, and the language server (`run :lsp`) parse too. A parse
failure that aborts the process therefore breaks the tooling for the whole repository, not just one target.
Confirmed: a single malformed file aborts `run :list` and `run :complete` for the entire project (see below).

Two later stages run only in `Host::load` (before every `run`, including `--dry-run`), in `run :lint`, and in the
LSP's `document::diagnostics` (NOT in :list/:complete/--help, which only parse):
`runfile_lang::resolve` (name check) then `runfile_lang::check::check` (fails-every-time check). Confirmed
`run --dry-run` reaches both; `run :list` reaches neither.

## Experiments and results

1. Existing finding re-verified -- non-ASCII slice panics (parser.rs:1419 `&s[..s.len().min(60)]`,
   parser.rs:920 `&l.raw[base..]`). `let x = 1 "` + 40x`é` + `"` aborts (101) at parser.rs:1419; an `exec`
   body indented with a tab then U+3000 aborts at parser.rs:920. BOTH reproduced through `run :lint`,
   `run :list` (whole project) and `run :complete` -- one bad file kills listing/completion for the repo.
   The existing finding's severity (low) and locations are correct; left unchanged, with this confirmation.

2. NEW -- unbounded parser recursion (stack overflow, exit 134). `let x = (((...)))` overflows the 8 MB
   main-thread stack at ~800 parens (500 is fine); `let x = [[[...]]]` at ~700; `if true`/`end` nested at
   ~500. Index chains `a[0][0]...` and `?` chains are iterative and do NOT overflow. All three abort
   `run :list` and `run :complete` over the project. No depth cap in parser.rs; no catch_unwind, and a guard-page
   overflow is uncatchable anyway. Filed low (same impact profile as the existing panic: broad read-only
   tooling DoS, recovers when the file is removed).

3. NEW -- quadratic spilled-list parse. `P::logical` (parser.rs:350) calls `lexer::brackets(&text)` -- a full
   re-tokenize of the whole accumulated statement -- once per continuation line. A one-per-line list: 2000
   elements = 2.8 s, 5000 = 17.8 s (clean N^2), 10000 killed at 60 s; `run :list` over the project with a
   6000-element spill = 8.4 s. Filed low.

4. NEW -- super-linear `resolve::suggest`. For every unresolved name, `read_hint`/`expr` call `suggest`, which
   runs a full O(len^2) `edits()` Levenshtein against EVERY in-scope binding. O(M reads x N bindings x L^2).
   3000+3000 short names = 22.7 s; the same file with DEFINED reads = 0.39 s (isolates `suggest`); 300+300
   reads with 200-char names = killed at 50 s (L^2 makes a ~120 KB file hang); `run --dry-run` on it = 21 s,
   `run :list` on it = 0.03 s. Filed medium -- it hangs the single-threaded LSP per keystroke and the
   documented-safe `--dry-run` preview, and the length factor makes a small file catastrophic.

5. Fuzzing. A mutation fuzzer over all repo `.run` files + golden fixtures, 1500 iters x {--dry-run, :lint},
   then a structural nesting/unicode fuzzer, 1500 iters x {:list, :list --names, :lint}. 1430 crash events,
   ALL mapping to exactly two panic sites (parser.rs:920 and parser.rs:1419) -- i.e. no NEW panic location
   beyond the existing finding. The recursion and quadratic bugs are not found by random fuzzing (they need
   ~hundreds of balanced brackets / thousands of lines) but are trivially constructed by an attacker.

## Checked and clean (with why)

- chrono: runfile-lang does not depend on it. `now_formatted` (functions.rs:755) is a closed match returning
  `None` for unknown formats and uses a hand-rolled `civil_parts`; no panic on a crafted format string.
  The task's "chrono panics on invalid specifiers" concern does not apply here.
- check.rs `invalid-literal` regex compile: `regex::Regex::new` default size_limit (~10 MB) rejects oversized
  patterns fast (`a{1000000}{1000000}`, `[a-z]{50000}{50000}` -> "not a regex" in <20 ms). regex is
  non-backtracking, so no ReDoS at match time (and the checker only compiles, never matches). Clean.
- check.rs `invalid-literal` glob compile: `glob_plan` on `*/`x5000 = 0.08 s, nested `{a,...}`x30 = <5 ms.
  globset bounds it. Clean.
- check.rs `now`/`printf`: `now_formatted` and `parse_format`/`format_count` are linear scans. Clean.
- Formatter meaning changes: `:lint --stdout` on adversarial inputs (a `#` in a `$` line, after an `exec`
  command, inside `{{ }}`, in a continuation line, tight `a#b`, an `exec` body with an `end`-like line) kept
  the `#` in its correct context every time. `format()` re-parses and compares fingerprints (format.rs:42),
  which refuses any tree change; comments are copied via source spans and re-attached to the same statement.
  No case found where a comment moved between shell and language context. Clean (no finding).
- Integer overflow in span arithmetic: `offset_in` (parser.rs:92) is bounded pointer subtraction within the
  slice; newline/line counts are usize adds that cannot realistically reach 2^64. Clean.
- types.rs union fixpoint (`Names::of`): bounded by type-bit growth (<=4 bits/name); 6000 bindings = 0.29 s.
  Clean.

## Notes on existing findings

- lang-parser.jsonl #1 (non-ASCII slice panic, low): re-verified at both sites and via :list/:complete over a
  whole project. Correct as written; not modified. Severity low is defensible (broad but recoverable, no exec
  or data loss) -- it sits at the low/medium boundary given it aborts listing for an entire repo.

## Open questions

- Release build: the shipped binary has no overflow checks, so the recursion needs marginally deeper nesting
  (smaller frames) but still aborts on a sub-10 KB file; not separately reproduced here (no release build, and
  the project rule forbids building).
- The LSP was traced in code (single main thread, no spawn, no catch_unwind) rather than driven over JSON-RPC;
  the recursion/quadratic/suggest costs all flow through functions the LSP calls (parse, resolve, check).

## FILES READ:
CLAUDE.md
security-audit-2026-10-03/00-project-map.md
crates/runfile-lang/src/lib.rs
crates/runfile-lang/src/span.rs
crates/runfile-lang/src/lexer.rs
crates/runfile-lang/src/parser.rs
crates/runfile-lang/src/ast.rs
crates/runfile-lang/src/format.rs
crates/runfile-lang/src/check.rs
crates/runfile-lang/src/types.rs
crates/runfile-lang/src/resolve.rs
crates/runfile-lang/src/functions.rs
crates/runfile-cli/src/list.rs
crates/runfile-runtime/src/dispatch.rs
