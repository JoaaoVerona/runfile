# Agent `lang-eval` — narrative

Scope: `crates/runfile-lang/Cargo.toml` and `src/{args,eval,functions,inputs,lib,span,structured,value}.rs`,
plus the unit tests in `src/tests/{args,eval,exec,expr,inputs,io,mod}.rs`. Commit a65dba1 (v1.8.2).
Binary: `target-linux/debug/run` (HEAD debug). Harness `$SCRATCH/lang-eval/R <projectdir> <args>`:
isolated HOME/XDG/RUNFILE_CONFIG_DIR, `TMPDIR=$SCRATCH/tmp`, CI vars stripped,
`DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent` (keyring never reached), `ulimit -v 4000000`,
`timeout -s KILL 20`. All fixtures and effects stayed under SCRATCH; no network, no repo target ever run.

## What I examined and how

Read the whole of value.rs, eval.rs, functions.rs, structured.rs, inputs.rs, args.rs, lib.rs, span.rs and
the scope's tests. Traced the untrusted-data path (ARG/ARGS/ENV, captured output, file contents, JSON,
glob results) to its sinks: the shell (`to_shell`/`shell_quote` + `interpolate_shell`), the `json` block
(`structured.rs`), and the filesystem (`call_io`). Proved each interesting point with an experiment against
the binary rather than by reasoning where an experiment was cheap. Shells present for the quoting matrix:
bash, dash (`/bin/sh`), busybox ash. zsh/ksh/brush not installed; I reasoned about them from the quoting
scheme (POSIX single-quote wrapping is inert in all of them in unquoted position).

## Experiments and results

1. **Shell-quoting matrix, unquoted context, all shells.** A target `$ argdump {{ ARGS }}` and
   `$ argdump x{{ ARGS }}y` run with 22 adversarial positionals (empty, spaces, `'`, `"`, `\`, `$HOME`,
   `` `id` ``, `$(id)`, `!!`, newline, CR, `*`, `~`, `~root`, `=ls`, `-n`, non-UTF-8 `ünï`, `{a,b}`, `#c`,
   `a;b`, `%s`). On bash, dash and busybox every element arrived as **exactly one argument**, byte-for-byte,
   no expansion, adjacency to literal text preserved. The core promise holds in unquoted position.
2. **Existing HIGH (double-quote / heredoc).** Reproduced and confirmed with a benign sentinel
   (`$(echo SUBST)`): `$ echo "v={{ ARG.v }}"` rendered `echo "v='$(echo SUBST)'"` and the substitution ran
   (output `v='SUBST'`); unquoted `$ echo v={{ ARG.v }}` was inert (`v=$(echo SUBST)`); an unquoted-delimiter
   heredoc expanded the value; `ssh host "cd {{ ARG.dir }} && make"` ran the payload locally. `run :lint`
   flagged **none** of the four forms (dq-echo, heredoc, ssh-nested, `echo >> rc`). See "Notes on existing
   findings" — the existing line is accurate; severity holds.
3. **serde_json deep nesting.** `json_type`/`json_query`/`json_format` on a 200k-deep array and a 100k-deep
   object all returned a clean `recursion limit exceeded` error (serde_json's default 128 limit is active,
   not disabled). No stack overflow. Clean.
4. **Float→int casts (DoS hunt).** Ran idx/substring/slice/exit/printf/range with `1e308`/`-1e308`.
   `as_index`, `count()` (substring), `range` and `exit` all reject or saturate safely. **`slice` panics**:
   `slice(xs, 1, number(ARG.n))` with `--n=1e20` aborts (`attempt to add with overflow` at functions.rs:515,
   exit 101). → NEW finding. Verified the abort leaves a `temp_file()` plaintext on disk (cleanup skipped).
