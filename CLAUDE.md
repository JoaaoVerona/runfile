# CLAUDE.md — Project Reference for Runfile

## What is Runfile

Runfile is a cross-platform command runner (a modern Makefile alternative). A project's tasks live in a
`runfiles/` directory: **one target per file**, written in a small language designed for the job. The `runfile`
CLI (invoked as `run`) discovers and executes them. Written entirely in Rust, compiles to a single binary,
works on Linux, macOS and Windows.

## Build & Test

NEVER use `cargo` commands directly. ALWAYS use `run <target>`; read `runfiles/` to see what exists.

```
run setup                  # One-time per clone: activates the committed git hooks. Gates everything else.
run build                  # Debug build
run check                  # Non-mutating gate: fmt --check + clippy (deny warnings)
run lint                   # Formats, then lints
run test                   # All workspace tests
run install                # Copies the debug build to ~/.local/bin as "rund"

run vscode:setup           # One-time: pnpm install for the VS Code extension. Gates its other targets.
run vscode:compile         # Type-check and compile the extension
run vscode:test            # Compile and run the extension's unit tests
run vscode:package         # Build editors/vscode/runfile-vscode.vsix

run tree-sitter:setup      # One-time: pnpm install for the tree-sitter CLI. Gates its other targets.
run tree-sitter:test       # Regenerate the parser, run the corpus, parse every .run file in the repo
```

`run setup` is a **preparation target** (see below): every other target in `runfiles/` refuses to run until it
has, and again whenever `setup.run` itself changes. `RUNFILE_SKIP_PREPARE=1` bypasses it; CI is exempt
automatically. Note the bypass must NOT be exported when running the CLI test suite — the fixture strips it, but
only because a developer setting it in their own shell would otherwise silently disable the tests that check
the gate.

## The `.run` language

One target is one file. `runfiles/build.run` is the target `build`; `runfiles/api/deploy.run` is `api:deploy`.
`_shared.run` is not a target — it holds settings every file in its directory inherits.

Line-oriented. Every line is one of:

| Form | Meaning |
| --- | --- |
| `# text` | Comment. The leading block is the target's description. |
| `.name = value` | A property. |
| `$ <line>` | Hand this line to a shell. |
| `exec <cmd>` … `end` | Run `<cmd>`, with the block's body as its stdin. |
| `let x = expr`, `x = expr` | Bind and rebind. |
| `if` / `else` / `end`, `for x in …`, `match` / `case` / `default` | Control flow. |
| `run <target> [args]` | Dispatch another target, in-process. |
| `expr` | Evaluated for effect, e.g. `write_file(…)`. |

**The one rule: the language is the default, the shell is marked.** `exec`, `end` and `$ ` were chosen because
they collide with none of the shell lines in the 1,897-line corpus this was designed against; a keyword-first
design would have collided with 69.

`$ run x` re-execs the binary. `run x` dispatches in-process — that is the normal form.

### Types

String, number (one `f64`), bool, list. **Strict, with no coercion**: `"a" + 1` is an error naming `concat`,
`"1" == 1` is false, `number(ARG.x)` is required before arithmetic, and `1 / 2 == 0.5`.

Lists index with `[n]` and work with `first`, `last`, `length`, `join`.

### Sources

`ARG.x` (from `--x=value`), `ENV.X`, `FLAG.x` (bool, from `--x`), `ARGS` (a list of positionals), and
`RUN.os` / `RUN.arch` / `RUN.cwd` / `RUN.file` / `RUN.parent` / `RUN.namespaces`.

**Arguments are `--key=value` only.** `--key value` is a flag plus a positional, because nothing declares which
names take values, so it cannot be disambiguated. It also cannot be silently guessed — so when `ARG.x` is
missing and a flag `x` was passed, the error says exactly that.

