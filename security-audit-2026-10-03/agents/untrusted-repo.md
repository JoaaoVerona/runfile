# Agent `untrusted-repo` — narrative (final)

Scope (accountable, line-by-line): `crates/runfile-discovery/{Cargo.toml, examples/list.rs, src/lib.rs,
src/tests.rs}`.

Cross-cutting owner of **threat (a)**: every entry point that processes a repository *without* the user
asking to run a target — `run :complete` (all four shells), `run :list[/--names/--json]`,
`run <t> --help`, `run --dry-run <t>`, `run :lint[/--include-global]`, `run :generate zed|jetbrains|vscode`,
the `--stdin-args` check path, and `Host::load`/`Host::check`. Plus the discovery-internal questions: walk
bounds, symlink loops, depth caps, file types read, name validation, `.only-in-directories`, machine-wide
selection, `AmbiguousGlobal`.

Commit a65dba1 (v1.8.2). Binary: `target-linux/debug/run` (debug, HEAD). Harness `$SCRATCH/g.sh` runs the
binary under `env -i` with isolated HOME/XDG/RUNFILE_CONFIG_DIR/TMPDIR, CI vars stripped,
`RUNFILE_SKIP_PREPARE=1`, DBUS pointed at `/nonexistent` (so nothing reaches the real keyring), `TERM=dumb`,
`ulimit -v 4000000`, `timeout -s KILL`.

## What I examined and how

Read in full: `runfile-discovery/src/lib.rs` + `tests.rs` + `examples/list.rs` + `Cargo.toml` (my scope);
and, tracing threat (a) end to end, `runfile-cli/src/{main.rs, list.rs, target_help.rs, cmd_lint.rs,
cmd_generate.rs, completions.rs}`, `runfile-lsp/src/{server.rs, document.rs}`, `runfile-runtime/src/dispatch.rs`,
and the `--dry-run` / side-effect guards in `runfile-lang/src/functions.rs` and `runfile-env/src/lib.rs` +
`runfile-state/src/{keyring_keys.rs, keyring_store.rs}`. Then ran experiments against each entry point.

## Experiments and results

1. **Walk-up trust (re-verify existing high).** `shared/runfiles/build.run` with `$ touch MARKER` planted in
   an ancestor; victim in `shared/victim/a/b/c` with no nearer `runfiles/`. `run :list` and
   `run :complete 1 run ""` listed the attacker target; `run build` **created the marker** — the attacker's
   code ran as the victim. Reproduces exactly. See "Notes on existing findings".
2. **Read-only entry points evaluate nothing (open question 4).** A target with a body `write_file(...)`, a
   `$ touch`, and a `_shared.run` `let write_file(...)`. Ran `:list`, `t --help`, `:complete 1 run t`,
   `:lint --check`, `:generate {vscode,jetbrains,zed} --stdout` each with a clean marker dir between — **no
   markers created by any of them**. Only a real run (and `--dry-run`, below) produced the markers. So these
   paths parse/resolve/check statically and never evaluate `_shared.run` lets, body expressions, or touch the
   keyring. `Host::check` (the `--stdin-args` pre-prompt check) = `load().map(|_|())`, which is parse +
   `resolve` + `check` + `runfile_shell::check` — all static; confirmed no side effects.
3. **`--dry-run` executes reads and `print` (open question 3).** A target doing
   `read_file(join_path(ENV.HOME, ".ssh/id_ed25519"))`, `read_file("/etc/passwd")`, `glob(~/.ssh/*)`,
   `directory_exists`, each followed by `print(...)`. `run --dry-run preview` printed `exfil-key:
   FAKE-SSH-KEY-0000`, `passwd-lines: 52`, and the globbed path. `$`/exec captures and dispatched `run`/`$`
   lines did **not** run (markers absent), and `write_file`/`temp_file`/`temp_dir`/`decrypt` are placeholder-
   guarded. With an encrypted `.env-file` + public-key line, `--dry-run` reached the keyring (strace showed a
   `keyctl(KEYCTL_GET_KEYRING_ID…)` call and the "could not load private keys" warning). → **finding 2**.
   Precise answer to open question 3: under `--dry-run` these RUN — `_shared.run` lets, header property
   values, body `let`/expression evaluation, `read_file`/`glob`/`file_exists`/`directory_exists`/
   `is_executable`/`json_*`/`now`/`uuid`/`base64_*`, `print`/`printf`, and `.env`/`.env-file` build +
   decryption (keyring). These do NOT run — `$` captures, `exec` captures, `code_of($…)`, `code_of(run …)`,
   dispatched `run` targets, `$`/`exec` statement bodies, `write_file`, `temp_file`, `temp_dir`,
   `decrypt(dst)`, `sleep`, `confirm` (auto-yes), and the `.watch` probe (`header_props`, skipped by
   main.rs:273 under dry-run). `.workdir` is just a property value (no cwd change without a command).
