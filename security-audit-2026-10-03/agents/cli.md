# Agent `cli` — narrative

Scope: `crates/runfile-cli/{Cargo.toml, ci_detect.rs, cmd_generate.rs, cmd_lint.rs, completions.rs,
help.rs, init.rs, list.rs, main.rs, prepare.rs, prompt.rs, stdin_args.rs, target_help.rs, watch.rs}`
and `tests/cli.rs`. (`cmd_env/` and `cmd_update.rs` belong to other agents.)

Commit a65dba1 (v1.8.2). Experiments used the prebuilt `target-linux/debug/run`, always under an
isolated `env -i` with `HOME`/XDG/`RUNFILE_CONFIG_DIR`/`TMPDIR` pointed at SCRATCH, DBUS pointed at a
dead socket, `RUNFILE_SKIP_PREPARE=1`, a 4 GB vlimit and a 30 s KILL timeout. No repository target was
ever run. Fixtures live only under SCRATCH. No network traffic.

## What I examined and how

Read every file in scope in full (main, ci_detect, list, completions, cmd_generate, cmd_lint,
prepare, prompt, stdin_args, init, watch, target_help, help, Cargo.toml). Read the consumer side of
the `:list --json` contract (`editors/vscode/src/catalog.ts` and `pure.ts`) and grepped
`extension.ts`/`codeLens.ts` for how the generated args reach a shell. Read
`crates/runfile-state/src/prepare_state.rs` and the relevant slices of
`crates/runfile-discovery/src/lib.rs` to settle the prepare key-derivation question. Grepped
`tests/cli.rs` for the confirm/CI fixtures to tell a bug from a decision.

Traced the untrusted-repo sources (target names = file names, descriptions = leading comments,
captured `:complete`/`:list` output) to their sinks (the generated completion scripts, the editor
task files, the terminal, in-place file writes), and the trusted-data sources (`--stdin-args`
answers, env) to theirs.

## Experiments and results

1. **Re-verified the existing HIGH (bash `compgen -W` RCE).** Built an untrusted repo with
   `runfiles/$(touch EVID_D).run` and ``runfiles/`touch EVID_B`.run`` (slash-free substitutions, since
   `/` is illegal in a file name). `run --dir repo :complete 1 run ""` printed both names verbatim on
   stdout. I then generated the real script with `run :completions output bash`, sourced it into a
   `bash --norc` I controlled (a `run` shim on PATH forwarded to the debug binary with `--dir repo`;
   cwd set to an empty `evidence/` dir), set `COMP_WORDS=(run "")`, `COMP_CWORD=1`, and called `_run`
   exactly as bash's completion machinery does. **Result: both `EVID_B` and `EVID_D` were created** —
   `compgen -W "$out"` expanded the command substitutions. One Tab press = RCE. The real `~/.bashrc`
   was never touched. The finding holds exactly as written.

2. **CI auto-confirm (NEW).** A target `if confirm("proceed?") print("PROCEEDED") else print("declined")`.
   With no CI var and stdin not a tty: `[runfile] error: cancelled`, exit 1. With `CI=1`: `PROCEEDED`,
   exit 0. With `GITHUB_ACTIONS=true`: `PROCEEDED`, exit 0. So a bare, extremely common env var
   silently disables the only safety prompt. `tests/cli.rs:409` pins this behaviour as intended, so I
   rated it a low footgun about the *breadth* of the trigger, not an unintended bypass.

3. **JetBrains SCRIPT_TEXT injection (NEW).** Repo with `runfiles/'build; touch INJECTED.run'` and
   `runfiles/'x$(id).run'`. `run :generate jetbrains` wrote
   `<option name="SCRIPT_TEXT" value="run --stdin-args build; touch INJECTED" />` and
   `value="run --stdin-args x$(id)"` — `xml_attr` escaped the XML layer but left `;`/`$()` intact.
   JetBrains ShConfigurationType runs SCRIPT_TEXT via a shell (confirmed against JetBrains docs), so
   running the generated config executes the injected commands. VS Code `--stdout` for the same repo
   emitted the names as JSON `args` array elements (VS Code quotes string args; Zed spawns without a
   shell), so those two generators are not the same raw-string hazard.

4. **`:lint <dir>` symlink walk (NEW, hardening).** A dir with `a/up -> ..` plus one real `x.run`:
   `run :lint loopdir` reported "41 files checked" — the walk followed the loop until the kernel's
   ELOOP (~40) stopped it, so ~40x redundant processing of the same file, not an unbounded hang. The
   walk uses `is_dir()` (follows symlinks), has no visited set, no skip list and no root confinement,
   so a symlink out of the named directory would have its `.run` files rewritten in place.

## NEW findings (5)

- **low / injection** — JetBrains `SCRIPT_TEXT` command injection from target name (cmd_generate.rs
  367-389, xml_attr 329-342). Double-gated (`:generate` + run-config click) but injects code beyond
  the target body.
- **low / other (fail-open)** — `confirm()` auto-approved and machine-wide dir disabled whenever any of
  10 CI vars (notably bare `CI`) is set, including on dev machines with a TTY (ci_detect.rs 7-31,
  main.rs:254). Fix: also require non-tty stdin for the auto-yes.
