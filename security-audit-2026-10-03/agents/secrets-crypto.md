# secrets-crypto: final narrative

Agent: secrets-crypto. Commit a65dba1 (v1.8.2). Binary: target-linux/debug/run (HEAD build). Date: 2026-10-03.
Findings: `findings/secrets-crypto.jsonl` (11 lines: 3 medium, 6 low, 2 hardening).

## What I examined and how

I read every file in scope in full: runfile-crypto (lib and tests), runfile-env (parse.rs, lib.rs and the
encryption/load/parse tests; build/called/path tests in part), runfile-state (all source and tests) and
cmd_env/{mod,crypt,dispatch,secret_keys}.rs. I also read the runtime code that drives them: env.rs,
run_target_with/build_env/value_of in run.rs, key_pool and prepare in dispatch.rs, the dry-run spawn path in
exec.rs, Keys in runfile-lang eval.rs, the decrypt()/decrypt_file functions in functions.rs, and the key-pool
wiring in main.rs. Outside the code I read the setup action, the README's encryption section, the vendored
linux-keyutils-keyring-store 1.0.0 sources, and the lockfile entries for every crypto and keyring crate. Then I
traced three things: where untrusted text can turn into a decryption request, where plaintext is written, and
which read-only entry points reach the key pool.

## Experiments (all under SCRATCH = scratchpad/secrets-crypto)

Harness `$SCRATCH/R`: `env -i` with HOME/XDG/RUNFILE_CONFIG_DIR/TMPDIR inside SCRATCH, RUNFILE_SKIP_PREPARE=1,
DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent, `ulimit -v`, `timeout -s KILL 30`, and the binary run under
`strace -f -e inject=keyctl,add_key,request_key:error=EPERM`. Strace's error injection means **no keyutils
syscall ever reached the kernel**, so the real keyrings were never read or written. The wrapper also counts the
keyctl and D-Bus connect attempts. I checked that the injection works: `:env secret-keys list` showed
`keyctl(KEYCTL_GET_KEYRING_ID, ...) = -1 EPERM (INJECTED)`. The test key came from python `secrets.token_hex(32)`,
was supplied only through RUNFILE_PRIVATE_KEYS, and was deleted afterwards. Every value used was fake.

1. **Decryption oracle (F1).** I made `.env` (with header) and set `DB_PASSWORD` through `:env set`, which
   encrypted it. A target with `.env-file = ".env"` and `$ echo "Building PR: $PR_TITLE"`, run with
   `PR_TITLE=<the DB_PASSWORD ciphertext>`, printed the plaintext. A target with `.env.LABEL = ARG.label` and
   `--label=<ciphertext>` printed the plaintext through `$LABEL` and `{{ ENV.LABEL }}`. A target with no
   `.env-file` printed the plaintext once RUNFILE_ENCRYPTION_PUBLIC_KEY was exported. Without a public key it
   failed instead: any inherited `encrypted:` value makes every target fail.
2. **`:env encrypt` with a multi-line PEM-style value (F2).** The body lines were copied in plaintext, a `...==`
   line was split into a bogus variable, and the command still reported success. The output then failed to
   parse, and the parse error printed a key line. Quoted and commented values decrypted with their quotes and
   comments included (checked with `:env get`).
3. **Malformed ciphertexts** (empty, 13 bytes, 28 bytes, bad symbols, embedded blank, 3 MB): clean errors and
   no panic. The errors name the variable, never the value.
4. **Read-only entry points on a hostile repository (F4).** The repository had a `.env` with a header plus an
   `encrypted:` value, `.env-file` in a target and in `_shared.run`. `--dry-run build` and `--dry-run other`
   each attempted the D-Bus connection and a keyctl call. `build --help`, `:list`, `:lint --check`,
   `:complete 1 run b` and a scripted LSP session (initialize, didOpen, hover, shutdown) attempted neither.
