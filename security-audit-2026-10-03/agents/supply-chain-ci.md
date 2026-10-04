# CI, build, release and installers — narrative (agent: supply-chain-ci, run by supply-chain)

Audit date 2026-10-03, commit a65dba1 (v1.8.2). Nothing in scope looked actively malicious.

## What I examined and how

Read in full: both forges' workflows (.cicd/{ci,release,audit}.yml, .github/workflows/{ci,release,audit}.yml),
the composite setup action, both installers, `crates/runfile-cli/src/cmd_update.rs` and its end-to-end test,
the npm launcher and manifest, .cargo/config.toml, rust-toolchain.toml, rustfmt.toml, .editorconfig,
.gitattributes, .gitignore, both git hooks, the workspace Cargo.toml, and every file under runfiles/ (plus
the editors' runfiles that the workflows call: vscode:setup/package, tree-sitter:setup/test). I traced each
untrusted input to its sink: PR content and refs (fork PRs on the public GitHub mirror), `workflow_dispatch`
inputs, tag names, the Cargo.toml version string, the `/releases/latest` redirect a server answers with,
consumer inputs to the setup action (`version`, `secret-keys`), and third-party actions and packages that
execute inside release jobs. Because every Gitea job here consumes the owner's
`shared-actions/rust-toolchain-and-cache@v1`, I also read that action from the local clone
(~/Workspace/shared-actions @ 0793d4f, read-only) and the fleet documentation
(~/Workspace/gitea-easy-runners/README.md, read-only) to learn how the self-hosted runners behave.

## Lookups and experiments (all 2026-10-03)

Action pins, resolved live through api.github.com (no runfile release assets were fetched):

| Action (as used) | Resolves to | Release immutable? | OSV (GitHub Actions) | Latest |
|---|---|---|---|---|
| actions/checkout@v6.0.2 | de0fac2e4500dabe0009e67214ff5f5447ce83dd | no | none | v7.0.1 |
| actions-rust-lang/setup-rust-toolchain@v1.16.1 (third-party; also nested in shared-actions) | 46268bd060767258de96ed93c1251119784f2ab6 | **no** | none | v2.0.0 |
| pnpm/setup@v2.1.0 (third-party, pnpm org, repo created 2026-05-11) | annotated tag ce3b1e5f… -> commit 703c52620218391530e48b9e8870d5c0082e1b9b | **yes** | none | v3.0.0 |
| actions/upload-artifact@v4.6.2 | ea165f8d65b6e75b540449e92b4886f43607fa02 | no | none | v7.0.1 |
| actions/download-artifact@v5.0.0 | 634f93cb2916e3fdff6788551b99b062d0335ce0 | no | none | v8.0.1 |
| actions/setup-node@v6.4.0 | 48b55a011bda9f5d6aeb4c2d9c7362e8dae4041e | no | none | v7.0.0 |
| Swatinem/rust-cache (nested in setup-rust-toolchain) | c19371144df3bb44fab255c43d04cbc2ab54d1c4 (v2.9.1), already SHA-pinned | — | none | — |
| shared-actions/*@v1 on git.joaoverona.com | owner's own; not resolved (no traffic to the project host) | — | — | — |

Other lookups: api.github.com/repos/JoaaoVerona/runfile — owner type User, created 2026-03-25, default
branch `github`, public. GitHub docs (managing Actions settings): a new personal-account repository's
GITHUB_TOKEN defaults to read-only `contents`/`packages`. GitHub hardening guide: only a full SHA is
immutable; a tag can be moved. OSV GHSA-mrrh-fwg8-r2c3 (tj-actions, CVE-2025-30066) as precedent.
docs.gitea.com/runner/2/cache and the fleet README: the cache server scopes entries by repository with
per-job tokens. curl docs: default redirect protocols are HTTP, HTTPS, FTP, FTPS since 7.65.2. Cargo
config reference: parent-directory and $CARGO_HOME config files are merged into every build.

Experiments (fixtures under SCRATCH only; no project host contacted; every process stopped):

1. **Setup action `version` traversal.** Replicated the action's URL construction with
   `VERSION=../../../../attacker/repo/releases/download/v1` against a local `python3 -m http.server`
   (127.0.0.1:18765). curl 8.5.0 requested `GET /attacker/repo/releases/download/v1/runfile-cli-x86_64-unknown-linux-musl.tar.xz`
   and saved the attacker file as the archive (three `..` reach `/JoaaoVerona/attacker/…`, four leave the
   owner). Server killed; `ps` confirms none left.
2. **install.sh ignores RUNFILE_VERSION.** Ran the repository's install.sh with a fake `curl` on PATH
   (same technique as tests/update.rs), RUNFILE_VERSION=v1.7.0 and RUNFILE_INSTALL_DIR in SCRATCH: it asked
   for `/releases/latest`, resolved the fake v9.9.9 and installed it. Its `mktemp -d` dir was removed by
   its own trap.

## New findings (supply-chain-ci.jsonl)

- **medium / likely** — Gitea release binaries (default channel) for macOS and Windows are built on
  persistent, shared host-mode runners whose $HOME (rustup toolchains, ~/.cargo) survives between jobs of
  every project on the fleet. A compromised package in any job can plant a linker/rustc that backdoors the
  next release. Evidence comes from the owner's own shared action ("$HOME persists between jobs on these
  runners and is shared by every project that lands on one"). Cheapest fix: a fresh CARGO_HOME/RUSTUP_HOME
  per release job. Better: ephemeral runners, plus a reproducible-build hash comparison against the GitHub
  mirror's independent build of the same tag.
- **medium / confirmed** — actions-rust-lang/setup-rust-toolchain is pinned by a mutable tag
  (v1.16.1, `immutable: false`). It runs in every job that builds release binaries on both forges (on
  Gitea through shared-actions). Pin it by SHA (46268bd…).
- **low / confirmed** — setup action `version` is used in the URL unvalidated, so dot-segments fetch an
  archive from any github.com repository (experiment 1).
- **low / confirmed** — version pins do not pin. Pinning the action to `@v1.0.0` or a SHA still installs
  the newest release, despite what README.md:90-91 says, and install.sh ignores the RUNFILE_VERSION that
  README.md:61-62 documents (experiment 2).
- **hardening** — the from-source `cargo build` in the setup action lacks `--locked`, so a stale lock is
  re-resolved in release jobs and the later `--locked` build accepts it.
- **hardening** — `secret-keys` goes to $GITHUB_ENV with a fixed heredoc delimiter, without validation or
  per-key `::add-mask::`.
- **hardening** — curl calls do not restrict protocols (`--proto '=https'`), so an HTTPS-to-HTTP
  redirect would be followed.

## Checked and found clean

- **No `pull_request_target` or `workflow_run`** anywhere, so no workflow runs fork code with secrets or a
  write token.
- **No expression injection.** Every `${{ }}` that reaches a shell (`inputs.tag`, `inputs.version`,
  `inputs.secret-keys`, `needs.resolve.outputs.tag`, matrix values) goes through `env:` or `with:`. The
  only `${{ }}` inside `run:` text are matrix/step values the workflow defines itself (grep of both trees).
- **Fork PRs on the GitHub mirror** run ci.yml with GitHub's forced read-only token and no secrets: ci.yml
  references no `secrets.*` and has no `permissions:` that could widen it. First-time contributors need
  approval by default.
- **Cache poisoning across trust levels.** GitHub scopes caches written by `pull_request` runs to
  `refs/pull/N/merge`, so release jobs (push to `github`, the default branch) cannot restore them. On
  Gitea the cache server scopes by repository and only the owner opens PRs. The pnpm store and rust-cache
  restores in the release `extension` jobs therefore only see caches written by the same repository's
  trusted pushes. (A compromised dependency inside this repo's own CI could still write a poisoned pnpm
  store entry that the Gitea release `extension` job restores. That is one more path under the existing
  job-token finding, not a separate one.)
- **npm publishing.** `id-token: write` exists only in the `npm` job, which is reached only by a push to
  `github` or by `workflow_dispatch` (write access), runs only GitHub-owned actions (checkout,
  download-artifact, setup-node), and publishes a package with no dependencies or lifecycle scripts.
- **The `v1` alias** is moved only by `major-tag`. That job holds `contents: write`, runs only
  actions/checkout and git, moves the alias forward only (sort -V comparison against every `vN.*.*` tag),
  refuses prerelease tags, and runs only after `release` succeeded. No job that runs third-party code
  holds a write token on GitHub, so a compromised action cannot move `v1` directly.
- **GitHub token scope.** `build` and `extension` have no `permissions:`, but the repository is
  personal-account-owned (created after GitHub made read-only the default), so they get read-only. The
  existing Gitea token finding already notes this.
- **`run :update` (cmd_update.rs).** A user-supplied version passes `is_valid_version_tag`
  (`[A-Za-z0-9._-]`, ≤64, no leading `-`). So does the tag read from the server's redirect
  (`tag_of_page`, lines 368-371), and the script is then fetched from the fixed channel host whatever host
  the redirect named. On Unix the tag is passed as an argv element (`sh -s -- <tag>`), never as shell
  text. On Windows the allow-list keeps `'` out of the single-quoted PowerShell string. A failed download
  is an error, not an empty script piped to sh. The `--version` post-check runs the new binary, which the
  existing verification finding already covers. No temp files are written in Rust.
- **install.sh**: `set -eu`, `mktemp -d` with an EXIT trap, only the expected `runfile-cli-<target>/run`
  is moved out of the extracted tree, GNU/bsd tar refuse `..` and absolute members by default, no profile
  edits (it only prints PATH advice), and RUNFILE_INSTALL_DIR is the caller's choice.
  **install.ps1**: GUID temp dir under the user temp, removed in `finally`; the User PATH is *appended*
  (existing entries keep precedence); no ExecutionPolicy change; `iex` is only the documented one-liner.
- **npm/run.js**: `spawnSync(binPath, argv, {stdio: 'inherit'})` with no shell; the binary is selected from
  a fixed platform-arch map; no postinstall. When the child dies from a signal it exits 1 instead of
  re-raising the signal (cosmetic).
- **release.run / mirror.run.** release.run refuses any branch but master, computes the next version
  from numbers, and interpolates only `{{ next }}`/`{{ tag }}`. The quoted `"bump to {{ next }}"` is a
  numeric string, so the lang-eval double-quote issue cannot bite. `cargo update --workspace` touches only
  workspace members. `git push --follow-tags` pushes to the branch's upstream, and mirror.run pushes to
  `branch.github.remote` after a fast-forward-only `git fetch . <upstream>:refs/heads/github`. Every input
  is the developer's local git config; neither script reads commit messages or remote branch names into a
  shell.
- **Git hooks**: pre-commit runs `run precommit` (rustfmt + git add of staged .rs paths), pre-push runs
  `run check`. Neither downloads anything. They apply only after `run setup` sets core.hooksPath.
  `rustfmt {{ files }}`/`git add {{ files }}` lack a `--`, so a staged file whose name starts with `-` would
  be read as an option. That is cosmetic in the owner's own repository.
- **.cargo/config.toml** sets only `+crt-static` rustflags for the two MSVC targets: no linker override,
  no `[net]`, `[source]` or `[registries]`. rust-toolchain.toml pins 1.94.1 (staleness is a dependencies
  finding). rustfmt.toml, .editorconfig and .gitattributes are formatting only. .gitignore does not
  ignore `.env`, but no `.env` file is tracked or present at the root.
- **Gitea release.yml** `workflow_dispatch` `tag` input (owner-only) goes through `env:` and `with:`, and
  `prerelease` keeps rc tags out of `latest`.

## Notes on existing findings

- **#0 (vsce unpinned)**: holds; dependencies.jsonl #0 duplicates it. Merge.
- **#1 (no download verification)**: holds; cited lines re-checked against HEAD. Two related new
  findings widen it rather than duplicate it: the setup action's `version` traversal, and the fact that
  pinning the action does not pin the binary (so even consumers who pin by SHA run each new release
  unverified). The Gitea `update` and `action` CI jobs also download and execute the published
  installers and binaries, so a tampered release asset would execute inside Gitea CI with the job token.
  That is a self-reinforcing loop with #2.
- **#2 (Gitea job token)**: holds. Add that every Gitea job, release legs included, also executes the
  mutable `actions-rust-lang/setup-rust-toolchain@v1.16.1` through shared-actions (new medium). Add also
  that on the macOS and Windows host-mode runners a dependency compromise does not need the token at all
  to persist into release builds (new medium). Its sentence "The fleet's runner VMs (gitea-ubuntu-26,
  gitea-macos-tah) run on the owner's workstation" could not be verified from the repository or the
  fleet docs.

## Open questions

1. Are the `macos-26` and `windows-2025` runners registered `--mode=persistent` or `--mode=ephemeral`?
   The persistent-runner finding assumes persistent (supported by shared-actions' comments and
   .cicd/ci.yml's "the runner's history"). If they are ephemeral with disk revert, it does not apply.
2. Does the GitHub mirror have a ruleset protecting `v*` tags and the `github` branch (only the owner and
   the release workflow may move `v1`)? Not visible without admin API access.
3. Is npm trusted publishing for `@runfile/cli` bound to `.github/workflows/release.yml` and a protected
   environment? Are token-based publishes disallowed on the package? Not visible from the registry.
4. Gitea instance settings: default Actions token mode (Permissive or Restricted), and whether release
   attachments are served directly from object storage (and if so over https). This affects existing
   finding #2 and the curl hardening note.
5. Does the published `shared-actions` `v1` tag point at the 0793d4f commit read locally?

## Other observations (not security)

- cmd_update.rs:397-411 says install.ps1 renames the running binary to a GUID-suffixed `.old-<guid>`
  name and that several aside-files can exist. install.ps1:62-66 actually uses the fixed name
  `run.exe.old`, so a second update while an earlier `.old` is still locked fails the rename silently
  and then fails `Move-Item` onto the running `run.exe`.

## FILES READ:
.cargo/config.toml
.cicd/audit.yml
.cicd/ci.yml
.cicd/release-assets/install.ps1
.cicd/release-assets/install.sh
.cicd/release.yml
.editorconfig
.gitattributes
.githooks/pre-commit
.githooks/pre-push
.github/actions/setup/action.yml
.github/workflows/audit.yml
.github/workflows/ci.yml
.github/workflows/release.yml
.gitignore
Cargo.toml
CLAUDE.md
README.md
crates/runfile-cli/src/cmd_update.rs
crates/runfile-cli/tests/update.rs
npm/package.json
npm/run.js
rust-toolchain.toml
rustfmt.toml
runfiles/_bump-vscode-extension.run
runfiles/_shared.run
runfiles/audit.run
runfiles/build-release.run
runfiles/build.run
runfiles/check.run
runfiles/ci/_shared.run
runfiles/ci/build.run
runfiles/ci/check.run
runfiles/ci/test.run
runfiles/clean.run
runfiles/cli.run
runfiles/install.run
runfiles/lint.run
runfiles/miri-setup.run
runfiles/miri.run
runfiles/mirror.run
runfiles/precommit.run
runfiles/release.run
runfiles/setup.run
runfiles/test.run
runfiles/wsl/_shared.run
runfiles/wsl/build-release.run
runfiles/wsl/build.run
runfiles/wsl/lint.run
runfiles/wsl/test.run
editors/vscode/package.json
editors/vscode/runfiles/_shared.run
editors/vscode/runfiles/compile.run
editors/vscode/runfiles/install.run
editors/vscode/runfiles/package.run
editors/vscode/runfiles/setup.run
editors/vscode/runfiles/test.run
editors/vscode/runfiles/uninstall.run
editors/vscode/runfiles/watch.run
editors/tree-sitter/package.json
editors/tree-sitter/pnpm-workspace.yaml
editors/tree-sitter/runfiles/_shared.run
editors/tree-sitter/runfiles/generate.run
editors/tree-sitter/runfiles/setup.run
editors/tree-sitter/runfiles/test.run
security-audit-2026-10-03/00-project-map.md
security-audit-2026-10-03/findings/supply-chain-ci.jsonl
security-audit-2026-10-03/findings/dependencies.jsonl
(outside repo, read-only) ~/Workspace/shared-actions/rust-toolchain-and-cache/action.yml
(outside repo, read-only) ~/Workspace/gitea-easy-runners/README.md (sections on modes, host mode, cache server, ephemeral mode)
