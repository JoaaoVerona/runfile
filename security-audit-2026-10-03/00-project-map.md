# Project map: runfile

Audit date: 2026-10-03   Commit: `a65dba1220e3698f0d13a544a42a9d31323ec28f` (master, v1.8.2, clean tree)

## Overview

Runfile is a cross-platform command runner (a Makefile alternative) shipped as a single Rust binary, `run`.
A project's tasks live in a `runfiles/` directory, one target per `.run` file, written in a small
line-oriented language: `$ <line>` hands a line to a shell, `exec <cmd> … end` pipes a body to a program,
`{{ … }}` interpolates values (and **self-quotes** them as shell words — the core injection-safety
promise), plus `let`, control flow, `parallel`, `retry`, a JSON block, and ~90 built-in functions
(file I/O, globbing, JSON, regex, temp files, `decrypt`, `confirm`, …). `run` discovers `runfiles/`
directories (walking **up** from the cwd for the nearest one, **down** up to 3 levels for subprojects,
plus a machine-wide `~/.runfiles` / `~/runfiles` / `~/Runfiles`), parses them, and executes targets.

It is not a server. There is no network listener, no database, no multi-tenant boundary. The security
model is that of a developer tool like `make`, `npm run`, `just` or `git`: **running a target executes
code the repository author wrote, by design.** What is *not* by design, and is therefore what this audit
is mostly about:

