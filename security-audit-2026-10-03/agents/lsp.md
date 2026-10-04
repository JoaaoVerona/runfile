# Agent `lsp` — narrative

Scope: `crates/runfile-lsp/{Cargo.toml,src/{analysis,document,lib,rpc,server}.rs,tests/protocol.rs}`,
`crates/runfile-cli/tests/lsp.rs`, and the `run :lsp` arm in `crates/runfile-cli/src/main.rs`.
Commit a65dba1 (v1.8.2). Binaries exercised: `target-linux/debug/run` and the installed release
`~/.local/bin/run` (both 1.8.2).

The crate is five source files; there are no other `runfile-lsp` source files (the directory listing shows
only `analysis.rs`, `document.rs`, `lib.rs`, `rpc.rs`, `server.rs`). I read all of them, the two test files,
`main.rs`'s `:lsp` arm, and the supporting code the server reaches: `runfile-discovery` (the discovery walk,
`shared_chain`, `scope_of`, `is_file` gating), `runfile-runtime::props::check` and `exec::body_is_shell` (the
property checker the diagnostics call), and the `invalid-literal` helpers in `runfile-lang::check`/`functions`
(the only places the "what fails every time" checker touches the outside world).

## What I examined and how

### 1. Does any diagnostic / completion / hover / definition / formatting path execute, write, touch the keyring, or hit the network?

Read-through first, then proved it by experiment. The architecture is clean by construction: `analysis.rs` is
pure functions over text; diagnostics are `runfile_lang::parse` + `resolve` (name resolution) +
`runfile_lang::check` (static "fails every time" rules) + `runfile_shell::check` + `props::check`. None of
these spawns a process or evaluates an expression for effect:

- `runfile_lang::check`'s only outside calls are read-only *validators*: `regex::Regex::new`,
  `functions::glob_plan` (plans a glob, does not walk the FS), `functions::now_formatted` (reads the clock,
  no write), `parse_format`. No `$` capture, `write_file`, `read_file`, `glob`, `decrypt`, `temp_file`, or
  `Command` is ever evaluated — the checker works on the AST and types, never the evaluator.
- `runfile_shell::check` is a static bash reader (`rules.rs`/`walk.rs`); its only `Command` references are its
  own AST enum variant, not `std::process::Command`.
- `props::check` and `exec::body_is_shell` are pure (string classification of the property line).
- The server reads files only via `std::fs::read_to_string` in `text_of` (open `_shared.run` chain) and via
  discovery's `read_dir`/`is_file`/`canonicalize`. It never opens anything for writing; formatting returns
  LSP edits and the client writes.

**No-execution proof (experiment).** I built a workspace whose root `_shared.run` and `build.run` are packed
with side-effecting constructs — `$ touch <sentinel>` captures (plain, in `let`, in `.env.X`, in `.workdir`,
in `for/if/until` headers, in a `json` block interpolation), `write_file`, `temp_file`, `temp_dir`,
`read_file("/etc/hostname")`, `exec sh ... end`, `detach $`, `decrypt`, `glob`, `code_of(run ...)`, `print`,
`confirm`, `sleep`, plus a machine-wide `~/.runfiles` with its own side-effecting `_shared.run` and a scoped
one — then drove `run :lsp` (isolated: `env -i`, redirected HOME/XDG/TMPDIR/RUNFILE_CONFIG_DIR,
`DBUS_SESSION_BUS_ADDRESS=/nonexistent`, `ulimit -v`) **under strace** and opened every file, ran completion
/ hover / definition at every line and several columns, formatted, and sent a didChange that added a new
`$ touch` capture. Results:

- `strace -e %process,%network,...`: exactly one `execve` (the server itself), **zero** clone/fork/execve of
  any child, **zero** socket/connect, and **zero** write-mode `openat` (no `O_WRONLY`/`O_CREAT`/`O_TRUNC`),
  no `unlink`/`mkdir`/`rename`/`truncate`/`symlink`/`chmod`.
- Files opened read-only: only the `.run`/`_shared.run` sources and shared libs; directories stat'd during
  discovery. No `.env`, no keyring, no network.
- The sentinel directory and TMPDIR stayed **empty** after the whole session. No `touch` fired, nothing was
  written, no temp file was created.

So opening/editing/completing/formatting a hostile repo executes none of its code, writes nothing, and
touches neither the keyring nor the network. The diagnostics correctly *report* the side-effecting lines
(e.g. `` `decrypt` takes 2 arguments ``, capture-position errors in `_shared.run`) without running them.

