# runtime agent -- final narrative

Scope: `crates/runfile-runtime` (Cargo.toml, examples/run.rs, src/*.rs, src/tests/*.rs), plus the
`runfile-env`/`runfile-lang` code these call (cross-referenced, not owned).
Binary under test: `target-linux/debug/run` 1.8.2 (HEAD a65dba1). Harness `SCRATCH/runtime/R`: `env -i`
with HOME/XDG/RUNFILE_CONFIG_DIR/TMPDIR pointed into SCRATCH, every `ci_detect` var and
RUNFILE_PRIVATE_KEYS stripped, `DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent` (so nothing reaches a
real keyring), `TERM=dumb`, `ulimit -v 4000000`, `timeout -s KILL`.

## What I examined and how

Read the whole crate (every src and tests file), the exec/env/props/run/dispatch/shell/term/interrupt
modules line by line, and the `runfile-env` build path and `runfile-lang` `value.rs`/`functions.rs`
quoting + temp code they reach. Traced each data source the brief named (ARG/ARGS/ENV, glob/list
values, captured output, `.env-file` contents, decrypted secrets) to its sinks (the shell, the
terminal, the credential store, child env, the trace). Proved effects with the isolated binary where
cheap; used two live lookups for the Windows platform items I cannot run.

## Experiments run and results

1. `.logging`/`traced` printf injection. A `$`-line value `x'; touch PWNED; echo '` under `.logging`:
   the announcement is `printf '%s\n' <to_shell(whole line)>` -- the entire rendered line is one
   single-quoted argument and the value lands on `%s`, not the format. No file created, no injection.
   A newline-bearing value trips the odd-single-quote test in `standalone()` and the block falls back
   to `announce()`. `traced` is safe against injection and `%`-misreport. CLEAN.
2. Captures under `--dry-run` (`let = $`, `if $`, `for in lines($…)`, `code_of($…)`, `match $`, `exec`
   in value position). None executed: zero marker files under `--dry-run`; all six markers under a
   real run. `exec::spawn`/`spawn_code` return early on `dry_run`. CLEAN.
3. `.env-file` decryption + keyring under `--dry-run`. A target with `.env-file` holding
   `RUNFILE_ENCRYPTION_PUBLIC_KEY=…` + `SECRET=encrypted:…` made `run --dry-run` reach
   `no matching private key is configured` -- produced only *after* the key pool is loaded
   (`provider.keys()` -> `all_private_keys` -> OS store). `build_env` is called unconditionally in
   `run_target_with` (run.rs:304). FINDING (medium): preview reads `.env-file`s and queries the keyring.
4. `.env-file` path reach. Absolute path outside the anchor and `../../outside.env` are both read
   (real run). `.workdir="../../../.../etc"` escapes the anchor. Both author-controlled (= running the
   target), so no boundary crossing beyond what a target's own `$ cat` already allows; noted, not a
   finding on its own, but it widens the dry-run finding's blast radius (arbitrary file reads on preview).
5. Deep acyclic dispatch chain. ~450 `.run` files each `run`ning the next overflow the 8 MB main stack
   and abort (SIGABRT) in debug; 100/200/300/350/400 succeed, 450 aborts; 3000 aborts. Happens under a
   real run AND under `--dry-run`. No depth cap exists (only discovery's unrelated `MAX_DEPTH=3`); the
   "step counter" in the dispatch.rs doc-comment is not implemented. Cycle detection stops only repeats.
   FINDING (low, dos).
6. `parallel for` label escape injection. `parallel for x in [ENV.EVIL, "plain"]` with EVIL carrying
   raw ESC(0x1b)+BEL(0x07): printed `aa^[[31mRED^[[0m^Gcc | done` verbatim. `item_label`/`segment`/
   `relay` never strip control bytes. FINDING (low) -- extension of the cli `:list` escape finding via
   a different path (run.rs `item_label`/`branch_labels` + exec.rs `relay`), different source (run-time
   list values).
7. `detach` env inheritance. A `detach $` child wrote both the `.env-file` value and
   `RUNFILE_PRIVATE_KEYS=deadbeefkey0000`. Confirms + extends the existing hardening finding: detached
   children receive the decrypted `.env`/`.env-file` values too, not just the key var.
8. `.add-path="."` + empty PATH segment. `.add-path="."` prepends the anchor so `$ date` runs the
   project's `./date` -- author-controlled (= `$ ./date`), relative entries anchor to the project (the
   documented, correct behaviour). An inherited `::` in PATH is the shell's/user-env's affair, not
   synthesized by runfile; `/usr/bin/date` won since the empty segment was last. CLEAN.
9. Unbounded branch threads (by inspection + code). `run_branches` does one `std::thread::scope`
   `s.spawn` per branch, no pool; `parallel_for` makes one fork per list element. N-element list = N
   threads. FINDING (hardening) -- documented as a deliberate non-decision, so rated hardening.

## Live lookups (2026-10-03)

- Rust 1.58 removed current-directory search from `std::process::Command` on Windows
  (blog.rust-lang.org/2022/01/13/Rust-1.58.0). The toolchain is pinned to 1.94.1, so `exec foo`/`$`
  program resolution never picks up the child's cwd (the project workdir). `shell::locate` uses PATH
  only (and skips WSL's launcher in System32/WindowsApps). CLEAN (mitigated).
- MSYS2 runtime emulates Unix de-quoting + wildcard expansion on the raw Windows command line before
  bash's `-c` parser (git.lorimer.id.au diff 9e9da23; claudeissues.com 93618 on backslash halving).
  `push_script` leaves a single-line no-whitespace body unquoted (`c.arg("-c").arg(body)`), so the
  value's self-quoting single quotes can be consumed by the MSYS2 layer and re-globbed by bash.
  FINDING (low, possible -- cannot run Windows).

## Checked and found clean (with reason)

- `traced`/`announce` printf injection: whole line single-quoted, value on `%s` -- experiment 1.
- Captures/`$`/`exec`/`code_of` under `--dry-run`: early return in `spawn` -- experiment 2.
- `term.rs` unsafe FFI: `winsize`/`CONSOLE_SCREEN_BUFFER_INFO` are POD, `mem::zeroed` is a valid value
  of each, the single ioctl/console call writes through a live, correctly-sized pointer, return codes
  are checked, and a non-tty is treated as "no answer". `KeepHandles` reads/sets one flag on handles
  `GetStdHandle` owns and restores them on drop. No OOB, no uninit read.
- `env::handed_over` precedence: the callee runs in a fresh `Runner` (env: Vec::new, built from its own
  props); nothing it sets flows back to the caller. A caller's decrypted secrets reaching a dispatched
  callee is the documented by-design hand-over (inherited.rs pins it), not a leak.
- `apply_add_to_path`: relative entries are pre-absolutized against the anchor in `env::build`, so the
  double-resolve is a no-op; no empty PATH segment is synthesized; a PATH none of the entries is on
  comes back byte-for-byte.
- Cycle detection itself (mutual `run`): A->B->A is refused with `HostError::Cycle`. (The uncapped
  *acyclic* depth is the separate finding.)
- Windows `Command` cwd hijack: mitigated by Rust >= 1.58 (toolchain 1.94.1).
- `Host::cleanup_temps` is called on every *normal* return path (main.rs:306, and the watch callback
  :298) and after `host.run` drops the key pool (:161). The gaps are signals/abort/panic -- the
  existing temp-survival finding, to which the stack-overflow abort (experiment 5) is one more entry.

## Notes on existing findings

- #5 (RUNFILE_PRIVATE_KEYS to children, hardening): confirmed, and EXTEND -- a `detach`ed child also
  receives the full decrypted `.env`/`.env-file` environment, not only the key var (experiment 7). Same
  mechanism (`exec.rs:349-351` copies all of `s.env`; `env::build` starts from the process env, nothing
  stripped). No new finding; the title already says "including detached processes".
- #2 (temp survival on signals/panic): confirmed. ADD: a stack-overflow abort from deep dispatch
  (experiment 5) is a hard abort (not a panic), so it also skips `cleanup_temps`. Release profile has no
  `panic = "abort"` (Cargo.toml), so ordinary panics unwind and still skip the non-Drop cleanup, as the
  finding states.
- #1 (secrets in failure/.logging/--dry-run output): confirmed. My new dry-run finding is deliberately
  scoped to the *side effects* of preview (keyring query + arbitrary `.env-file` reads), not the value
  reaching stdout, to avoid duplication.

## New findings written (runtime.jsonl, lines 6-10)

- medium: `--dry-run` builds the environment -- reads `.env-file`s and queries the OS keyring.
- low: unbounded acyclic `run` dispatch recursion -> stack-overflow abort (also under `--dry-run`).
- low: terminal escape injection via `parallel` branch labels (list values).
- hardening: `parallel for` spawns one unbounded OS thread per list element.
- low/possible: Windows Git Bash / MSYS2 re-parsing can defeat interpolation self-quoting for a
  no-whitespace body.

## Open questions

- The MSYS2 self-quoting hole (finding, possible) needs a Windows + Git Bash runner to confirm: does
  `bash -c ./x'*'` with the project's workdir as cwd actually glob `./x*`, and does the double-quoted
  common case stay safe? I could not run it.
- `temp_file()`/`temp_dir()` under `--dry-run`: `Scope.dry_run` gates `write_file`/`decrypt` (lang
  crate), but whether `temp_file` still creates a file under dry-run is a `functions.rs` question
  (lang-eval scope) -- not re-tested here.
- Whether a `detach`ed child on Unix is placed in its own process group; if not, a terminal Ctrl+C
  (group signal) would also kill the "fire-and-forget" child. Correctness, not security; left to note.

## FILES READ:
security-audit-2026-10-03/00-project-map.md
security-audit-2026-10-03/agents/runtime.md
security-audit-2026-10-03/findings/runtime.jsonl
CLAUDE.md
Cargo.toml
rust-toolchain.toml
crates/runfile-runtime/Cargo.toml
crates/runfile-runtime/examples/run.rs
crates/runfile-runtime/src/lib.rs
crates/runfile-runtime/src/exec.rs
crates/runfile-runtime/src/run.rs
crates/runfile-runtime/src/dispatch.rs
crates/runfile-runtime/src/env.rs
crates/runfile-runtime/src/props.rs
crates/runfile-runtime/src/shell.rs
crates/runfile-runtime/src/term.rs
crates/runfile-runtime/src/interrupt.rs
crates/runfile-runtime/src/tests/args.rs
crates/runfile-runtime/src/tests/checks.rs
crates/runfile-runtime/src/tests/keys.rs
crates/runfile-runtime/src/tests/mod.rs (surveyed)
crates/runfile-runtime/src/tests/parallel.rs (surveyed)
crates/runfile-runtime/src/tests/interrupt.rs (surveyed)
crates/runfile-runtime/src/tests/loops.rs (surveyed)
crates/runfile-runtime/src/tests/inherited.rs (surveyed)
crates/runfile-runtime/src/tests/exit.rs (surveyed)
crates/runfile-runtime/src/tests/exec.rs (surveyed)
crates/runfile-runtime/src/tests/walk.rs (surveyed)
crates/runfile-runtime/src/tests/names.rs (surveyed)
crates/runfile-runtime/src/tests/term.rs (surveyed)
crates/runfile-cli/src/main.rs
crates/runfile-env/src/lib.rs
crates/runfile-lang/src/value.rs
crates/runfile-crypto/src/lib.rs (grep)