1. **Processing an untrusted repository without the user asking to execute it** — opening it in an
   editor (language server, VS Code extension, tree-sitter grammar), pressing Tab (shell completion runs
   `run :complete`), `run :list`, `run <target> --help`, `run --dry-run <target>` (documented as "changes
   nothing… a reasonable thing to want before setting a project up"), `run :lint`, `run :generate`.
   Any command execution, file write, keyring access or network access reachable from these is a bug.
2. **Data flowing into a trusted runfile** — values from `ARG.x`/`ARGS` (often supplied by CI from
   PR titles, branch names, tags), `ENV.X`, file names from `glob()`, captured command output,
   `json_query` results, `read_file` contents — reaching a shell, a JSON document or a path. The
   self-quoting interpolation must hold for every value, every shell (`bash`, `sh`, `dash`, `zsh`,
   `busybox`, `brush`, Git Bash on Windows) and every context (`$` lines, `exec` shell bodies, `exec`
   command words, `json` blocks, `run` arguments). Non-shell `exec` bodies are deliberately *not* quoted.
3. **Local multi-user / shared-directory boundaries** — discovery walking up into directories other
   users can write (`C:\runfiles` on Windows, `/tmp/runfiles`), temp files for decrypted secrets,
   state files, the keyring, installer temp dirs.
4. **Secrets** — `.env` files with `encrypted:` values (AES-256-GCM, `runfile-crypto`), private keys in
   the OS credential store (Secret Service / Keychain / Windows Credential Manager via `keyring`) or in
   `RUNFILE_PRIVATE_KEYS` (CI). Leaks via `.logging` traces, `--dry-run` output, error messages, child
   process environments, temp files, and crypto misuse.
5. **Supply chain** — the binary's own release pipeline (self-hosted Gitea + a GitHub mirror),
   `curl | sh` / `irm | iex` installers, `run :update`, the composite GitHub Action consumed by other
   repositories (`JoaaoVerona/runfile/.github/actions/setup@v1`), the npm package `@runfile/cli`,
   the VS Code extension (`joaaoverona.runfile`), and Cargo/pnpm dependencies.
6. **Data integrity** — `run` edits user files: `:lint` rewrites `.run` files in place, `:generate`
   rewrites `.vscode/tasks.json` / `.zed/tasks.json` / JetBrains run configs, `:completions
   install|uninstall` edits `~/.bashrc` / `~/.zshrc` / PowerShell profile / fish dir, `:env` rewrites
   `.env` files (encrypt/decrypt in place), `:update` replaces the binary, `write_file()` in targets,
   `prepare` writes `state.json`.

## Components

| Path | Language | Purpose | Shipped as | Untrusted input reaches it via |
|---|---|---|---|---|
| `crates/runfile-cli` | Rust | The `run` binary: flags, subcommands (`:list :init :lint :env :completions :generate :lsp :update :complete`), prepare gate, prompts, watch mode | `run` binary in 6 archives, npm, setup action | argv, cwd, every discovered `.run` file, `.env` files, env vars, network (`:update`) |
| `crates/runfile-lang` | Rust | Lexer, parser, evaluator, value + shell quoting, ~90 built-in functions (`functions.rs`: file I/O, glob, JSON, regex, temp files, decrypt), formatter, static checker, name resolver | linked into `run` | `.run` file text; values at run time |
| `crates/runfile-runtime` | Rust | Walker, properties, env building, process spawning (`exec.rs`), shell selection (`shell.rs`), dispatch (`dispatch.rs`, `Host`), parallel branches, Ctrl+C handling, terminal width (`unsafe` FFI) | linked into `run` | parsed targets; values |
| `crates/runfile-shell` | Rust | Static bash reader/checker for `$` lines and shell `exec` bodies (replaced ShellCheck) | linked into `run` | `.run` file text (also in LSP) |
| `crates/runfile-discovery` | Rust | Finds `runfiles/` dirs (walk up, walk down ≤3, machine-wide dir), builds the catalog, `.only-in-directories` scoping, `_shared.run` chain | linked into `run` | filesystem layout, file names |
| `crates/runfile-lsp` | Rust | Language server (hand-rolled JSON-RPC over stdio), diagnostics, completion, hover, go-to-definition, formatting. Served by `run :lsp` | linked into `run` | editor-opened documents (untrusted repos), LSP messages from the editor |
| `crates/runfile-env` | Rust | `.env` parsing, env-map building, precedence, PATH handling, decryption hook | linked into `run` | `.env` files |
| `crates/runfile-crypto` | Rust | AES-256-GCM encryption of env values, key handling, zeroize | linked into `run` | ciphertext in `.env` files, keys |
| `crates/runfile-state` | Rust | Prepare-gate `state.json`, OS credential store access (`keyring`, Secret Service) | linked into `run` | state dir, keyring |
| `editors/vscode` | TypeScript | VS Code extension: task provider (`run :list --json`), CodeLens "Run" buttons, tree view, hand-rolled LSP client for `run :lsp`, TextMate grammar | `.vsix` on the marketplace / releases | workspace folder contents, workspace settings |
| `editors/tree-sitter` | JS grammar + C scanner | tree-sitter grammar for Zed/Neovim/Helix; `src/scanner.c` is hand-written C, `src/parser.c` generated | consumed by editors | `.run` file text opened in an editor (C code — memory safety) |
| `.github/actions/setup` | composite action (bash) | Installs `run` in a consumer's CI job; optional `secret-keys` → `RUNFILE_PRIVATE_KEYS` in `$GITHUB_ENV` | consumed by third-party repos via `@v1` | consumer inputs |
| `.cicd/` + `.github/workflows/` | YAML | Gitea CI/release (self-hosted "fleet" runners, `git.joaoverona.com`) and GitHub mirror CI/release (npm trusted publishing, `v1` alias) | — | pushes, PRs (`pull_request` trigger on both), tags |
| `.cicd/release-assets/install.{sh,ps1}` | sh / PowerShell | `curl … \| sh` and `irm … \| iex` installers; also what `run :update` executes | release assets | network |
| `npm/` | JS | `@runfile/cli` launcher (`run.js` → `bin/<platform>-<arch>/run`) | npm | argv |
| `runfiles/` | `.run` | This repo's own targets (build, test, release, mirror, install, clean, wsl:*, ci:*) — run by the developer and by CI | — | — |
| `.githooks/` | sh | pre-commit → `run precommit`, pre-push → `run check` (activated by `run setup`) | — | — |

Excluded from line-by-line audit (recorded in `coverage.tsv`): `target/`, `target-linux/` (build output,
gitignored); `editors/vscode/out/` (tsc output of `editors/vscode/src`, gitignored — audited via
source); `editors/tree-sitter/src/{parser.c,grammar.json,node-types.json}` (generated by `tree-sitter
generate` from `grammar.js`); `editors/tree-sitter/src/tree_sitter/*.h` (vendored tree-sitter
headers); lockfiles (audited by the dependency agent, not line by line); `crates/runfile-lang/tests/golden/`
and `editors/tree-sitter/test/corpus/` (fixtures); docs and media.

## Entry points

| Component | Type | Entry | Handler | Who triggers it | Executes runfile code? |
|---|---|---|---|---|---|
| cli | argv | `run <target> [args]` | `main.rs` → `Host::run` (runtime `dispatch.rs`) | user, CI, git hooks | **yes, by design** |
| cli | argv | `run --dry-run <target>` | `main.rs` → `Host` with `dry_run` | user previewing | must not (documented "changes nothing") — verify captures, `exec` in value position, `_shared.run` lets, `.env-file` decryption |
| cli | argv | `run <target> --help` | `target_help.rs` | user | must not |
| cli | argv | `run :list [--names\|--json]` | `list.rs` | user, VS Code extension (`runfile.catalogCommand`), completion scripts | must not |
| cli | argv (hidden) | `run :complete <cword> <words…>` | `completions.rs` | **shell Tab key** in any directory | must not |
| cli | argv | `run :lint [paths] [--check\|--stdout\|--include-global]` | `cmd_lint.rs` | user, pre-commit | must not; **writes** `.run` files |
| cli | argv | `run :generate zed\|jetbrains\|vscode` | `cmd_generate.rs` | user | must not; **writes** editor config |
| cli | argv | `run :completions install\|uninstall\|output <shell>` | `completions.rs` | user | **edits shell profiles** |
| cli | argv | `run :env …` (encrypt/decrypt/keys/inject…) | `cmd_env/*` | user, CI | handles secrets; **writes** `.env` files |
| cli | argv + network | `run :update [version] [--channel] [--force]` | `cmd_update.rs` | user | downloads + runs the release's installer |
| cli | argv | `run :init` | `init.rs` | user | writes `runfiles/` example |
| cli/lsp | stdio JSON-RPC | `run :lsp` | `runfile-lsp/src/server.rs`, `rpc.rs` | editor on file open | must not; reads `_shared.run` chain, discovery |
| runtime | header probe | `Host::header_props` (`.watch` probe) | `dispatch.rs` | every non-dry run | evaluates declaration region |
| runtime | `--stdin-args` | `Host::check` + prompts | `stdin_args.rs`, `prompt.rs` | user | before running |
| vscode ext | activation `onStartupFinished` | `extension.ts` | VS Code startup, any workspace | runs `runfile.catalogCommand` (default `run :list --json`, a **shell command string from settings**) and `runfile.lspPath :lsp` (default `run`) |
| vscode ext | CodeLens / tree view / tasks | `codeLens.ts`, `extension.ts` | user click | runs `run --stdin-args --dir <dir> <target>` |
| tree-sitter | parser | `scanner.c` external scanner | Zed/Neovim/Helix on file open | C code over untrusted text |
| setup action | composite | `action.yml` inputs `version`, `from-source`, `secret-keys` | consumer workflows | downloads archive, writes `$GITHUB_PATH` / `$GITHUB_ENV` |
| installers | script | `install.sh` (`RUNFILE_VERSION`, `RUNFILE_CHANNEL`, `RUNFILE_INSTALL_DIR`), `install.ps1` | user via `curl \| sh`, `irm \| iex`; `run :update` | download + install |
| CI | Gitea Actions | `.cicd/ci.yml` (`push` master, `pull_request`, dispatch), `.cicd/release.yml` (tags `v*.*.*`), `.cicd/audit.yml` | pushes/PRs on git.joaoverona.com | self-hosted fleet runners |
| CI | GitHub Actions | `.github/workflows/ci.yml` (`push` github, `pull_request`), `release.yml` (`push` github + dispatch; npm publish w/ OIDC), `audit.yml` | mirror pushes, PRs on GitHub | GitHub-hosted runners |

## Trust boundaries

- **Repository content → developer machine.** Running a target = executing the repo's code (accepted,
  like `make`). Everything else on the entry-point table above that says "must not" is a boundary.
