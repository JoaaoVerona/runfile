# Dependencies — narrative (agent: dependencies, run by supply-chain)

Audit date 2026-10-03, commit a65dba1 (v1.8.2). Scope: Cargo.lock, every crates/*/Cargo.toml and the
workspace Cargo.toml, editors/vscode/{package.json,pnpm-lock.yaml}, editors/tree-sitter/{package.json,
pnpm-lock.yaml,pnpm-workspace.yaml}, plus the toolchain pins (rust-toolchain.toml, Node in CI).

## What I examined and how

- **Cargo.lock** (223 packages: 9 workspace members, 214 from crates.io). Ran the installed
  `cargo audit` 0.22.2 (`cargo audit --json`, RustSec DB fetched today, head ef6173cb, last-updated
  2026-10-03T10:14+02:00, 1,290 advisories). Cross-checked with an OSV `querybatch` of all 214 crates.
- **Reachability**: `cargo tree --frozen` (offline, builds nothing) for each of the six release triples,
  three ways: `-p runfile-cli -e normal,no-proc-macro` (linked into `run`), `-e normal,build` (adds
  proc-macros and build deps), and `--workspace -e normal,build,dev` (adds test-only). Anything in none of
  the six is "not compiled for any shipped target". Version-aware reverse-dependency walk over
  Cargo.lock for the two advisory hits.
- **npm**: `dep_audit.py` earlier today parsed **0 entries** from both pnpm lockfiles (they are pnpm 12
  multi-document YAML with a `packageManagerDependencies` document first), so the npm side had never
  been audited. I ran `pnpm audit --json` (pnpm 12.4.1 was already installed; no install step; repo left
  unchanged) in both directories, OSV `querybatch` for all 22 npm entries, and compared every lockfile
  `integrity` with the registry's `dist.integrity` (37/37 match). The Node runtime that
  `devEngines.runtime` pins in editors/vscode/pnpm-lock.yaml (node@runtime:24.21.0, 12 platform
  variants) was checked against nodejs.org's and unofficial-builds.nodejs.org's SHASUMS256.txt: all 12
  sha256 values match their publisher.
- **Registry metadata** for install scripts, signatures and provenance (registry.npmjs.org), crates.io
  owners for unfamiliar names, crates.io for fixed versions.
- **Toolchains**: static.rust-lang.org stable manifest, the Rust blog's 2026 security posts, nodejs.org
  `dist/index.json` and nodejs/Release `schedule.json`.

## Lookups and experiments (all 2026-10-03)

| Lookup | Result |
|---|---|
| `cargo audit --json` | 0 vulnerabilities; 2 `unsound` warnings: anyhow 1.0.102 RUSTSEC-2026-0190 (patched >=1.0.103), event-listener 5.4.1 RUSTSEC-2026-0221 (patched >=5.4.2, unaffected <5.1.0); no yanked, no unmaintained |
| https://api.osv.dev/v1/vulns/RUSTSEC-2026-0190 | exists; published 2026-06-25; anyhow introduced 0.0.0-0, fixed 1.0.103; function `anyhow::Error::downcast_mut`; informational unsound |
| https://api.osv.dev/v1/vulns/RUSTSEC-2026-0221 | exists; published 2026-07-13; event-listener introduced 5.1.0, fixed 5.4.2; informational unsound |
| https://rustsec.org/advisories/RUSTSEC-2026-0190.html, ...-0221.html | HTTP 200 both |
| https://crates.io/api/v1/crates/{anyhow,event-listener} | anyhow 1.0.103 (2026-06-25), 1.0.104 (2026-07-18, max stable); event-listener 5.4.2 (2026-07-27, max stable); none yanked |
| OSV querybatch, 214 crates + 22 npm | only the two RUSTSEC IDs above |
| `pnpm audit --json` editors/vscode | 0 advisories (22 deps) |
| `pnpm audit --json` editors/tree-sitter | 0 advisories (16 deps) |
| registry.npmjs.org integrity vs lockfiles | 37/37 MATCH |
| nodejs.org SHASUMS256 v24.21.0 vs lockfile runtime hashes | 10/10 official MATCH; 2 musl variants MATCH unofficial-builds' SHASUMS256 |
| https://static.rust-lang.org/dist/channel-rust-stable.toml | stable = 1.99.0 (2026-09-28); repo pins 1.94.1 |
| https://blog.rust-lang.org/ (2026 posts) | Cargo CVE-2026-33056 (fixed 1.94.1), CVE-2026-5223 and CVE-2026-5222 (fixed 1.96.0, third-party registries only); arrayref supply-chain attack 2026-08-20; no std advisory |
| https://blog.rust-lang.org/2026/08/20/supply-chain-attack-on-arrayref/ | malicious: append-only-vec 0.1.9, arrayref 0.3.10, internment 0.8.7, proc-macro1, proc-macro-en, aovine, arone, aronenao, tinymember — **none in Cargo.lock** |
| https://nodejs.org/dist/index.json | latest 24.x = 24.21.0 (2026-09-07, LTS Krypton) — what the lock pins |
| https://raw.githubusercontent.com/nodejs/Release/main/schedule.json | Node 24: maintenance 2026-10-20, EOL 2028-04-30; Node 20 EOL 2026-04-30 |
| crates.io owners | zmij: dtolnay; endi: zeenix; leb128fmt: bluk (wasm-tools dependency, not compiled for shipped targets) |

## Advisory table (confirmed)

| Package | Installed | Ecosystem | Advisory | Severity | Fixed in | Reachable? | Type |
|---|---|---|---|---|---|---|---|
| event-listener | 5.4.1 | crates.io | RUSTSEC-2026-0221 | informational (unsound) | 5.4.2 | Linked into the Linux binaries (runfile-state -> secret-service 5.1.0 -> zbus 5.16.0 -> async-*); the unsound path needs a `!Send` tag via `Event::with_tag`, which no dependent calls (existing finding) | runtime, Linux only |
| anyhow | 1.0.102 | crates.io | RUSTSEC-2026-0190 | informational (unsound) | 1.0.103 | No: only via tempfile 3.27.0 -> getrandom 0.4.2 -> wasip3 -> wit-bindgen* -> wasm-metadata/wit-*; wasip3 is cfg(wasm32-wasi-p3) and anyhow is used by a proc-macro there. Not compiled for any of the six release triples. | not compiled for shipped targets |

No npm advisories. Advisory data retrieved from OSV.dev, RustSec (cargo-audit DB) and the npm audit
endpoint on 2026-10-03; current-version data from crates.io and registry.npmjs.org the same day.
Advisories published after this date are not reflected.

## Checked and found clean

- **Lockfiles honoured.** Every cargo invocation in runfiles/ and runfiles/ci uses `--locked`
  (build, build-release, ci:build, ci:check, ci:test, test, check, lint, cli, wsl:*). Both pnpm setups
  use `pnpm install --frozen-lockfile`, and pnpm also refuses lockfile writes when CI is set. Two
  exceptions, filed/listed elsewhere: the setup action's from-source `cargo build` (no `--locked`,
  hardening finding in supply-chain-ci.jsonl) and `npx @vscode/vsce` (existing medium finding).
  `cargo install cargo-audit --locked` in both audit workflows installs whatever cargo-audit is newest
  (with its own lockfile): unpinned version, low risk, noted only.
- **Sources.** All 214 registry packages come from `registry+https://github.com/rust-lang/crates.io-index`
  and all 214 have a `checksum`; no git, path-outside-workspace or HTTP sources; no `[patch]`,
  `[replace]` or `[source]` sections in any Cargo.toml or .cargo/config.toml; no ~/.cargo/config.toml on
  this machine. npm: all tarballs from registry.npmjs.org with sha512 integrity; the Node runtime from
  nodejs.org (and unofficial-builds.nodejs.org for two musl variants) with sha256 integrity.
- **Direct Rust dependencies are exact (`=`) pins** in [workspace.dependencies]; crates take them with
  `.workspace = true`.
- **Typosquats / known-malicious.** Every crate name compared against common crates (only `inotify`
  ~ `notify`, both legitimate); none of the nine crates from the 2026-08-20 arrayref incident is
  present; unfamiliar names checked for owners. npm names are all first-party (`@types/*`,
  `typescript`, `vscode-*` from Microsoft, `pnpm`/`@pnpm/exe.*`, `tree-sitter-cli`).
- **Install scripts.** In the locked npm trees: `tree-sitter-cli` (`install: node install.js`,
  explicitly allowed; existing low finding) and `pnpm@12.4.1` itself (`preinstall`/`postinstall:
  node install.js`), which is pnpm's self-install of the `packageManager` version, integrity-pinned in
  the lockfile and carrying an npm provenance attestation. No other package in either tree has an
  install script. editors/vscode has no `allowBuilds`, so pnpm 12 runs no dependency scripts there.
  `@vscode/vsce-sign`'s postinstall lives in the unpinned vsce tree (existing finding).
- **Shipped npm package** (`npm/package.json`): no dependencies, no lifecycle scripts, `files: ["bin"]`.
- **VS Code extension**: no runtime dependencies at all (all five are devDependencies; `.vscodeignore`
  excludes node_modules; vsce runs with `--no-dependencies`).
- **Node version** used in CI: the vscode lockfile pins Node 24.21.0 (newest 24.x, LTS until
  2028-04-30); the npm publish job uses setup-node `node-version: 24`. Not EOL. `@types/node` is on the
  ^20 line (Node 20 went EOL 2026-04-30) but it is types only, so nothing executes from it.
- **Dependency confusion**: no private registries or scopes; `@runfile/cli` is the project's own
  published name on the public registry.
- **Vendored code**: `editors/tree-sitter/src/tree_sitter/*.h` are generated/vendored headers from
  tree-sitter (excluded from this scope by the project map; produced by tree-sitter-cli 0.25.6).

## Notes on existing findings

- **dependencies.jsonl #0 (vsce unpinned, medium)** is a duplicate of **supply-chain-ci.jsonl #0**; same
  root cause, same fix. Recommend merging into one finding.
- **#1 (tree-sitter-cli binary, low)**: holds. Confirmed the lockfile sha512 for tree-sitter-cli
  0.25.6 matches the registry, so the pin covers the wrapper only, as stated.
- **#2 (event-listener, hardening)** and **#3 (anyhow, hardening)**: both advisory IDs exist with the
  stated ranges (OSV and rustsec.org fetched today); cargo-audit agrees; no correction needed. One
  nuance for #3: tempfile is also a normal dependency of `uds_windows` (zbus -> uds_windows, Windows-only
  in zbus), but zbus is only pulled in on Linux here, so that edge is never compiled either; the
  conclusion stands.
- The earlier `dependencies.md`/`dependencies.json` report "Manifests parsed: editors/*/pnpm-lock.yaml:
  0 entries". That was a parser gap, not a clean result; it is now covered by pnpm audit, OSV and the
  integrity comparison above.