4. **Symlink-directory walk DoS.** `runfiles/a -> .` alone → `:list --names` returns 42 names and exits
   (PATH_MAX bounds the linear growth). `runfiles/a -> .` **plus** `runfiles/b -> .` → `run :list` and
   `:complete` were **OOM-killed (SIGKILL, rc=137)** inside the 20 s timeout under the 4 GB ulimit (branching
   factor 2 ⇒ exponential directory visits before PATH_MAX). → **finding 1**.
5. **`:generate` injection via target name.** Repo with `runfiles/evil$(id).run` and `runfiles/x; id #.run`.
   `:generate jetbrains --stdout` emitted `<option name="SCRIPT_TEXT" value="run --stdin-args evil$(id)" />`
   and `value="run --stdin-args x; id #" />` — `xml_attr` escaped nothing (no XML metachars), so the shell
   metacharacters land verbatim in a string JetBrains runs as a shell script. `:generate vscode`/`zed` carry
   the same name as a `type:shell` task arg. → **finding 3**.
6. **`:lint` symlink write.** `runfiles/build.run -> ../outside/victim.run` (victim is valid but unformatted
   runfile syntax). `run :lint` reported `runfiles/build.run: formatted` and **rewrote the outside file**
   through the symlink (symlink preserved, target truncated+rewritten; non-atomic). A symlink to a non-
   runfile (e.g. `.bashrc`) is left alone because it fails to parse. → **finding 4**.

## New findings (4)

- **medium** — Discovery follows directory symlinks with no loop detection → exponential walk (OOM/hang) on
  `:list`/`:complete`/`--help`/`:lint`/LSP/VS Code folder-open. `walk_runs` (lib.rs:449) uses `is_dir()` which
  follows symlinks, keeps no visited-set, and has no depth cap (the `MAX_DEPTH=3` cap + `SKIP` list are only
  in `scan_subprojects`). Distinct from the existing read_to_string DoS (file size) and the cli FIFO (blocking
  read).
- **medium** — `run --dry-run <t>` reads arbitrary files the user can access and prints them (`read_file` +
  `print` both run under dry-run), and builds/decrypts `.env`/`.env-file` (keyring). A documented-safe preview
  of an untrusted repo becomes a read-anything-and-exfiltrate primitive. Distinct from the runtime agent's
  "dry-run prints the rendered command" (that is the preview TEXT echoing the target's own interpolated
  secret; this is active arbitrary file reads + `print`).
- **low** — `:generate jetbrains` injects the target name into the run-config `SCRIPT_TEXT` shell string
  unescaped (`xml_attr` covers XML, not shell), so a generated config runs a different command than its
  target; `vscode`/`zed` carry it as a `type:shell` arg (same class the editors agent found for the live
  extension — different sink/code path in `cmd_generate.rs`).
- **low** — `run :lint` (and the pre-commit/pre-push hooks) follows a `.run` symlink out of the repo and
  rewrites the target in place (non-atomic, through the link). Confined to files that parse as runfiles, so it
  reformats rather than arbitrarily corrupts.

## Checked and clean (with why)

- **`:list`/`:list --json`/`--names`.** JSON is hand-escaped by `list::quote` (control chars → `\u00XX`,
  verified), so no JSON-structure injection and the VS Code consumer is safe; the human path's raw-ESC issue
  is the cli agent's existing low finding. No evaluation (experiment 2).
- **`run <t> --help`.** `target_help::inputs` uses `inputs::of_chain`, a static tree walk — no evaluation, no
  keyring; only parses the target and its `_shared.run` chain (experiment 2). It is correctly placed before
  the prepare gate and before any run.
- **`:complete` (zsh/fish/PowerShell).** The bash `compgen -W` RCE is the cli agent's existing high; I checked
  the other three scripts: zsh adds candidates with `compadd -- $out` (array elements are literal, not re-
  expanded), fish emits literal completion candidates via `complete -a`, PowerShell builds literal
  `CompletionResult`s — none re-evaluates target names through `eval`/command-substitution/`Invoke-Expression`.
  The binary's `:complete` only prints names (experiment 2). So the completion-name-injection class is bash-
  only, already recorded.
- **`Host::load`/`Host::check`/`--stdin-args` check.** Static (parse + `resolve` + `check` +
  `runfile_shell::check`); no Scope, no evaluation, no spawning, no keyring. The `--stdin-args` prompt reads
  from `inputs::of_chain` (static). `run --stdin-args <t>` then runs `<t>` — that is a run the user asked for,
  by design.
- **LSP (`run :lsp`).** `document::diagnostics` → `analysis::diagnose` (parser + `resolve` + `runfile_lang::check`
  + `runfile_shell::check`) and `props::check` (no Scope — line 278 signature takes no scope), all static. It
  reads `_shared.run` text and other open docs from disk (`text_of`, server.rs:223) but only parses them; no
  evaluation, no keyring, no spawning. Uses `discover_unscoped`, so it reads no `.only-in-directories` scope.