**A bare `--` ends parsing**: everything after it is a positional exactly as typed, flags included. That is how
a wrapper forwards a command line (`run _aws -- s3api --bucket X`). Chosen over an `ARGV` source or making
`ARGS` mean everything, so `ARGS` keeps one meaning. Because forgetting the `--` drops the flag silently, `Host`
warns (via `Host::warn`) when a target is handed a `--flag` or `--key=value` that its text never reads as
`FLAG.x` / `ARG.x`. The check is textual — the keys have no dynamic form — and includes `_shared.run`; it runs
once per real run, not per `header_props` lookup.

**`run` statement arguments are values, not shell text.** A `{{ x }}` in `run w {{ x }}` arrives at `w` as one
positional even with spaces, and a list expands to one positional per item — there is no shell in between to
quote for.

`a ? b` takes `a`, or `b` if `a` does not resolve. It is not a ternary; branch with `if`.

### Interpolation self-quotes

`{{ … }}` inside a `$` line or `exec` body becomes **one shell argument** for a string, or **N arguments** for a
list. **Never wrap an interpolation in shell quotes.** This is why there is no `shell_quote` function: of 49
interpolation sites in the corpus, 48 would have been unsafe under manual quoting.

Quotes inside `{{ }}` need no escaping — an interpolation is opaque to the string containing it. `.confirm =
"Greet {{ ARG.name ? "world" }}?"` is correct as written.

### `$` and `exec` in value position

`let files = $ git diff --cached --name-only` captures stdout. This replaced a `capture()` function. **It cannot
nest inside a call**: a `$` capture runs to end of line, so a closing `)` would be ambiguous. Split it into two
statements.

### Three context-sensitive lexer rules

Found by prototyping the grammar against the corpus; an EBNF cannot express them, and `GRAMMAR.ebnf` documents
the rest.

1. A string skips `{{ … }}` recursively, so a quote inside an interpolation does not terminate it.
2. A raw string (`r"…"`) keeps its backslashes, but `\"` still does not terminate it — Python's rule. **An
   unescaped `"` does**, which is why a regex containing quotes must use a normal string with `\"` escapes
   rather than a raw one.
3. `exec` closes on an `end` at the *opener's* indentation, and its body is dedented by its own base indent.

## Project Layout

```
GRAMMAR.ebnf                   # Normative grammar reference
runfiles/                      # This project's own targets (self-hosting); ci/ and wsl/ are namespaces
editors/vscode/                # The VS Code extension (TypeScript) + its own runfiles/
editors/tree-sitter/   # The tree-sitter grammar (Zed, Neovim, Helix) + its own runfiles/

crates/
  runfile-lang/                # Lexer, parser, evaluator, values, the function library
  runfile-discovery/           # Finding runfiles/ directories and building the catalog
  runfile-runtime/             # Properties, env building, process spawning, the walker, dispatch
  runfile-lsp/                 # Language server: diagnostics, completion, shellcheck delegation
  runfile-cli/                 # The `run` binary
  runfile-env/                 # .env parsing and env-map building
  runfile-crypto/              # AES-256-GCM for encrypted env values
  runfile-state/               # Prepare state, OS credential store access
```

## Crate Responsibilities

### runfile-lang

`lexer.rs` is a hand-rolled scanner (`skip_interp`, `split_interp`, `scan_string`, `tokenize`). `parser.rs`
classifies lines, then climbs precedence for expressions. `eval.rs` holds `Scope` and evaluation; `functions.rs`
holds the pure standard library plus `call_io` for filesystem and regex; `value.rs` holds `Value` and shell
quoting.

- `Scope.private_keys` is a `Keys`: a **deferred, memoized** key pool. Loading is deferred because the pool
  comes from an OS credential store, and a locked keyring blocks on an interactive unlock prompt — an eager load
  turned every `run <target>` into a hang. Memoized so a run that decrypts twice still prompts once.
- `Scope.dry_run` exists so `write_file` and `decrypt` can refuse to write. A preview that edits the working
  tree is worse than no preview.