- CLAUDE.md lists `json5` and `md-5` under "Dropped dependencies", but both are still direct
  dependencies (runfile-state uses json5 1.3.1, runfile-lang uses md-5 0.11.0) and are linked into
  `run`. No advisories; documentation drift only.

## New findings

- **low** — pinned Rust toolchain 1.94.1 is five stable releases behind 1.99.0 and unsupported; std is
  statically linked into every shipped binary. No exploitable advisory today (the two Cargo CVEs since
  are third-party-registry only).

## Currency (no advisories; for planning)

Majors behind among linked crates: the RustCrypto stack (aes-gcm 0.10.3 vs 0.11.1, aes 0.8 vs 0.9,
cipher 0.4 vs 0.5, digest 0.10 vs 0.11, generic-array 0.14 vs 1.4), base64 0.22 vs 0.23, dirs 6 vs 7,
secret-service 5.1.0 vs 5.2.0. aes-gcm 0.10 is still the maintained line for its API; upgrading the
RustCrypto stack is an API migration in runfile-crypto, not a patch bump. npm devDependencies:
typescript 5.9.3 vs 7.0.2, @types/node 20 vs 26, tree-sitter-cli 0.25.6 vs 0.27.0, pnpm 12.4.1 vs
12.9.1.

## Open questions

