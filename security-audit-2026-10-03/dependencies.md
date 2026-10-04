# Dependency audit

Generated: 2026-10-03

Advisory data: OSV.dev, retrieved 2026-10-03.
Current-version data: package registries, retrieved 2026-10-03.
Advisories published after this date are not reflected.

## Summary

- Manifests/lockfiles parsed: **6**
- Unique package entries: **229** (224 pinned, 5 unpinned)
- Packages with advisories: **2**

## Packages with known advisories

Confirm each against the advisory page before reporting: check that the
affected range really includes the installed version, and establish whether
the vulnerable code is reachable in this project.

| Package | Installed | Ecosystem | Advisory | Severity | Fixed in | Scope |
|---|---|---|---|---|---|---|
| `anyhow` | 1.0.102 | crates.io | [RUSTSEC-2026-0190](https://osv.dev/vulnerability/RUSTSEC-2026-0190) | Unknown | 1.0.103 | runtime |
| `event-listener` | 5.4.1 | crates.io | [RUSTSEC-2026-0221](https://osv.dev/vulnerability/RUSTSEC-2026-0221) | Unknown | 5.4.2 | runtime |

### Details

#### `anyhow` 1.0.102 (crates.io)

Declared in: `Cargo.lock`
Current release: **1.0.104**

- **RUSTSEC-2026-0190** (Unknown) — Unsoundness in `Error::downcast_mut()`
  - Fixed in: 1.0.103
  - https://osv.dev/vulnerability/RUSTSEC-2026-0190 (retrieved 2026-10-03)

  Reachability: _to determine — is this a runtime dependency, is the affected API called, can attacker-controlled data reach it?_

#### `event-listener` 5.4.1 (crates.io)

Declared in: `Cargo.lock`
Current release: **5.4.2**

- **RUSTSEC-2026-0221** (Unknown) — `event-listener` allows `!Send` tags to cross thread boundaries via `StackSlot`
  - Fixed in: 5.4.2
  - https://osv.dev/vulnerability/RUSTSEC-2026-0221 (retrieved 2026-10-03)

  Reachability: _to determine — is this a runtime dependency, is the affected API called, can attacker-controlled data reach it?_

## Outdated packages (126)

Not vulnerabilities in themselves — relevant because a large version gap makes the eventual security upgrade a project rather than a patch.

| Package | Installed | Current | Ecosystem |
|---|---|---|---|
| `aead` | 0.5.2 | 0.6.1 | crates.io |
| `aes` | 0.8.4 | 0.9.3 | crates.io |
| `aes-gcm` | 0.10.3 | 0.11.1 | crates.io |
| `aho-corasick` | 1.1.4 | 1.1.5 | crates.io |
| `anyhow` | 1.0.102 | 1.0.104 | crates.io |
| `apple-native-keyring-store` | 1.0.0 | 1.0.2 | crates.io |
| `async-trait` | 0.1.89 | 0.1.92 | crates.io |
| `autocfg` | 1.5.0 | 1.5.1 | crates.io |
| `base64` | 0.22.1 | 0.23.1 | crates.io |
| `bitflags` | 2.11.1 | 2.13.2 | crates.io |
| `block-buffer` | 0.10.4 | 0.12.1 | crates.io |
| `block-buffer` | 0.12.0 | 0.12.1 | crates.io |
| `block-padding` | 0.3.3 | 0.4.2 | crates.io |
| `blocking` | 1.6.2 | 1.7.0 | crates.io |
| `bstr` | 1.12.1 | 1.13.1 | crates.io |
| `bumpalo` | 3.20.2 | 3.20.3 | crates.io |
| `cbc` | 0.1.2 | 0.2.1 | crates.io |
| `cfg-if` | 1.0.4 | 1.0.5 | crates.io |
| `cipher` | 0.4.4 | 0.5.2 | crates.io |
| `clap` | 4.6.1 | 4.6.7 | crates.io |
| `clap_builder` | 4.6.0 | 4.6.7 | crates.io |
| `clap_derive` | 4.6.1 | 4.6.7 | crates.io |
| `clap_lex` | 1.1.0 | 1.1.1 | crates.io |
| `cpufeatures` | 0.2.17 | 0.3.1 | crates.io |
| `cpufeatures` | 0.3.0 | 0.3.1 | crates.io |
| `crossbeam-utils` | 0.8.21 | 0.8.23 | crates.io |
| `crypto-common` | 0.1.7 | 0.2.2 | crates.io |
| `crypto-common` | 0.2.1 | 0.2.2 | crates.io |
| `ctr` | 0.9.2 | 0.10.1 | crates.io |
| `digest` | 0.10.7 | 0.11.3 | crates.io |
| `dirs` | 6.0.0 | 7.0.0 | crates.io |
| `event-listener` | 5.4.1 | 5.4.2 | crates.io |
| `fastrand` | 2.4.1 | 2.5.0 | crates.io |
| `foldhash` | 0.1.5 | 0.2.0 | crates.io |
| `fsevent-sys` | 4.1.0 | 5.2.0 | crates.io |
| `futures-core` | 0.3.32 | 0.3.34 | crates.io |
| `futures-io` | 0.3.32 | 0.3.34 | crates.io |
| `futures-macro` | 0.3.32 | 0.3.34 | crates.io |
| `futures-task` | 0.3.32 | 0.3.34 | crates.io |
| `futures-util` | 0.3.32 | 0.3.34 | crates.io |
| `generic-array` | 0.14.7 | 1.4.5 | crates.io |
| `getrandom` | 0.2.17 | 0.4.3 | crates.io |
| `getrandom` | 0.4.2 | 0.4.3 | crates.io |
| `ghash` | 0.5.1 | 0.6.0 | crates.io |
| `globset` | 0.4.18 | 0.4.20 | crates.io |
| `hashbrown` | 0.15.5 | 0.17.1 | crates.io |
| `hashbrown` | 0.17.0 | 0.17.1 | crates.io |
| `hermit-abi` | 0.5.2 | 0.5.3 | crates.io |
| `hkdf` | 0.12.4 | 0.13.0 | crates.io |
| `hmac` | 0.12.1 | 0.13.0 | crates.io |
| `hybrid-array` | 0.4.11 | 0.4.15 | crates.io |
| `indexmap` | 2.14.0 | 2.14.2 | crates.io |
| `inotify` | 0.11.1 | 0.11.5 | crates.io |
| `inotify-sys` | 0.1.5 | 0.1.8 | crates.io |
| `inout` | 0.1.4 | 0.2.2 | crates.io |
| `js-sys` | 0.3.98 | 0.3.106 | crates.io |
| `kqueue` | 1.1.1 | 1.2.1 | crates.io |
| `kqueue-sys` | 1.1.0 | 1.1.2 | crates.io |
| `libc` | 0.2.186 | 0.2.190 | crates.io |
| `libredox` | 0.1.16 | 0.1.25 | crates.io |
| `log` | 0.4.29 | 0.4.34 | crates.io |
| `memchr` | 2.8.0 | 2.8.3 | crates.io |
| `mio` | 1.2.0 | 1.2.3 | crates.io |
| `num-bigint` | 0.4.6 | 0.5.1 | crates.io |
| `num-integer` | 0.1.46 | 0.1.47 | crates.io |
| `num-iter` | 0.1.45 | 0.1.46 | crates.io |
| `opaque-debug` | 0.3.1 | 0.4.0 | crates.io |
| `polyval` | 0.6.2 | 0.7.3 | crates.io |
| `prettyplease` | 0.2.37 | 0.3.0 | crates.io |
| `proc-macro2` | 1.0.106 | 1.0.107 | crates.io |
| `quote` | 1.0.45 | 1.0.47 | crates.io |
| `r-efi` | 6.0.0 | 7.1.0 | crates.io |
| `rand_core` | 0.6.4 | 0.10.1 | crates.io |
| `redox_users` | 0.5.2 | 0.5.3 | crates.io |
| `regex` | 1.12.3 | 1.13.1 | crates.io |
| `regex-automata` | 0.4.14 | 0.4.18 | crates.io |
| `regex-syntax` | 0.8.10 | 0.8.11 | crates.io |
| `rustix` | 1.1.4 | 1.1.5 | crates.io |
| `rustversion` | 1.0.22 | 1.0.23 | crates.io |
| `secret-service` | 5.1.0 | 5.2.0 | crates.io |

_...and 46 more — see dependencies.json_

## Unpinned dependencies (5)

Declared as ranges with no lockfile entry parsed, so the installed version is whatever resolved at build time — builds aren't reproducible and the audit can't confirm what's deployed.

- `@types/node` (npm)
- `@types/vscode` (npm)
- `typescript` (npm)
- `vscode-oniguruma` (npm)
- `vscode-textmate` (npm)

## Manifests parsed

| File | Ecosystem | Lockfile | Entries |
|---|---|---|---|
| `Cargo.lock` | crates.io | yes | 223 |
| `editors/tree-sitter/package.json` | npm | no | 1 |
| `editors/tree-sitter/pnpm-lock.yaml` | npm | yes | 0 |
| `editors/vscode/package.json` | npm | no | 5 |
| `editors/vscode/pnpm-lock.yaml` | npm | yes | 0 |
| `npm/package.json` | npm | no | 0 |

---

Next: confirm each advisory against its page, establish reachability, check
supply-chain integrity (install scripts, dependency confusion, unpinned CI
actions, vendored code), and verify runtime EOL status with a live lookup.
See references/dependencies.md.