5. **structured.rs JSON rendering.** `"{{ x }}"` (quotes-wrapped) in a `json` block is correctly rejected by
   post-render validation (the interpolation already renders a complete JSON value). The correct unquoted form
   `{ {{ k }}: {{ v }} }` escapes string values via serde: an injection payload `"a", "admin": true` became
   one escaped scalar `"\"a\", \"admin\": true"`; control chars/`"`/`\`/`\u0001` escaped; non-finite numbers
   (`inf` from `power(10,300)*power(10,300)`) render as `null`; `number("1e400")` rejected. A list in scalar
   position renders as a JSON array. No structure injection. Clean. (One note: serde leaves U+2028/U+2029 raw
   in strings — valid JSON, only a hazard if the output is later eval'd as JavaScript; runfile never does.)
6. **regex / ReDoS.** `regex_matches(repeat("a",100000), "(a+)+$")` finished in 7 ms — the `regex` crate is
   linear-time, no fancy-regex, with default size limits. No ReDoS. Clean.
7. **read_file on a FIFO** hangs (timed out). Requires a target (or a `_shared.run` let / `--dry-run`) to
   `read_file` an attacker-controlled path; same unbounded-`read_to_string` DoS class the untrusted-repo agent
   already filed for discovery's own `.run` read. Not re-filed — see open questions.
8. **NUL byte.** `read_file` of a file containing `\0` returns a 3-char string; passing it to a `$` line
   produces a clean `nul byte found in provided data` error (CString rejection), no panic. Clean.
9. **`?` / `try` cannot swallow a stop.** `confirm("…") ? "forced"` and `try(confirm(…))` both stay
   `cancelled` (exit 1); `exit(7) ? "forced"` propagates exit 7. The security-relevant controls (an explicit
   decline, an explicit exit) are not bypassable by a fallback chain. Confirmed in code (eval.rs:362) and by
   experiment. Clean.
10. **args.rs / inputs.rs smuggling.** `--key value` classification is driven by `inputs::of` (what the tree
    reads), a value beginning with `-` is refused rather than swallowed, `--` ends parsing. No word can smuggle
    itself into a different class across the boundary. Argument injection into a *downstream* command a target
    forwards `ARGS`/`ARG.x` to (e.g. a value `--output=/x` reaching a wrapped tool) is real but is the
    documented wrapper behavior (CLAUDE.md: "a wrapper forwards … and that command is the only thing that knows
    its own flags"); shell-injection is still prevented by self-quoting. By design, not a finding.

## NEW findings (1)

- **medium (confirmed)** — `slice(list, start, length)` panics on a data-driven `length`
  (functions.rs:508-519). `num(2)?.max(0.0) as usize` saturates to `usize::MAX` for a large finite float, and
  `start + …` is unchecked: debug aborts on overflow; release (overflow-checks off) wraps, producing
  `end < start`, so `items[start..end]` aborts one line later. The abort unwinds past `cleanup_temps`, leaving
  decrypted-secret temp files behind (same consequence as the existing sleep() / runtime temp-survival
  findings; the panic *trigger* is new and distinct — different function, different root cause).

## Notes on existing findings

- **HIGH "Self-quoted interpolation is executed inside double-quotes and heredoc bodies" (lang-eval).**
  Independently reproduced all four forms with a benign sentinel; `run :lint` refuses none of them. The
  finding, its locations (value.rs:101-129, eval.rs:560-569), attack path and severity are accurate. No
  correction. The one nuance worth recording: the danger is not only "a second shell re-reads the word" — the
  command substitution in `echo "v={{ x }}"` runs in the *same* shell that executes `echo`, so the checker's
  "judged only where no shell reads the word again" exemption misses the plain single-shell case too. The
  fix section already recommends flagging every double-quoted/heredoc interpolation, which covers this.
- **LOW "Empty list interpolation silently collapses adjacent literal text" (lang-eval).** Re-read value.rs;
  behavior and location (to_shell join of an empty list → empty string) are accurate. Nothing to add.
- **MEDIUM "sleep() huge finite value panics" (lang-eval).** Accurate. My slice finding is the sibling bug it
  did not cover (functions.rs:515 vs the sleep path), and both share the temp-leak consequence the runtime
  agent's "temp files survive … panic" finding owns. A single panic-safe-cleanup fix (Drop guard / catch_unwind
  around `host.run`) closes the confidentiality side of all three at once.

## Checked and found clean (with why)

- **value.rs `shell_quote`/`to_shell`** — correct in unquoted position for every byte and every installed
  shell (experiment 1); the one gap is quoted/heredoc context, already filed.
- **structured.rs** — strings escaped by serde, non-finite numbers → `null`, quotes-wrapped interpolation
  rejected by post-render validation, no structure/key injection (experiment 5).
- **serde_json paths** — default recursion limit active, deep input errors cleanly, no stack overflow (3).
- **regex functions** — linear-time crate, default size limits, no ReDoS (6).
- **`?`/`try`** — never swallow `confirm()`/`exit()` (9), so a decline or explicit stop cannot be bypassed.
- **args.rs/inputs.rs** — no flag/positional smuggling, `--` correct (10).
- **cast sites** — `as_index` (value.rs:95), `count()` (functions.rs:699), `exit` i32 (62) all guarded or
  saturate harmlessly; only `slice`'s `start + …` is unchecked (filed).
- **dry-run effect discipline** — write_file/decrypt/temp_file/temp_dir/sleep guarded; decrypt's dry-run arm
  returns a placeholder *before* touching `private_keys`, so the keyring is not reached under `--dry-run`;
  print/printf/now/uuid run by design. No function in this scope spawns a process (Capture/Dispatch error out
  in pure eval and are handled by the runtime crate).

## Open questions

- `read_file()`/`glob()` have no size or file-type limit. A target or `_shared.run` `let` pointed at a FIFO
  hangs and at `/dev/zero` OOMs. Whether a data-controlled path reaching these under a *read-only* entry point
  (`--dry-run`, `:lint`, `:list`) is in practice reachable is the untrusted-repo/runtime agents' call; the
  unbounded-`read_to_string` DoS class is already filed by untrusted-repo for discovery's own read.
- Does `--dry-run` / `:list` evaluate a `_shared.run` `let x = slice(…, number(ENV.N))`? If so the slice panic
  is reachable from a read-only entry point with an attacker-set ENV; left to the runtime agent's env-building
  trace.

## FILES READ:
crates/runfile-lang/Cargo.toml
crates/runfile-lang/src/value.rs
crates/runfile-lang/src/eval.rs
crates/runfile-lang/src/functions.rs
crates/runfile-lang/src/structured.rs
crates/runfile-lang/src/inputs.rs
crates/runfile-lang/src/args.rs
crates/runfile-lang/src/lib.rs
crates/runfile-lang/src/span.rs
crates/runfile-lang/src/tests/mod.rs
crates/runfile-lang/src/tests/exec.rs
crates/runfile-lang/src/tests/expr.rs
crates/runfile-lang/src/tests/eval.rs
crates/runfile-lang/src/tests/io.rs
crates/runfile-lang/src/tests/args.rs
crates/runfile-lang/src/tests/inputs.rs
crates/runfile-runtime/src/exec.rs
Cargo.toml
Cargo.lock
security-audit-2026-10-03/00-project-map.md
security-audit-2026-10-03/findings/lang-eval.jsonl
security-audit-2026-10-03/agents/lang-parser.md
security-audit-2026-10-03/agents/runtime.md
security-audit-2026-10-03/agents/secrets-crypto.md
CLAUDE.md