- Should rust-toolchain.toml be watched by an updater (Renovate's rust-toolchain manager) so the next
  std fix is not missed?
- The two musl variants of the pinned Node runtime come from unofficial-builds.nodejs.org (community
  build infrastructure). Only relevant to musl hosts; CI uses glibc runners.

## Full dependency table

### npm (lockfiles; all dev/build tooling — nothing here ships in a product artifact)

| Package | Version | Latest | Ecosystem | Advisories | Lockfile | Class |
|---|---|---|---|---|---|---|
| @types/node | 20.19.43 | 26.6.4 | npm | none | editors/vscode | dev (types) |
| @types/vscode | 1.125.0 | 1.140.0 | npm | none | editors/vscode | dev (types) |
| typescript | 5.9.3 | 7.0.2 | npm | none | editors/vscode | dev (compiler) |
| undici-types | 6.21.0 | 8.11.2 | npm | none | editors/vscode | dev (types, via @types/node) |
| vscode-oniguruma | 2.0.1 | 2.0.1 | npm | none | editors/vscode | dev (grammar tests) |
| vscode-textmate | 9.3.2 | 9.3.2 | npm | none | editors/vscode | dev (grammar tests) |
| node (runtime:) | 24.21.0 | 24.21.0 (24.x) | nodejs.org binary | none (Node 24 LTS, EOL 2028-04-30) | editors/vscode | build runtime |
| tree-sitter-cli | 0.25.6 | 0.27.0 | npm | none; install script downloads unverified binary (existing finding) | editors/tree-sitter | dev (grammar generator) |
| pnpm | 12.4.1 | 12.9.1 | npm | none; pre/postinstall self-install, provenance attested | both (packageManagerDependencies) | build tool |
| @pnpm/exe.{android-arm64, android-x64, darwin-arm64, darwin-x64, freebsd-x64, linux-arm64, linux-arm64-musl, linux-ppc64, linux-riscv64, linux-s390x, linux-x64, linux-x64-musl, win32-arm64, win32-x64} | 12.4.1 | 12.9.1 | npm | none | both | build tool (optional platform binaries) |
| @vscode/vsce (+ ~136 transitive) | resolved at run time (4.0.0 today) | 4.0.0 | npm | none today (OSV, earlier run); **not in any lockfile** (existing medium finding) | none | release packaging tool |

### crates.io (Cargo.lock, 214 registry packages)

Class: **runtime (linked: os…)** = compiled into the `run` binary for that OS family (normal, non-proc-macro
dependency edges for the release triples); **build-time** = proc-macro or build-dependency, runs on the
build host only; **dev/test only** = reached only through `[dev-dependencies]`; **not compiled for any
shipped target** = only behind cfgs none of the six triples satisfies (wasm, wasi, redox, hermit, uefi,
gnu/i686 Windows). Counts: 152 runtime, 18 build-time, 2 dev/test only, 42 not compiled. "Latest" is
from crates.io via dep_audit.py, retrieved 2026-10-03.

| Crate | Version | Latest (2026-10-03) | Advisories | Class |
|---|---|---|---|---|
| `aead` | 0.5.2 | 0.6.1 | none | runtime (linked: linux/mac/win) |
| `aes` | 0.8.4 | 0.9.3 | none | runtime (linked: linux/mac/win) |
| `aes-gcm` | 0.10.3 | 0.11.1 | none | runtime (linked: linux/mac/win) |
| `aho-corasick` | 1.1.4 | 1.1.5 | none | runtime (linked: linux/mac/win) |
| `anstream` | 1.0.0 | 1.0.0 | none | runtime (linked: linux/mac/win) |
| `anstyle` | 1.0.14 | 1.0.14 | none | runtime (linked: linux/mac/win) |
| `anstyle-parse` | 1.0.0 | 1.0.0 | none | runtime (linked: linux/mac/win) |
| `anstyle-query` | 1.1.5 | 1.1.5 | none | runtime (linked: linux/mac/win) |
| `anstyle-wincon` | 3.0.11 | 3.0.11 | none | runtime (linked: win) |
| `anyhow` | 1.0.102 | 1.0.104 | RUSTSEC-2026-0190 (unsound, fixed 1.0.103) | not compiled for any shipped target |
| `apple-native-keyring-store` | 1.0.0 | 1.0.2 | none | runtime (linked: mac) |
| `async-broadcast` | 0.7.2 | 0.7.2 | none | runtime (linked: linux) |
| `async-channel` | 2.5.0 | 2.5.0 | none | runtime (linked: linux) |
| `async-executor` | 1.14.0 | 1.14.0 | none | runtime (linked: linux) |
| `async-io` | 2.6.0 | 2.6.0 | none | runtime (linked: linux) |
| `async-lock` | 3.4.2 | 3.4.2 | none | runtime (linked: linux) |
| `async-process` | 2.5.0 | 2.5.0 | none | runtime (linked: linux) |
| `async-recursion` | 1.1.1 | 1.1.1 | none | build-time (proc-macro/build-dep: linux) |
| `async-signal` | 0.2.14 | 0.2.14 | none | runtime (linked: linux) |
| `async-task` | 4.7.1 | 4.7.1 | none | runtime (linked: linux) |
| `async-trait` | 0.1.89 | 0.1.92 | none | build-time (proc-macro/build-dep: linux) |
| `atomic-waker` | 1.1.2 | 1.1.2 | none | runtime (linked: linux) |
| `autocfg` | 1.5.0 | 1.5.1 | none | build-time (proc-macro/build-dep: linux) |
| `base64` | 0.22.1 | 0.23.1 | none | runtime (linked: linux/mac/win) |
| `bitflags` | 2.11.1 | 2.13.2 | none | runtime (linked: linux/mac/win) |
| `block-buffer` | 0.10.4 | 0.12.1 | none | runtime (linked: linux) |
| `block-buffer` | 0.12.0 | 0.12.1 | none | runtime (linked: linux/mac/win) |
| `block-padding` | 0.3.3 | 0.4.2 | none | runtime (linked: linux) |
| `blocking` | 1.6.2 | 1.7.0 | none | runtime (linked: linux) |
| `bstr` | 1.12.1 | 1.13.1 | none | runtime (linked: linux/mac/win) |
| `bumpalo` | 3.20.2 | 3.20.3 | none | not compiled for any shipped target |
| `byteorder` | 1.5.0 | 1.5.0 | none | runtime (linked: win) |
| `cbc` | 0.1.2 | 0.2.1 | none | runtime (linked: linux) |
| `cfg-if` | 1.0.4 | 1.0.5 | none | runtime (linked: linux/mac/win) |
| `cipher` | 0.4.4 | 0.5.2 | none | runtime (linked: linux/mac/win) |
| `clap` | 4.6.1 | 4.6.7 | none | runtime (linked: linux/mac/win) |
| `clap_builder` | 4.6.0 | 4.6.7 | none | runtime (linked: linux/mac/win) |
| `clap_derive` | 4.6.1 | 4.6.7 | none | build-time (proc-macro/build-dep: linux/mac/win) |
| `clap_lex` | 1.1.0 | 1.1.1 | none | runtime (linked: linux/mac/win) |
| `colorchoice` | 1.0.5 | 1.0.5 | none | runtime (linked: linux/mac/win) |
| `concurrent-queue` | 2.5.0 | 2.5.0 | none | runtime (linked: linux) |
| `const-oid` | 0.10.2 | 0.10.2 | none | runtime (linked: linux/mac/win) |
| `core-foundation` | 0.10.1 | 0.10.1 | none | runtime (linked: mac) |
| `core-foundation-sys` | 0.8.7 | 0.8.7 | none | runtime (linked: mac) |
| `cpufeatures` | 0.2.17 | 0.3.1 | none | runtime (linked: linux/mac/win) |
| `cpufeatures` | 0.3.0 | 0.3.1 | none | runtime (linked: linux/mac/win) |
| `crossbeam-utils` | 0.8.21 | 0.8.23 | none | runtime (linked: linux) |
| `crypto-common` | 0.1.7 | 0.2.2 | none | runtime (linked: linux/mac/win) |
| `crypto-common` | 0.2.1 | 0.2.2 | none | runtime (linked: linux/mac/win) |
| `ctr` | 0.9.2 | 0.10.1 | none | runtime (linked: linux/mac/win) |
| `digest` | 0.10.7 | 0.11.3 | none | runtime (linked: linux) |
| `digest` | 0.11.3 | 0.11.3 | none | runtime (linked: linux/mac/win) |
| `dirs` | 6.0.0 | 7.0.0 | none | runtime (linked: linux/mac/win) |
| `dirs-sys` | 0.5.0 | 0.5.0 | none | runtime (linked: linux/mac/win) |
| `endi` | 1.1.1 | 1.1.1 | none | runtime (linked: linux) |
| `enumflags2` | 0.7.12 | 0.7.12 | none | runtime (linked: linux) |
| `enumflags2_derive` | 0.7.12 | 0.7.12 | none | build-time (proc-macro/build-dep: linux) |
| `equivalent` | 1.0.2 | 1.0.2 | none | runtime (linked: linux/mac/win) |
| `errno` | 0.3.14 | 0.3.14 | none | runtime (linked: linux) |
| `event-listener` | 5.4.1 | 5.4.2 | RUSTSEC-2026-0221 (unsound, fixed 5.4.2) | runtime (linked: linux) |
| `event-listener-strategy` | 0.5.4 | 0.5.4 | none | runtime (linked: linux) |
| `fastrand` | 2.4.1 | 2.5.0 | none | runtime (linked: linux) |
| `foldhash` | 0.1.5 | 0.2.0 | none | not compiled for any shipped target |
| `fsevent-sys` | 4.1.0 | 5.2.0 | none | runtime (linked: mac) |
| `futures-core` | 0.3.32 | 0.3.34 | none | runtime (linked: linux) |
| `futures-io` | 0.3.32 | 0.3.34 | none | runtime (linked: linux) |
| `futures-lite` | 2.6.1 | 2.6.1 | none | runtime (linked: linux) |
| `futures-macro` | 0.3.32 | 0.3.34 | none | build-time (proc-macro/build-dep: linux) |
| `futures-task` | 0.3.32 | 0.3.34 | none | runtime (linked: linux) |
| `futures-util` | 0.3.32 | 0.3.34 | none | runtime (linked: linux) |
| `generic-array` | 0.14.7 | 1.4.5 | none | runtime (linked: linux/mac/win) |
| `getrandom` | 0.2.17 | 0.4.3 | none | runtime (linked: linux/mac/win) |
| `getrandom` | 0.4.2 | 0.4.3 | none | dev/test only (linux/mac/win) |
| `ghash` | 0.5.1 | 0.6.0 | none | runtime (linked: linux/mac/win) |
| `globset` | 0.4.18 | 0.4.20 | none | runtime (linked: linux/mac/win) |
| `hashbrown` | 0.15.5 | 0.17.1 | none | not compiled for any shipped target |
| `hashbrown` | 0.17.0 | 0.17.1 | none | runtime (linked: linux/mac/win) |
| `heck` | 0.5.0 | 0.5.0 | none | build-time (proc-macro/build-dep: linux/mac/win) |
| `hermit-abi` | 0.5.2 | 0.5.3 | none | not compiled for any shipped target |
| `hex` | 0.4.3 | 0.4.3 | none | runtime (linked: linux/mac/win) |
| `hkdf` | 0.12.4 | 0.13.0 | none | runtime (linked: linux) |
| `hmac` | 0.12.1 | 0.13.0 | none | runtime (linked: linux) |
| `hybrid-array` | 0.4.11 | 0.4.15 | none | runtime (linked: linux/mac/win) |
| `id-arena` | 2.3.0 | 2.3.0 | none | not compiled for any shipped target |
| `indexmap` | 2.14.0 | 2.14.2 | none | runtime (linked: linux/mac/win) |
| `inotify` | 0.11.1 | 0.11.5 | none | runtime (linked: linux) |
| `inotify-sys` | 0.1.5 | 0.1.8 | none | runtime (linked: linux) |
| `inout` | 0.1.4 | 0.2.2 | none | runtime (linked: linux/mac/win) |
| `is_terminal_polyfill` | 1.70.2 | 1.70.2 | none | runtime (linked: linux/mac/win) |
| `itoa` | 1.0.18 | 1.0.18 | none | runtime (linked: linux/mac/win) |
| `js-sys` | 0.3.98 | 0.3.106 | none | not compiled for any shipped target |
| `json5` | 1.3.1 | 1.3.1 | none | runtime (linked: linux/mac/win) |
| `keyring-core` | 1.0.0 | 1.0.0 | none | runtime (linked: linux/mac/win) |
| `kqueue` | 1.1.1 | 1.2.1 | none | not compiled for any shipped target |
| `kqueue-sys` | 1.1.0 | 1.1.2 | none | not compiled for any shipped target |
| `leb128fmt` | 0.1.0 | 0.1.0 | none | not compiled for any shipped target |
| `libc` | 0.2.186 | 0.2.190 | none | runtime (linked: linux/mac/win) |
| `libredox` | 0.1.16 | 0.1.25 | none | not compiled for any shipped target |
| `linux-keyutils` | 0.2.5 | 0.2.5 | none | runtime (linked: linux) |
| `linux-keyutils-keyring-store` | 1.0.0 | 1.0.0 | none | runtime (linked: linux) |
| `linux-raw-sys` | 0.12.1 | 0.12.1 | none | runtime (linked: linux) |
| `log` | 0.4.29 | 0.4.34 | none | runtime (linked: linux/mac/win) |
| `md-5` | 0.11.0 | 0.11.0 | none | runtime (linked: linux/mac/win) |
| `memchr` | 2.8.0 | 2.8.3 | none | runtime (linked: linux/mac/win) |
| `memoffset` | 0.9.1 | 0.9.1 | none | not compiled for any shipped target |
| `mio` | 1.2.0 | 1.2.3 | none | runtime (linked: linux) |
| `notify` | 8.2.0 | 8.2.0 | none | runtime (linked: linux/mac/win) |
| `notify-types` | 2.1.0 | 2.1.0 | none | runtime (linked: linux/mac/win) |
| `num` | 0.4.3 | 0.4.3 | none | runtime (linked: linux) |
| `num-bigint` | 0.4.6 | 0.5.1 | none | runtime (linked: linux) |
| `num-complex` | 0.4.6 | 0.4.6 | none | runtime (linked: linux) |
| `num-integer` | 0.1.46 | 0.1.47 | none | runtime (linked: linux) |
| `num-iter` | 0.1.45 | 0.1.46 | none | runtime (linked: linux) |
| `num-rational` | 0.4.2 | 0.4.2 | none | runtime (linked: linux) |
| `num-traits` | 0.2.19 | 0.2.19 | none | runtime (linked: linux) |
| `once_cell` | 1.21.4 | 1.21.4 | none | runtime (linked: linux) |
| `once_cell_polyfill` | 1.70.2 | 1.70.2 | none | runtime (linked: win) |
| `opaque-debug` | 0.3.1 | 0.4.0 | none | runtime (linked: linux/mac/win) |
| `option-ext` | 0.2.0 | 0.2.0 | none | runtime (linked: linux/mac/win) |
| `ordered-stream` | 0.2.0 | 0.2.0 | none | runtime (linked: linux) |
| `parking` | 2.2.1 | 2.2.1 | none | runtime (linked: linux) |
| `pin-project-lite` | 0.2.17 | 0.2.17 | none | runtime (linked: linux) |
| `piper` | 0.2.5 | 0.2.5 | none | runtime (linked: linux) |
| `polling` | 3.11.0 | 3.11.0 | none | runtime (linked: linux) |
| `polyval` | 0.6.2 | 0.7.3 | none | runtime (linked: linux/mac/win) |
| `prettyplease` | 0.2.37 | 0.3.0 | none | not compiled for any shipped target |
| `proc-macro-crate` | 3.5.0 | 3.5.0 | none | build-time (proc-macro/build-dep: linux) |
| `proc-macro2` | 1.0.106 | 1.0.107 | none | runtime (linked: linux) |
| `quote` | 1.0.45 | 1.0.47 | none | runtime (linked: linux) |
| `r-efi` | 6.0.0 | 7.1.0 | none | not compiled for any shipped target |
| `rand_core` | 0.6.4 | 0.10.1 | none | runtime (linked: linux/mac/win) |
| `redox_users` | 0.5.2 | 0.5.3 | none | not compiled for any shipped target |
| `regex` | 1.12.3 | 1.13.1 | none | runtime (linked: linux/mac/win) |
| `regex-automata` | 0.4.14 | 0.4.18 | none | runtime (linked: linux/mac/win) |
| `regex-syntax` | 0.8.10 | 0.8.11 | none | runtime (linked: linux/mac/win) |
| `rustix` | 1.1.4 | 1.1.5 | none | runtime (linked: linux) |
| `rustversion` | 1.0.22 | 1.0.23 | none | not compiled for any shipped target |
| `same-file` | 1.0.6 | 1.0.6 | none | runtime (linked: linux/mac/win) |
| `secret-service` | 5.1.0 | 5.2.0 | none | runtime (linked: linux) |
| `security-framework` | 3.7.0 | 3.7.0 | none | runtime (linked: mac) |
| `security-framework-sys` | 2.17.0 | 2.17.0 | none | runtime (linked: mac) |
| `semver` | 1.0.28 | 1.0.28 | none | not compiled for any shipped target |
| `serde` | 1.0.228 | 1.0.229 | none | runtime (linked: linux/mac/win) |
| `serde_core` | 1.0.228 | 1.0.229 | none | runtime (linked: linux/mac/win) |
| `serde_derive` | 1.0.228 | 1.0.229 | none | build-time (proc-macro/build-dep: linux/mac/win) |
| `serde_json` | 1.0.149 | 1.0.151 | none | runtime (linked: linux/mac/win) |
| `serde_repr` | 0.1.20 | 0.1.21 | none | build-time (proc-macro/build-dep: linux) |
| `sha2` | 0.10.9 | 0.11.0 | none | runtime (linked: linux) |
| `sha2` | 0.11.0 | 0.11.0 | none | runtime (linked: linux/mac/win) |
| `signal-hook-registry` | 1.4.8 | 1.4.8 | none | runtime (linked: linux) |
| `slab` | 0.4.12 | 0.4.12 | none | runtime (linked: linux) |
| `strsim` | 0.11.1 | 0.11.1 | none | runtime (linked: linux/mac/win) |
| `subtle` | 2.6.1 | 2.6.1 | none | runtime (linked: linux/mac/win) |
| `syn` | 2.0.117 | 3.0.6 | none | runtime (linked: linux) |
| `tempfile` | 3.27.0 | 3.27.0 | none | dev/test only (linux/mac/win) |
| `thiserror` | 2.0.18 | 2.0.21 | none | runtime (linked: linux/mac/win) |
| `thiserror-impl` | 2.0.18 | 2.0.21 | none | build-time (proc-macro/build-dep: linux/mac/win) |
| `toml_datetime` | 1.1.1+spec-1.1.0 | 1.1.1+spec-1.1.0 | none | build-time (proc-macro/build-dep: linux) |
| `toml_edit` | 0.25.12+spec-1.1.0 | 0.25.15+spec-1.1.0 | none | build-time (proc-macro/build-dep: linux) |
| `toml_parser` | 1.1.2+spec-1.1.0 | 1.1.3+spec-1.1.0 | none | build-time (proc-macro/build-dep: linux) |
| `tracing` | 0.1.44 | 0.1.44 | none | runtime (linked: linux) |
| `tracing-attributes` | 0.1.31 | 0.1.31 | none | build-time (proc-macro/build-dep: linux) |
| `tracing-core` | 0.1.36 | 0.1.36 | none | runtime (linked: linux) |
| `typenum` | 1.20.0 | 1.20.1 | none | runtime (linked: linux/mac/win) |
| `ucd-trie` | 0.1.7 | 0.1.7 | none | runtime (linked: linux/mac/win) |
| `uds_windows` | 1.2.1 | 1.2.1 | none | not compiled for any shipped target |
| `unicode-ident` | 1.0.24 | 1.0.26 | none | runtime (linked: linux) |
| `unicode-xid` | 0.2.6 | 0.2.6 | none | not compiled for any shipped target |
| `universal-hash` | 0.5.1 | 0.6.1 | none | runtime (linked: linux/mac/win) |
| `utf8parse` | 0.2.2 | 0.2.2 | none | runtime (linked: linux/mac/win) |
| `uuid` | 1.23.3 | 1.27.0 | none | runtime (linked: linux) |
| `version_check` | 0.9.5 | - | none | build-time (proc-macro/build-dep: linux/mac/win) |
| `walkdir` | 2.5.0 | 2.5.0 | none | runtime (linked: linux/mac/win) |
| `wasi` | 0.11.1+wasi-snapshot-preview1 | 0.14.7+wasi-0.2.4 | none | not compiled for any shipped target |
| `wasip2` | 1.0.3+wasi-0.2.9 | 2.0.1+wasi-0.2.12 | none | not compiled for any shipped target |
| `wasip3` | 0.4.0+wasi-0.3.0-rc-2026-01-06 | 0.9.0+wasi-0.3.0 | none | not compiled for any shipped target |
| `wasm-bindgen` | 0.2.121 | 0.2.129 | none | not compiled for any shipped target |
| `wasm-bindgen-macro` | 0.2.121 | 0.2.129 | none | not compiled for any shipped target |
| `wasm-bindgen-macro-support` | 0.2.121 | 0.2.129 | none | not compiled for any shipped target |
| `wasm-bindgen-shared` | 0.2.121 | 0.2.129 | none | not compiled for any shipped target |
| `wasm-encoder` | 0.244.0 | 0.261.0 | none | not compiled for any shipped target |
| `wasm-metadata` | 0.244.0 | 0.261.0 | none | not compiled for any shipped target |
| `wasmparser` | 0.244.0 | 0.261.0 | none | not compiled for any shipped target |
| `which` | 8.0.2 | 8.0.6 | none | runtime (linked: linux/mac/win) |
| `winapi-util` | 0.1.11 | 0.1.11 | none | runtime (linked: win) |
| `windows-link` | 0.2.1 | 0.100.0 | none | runtime (linked: win) |
| `windows-native-keyring-store` | 1.0.0 | 1.1.0 | none | runtime (linked: win) |
| `windows-sys` | 0.60.2 | 0.61.2 | none | runtime (linked: win) |
| `windows-sys` | 0.61.2 | 0.61.2 | none | runtime (linked: win) |
| `windows-targets` | 0.53.5 | 0.53.5 | none | runtime (linked: win) |
| `windows_aarch64_gnullvm` | 0.53.1 | 0.53.1 | none | not compiled for any shipped target |
| `windows_aarch64_msvc` | 0.53.1 | 0.53.1 | none | runtime (linked: win) |
| `windows_i686_gnu` | 0.53.1 | 0.53.1 | none | not compiled for any shipped target |
| `windows_i686_gnullvm` | 0.53.1 | 0.53.1 | none | not compiled for any shipped target |
| `windows_i686_msvc` | 0.53.1 | 0.53.1 | none | not compiled for any shipped target |
| `windows_x86_64_gnu` | 0.53.1 | 0.53.1 | none | not compiled for any shipped target |
| `windows_x86_64_gnullvm` | 0.53.1 | 0.53.1 | none | not compiled for any shipped target |
| `windows_x86_64_msvc` | 0.53.1 | 0.53.1 | none | runtime (linked: win) |
| `winnow` | 1.0.3 | 1.0.4 | none | runtime (linked: linux) |
| `wit-bindgen` | 0.51.0 | 0.62.0 | none | not compiled for any shipped target |
| `wit-bindgen` | 0.57.1 | 0.62.0 | none | not compiled for any shipped target |
| `wit-bindgen-core` | 0.51.0 | 0.62.0 | none | not compiled for any shipped target |
| `wit-bindgen-rust` | 0.51.0 | 0.62.0 | none | not compiled for any shipped target |
| `wit-bindgen-rust-macro` | 0.51.0 | 0.62.0 | none | not compiled for any shipped target |
| `wit-component` | 0.244.0 | 0.261.0 | none | not compiled for any shipped target |
| `wit-parser` | 0.244.0 | 0.261.0 | none | not compiled for any shipped target |
| `zbus` | 5.16.0 | 5.19.0 | none | runtime (linked: linux) |
| `zbus_macros` | 5.16.0 | 5.19.0 | none | build-time (proc-macro/build-dep: linux) |
| `zbus_names` | 4.3.2 | 4.3.4 | none | runtime (linked: linux) |
| `zeroize` | 1.8.2 | 1.9.0 | none | runtime (linked: linux/mac/win) |
| `zmij` | 1.0.21 | 1.0.23 | none | runtime (linked: linux/mac/win) |
| `zvariant` | 5.12.0 | 5.15.0 | none | runtime (linked: linux) |
| `zvariant_derive` | 5.12.0 | 5.15.0 | none | build-time (proc-macro/build-dep: linux) |
| `zvariant_utils` | 3.4.0 | 4.2.0 | none | runtime (linked: linux) |

## FILES READ:
Cargo.lock
Cargo.toml
crates/runfile-cli/Cargo.toml
crates/runfile-crypto/Cargo.toml
crates/runfile-discovery/Cargo.toml
crates/runfile-env/Cargo.toml
crates/runfile-lang/Cargo.toml
crates/runfile-lsp/Cargo.toml
crates/runfile-runtime/Cargo.toml
crates/runfile-shell/Cargo.toml
crates/runfile-state/Cargo.toml
editors/vscode/package.json
editors/vscode/pnpm-lock.yaml
editors/vscode/runfiles/_shared.run
editors/vscode/runfiles/setup.run
editors/vscode/runfiles/package.run
editors/vscode/runfiles/compile.run
editors/vscode/runfiles/test.run
editors/vscode/runfiles/install.run
editors/vscode/runfiles/uninstall.run
editors/vscode/runfiles/watch.run
editors/tree-sitter/package.json
editors/tree-sitter/pnpm-lock.yaml
editors/tree-sitter/pnpm-workspace.yaml
editors/tree-sitter/.gitignore
editors/tree-sitter/runfiles/_shared.run
editors/tree-sitter/runfiles/setup.run
editors/tree-sitter/runfiles/generate.run
editors/tree-sitter/runfiles/test.run
npm/package.json
rust-toolchain.toml
.cargo/config.toml
CLAUDE.md
security-audit-2026-10-03/00-project-map.md
security-audit-2026-10-03/dependencies.md
security-audit-2026-10-03/dependencies.json
security-audit-2026-10-03/findings/dependencies.jsonl
security-audit-2026-10-03/findings/supply-chain-ci.jsonl