- **Data → shell.** Interpolated values must never be parsed as shell syntax (`value.rs` quoting,
  `exec.rs` script assembly, Windows command-line quoting `exec::windows_quoted` for Git Bash).
- **Discovery reach.** Walk-up stops at the *nearest* `runfiles/`; nothing checks (to be verified)
  that it is owned by the current user. On Windows any authenticated user can create `C:\runfiles\`;
  on Unix anyone can create `/tmp/runfiles/`. The machine-wide dir is the user's own `$HOME` and is not
  read in CI (`main::discovery_home` → `None` when `ci_detect` says CI).
- **Editor ↔ workspace.** VS Code Workspace Trust: the extension declares **no**
  `capabilities.untrustedWorkspaces` and activates `onStartupFinished`; two settings name things to
  execute (`runfile.catalogCommand` — a shell command string, `runfile.lspPath` — an executable) with no
  `scope` restriction, so a workspace `.vscode/settings.json` may be able to set them. Windows
  `child_process` resolution of a bare `run` with `cwd` = workspace may pick up `<workspace>\run.exe` /
  `run.bat`.
- **CI.** Both forges run CI on `pull_request`. Gitea jobs run on a self-hosted fleet (including the
  fleet's single Windows runner); whether outside contributors can open PRs there, and whether fleet
  runners are ephemeral, is not visible from the repo. GitHub release job publishes to npm via OIDC
  trusted publishing and moves the `v1` tag consumers pin.
- **Downstream consumers.** The setup action and installers are executed in other people's CI/machines.

## AuthN / AuthZ

None in the web sense. The only access-control-like mechanisms:
- The **prepare gate** (`setup` target must run first; fingerprinted; `RUNFILE_SKIP_PREPARE`; skipped in
  CI) — a convenience, not a security boundary.
- **`confirm()`** prompts, `-y`, and CI auto-yes.
- **`.only-in-directories`** — scoping of machine-wide targets (not a security boundary).
- **OS credential store** for private keys; `RUNFILE_PRIVATE_KEYS` env var in CI.

## Data

- **Secrets**: `.env` files may contain `encrypted:` values (AES-256-GCM via `runfile-crypto`);
  private keys (64-char hex) in the OS keyring (service/account names in `runfile-state`) or
  `RUNFILE_PRIVATE_KEYS`. Decrypted values land in process env, in child process envs, possibly in
  `temp_file()` outputs (`Scope.temps` registry, cleaned by `Host::cleanup_temps`, including on Ctrl+C).
- **State**: `state.json` (prepare fingerprints) in the platform state dir (`RUNFILE_CONFIG_DIR`
  override); not written in CI.
- **Logging**: `.logging` traces commands to stderr via `printf … >&2` injected into the script;
  `--dry-run` prints commands to stdout; errors name commands. Whether these show *rendered*
  (secret-bearing) values is to be verified.
- **Egress**: only `run :update` / installers (curl to git.joaoverona.com or github.com) — plus
  whatever targets themselves do.

## Build and deployment

- Rust workspace, toolchain pinned in `rust-toolchain.toml`; `+crt-static` on Windows MSVC.
- Release: tag `vX.Y.Z` on master (via `run release`) → Gitea `.cicd/release.yml` builds 6 triples on
  the fleet, uploads archives + `.vsix` + installers to a Gitea release. `run mirror` fast-forwards the
  `github` branch → GitHub `release.yml` rebuilds the version Cargo.toml names, publishes a GitHub
  release, moves the `v1` alias, publishes `@runfile/cli` to npm (trusted publishing/provenance).
- Installers resolve `latest` via the `/releases/latest` redirect and download
  `/releases/download/<tag>/<asset>`; **no checksum or signature verification is visible**
  (to be confirmed by the supply-chain agent).
- Remotes: `gitea ssh://git@git.joaoverona.com:2222/joaaoverona/runfile.git`, `github git@github.com:JoaaoVerona/runfile.git`.

