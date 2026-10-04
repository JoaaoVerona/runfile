# Recon: runfile

Generated: 2026-10-03T04:23:57Z
Root: /home/verona/Workspace/runfile

## Git

- Commit: `a65dba1220e3698f0d13a544a42a9d31323ec28f`
- Branch: `master`
- Remote: `none`
- Commits: 355
- Last commit: 2026-09-30 00:35:06 -0300 by João Vitor Verona Biazibetti
- Contributors: 1

Recent activity (last 15):
```
a65dba1 bump to 1.8.2
9ea475b faster glob
d014d47 bump to 1.8.1
f0486c8 fix absolute patterns in glob() function
64ad28d bump to 1.8.0
a8c748b fix exit() behavior
ff5d78a bump to 1.7.2
5696f14 fix bash detection on windows with wsl
b764d5b tweaks
d0c333b bump to 1.7.1
80042db .env and .env-file passthrough on nested run calls + only request keyring for env decryption once
4af7c9b fix test
1574bd9 bump to 1.7.0
2706d88 more lint checks
223b02c bump to 1.6.2
```
- `.gitignore` present

## Size and languages

Files (excluding vendored/build dirs): **41780**

Top extensions by file count:

| Ext | Files | Lines |
|---|---|---|
| .json | 6871 | 13321 |
| .timestamp | 6760 | 6760 |
| .d | 6744 | 89160 |
| .rmeta | 6105 | 4231126 |
| .rlib | 760 | 4009415 |
| .rs | 119 | 46222 |
| .run | 49 | 545 |
| .o | 40 | 16596 |
| .so | 38 | 258186 |
| .toml | 13 | 286 |
| .ast | 11 | 1676 |
| .js | 10 | 2709 |
| .map | 9 | 9 |
| .yml | 7 | 1264 |
| .ts | 7 | 2250 |
| .TAG | 7 | 28 |
| .txt | 6 | 972 |
| .md | 6 | 4215 |
| .yaml | 3 | 545 |
| .h | 3 | 634 |
| .svg | 2 | 34 |
| .scm | 2 | 109 |
| .gitignore | 2 | 26 |
| .gitattributes | 2 | 7 |
| .c | 2 | 19232 |

## Top-level layout

```
crates                                   150 files
editors                                  3165 files
npm                                      2 files
runfiles                                 26 files
security-audit-2026-10-03                1 files
target-linux                             42143 files
```

## Package manifests and lockfiles

These define the dependency graph. Lockfiles give resolved versions — audit those.

```
./Cargo.lock
./Cargo.toml
./crates/runfile-cli/Cargo.toml
./crates/runfile-crypto/Cargo.toml
./crates/runfile-discovery/Cargo.toml
./crates/runfile-env/Cargo.toml
./crates/runfile-lang/Cargo.toml
./crates/runfile-lsp/Cargo.toml
./crates/runfile-runtime/Cargo.toml
./crates/runfile-shell/Cargo.toml
./crates/runfile-state/Cargo.toml
./editors/tree-sitter/package.json
./editors/tree-sitter/pnpm-lock.yaml
./editors/vscode/package.json
./editors/vscode/pnpm-lock.yaml
./npm/package.json
```

## Monorepo indicators

- Cargo workspace

## Build and deployment config

Severity depends on how this runs in production. Read every one of these.

```
```

## CI/CD

```
./.github/workflows/audit.yml
./.github/workflows/ci.yml
./.github/workflows/release.yml
```

Check: pull_request_target with untrusted checkout, unpinned third-party actions,
script injection via ${{ github.event.* }}, token permissions, secret exposure to forks.

## Config and secret-shaped files

```
./.cargo/config.toml
```

Anything matching `.env`, `*.pem`, `*.key` that is tracked by git is a finding.

## Entry point candidates

Where untrusted data enters. Every attack path starts at one of these.

**Main / bootstrap files:**
```
./crates/runfile-cli/src/main.rs
./crates/runfile-lsp/src/server.rs
```

**Route declaration density** (files with the most route-like declarations):
```
 145  ./crates/runfile-cli/tests/cli.rs
 125  ./crates/runfile-discovery/src/tests.rs
  32  ./crates/runfile-lang/src/tests/io.rs
  30  ./crates/runfile-env/src/tests/build.rs
  20  ./crates/runfile-env/src/tests/path.rs
  18  ./crates/runfile-env/src/tests/called.rs
  17  ./crates/runfile-runtime/src/tests/keys.rs
  14  ./crates/runfile-runtime/src/tests/parallel.rs
  14  ./crates/runfile-env/src/tests/load.rs
  13  ./crates/runfile-runtime/src/tests/exit.rs
  12  ./crates/runfile-runtime/src/tests/inherited.rs
  11  ./crates/runfile-lsp/tests/protocol.rs
  11  ./crates/runfile-lang/src/functions.rs
  11  ./crates/runfile-env/src/tests/encryption.rs
  11  ./crates/runfile-cli/tests/update.rs
   9  ./crates/runfile-runtime/src/tests/walk.rs
   8  ./crates/runfile-runtime/src/tests/interrupt.rs
   6  ./crates/runfile-cli/src/init.rs
   5  ./crates/runfile-state/src/tests/prepare_state.rs
   5  ./crates/runfile-runtime/src/shell.rs
   5  ./crates/runfile-cli/tests/lsp.rs
   4  ./crates/runfile-runtime/src/tests/checks.rs
   4  ./crates/runfile-lsp/src/server.rs
   3  ./crates/runfile-runtime/src/tests/names.rs
   3  ./crates/runfile-runtime/src/tests/exec.rs
   3  ./crates/runfile-lang/src/tests/eval.rs
   2  ./crates/runfile-state/src/prepare_state.rs
   2  ./crates/runfile-runtime/src/tests/mod.rs
   2  ./crates/runfile-runtime/src/tests/args.rs
   2  ./crates/runfile-lsp/src/analysis.rs
```

**Other entry points to check manually:** queue consumers, webhooks, cron jobs, gRPC
services, GraphQL resolvers, WebSocket handlers, CLI commands, file upload handlers.

## Test setup

Needed for writing regression tests.

```
./crates/runfile-env/src/tests
./crates/runfile-lang/src/tests
./crates/runfile-lang/tests
./crates/runfile-cli/tests
./crates/runfile-runtime/src/tests
./crates/runfile-lsp/tests
./crates/runfile-crypto/src/tests
./crates/runfile-shell/tests
./crates/runfile-state/src/tests
./editors/tree-sitter/test
```

Test files found: **4**

## Quick signals

Leads, not findings. Each needs the full evidence bar before it goes in the report.

- **TODO/FIXME/HACK/XXX**: 810 file(s)
- **Shell execution**: 14 file(s)
- **Dynamic code evaluation**: 18 file(s)
- **Raw SQL construction**: 1 file(s)
- **Weak hashing**: 1224 file(s)
- **Possible hardcoded secret**: 8 file(s)

---

This is a fingerprint, not an audit. Next: read the files above, build
`.security-audit/00-project-map.md`, then plan the agent fan-out.
