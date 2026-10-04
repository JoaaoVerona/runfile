# Main agent — Phase 4 verification and coverage sweep

## Verification of critical/high findings

Each high finding was opened at its cited lines and reproduced on the HEAD debug binary (1.8.2) in an isolated
scratch directory (HOME/RUNFILE_CONFIG_DIR/TMPDIR in the scratchpad, DBUS pointed nowhere):

- **Bash completion `compgen -W`** — `crates/runfile-cli/src/completions.rs:491-502` quoted exactly. A project
  holding `runfiles/$(touch TABPWNED).run`, with the generated bash script sourced into `bash --norc` and `_run`
  called for `run ` + Tab, created `TABPWNED`.
- **Double-quoted interpolation** — `crates/runfile-lang/src/value.rs:101-129` and
  `crates/runfile-lang/src/eval.rs:560-569` quoted exactly. `runfiles/deploy.run` = `$ echo "Deploying {{ ARG.version }}"`
  passes `run :lint --check` with no errors; `run deploy --version='$(touch PWNED)'` printed `Deploying ''` and
  created `PWNED`. The pattern is the one SHELL-CHECK-RULES.md lists as "not flagged" (`echo "Deploying {{ ARG.version }}"`,
  `ssh deploy@host "cd {{ ARG.dir }} && ./restart"`).
- **Walk-up discovery** — `crates/runfile-discovery/src/lib.rs:361-371` (`find_upward`) and `264-295`
  (`discover_with`) quoted exactly: `c.is_dir()` is the only check before the directory's files are collected.

## Sweep

`.editorconfig` — formatting settings only (charset, line endings, indentation); nothing executable, nothing
security-relevant. Cleared.

FILES READ:
.editorconfig
crates/runfile-cli/src/completions.rs
crates/runfile-lang/src/value.rs
crates/runfile-lang/src/eval.rs
crates/runfile-discovery/src/lib.rs
crates/runfile-env/src/lib.rs
crates/runfile-cli/tests/cli.rs
SHELL-CHECK-RULES.md
README.md