- **`.only-in-directories` / `AmbiguousGlobal` / machine-wide selection.** Scoping is not a security boundary
  (CLAUDE.md), and the logic matches whole path components (`covers_cwd`), refuses non-literal scopes
  (`DiscoverError::UnreadableScope`), and leaves non-parsing files alone — all exercised by the crate's own
  tests, which I read. `AmbiguousGlobal` correctly refuses two populated machine-wide dirs and canonicalizes
  to avoid self-clash. No issue beyond the scoped-read question the project map already notes.
- **Machine-wide dir in CI.** `main::discovery_home` → `None` in CI (`ci_detect::is_ci`), so the home walk is
  suppressed there; the local walk of the checked-out repo always runs (which is the point). Correct.
- **Subproject walk bounds.** `scan_subprojects` is depth-capped at `MAX_DEPTH=3`, skips
  `node_modules/target/dist/build/.git/vendor` and dotdirs, and does not re-collect `$HOME/runfiles` found by
  the upward walk. Fine. (The unbounded recursion is only inside `walk_runs` — finding 1.)

## Notes on existing findings

- **high — walk-up trusts the nearest `runfiles/` with no ownership check (SA, untrusted-repo).** Re-verified
  by experiment 1: reproduces exactly — an attacker `runfiles/` in a shared ancestor runs as the victim on
  `run <t>`, and is listed/parsed by `:list`/`:complete`/the VS Code catalog. Verdict: **keep high.** The
  precondition is realistic: on Windows `C:\runfiles\` is creatable by authenticated users under the default
  `C:\` ACL (the exact shape of git CVE-2022-24765's `C:\.git`), and the git advisory explicitly says "opening
  such a directory in an editor or IDE such as VS Code … will potentially run commands defined by that other
  user" (github.blog, retrieved 2026-10-03). On Linux/macOS the strongest vectors are world-writable
  `/tmp`→`/tmp/runfiles` and `/var/tmp`, and group-writable shared project roots (`/srv`, `/opt`, NFS); a
  random user's home is a weaker vector. The finding's location and fix (ownership check + `safe.directory`-
  style allowlist) are correct. One refinement: the finding's own fix also fixes the zero-click VS Code vector
  it cites, and my findings 1 and 4 share the same root (discovery registering whatever the walk reaches with
  no owner/file-type check), so an ownership check + a symlink/file-type guard belong together.
- **medium — unbounded `read_to_string` DoS (SA, untrusted-repo).** Accurate; the six sink locations check
  out. My finding 1 is a *separate* DoS (the directory walk exploding via symlinks, independent of any file's
  byte size) and my finding 4 is a *separate* integrity issue (write through a symlink); all three share the
  "classify by extension/is_dir only, never `symlink_metadata`/`is_file`" root cause in `walk_runs`, so a
  single file-type guard there plus a bounded reader addresses the DoS family.

## Open questions

- Windows-only, not reproducible here: libuv/`C:\` ACL behaviour behind the walk-up and the editors agent's
  `run.exe`-in-workspace finding. Worth a Windows host to confirm `C:\runfiles` creatability under the real
  default ACL.
- Whether a `.run` symlink to a *FIFO/device* (not just a regular file) reaches a blocking read in `:lint`'s
  `shape()` as it does in `:list` (cli agent's FIFO finding) — likely yes via the same `read_to_string`, but I
  did not separately reproduce it for `:lint`.

## FILES READ:
CLAUDE.md
README.md
security-audit-2026-10-03/00-project-map.md
security-audit-2026-10-03/recon.md
security-audit-2026-10-03/agents/lang-parser.md
security-audit-2026-10-03/agents/runtime.md
security-audit-2026-10-03/agents/secrets-crypto.md
security-audit-2026-10-03/findings/untrusted-repo.jsonl
security-audit-2026-10-03/findings/cli.jsonl
security-audit-2026-10-03/findings/editors.jsonl
security-audit-2026-10-03/findings/lang-eval.jsonl
security-audit-2026-10-03/findings/runtime.jsonl
security-audit-2026-10-03/findings/lang-parser.jsonl
security-audit-2026-10-03/findings/shell-checker.jsonl
crates/runfile-discovery/Cargo.toml
crates/runfile-discovery/examples/list.rs
crates/runfile-discovery/src/lib.rs
crates/runfile-discovery/src/tests.rs
crates/runfile-cli/src/main.rs
crates/runfile-cli/src/list.rs
crates/runfile-cli/src/target_help.rs
crates/runfile-cli/src/cmd_lint.rs
crates/runfile-cli/src/cmd_generate.rs
crates/runfile-cli/src/completions.rs
crates/runfile-lsp/src/server.rs
crates/runfile-lsp/src/document.rs
crates/runfile-runtime/src/dispatch.rs
crates/runfile-runtime/src/run.rs
crates/runfile-runtime/src/exec.rs
crates/runfile-runtime/src/props.rs
crates/runfile-lang/src/functions.rs
crates/runfile-lang/src/inputs.rs
crates/runfile-env/src/lib.rs
crates/runfile-state/src/keyring_keys.rs
crates/runfile-state/src/keyring_store.rs