### 2. Paths and URIs; reading/writing outside the workspace

- `uri_to_path` (server.rs:345) strips `file://`, percent-decodes, and builds a `PathBuf`. It does not handle
  a `file://host/path` authority (the host would become a leading path component) and maps Windows
  `file:///C:/x` to `/C:/x`; these are robustness quirks, not an escalation — the server only ever *reads*
  from paths discovery hands it, never writes.
- The server reads `_shared.run` files found by discovery. A **symlinked** `_shared.run` is followed
  (`is_file()` follows links), so a symlink to a large regular file, or a genuinely huge committed
  `_shared.run`, is read whole by `text_of` → unbounded `read_to_string`. Confirmed: a 1.7 GB `_shared.run`
  made `didOpen` of a sibling hang past 20 s (no reply). This is the untrusted-repo agent's unbounded-read
  root cause reaching the LSP — recorded there, not duplicated. A **FIFO** named `_shared.run` does *not*
  hang the server: `Catalog`/`shared_chain` gate on `is_file()`, which is false for a FIFO, so it is skipped
  (verified by strace — the FIFO `_shared.run` is never opened). A FIFO/`/dev/zero` only bites the CLI paths
  (`:list`/`:complete`/`--help`) that read *target* files, not the LSP.
- No write path exists in the server. Formatting returns one whole-document edit (confirmed in strace: no FS
  write).

### 3. Framing (rpc.rs)

Drove the server with malformed frames. Results: bad / negative / non-numeric Content-Length → `Protocol`
error, loop continues, server stays up and answers the next request (good). Missing Content-Length → the read
blocks for more header lines (expected: it is waiting for the body length). Invalid JSON body / invalid UTF-8
in the *body* → `Protocol`, continues (good). Invalid UTF-8 in a *header* → `read_line` returns an IO
"stream did not contain valid UTF-8" → `ReadError::Io` → `serve` returns Err and the process exits 1 (a trusted
client never sends this; minor). Non-object JSON, wrong-typed params, missing params → handled gracefully via
`unwrap_or_default`, every request answered. **Huge Content-Length** (`99999999999999`) → `vec![0u8; len]`
pre-allocates and the allocator aborts the process (SIGABRT) — filed as a hardening note (client trusted).

### 4. Robustness against document content

- **Fuzzing**: ~20k randomly-generated documents (runfile atoms + multibyte/`\r`/`\ufeff`/NUL/emoji), each
  followed by 6 completion/hover/definition requests at random positions and a formatting request. The
  server survived every malformed document *except* when it hit the two already-known parser panics, which it
  does reproduce (see below). No new analysis-layer panic surfaced in `analysis.rs` itself over the sweep.
- **Stack overflow (new, filed medium)**: deeply nested parens/brackets/`if`/`else if` in a `.run` file
  overflow the recursive-descent parser's stack and `abort()` the server on didOpen/didChange (and abort
  every read-only CLI path). A Rust stack overflow cannot be caught; only a depth cap fixes it. Confirmed on
  debug and release. This is a *new* root cause (not the listed non-ASCII panic, not the shell-checker
  exponential, not the dispatch-recursion), so I filed it with the LSP reach traced.
- **Client-position overflow/hang (hardening)**: `complete` does `no + 1` with `no` from the client position,
  which panics in debug (wraps in release) at `usize::MAX`; a large line number drives `parse_around`'s
  padding loop into a >35 s stall. Client-trusted, so hardening.
- `parse_around`'s repair loop (16 reparses × up to 4 closers) is quadratic-ish on broken documents but
  bounded: completion on a 1 MB document with 16 broken lines took ~1.9 s (release) — slow but not a hang,
  and real files are far smaller.

### 5. Cross-file cost

Opening one file triggers a full `discover_unscoped` walk (up for the nearest `runfiles/`, down ≤3 for
subprojects) plus parsing the `_shared.run` chain. Measured with a 20,002-file project: `didChange` on a
target ≈ 0.08 s; on a `_shared.run` ≈ 0.81 s (release) — the discovery walk dominates but stays sub-second.
The symlink-loop and unbounded-read discovery DoSes (untrusted-repo agent) reach the LSP the same way they
reach `:list`; recorded there.

### 6. Information disclosure