- `FUNCTIONS` is exported and driven into editor completion, with tests in **both** directions: every listed
  name must dispatch, and every dispatch arm must be listed. Only the first existed at one point, and `min`
  sat implemented but unlisted, so completion never offered it.
- `Scope.temps` is a `TempFiles`: a shared handle, not a process-global, holding what `temp_file` and
  `temp_dir` made. `Host::cleanup_temps` drains it however the run ended, which is the point — a target that
  fails half-way is exactly when a decoded credential must not be left in the temp directory. Watch mode
  drains after every iteration.
- Binding names are validated in both `let` and reassignment; a block closer (`end`/`else`/`case`/`default`)
  with nothing open is a parse error rather than an expression statement.
- `Statement::Exec` carries `lines: Vec<usize>`, the source line of each body line. The two are not derivable
  from each other: a `$` run skips blank and comment lines, and a backslash continuation folds several source
  lines into one.

### runfile-discovery

Walks **up** for the nearest `runfiles/`, then **down** for `*/runfiles/` (depth cap 3, skipping
`node_modules`, `target`, `dist`, `build`, `.git`, `vendor`). Nested directories become `:`-separated namespace
segments. `$HOME/.runfiles/` is machine-wide, at a **fixed path with no setting to move it**. This replaced
`includes` entirely.

- **The anchor rule**: the parent of `runfiles/` is the single anchor for cwd, `.env-file`, `.add-path`,
  `glob`, `read_file` and `{{ RUN.parent }}`.
- **`_shared.run` layers by directory.** `Catalog::shared_chain` returns every one that applies, outermost
  first, so `runfiles/api/_shared.run` adds to `runfiles/_shared.run` rather than replacing it. Only the top
  one used to be registered at all, so a nested one was read by nothing. The walk stops at the target's own
  `runfiles/` tree: a subproject does not inherit the root's, for the same reason its targets are namespaced.
- `resolve` is one hash lookup; `.alias` is only scanned on a miss, so aliases cost nothing in the common case.
  A real file name always wins over an alias, and two targets claiming one alias is an error naming both.
- **An alias carries its target's namespace.** `web/runfiles/setup.run` declaring `deps` answers to `web:deps`,
  never a bare `deps` — a subproject must not claim a name in the root.
- `.only-in-directories` in a global `_shared.run` scopes the machine-wide directory: registered everywhere,
  active only inside the paths it names. Compared on path components, so `work/acme` does not admit
  `work/acme-other`. `~` expands.

### runfile-runtime

`props.rs` (property resolution), `env.rs` (env building), `exec.rs` (spawning), `run.rs` (the walker),
`dispatch.rs` (`Host`, target resolution, cycle detection), `shell.rs` (shell selection).

- `PROPERTIES` is exported with a block-scoped flag per name, tested against `extend` the same way `FUNCTIONS`
  is. Block-scoped: `shell`, `parallel`, `ignore-errors`, `workdir`, `env`. The rest are header-only.
- **Shell resolution**: bash → Git Bash (four known Windows paths) → sh. `System32\bash.exe` is deliberately
  excluded: it is the WSL launcher, and a different filesystem.
- `is_shell()` matches `sh|bash|dash|ash|zsh|ksh|busybox` on the **first word only**, and inserts `-e`.
- **`Dispatch::run` returns the child's trace** rather than writing to shared state. A child finishes while its
  parent is still walking, so a shared buffer printed every dependency *before* the line that called it. The
  caller splices the trace in where the call appeared, which is what makes `--dry-run` order match execution
  order.
- **A subproject calls its own siblings.** `run compile` inside `web/runfiles/` resolves `web:compile` first,
  falling through to a root `compile` when there is no sibling — so a file spells its neighbours the same way
  wherever `run` was invoked from.
- `Host::header_props` **probes**: it evaluates the declaration region only to read `.watch`, so it neither
  warns about unread inputs nor lets a writing function write. Without the second half, `.env.X =
  temp_file(...)` made two files per run, one an orphan nothing referenced.