5. **`:env set` fails open (F5).** With a UTF-8 BOM before the header, and with a headerless secrets file, it
   wrote the token in plaintext and printed "set in". The BOM also silently renames the first key at run time
   (`ENV.FIRST` unset, `﻿FIRST` in the child environment).
6. **Newline injection (F8).** A multi-line stdin value set into an encrypted file, then `:env decrypt` to a
   file, produced an extra `NODE_OPTIONS=` variable. `:env set --plain` did the same.
7. **Write ordering (F7).** Strace of `:env decrypt .env out2.env` under umask 022 shows openat O_CREAT 0666,
   then write of the plaintext, then close, then chmod 0600.
8. **Parse-error echo (F9).** `.env-file` naming a file with a bare token line produced `[runfile] error: ...
   expected KEY=VALUE, got: ghp_FAKE...`.
9. **.env edge cases.** CRLF is handled. A NUL in a value gives `could not start ... nul byte`, without the
   value. A BOM: see 5.

## Checked and clean (one line of why each)

- **AEAD and its version.** aes-gcm 0.10.3 (RustCrypto, pinned `=0.10.3`). RUSTSEC-2023-0096 / CVE-2023-42811
  affects >=0.10.0,<0.10.3, so 0.10.3 is the patched release (rustsec.org and osv.dev, retrieved 2026-10-03).
  OSV lists no other advisory for it.
- **Key generation.** `Aes256Gcm::generate_key(OsRng)` uses rand_core 0.6 OsRng, which is getrandom 0.2.17, the
  OS CSPRNG. Both `init`, `rotate` and `secret-keys add` -> Generate use it.
- **Nonces.** 96-bit random from OsRng for every encryption, with no counter and no reuse. Per-key volumes are
  tiny next to the 2^32 random-nonce limit.
- **Ciphertext parsing.** Strict base64, a length check (>= 13) before `split_at`, and aes-gcm rejecting
  anything shorter than the tag. Fuzzed by hand (exp. 3) with no panic.
- **Key format and validation.** Keys are hex with an exact 32-byte check. Intermediate byte buffers are
  zeroised. Hex errors from bad pool entries are swallowed (`if let Ok(derived)`), so an invalid
  RUNFILE_PRIVATE_KEYS line is never echoed. The pool is newline-separated and trimmed; a comma-separated list
  silently fails to match, which is a usability issue only.
- **No password KDF.** Keys are random 256-bit values, so none is needed.
- **Public key.** It is SHA-256 of the raw key: a one-way fingerprint, safe to publish. The comparison uses
  `ct_eq`, which is not needed because the value is public, and is harmless. Prefix matching is used only for
  CLI arguments.
- **Key pool, trial decryption.** `decrypt()` tries each key and GCM's tag makes a wrong key fail
  authentication, so the trial reveals nothing beyond which key works. build_env picks by public key.
- **Decryption failure.** A value that fails to decrypt is always a hard error: no silent empty value and no
  plaintext pass-through, in build_env, `:env get/inject/decrypt/rotate` and `decrypt()` alike.
- **Key prompts.** Importing a key reads it with echo disabled (termios / SetConsoleMode) and restores echo on
  drop. `init` never prints the private key. `get-private` prints it to stdout by design. `--key` is
  refused outside CI.
- **`:env inject`.** It decrypts file values only, the parent environment wins, it writes nothing, and
  RUNFILE_ENV_FILE_TARGET is only read.
- **state.json.** It holds canonical setup paths and fingerprints, nothing secret. It is written at the default
  mode, honours RUNFILE_CONFIG_DIR verbatim, and is not a security boundary.
- **Secret Service.** The item lives in the default collection with attributes service=runfile and
  user=__keystore__. Transport is DH-encrypted. The probe does not unlock; load, store and delete do unlock,
  with no timeout (see F11 and F4). Readability by other same-user processes is inherent to Secret Service.
- **keyutils.** The fallback uses the session keyring linked to the user's persistent keyring, with `user`-type
  keys at kernel-default permissions (not inspected live, by rule). Its volatility is F3.