- **low / data-loss** — non-atomic `std::fs::write` (truncate-then-write) in `:lint` (219), `:generate`
  (217), `:completions` (225) destroys the original on ENOSPC/EIO. Blast radius includes `~/.zshrc`,
  the PowerShell profile, `~/.bashrc` and user `.run` source files.
- **hardening / info-disclosure** — `--stdin-args` prompts for secret env inputs (ENV.TOKEN etc.) echo
  in cleartext; no no-echo path (prompt.rs 40-56, stdin_args.rs 42-53).
- **hardening / path-traversal** — `:lint` directory walk follows symlinks out of the named root and
  into node_modules/.git with no cap (cmd_lint.rs 110-137); mirror runfile-discovery's walk policy.

## Checked and found clean (with reasons)

- **`--stdin-args` answer smuggling.** Answers are appended as `--{name}={v}` (args) or `--{name}`
  (flags). The `=` form binds the whole remainder as the value, so an answer of `--foo` or `--x=y`
  becomes the *value* of `name`, not a new flag — no flag smuggling. (stdin_args.rs:30-32,37-40.)
- **`:list --json` hand escaping / VS Code consumer.** `list::quote` (list.rs:77-93) escapes `"` `\`
  and every char < 0x20 as `\uXXXX`; the only sink is `catalog.ts`/`pure.ts`, which `JSON.parse` the
  whole document and read string fields — so a crafted name/description/path cannot inject extra JSON
  fields or alter another entry's `dir`/`name`. U+2028/U+2029 are valid in JSON strings and in
  `JSON.parse` (they only break JS *source*, not `JSON.parse`), so they are harmless here. Non-UTF-8
  Linux file names reach `to_string_lossy` (list.rs:66) and become U+FFFD — lossy, but not an
  injection. The terminal (human) `:list`/`--help` path is the *existing* low escape-injection
  finding; the JSON path is safe.
- **prepare cross-project confusion.** `PrepareState` keys on `path_key` (prepare_state.rs:92), which
  `canonicalize`s the gate path; at enforce/record time the setup file exists (it was just read for
  the digest), so the key is the absolute canonical path and two projects cannot collide. The lossy
  fallback only triggers when the file moved between record and lookup, which fails to match (re-prompt)
  rather than falsely matching. The CI exemption (no read, no write) is correct and pinned by tests.
- **main.rs flag parsing.** Runner flags are only consumed before the target name; `--dir` with no
  value falls back to cwd; `-v`/`-V`/`--version` and `:`-prefixed unknown commands are handled. No
  data reaches a shell here.
- **watch.rs.** Entered only for a target the user ran (declares `.watch`); patterns compile through
  globset with errors surfaced; the notify watcher handles its own symlink/loop behaviour. The
  `.watch` probe is evaluated once before the loop (not per iteration), which is a design choice, not
  a security bug.
- **init.rs.** Refuses to overwrite an existing `runfiles/hello.run`; writes a fixed template.

## Notes on existing findings

- **HIGH (compgen RCE):** confirmed by reproduction above; no change.
- **LOW (terminal escape injection in `:list`/`--help`):** confirmed. Minor widening worth noting when
  the fix lands: `target_help::render` also prints `target.path.display()` ("Defined in …") and the
  `reads`/usage line raw, and `list::unknown` echoes the bad name raw in the error — all the same C0/C1
  class the finding's fix should cover (its fix text already mentions the unknown/error message). Not a
  separate finding.
- **LOW (FIFO hang):** confirmed; my `:lint` symlink finding is adjacent (both are
  `read`/`read_dir`-without-a-file-type-check), and the untrusted-repo agent's
  "Unbounded read_to_string on discovered .run files" overlaps the discovery side. Left as distinct
  because `cmd_lint::walk` is a separate code path with its own missing confinement; merge may fold
  them.

## Open questions

- Does any real downstream commit `.idea/runConfigurations/` generated by `:generate`? If so the
  JetBrains injection could reach teammates; if not it is purely local.
- Whether a genuine CI runner ever has a TTY (would change the suggested CI-auto-yes fix's blast
  radius). Standard hosted runners do not.

## FILES READ:
CLAUDE.md
security-audit-2026-10-03/00-project-map.md
security-audit-2026-10-03/findings/cli.jsonl
crates/runfile-cli/Cargo.toml
crates/runfile-cli/src/main.rs
crates/runfile-cli/src/ci_detect.rs
crates/runfile-cli/src/list.rs
crates/runfile-cli/src/completions.rs
crates/runfile-cli/src/cmd_generate.rs
crates/runfile-cli/src/cmd_lint.rs
crates/runfile-cli/src/prepare.rs
crates/runfile-cli/src/prompt.rs
crates/runfile-cli/src/stdin_args.rs
crates/runfile-cli/src/init.rs
crates/runfile-cli/src/watch.rs
crates/runfile-cli/src/target_help.rs
crates/runfile-cli/src/help.rs
crates/runfile-cli/tests/cli.rs
editors/vscode/src/catalog.ts
editors/vscode/src/pure.ts
editors/vscode/src/extension.ts
crates/runfile-state/src/prepare_state.rs
crates/runfile-discovery/src/lib.rs