- **Ctrl+C** is caught so the run can stop between statements, delete its temp files, and exit 130. The flag
  is process-global because a signal handler has nowhere else to write, but the runtime reads an injected
  predicate (`Host::interrupted`), so it reaches for no process state of its own and one test cannot
  interrupt another. `.ignore-errors` does not apply to it.
- `Dispatch` is `Sync` with `&self` and an explicit `chain: &[String]`. Per-path rather than shared, so
  parallel siblings are not mistaken for a cycle.
- `.parallel`: bindings evaluate in source order, then executable leaves fan out via `std::thread::scope`.
  Control flow expands into the same batch, and every branch completes before a failure surfaces.
- **The runner announces every command on stderr**, natively, with no property to turn it on: that is why the
  old `logging` field was cut rather than renamed. stderr, so a pipeline reading stdout is unaffected, and
  never under `--dry-run`, which already prints the commands to stdout.
- **`.detach`** starts the commands and does not wait. Its streams go to null: inherited, they would hold the
  runner's own stdout and stderr open after it exits, so whoever is reading them waits for the very command
  that was meant to outlive the run.
- **`.parallel` on a `for` body fans out the iterations**, not just each body's statements. Leaves are
  collected across every iteration first, so they form one batch. Without that the property read as
  "parallel" and behaved as "in turn".
- **Parallel output is labelled per line**, since several children write at once. The label is the target
  name for a `run` leaf and the `exec` header otherwise, falling back to the first word of the body — never
  the shell, or every `$` branch would be called `bash`. It is threaded through `Dispatch::run`, so a
  branch's dependencies carry the branch's name rather than their own. A sequential run inherits the
  terminal and adds no prefix: nothing to disambiguate, and a pipeline reading `run`'s output keeps working.
- `.add-path` is this target's own. The ancestor chain the old model carried across a re-exec is gone with
  the re-exec: dispatch is in-process, and every target builds PATH from its own properties.
- `env::build` receives the same deferred key pool the `decrypt` function uses. It was previously passed `None`,
  which meant an encrypted `.env-file` value could never be decrypted at all.

### runfile-lsp

`analysis.rs` (diagnostics and completion, pure), `rpc.rs` (framing), `server.rs` (dispatch), `shell.rs`
(shellcheck delegation).

- Diagnostics come from the **real parser**, so an editor and the runner cannot disagree about validity.
- The binary ships in the release archive beside `run`; both installers, the npm package (one launcher script
  copied under each name) and `:update` install both. The editor integrations find it on PATH by name.
- The transport is hand-rolled. LSP framing is a header and a byte count; a framework would reintroduce the
  async runtime this rewrite removed, for a server that answers one client, one message at a time.
- Full document sync, deliberately: these files are small, and an incremental applier is a source of drift.
- Every request is answered — an unanswered one hangs the client — and every notification is silent.
- `FUNCTIONS` and `PROPERTIES` carry a signature and a one-sentence doc, so completion shows detail and
  hover has something to say. Hover reads the word under the cursor rather than the tree, so it keeps working
  while the document does not parse. `RUN.` is the only source whose keys are known ahead of time; `ARG`,
  `ENV` and `FLAG` are whatever the caller passed, so there is nothing to offer for them.
- **Shellcheck delegation**: `$` runs and `exec sh|bash|dash|ash|ksh` bodies are handed to shellcheck. An
  interpolation renders as one quoted placeholder, because it resolves to exactly one shell word; leaving the
  braces in would have shellcheck reporting on a command nobody wrote. Since a placeholder is a different width
  from what it stands for, a line containing one is reported **whole** rather than with a confidently wrong
  column. Shellcheck's four levels map onto LSP's four. Checking is skipped when the document does not parse,
  and a missing shellcheck is silent.

### editors/tree-sitter