- **Zeroize.** It is cosmetic: keys are cloned as Strings into pool Vecs and the process environment already
  holds RUNFILE_PRIVATE_KEYS, decrypted values sit in HashMaps and child environments, and aes-gcm is built
  without its `zeroize` feature, so round keys are not wiped. That is not meaningful enough for a finding.
- **Constant time elsewhere.** No other secret comparison exists.

## History scan (secrets in git)

There are 355 commits (all refs), 89 tags and no stash. Commands:
- `git log --all --diff-filter=A --name-only --format= | sort -u | grep -iE '(^|/)\.env|\.pem$|\.key$|id_rsa|id_ed25519|\.p12$|\.pfx$|credential|secret|token|\.npmrc|\.netrc|keystore'`
  matched three source files named `*secret*` and nothing else. No `.env`, key or credential file was ever
  committed.
- `git log --all -p --no-color` dumped to SCRATCH (17 MB; deleted afterwards) and grepped for:
  - GitHub, npm, GitLab, AWS, Slack and OpenAI-style tokens and `-----BEGIN ... PRIVATE KEY`: none.
  - 64-hex strings outside lockfile checksums: Cargo checksums, SHA-256 of "", "abc", "hello" and "test"
    in tests and docs, pattern fillers (`dead…`, `abcd…`, `aabb…`), a docs public key `9f86…` (= sha256("test")),
    and one test fixture `FIXTURE_KEY 82ef…` / `FIXTURE_PUBLIC e4fc…` in
    crates/runfile-runtime/src/tests/keys.rs. That fixture is commented "A key made for these tests and nothing
    else" and encrypts the fixed string `the-secret`. It is obviously a test key, and no committed `.env` uses
    its public key.
  - `encrypted:` values: only that fixture's.
  - RUNFILE_PRIVATE_KEYS with a value, password/token/api_key literals, credentialed URLs, 40-hex tokens near
    the word "token", `_authToken`, and NODE_AUTH_TOKEN / GITEA_TOKEN / NPM_TOKEN literals: none.
- `git grep` of the working tree for the same patterns: none. No ignored `.env`-like files are present outside
  build output.
- Commit messages mentioning secrets and keys describe storage changes only. The initial public commit's
  DOCS.md described a plaintext `secretKeys` array, but the code at that commit already stored only
  fingerprints in settings and keys in the keyring. No shipped version kept keys in a plaintext file.
- **Result: no committed secret, live or otherwise.**

Setup action (`.github/actions/setup/action.yml`): `secret-keys` goes to GITHUB_ENV with a fixed heredoc
delimiter, no format check and no add-mask (F10, hardening). The repository's own CI never passes
`secret-keys`.

## Notes on existing findings

- runtime / hardening "RUNFILE_PRIVATE_KEYS passed to every child": the blast radius is wider than targets.
  `run :env inject` children inherit it too (cmd_inject builds a Command over the inherited environment,
  cmd_env/mod.rs:373-378). The setup action puts it in GITHUB_ENV, so every later step of the job sees it,
  third-party actions included (F10).
- runtime / medium "rendered secrets in --dry-run previews": the same dry-run also loads the key pool from the
  OS store (F4). Fixing F4 with placeholders also removes plaintext from previews.
- runtime / medium "decrypt(src,dst) world-readable": separately, decrypt_file writes `k=plain` unquoted (F8).
  The CLI's `:env decrypt` does chmod 0600, but only after writing (F7).
- Project-map open question 3 is answered. `--dry-run` runs no captures (exec.rs:306-310 returns an empty
  string), but it does build the environment, decrypt `.env-file` values and touch the credential store (F4).
- Dependencies: aes-gcm 0.10.3 is confirmed not affected by RUSTSEC-2023-0096. runfile-state still depends on
  json5 (used only for an error variant, `PrepareStateError::Json5`), although CLAUDE.md lists json5 among the
  dropped dependencies. That is supply-chain hygiene only.