## Test setup

- `run test` → `cargo test --workspace --all-features --locked {{ ARGS }}`; filter with
  `run test -- -- <filter>` (`--no-fail-fast` goes before the first `--`). **Never call `cargo`
  directly** (project rule): use `run <target>`.
- `run setup` must have been run (prepare gate); `RUNFILE_SKIP_PREPARE=1` bypasses it but must NOT be
  exported when running the CLI test suite.
- Unit tests live in `crates/*/src/tests/` (and `src/tests.rs`); integration tests in
  `crates/*/tests/`. CLI behaviour: `crates/runfile-cli/tests/cli.rs` drives the compiled binary with
  `HOME`/`USERPROFILE`/`XDG_*`/`APPDATA`/`RUNFILE_CONFIG_DIR` pointed at a temp dir and CI variables
  stripped (fixtures in that file). LSP: `crates/runfile-lsp/tests/protocol.rs`,
  `crates/runfile-cli/tests/lsp.rs`. Runtime: `crates/runfile-runtime/src/tests/*` (e.g. `keys.rs`
  counts keyring loads with a fake pool). Extension: `run vscode:test`.
- CI runs the full suite on every push and PR on both forges, so a failing regression test turns
  both red.
- Build output is per-OS: `target-linux/` (a stale `target/debug/run` is misleading).

## Open questions

1. ~~Can outside users open PRs that run `.cicd/ci.yml` on the self-hosted fleet?~~ **Answered by the
   owner (2026-10-03): no — only the owner opens PRs on Gitea.** Fork-PR attack paths against the
   Gitea fleet are therefore theoretical; rate them accordingly. The **GitHub mirror is public**, and
   its `.github/workflows/ci.yml` runs on `pull_request` from forks (GitHub-hosted runners, fork PRs
   get a read-only token and no secrets by GitHub's default) — check nothing there weakens that.
2. Is the VS Code extension enabled in Restricted Mode? (No `untrustedWorkspaces` declared — VS Code's
   default for undeclared extensions must be confirmed with a live lookup.)
3. Does `--dry-run` execute `$` captures / `exec` captures (needed to compute values), and does it
   decrypt `.env-file` values or touch the keyring?
4. Does `:complete` / `:list` / `--help` / the LSP evaluate anything in `_shared.run` (e.g. `write_file`
   in a shared `let`), or touch the keyring?
5. Does discovery check ownership of a walked-up `runfiles/`?
6. Does `RUNFILE_PRIVATE_KEYS` get passed to every child process a target spawns?
7. Are `.logging` traces, `--dry-run` previews and failure messages rendered *after* interpolation
   (so `{{ decrypt(…) }}` / `{{ ENV.TOKEN }}` would print secrets)?