`grammar.js` mirrors `GRAMMAR.ebnf`. Newlines are tokens rather than extras, so every line form ends in one;
a file without a trailing newline gets a zero-width one from the scanner, exactly once.

- `src/scanner.c` carries the rules an EBNF cannot: an `exec` body closes only on an `end` at the opener's
  indentation, a `$` or `exec` body stops at `{{` so an interpolation is a node the grammar parses, and a
  `run` argument is one whitespace-delimited word with its interpolations kept whole. The string rule needs no
  scanner: the interpolation's expression is parsed as an expression, quotes and all.
- **Scanner state is carried forward only by a successful token**; what a false return records is discarded.
  The indentation a capture `exec` needs is therefore recorded on the newline token that precedes its line, by
  looking past the token's marked end — not in the column-0 check, which says no to every non-`exec` line.
- `conflicts: [[list]]`: a newline between the last element and `]` can belong to the separator or the closer.
  Both readings produce the same tree, since newlines are hidden, so GLR may pick either.
- Tested by `test/corpus/` (tree shapes) and a sweep that parses every `.run` file in the repository — the
  check that the grammar accepts what the runner accepts.
- pnpm 11 blocks tree-sitter-cli's install script, which downloads the binary; `pnpm-workspace.yaml`
  (`allowBuilds`) approves it. The old `pnpm` field in `package.json` is no longer read.

### runfile-cli

`main.rs` (flags and dispatch), `list.rs`, `prepare.rs`, `prompt.rs`, `watch.rs`, `completions.rs`, `init.rs`,
`cmd_env/`, `cmd_update.rs`, `ci_detect.rs`.

- **Runner flags are recognised only before the target name**; everything after it belongs to the target. So
  `run echoes --dry-run` passes `--dry-run` through as `FLAG.dry-run`.
- `:list` shows the aliases a target answers to. Without that a documented alias is undiscoverable, since
  the listing otherwise reports only the file name -- which the corpus inventory diff is what surfaced.
- `:list` has three forms: human, `--names` (for completion scripts), `--json` (for tooling). The JSON is
  serialized by hand — four string fields do not justify a serde dependency in the CLI — and carries a
  `formatVersion` that CI checks against the extension's constant.
- `--dry-run` is **not** gated by prepare: it changes nothing, and reading what a target would do is a
  reasonable thing to want before setting a project up.
- Watch mode is entered automatically by any target declaring `.watch`; there is no flag, because the file
  already said what it wants. Patterns interpolate, so they resolve through `Host::header_props` rather than
  being read off the source text. A failing run is reported and watching continues.
- Completion scripts are hand-written per shell and ask the binary itself for names, so they can never drift
  from the language. The bash one is tested by sourcing it and driving `_run` the way the shell does.
  `:completions install <shell>` puts a marked hook in the shell's own profile — fish gets a file, since it
  reads a directory — and the hook calls the binary rather than embedding the script, so an upgrade needs no
  reinstall. The marker is what makes a second install a no-op and `uninstall` exact.
- `:generate zed|jetbrains|vscode` is a lean port of the old generators: an entry is recognised as ours by its
  shape (command `run`, label `run <target>`), so a rerun replaces exactly those and keeps a person's own; a
  file is rewritten with the indentation it already uses. The 661-line `.editorconfig` reader did not come
  back. Global targets are left out unless `--include-global`, since a task file is committed and
  `~/.runfiles/` is one person's. `Catalog.root` (the parent of the nearest `runfiles/`) is where the files go.

## Properties

Header-only: `alias`, `confirm`, `env-file`, `add-path`, `hide`, `watch`, `only-in-directories`, `detach`.
Block-scoped (may also appear inside `if` / `for` / `match`): `shell`, `parallel`, `ignore-errors`, `workdir`,
`env` (addressed by sub-key, `.env.NAME = "value"`).

A nested block inherits behaviour but never a parent's one-shot header state — a `confirm` must not fire again
per loop iteration.