## Open questions

- Does `collection.unlock()` block indefinitely when the keyring is locked and no prompter can be shown? The
  owner's memory notes say yes; I did not test it, by rule.
- How common are consumer workflows that both hold a runfile key and pass untrusted text (PR or issue titles,
  commit messages) through env or `ARG -> .env` into a target that reflects it? That determines F1's real
  exposure.
- macOS Keychain ACL prompts and Windows Credential Manager behaviour were not exercised (Linux host).
- `--stdin-args` and the lazy prompt (prompt.rs) read values with echo on, including `ENV.X` inputs that may
  be secrets. That is out of my file scope and is left to the CLI agent as a hardening question.
- `:env decrypt` with no destination prints every secret to stdout, while its help row says "decrypt to a
  plain file". This is documented behaviour, but a foot-gun in CI logs.

FILES READ:
security-audit-2026-10-03/00-project-map.md
security-audit-2026-10-03/agents/secrets-crypto.md
security-audit-2026-10-03/agents/runtime.md
security-audit-2026-10-03/agents/lang-parser.md
security-audit-2026-10-03/dependencies.md
security-audit-2026-10-03/findings/cli.jsonl
security-audit-2026-10-03/findings/dependencies.jsonl
security-audit-2026-10-03/findings/editors.jsonl
security-audit-2026-10-03/findings/lang-eval.jsonl
security-audit-2026-10-03/findings/lang-parser.jsonl
security-audit-2026-10-03/findings/runtime.jsonl
security-audit-2026-10-03/findings/shell-checker.jsonl
security-audit-2026-10-03/findings/supply-chain-ci.jsonl
security-audit-2026-10-03/findings/untrusted-repo.jsonl
CLAUDE.md
README.md
Cargo.toml
Cargo.lock
.github/actions/setup/action.yml
crates/runfile-crypto/Cargo.toml
crates/runfile-crypto/src/lib.rs
crates/runfile-crypto/src/tests/mod.rs
crates/runfile-crypto/src/tests/constant_time.rs
crates/runfile-crypto/src/tests/core.rs
crates/runfile-crypto/src/tests/zeroize.rs
crates/runfile-env/Cargo.toml
crates/runfile-env/src/lib.rs
crates/runfile-env/src/parse.rs
crates/runfile-env/src/tests/mod.rs
crates/runfile-env/src/tests/encryption.rs
crates/runfile-env/src/tests/load.rs
crates/runfile-env/src/tests/parse.rs
crates/runfile-env/src/tests/build.rs
crates/runfile-env/src/tests/called.rs
crates/runfile-env/src/tests/path.rs
crates/runfile-state/Cargo.toml
crates/runfile-state/src/lib.rs
crates/runfile-state/src/paths.rs
crates/runfile-state/src/keyring_keys.rs
crates/runfile-state/src/keyring_store.rs
crates/runfile-state/src/secret_service_store.rs
crates/runfile-state/src/prepare_state.rs
crates/runfile-state/src/tests/mod.rs
crates/runfile-state/src/tests/keyring_keys.rs
crates/runfile-state/src/tests/prepare_state.rs
crates/runfile-state/src/tests/secret_keys.rs
crates/runfile-cli/src/cmd_env/mod.rs
crates/runfile-cli/src/cmd_env/crypt.rs
crates/runfile-cli/src/cmd_env/dispatch.rs
crates/runfile-cli/src/cmd_env/secret_keys.rs
crates/runfile-cli/src/main.rs
crates/runfile-cli/src/prompt.rs
crates/runfile-runtime/src/env.rs
crates/runfile-runtime/src/run.rs
crates/runfile-runtime/src/dispatch.rs
crates/runfile-runtime/src/exec.rs
crates/runfile-runtime/src/tests/keys.rs
crates/runfile-lang/src/eval.rs
crates/runfile-lang/src/functions.rs