Diagnostic `message` fields are plain text (LSP renders them as text), so no markdown injection there.
Hover cards are built from built-in constant docs (`FUNCTIONS`/`KEYWORDS`/`SOURCES`), not user text.
**But** completion `documentation` for an in-scope binding embeds the binding's raw source line inside a
````runfile` fence, and a `_shared.run` binding line is attacker-controlled on-disk text. A lone `\r` in it
breaks out of the fence after the client's markdown renderer normalises CR→LF, turning the rest into live
markdown — an auto-loading remote image (tracking beacon) and a disguised clickable link in the popup. Filed
low (trusted-workspace precondition; `command:` links are stripped so no RCE). Confirmed the breakout by
capturing the server's documentation value and rendering it through a CommonMark renderer; confirmed VS Code's
untrusted-markdown image/link policy and `img-src https:` CSP from the microsoft/vscode source.

## Did the existing root-cause DoS bugs reach the server?

- **lang-parser non-ASCII byte-boundary panic** — **KILLS the server.** `didOpen` of `let x = 1 "éé…"`
  (trailing-token message sliced mid-char, parser.rs:1419) and of an `exec` body indented with mixed-width
  Unicode whitespace (parser.rs:920) both abort `run :lsp` (process exits, all diagnostics for the editor go
  down; VS Code restarts it a few times then gives up). Reproduced via didOpen *and* via didChange. Root cause
  recorded by lang-parser; not duplicated.
- **shell-checker exponential nested-loop walk** — **HANGS the server (CPU).** Reached through
  `document::diagnostics → runfile_shell::check`. Not infinite at modest depth (depth 18 ≈ 8 s of CPU in
  debug; doubling per level), but a few hundred bytes of ~27+ nested loops pegs a core for minutes and each
  edit restarts it. Recorded by shell-checker; not duplicated.
- **untrusted-repo unbounded `read_to_string`** — **reaches the server** via `text_of` for a regular (or
  symlink-to-regular) `_shared.run`: a 1.7 GB file hung `didOpen` past 20 s. A FIFO `_shared.run` is *not*
  reached (gated by `is_file()`), so the FIFO hang is a CLI-only effect. Recorded by untrusted-repo.
- **untrusted-repo symlink-loop discovery** — reaches the server through the same `discover_unscoped` the LSP
  calls on every open; recorded there.

A crash in any of these takes the editor's language features down for that workspace; none of them, nor my
own findings, causes execution or a write.

## Clean / checked (and why)

- **No execution / write / keyring / network** from any server path — proved by strace with side-effecting
  fixtures and empty sentinels (above).
- **Formatting never writes** — returns edits only; strace shows no write-mode open.
- **Framing** handles negative/non-numeric/missing/invalid-JSON/invalid-UTF-8-body gracefully and keeps
  serving; every request is answered (unknown method → `null`, server.rs:94).
- **go-to-definition / in_shared** only return `uri`+position locations into files discovery already found;
  no content of those files is sent except the completion-documentation line (finding 2).
- **Discovery ownership / `.only-in-directories` / machine-wide** handled by `discover_unscoped` — the LSP
  reads scopes only as diagnostics, never enforces them to run anything.
- **serde_json deep nesting** in the `json_*` checker helpers — lang-eval verified serde's recursion limit
  holds; the checker only calls the read-only validators, so no deep-structure crash via the LSP.

## Open questions

- The completion-documentation beacon (finding 2) was confirmed at the protocol/markdown level but not inside
  a live VS Code instance (no GUI here). Whether VS Code additionally gates remote images behind a per-image
  prompt in recent builds would lower it further; current source does not.
- Non-ASCII workspace *paths* (e.g. `ws_ção/`) round-trip through `path_to_uri`/`uri_to_path` correctly and
  diagnostics/definition work; I did not find a path-handling bug there, but Windows UNC/`file://host/` URIs
  were not testable on this host.

FILES READ:
crates/runfile-lsp/Cargo.toml
crates/runfile-lsp/src/lib.rs
crates/runfile-lsp/src/rpc.rs
crates/runfile-lsp/src/server.rs
crates/runfile-lsp/src/document.rs
crates/runfile-lsp/src/analysis.rs
crates/runfile-lsp/tests/protocol.rs
crates/runfile-cli/tests/lsp.rs
crates/runfile-cli/src/main.rs
crates/runfile-discovery/src/lib.rs
crates/runfile-runtime/src/props.rs
crates/runfile-runtime/src/exec.rs
crates/runfile-lang/src/check.rs
crates/runfile-lang/src/functions.rs
crates/runfile-lang/src/parser.rs
crates/runfile-lang/src/resolve.rs
crates/runfile-lang/src/types.rs
editors/vscode/src/lsp.ts
editors/vscode/src/pure.ts
security-audit-2026-10-03/00-project-map.md
CLAUDE.md