## Preparation targets

A target named `setup` gates every other target in its directory, fingerprinted by its own text, so editing the
setup re-triggers the requirement. State lives in `state.json` in the platform state directory. There is no
settings file: global registrations, path aliases and custom shell paths were all replaced by conventions
(`$HOME/.runfiles/`, discovery, shell detection).

## Removed, and not coming back

MCP server, `:convert`, `:config` (all subcommands), the user settings file, `-p` / target globs, `capture()`
(now `$` in value position), `shell_quote()` (interpolation self-quotes), `set_cwd()` (now `.workdir`),
`define()` (now `let`), `nth()` / `count_parts()` (now `split()` and indexing), the arithmetic and comparison
functions (now operators), `when:` blocks, `sameShell`, `extendStdio`, `forceKillOnSigInt`, the JSON schema,
`VAR.` (replaced by `let`), and `RUNFILE_TARGET`. Dropped dependencies: `rmcp`, `tokio`, `json5`, `md-5`,
`shlex`.

Every other function from the old surface is present. Seventeen were missing at one point, dropped by
oversight rather than decision, and all are back. `try` is the one exception, replaced by `a ? b`.

`now` and `uuid` are read-only, so a preview shows a real value rather than a placeholder: `--dry-run` is
about not changing anything. `json_get` returns a number, bool or string directly, and an object or array as
its compact JSON text, since the language has no map type.

## Testing Requirements

The standard is **not coverage but coverage of behaviour**: nothing should need checking by hand. Several of
the bugs found during this rewrite — an eagerly-loaded keyring hanging every invocation, `--dry-run` writing
files, a dependency's trace printing before its call site — were invisible in normal use and are now pinned by
tests that assert the mechanism rather than the symptom.

1. Run `run test` for the whole workspace, not just the crate you changed. Run `run vscode:test` for the
   extension.
2. New behaviour needs a test that would fail without it. Prefer asserting the mechanism (count the keyring
   loads) over the symptom (notice the hang).
3. CLI behaviour is tested by driving the compiled binary in `crates/runfile-cli/tests/cli.rs`, with
   `HOME`, `USERPROFILE`, `RUNFILE_CONFIG_DIR`, `XDG_*` and `APPDATA` pointed at an empty directory and
   `CI`, `GITHUB_ACTIONS`, `RUNFILE_SKIP_PREPARE` and `RUNFILE_PRIVATE_KEYS` stripped. The first three
   matter on Windows, where the Known Folder API ignores `HOME` and `APPDATA`; the CLI reads `HOME` before
   asking the platform for exactly this reason.
4. LSP behaviour is tested by scripting a whole client conversation through the real transport
   (`crates/runfile-lsp/tests/protocol.rs`), and the compiled binary is started as a subprocess
   (`tests/binary.rs`) — nothing in the former would notice a broken `main.rs` or a renamed binary, and it
   is what ships.
5. Tests that need an external tool (shellcheck) skip cleanly when it is absent, so a contributor without it
   does not see a broken build. One of them is a gate: `runfile-lsp/tests/repo_shell.rs` shellchecks every
   `.run` file in this repository through the same extraction an editor uses, so the repo's own shell cannot
   rot. It caught an unbalanced `if` in a golden fixture the first time it ran.
6. `runfile-lang/tests/golden/` holds one file per AST shape beside the tree it parses to, with source
   positions stripped so a diff is about structure rather than whitespace. Regenerate a deliberate change
   with `UPDATE_GOLDEN=1 cargo test -p runfile-lang --test golden`.
7. Watch tests poll rather than sleep; a fixed sleep is either flaky or slow.
8. Cross-platform: normalize backslashes in path assertions. A test that can only hold on one platform should
   be `#[cfg]`-gated there rather than weakened.

## Documentation

`README.md` is the public documentation. `GRAMMAR.ebnf` is the normative grammar. Update this file with any new
design decision, crate, or behaviour change.
