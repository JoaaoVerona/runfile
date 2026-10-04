# editors: VS Code extension and tree-sitter grammar

Agent: editors. Date: 2026-10-03. Commit a65dba1 (v1.8.2).

## What I examined and how

Scope: `editors/vscode` (TypeScript extension, TextMate grammars, package manifest, `.vscodeignore`, its
runfiles) and `editors/tree-sitter` (grammar, hand-written C external scanner, queries, package files,
runfiles). I read every source file in scope in full except `grammar.test.ts` and `pure.test.ts`, which
are test harnesses (skimmed for anything that executes or reads the environment). For the TextMate grammar I
extracted and reviewed every `match`/`begin`/`end` regex rather than reading the 579-line JSON top to
bottom.

I traced untrusted data from three sources:

1. **The workspace's file names and comments** -> `run :list --json` -> `parseCatalog` -> `entryFor` /
   `buildTask` -> `vscode.Task` (definition, `ShellExecution` args, or the `CustomExecution` pty's argv),
   and from the file path directly through `targetNameFor` / `anchorFor` -> CodeLens -> `buildFileTargetTask`.
   I then followed the task into VS Code itself, reading the current sources of `terminalTaskSystem.ts`,
   `mainThreadTask.ts`, `extHostTask.ts` (node), `baseConfigurationResolverService.ts` and
   `variableResolver.ts`.
2. **The workspace's `.vscode/settings.json`** -> `runfile.catalogCommand`, `runfile.lspPath`,
   `runfile.interactive` -> `execFile` / `spawn`.
3. **The bytes of a `.run` file opened in an editor** -> the TextMate tokenizer (VS Code) and the
   tree-sitter parser + `scanner.c` (Zed, Neovim, Helix).

Workspace Trust was looked up live (below), since it decides the severity of everything in source 2 and
most of source 1.

## Experiments and results

All fixtures under `$SCRATCH` (`/tmp/claude-1000/.../scratchpad/editors`). The `run` binary was run only as
`run :list --json` under the isolated `env -i` wrapper. No repository runfile was run, nothing was
installed or downloaded except read-only source/documentation fetches, and no process was left running.

1. **Crafted target names through the real binary.** A scratch repo with `runfiles/${command/workbench.action.reloadWindow}.run`,
   `runfiles/a;touch${IFS}PWNED.run` and `build.run`. `run :list --json` lists them as
   `${command:workbench.action.reloadWindow}` and `a;touch${IFS}PWNED`: discovery joins path components
   with `:` and checks no characters.
2. **VS Code's shell command-line builder, replicated.** A byte-for-byte copy of `needsQuotes` / `quote` /
   `quoteIfNecessary` from the current `terminalTaskSystem.ts` (bash quoting options), fed the catalog from
   experiment 1, and `bash -c` with a stub `run` on PATH. `a;touch${IFS}PWNED` created `PWNED` (re-verifies
   existing finding 1). New: names with a space are *not* safe either: `x'$(touch Q1)' y` became
   `run --stdin-args 'x'$(touch Q1)' y'` and created `Q1`; the same through a directory component
   (`dir with space/a'$(touch Q2)'b`) created `Q2`. VS Code's strong quoting wraps the word in `'...'`
   without escaping an inner `'`.
3. **VS Code task variable collection.** VS Code's `/\$\{(.*?)\}/g` collector applied to the same names
   picks out `${command:workbench.action.reloadWindow}` (and `${IFS}`, which VS Code's resolver passes
   through unchanged, so finding 1's payload survives resolution). Source tracing for what happens next is
   in new finding A.
4. **TextMate tokenization timing** with the repository's own `vscode-textmate` + `vscode-oniguruma`
   (from `editors/vscode/node_modules`, used read-only) and **VS Code's real shell grammar**
   (`/usr/share/code/.../shell-unix-bash.tmLanguage.json`), 24 crafted 20,000-character lines. No regex in
   our grammar backtracks: every case was 0.4-64 ms except three nesting cases, which are quadratic in
   nesting depth rather than in backtracking: `"{{ "` x5000 = 5.5 s, `$ "$('` x5000 = 16.3 s,
   `$ ${` x10000 = 3.7 s, and 5,000 lines each opening `{{ "` = 8.3 s total. Our interpolation injection
   makes the shell cases ~6x slower than the shell grammar alone, but the shell grammar alone has the same
   quadratic curve in a `.sh` file. VS Code tokenizes with `tokenizeLine2(line, state, 500)` (current
   `textMateTokenizationSupport.ts`): a 500 ms per-line cap, after which the line's start state is kept,
   so nesting cannot carry over from a stopped line. Not reported (see "clean").
5. **libFuzzer + ASan + UBSan harness over `scanner.c`** (clang 18): it implements `TSLexer` over a byte
   buffer, calls `scan` with arbitrary `valid[]` sets (including all-true, as tree-sitter does in error
   recovery), round-trips every state through `serialize`/`deserialize` into a 1024-byte buffer, asserts
   the token end lies within the input, and traps on a runaway advance or on more than 64 consecutive
   zero-width tokens with an unchanged state. Seeded with this repo's runfiles, the tree-sitter corpus, and
   hand-written edge cases (300-blank indents, 256-blank wraparound, 70 tabs, CRLF, no final newline, nested
   interpolations, markers). 141,428 runs in 240 s: no crash, no sanitizer report, no loop.
6. **Real parse timing** with the repository's own tree-sitter CLI 0.25.6 binary run directly (not via
   any runfile) on a scratch copy of the grammar, with `HOME`/`XDG_*` pointed into scratch so the compiled
   `runfile.so` landed there. Quadratic in line length for one-line lists, `+` chains, and exec/json bodies
   with many interpolations (96 KB list 13.1 s, 192 KB exec body 26.1 s); linear for the same content over
   many lines, for `$` lines, `run` arguments, and call arguments. Two modified scratch copies isolated the
   cause to the scanner's `get_column` calls (list 13.1 s -> 0.29 s, sum 8.9 s -> 0.02 s, exec body
   26.1 s -> 0.07 s). New finding B.

## New findings

- **A (low, likely): `${…}` in a target name is expanded by VS Code's task variable resolver.** A target
  file `runfiles/${command/<id>}.run` is the target `${command:<id>}`. With the default
  `runfile.interactive=true` the task is a `CustomExecution`, and VS Code resolves `${}` variables in a
  custom-execution task's *definition* (documented in `vscode.d.ts`), which carries the raw name. The
  resolution chain ends in `commandService.executeCommand('<id>', …)` (traced through
  `terminalTaskSystem._collectTaskVariables` -> `_acquireInput` -> `mainThreadTask.resolveVariables` ->
  `resolveWithInteraction`). Since most commands return `undefined`, the task is then cancelled, so
  clicking Run executes the VS Code command and never the target. It needs trust and a click, so it is the
  same class and rating as finding 1, but it needs no setting change. Not run inside VS Code.
- **B (low, confirmed): quadratic parsing in the tree-sitter scanner.** `scan_line_start` asks
  `lexer->get_column()` first thing whenever COMMENT is valid, which is after almost every expression token,
  and `scan_exec_content` asks on entry to each segment between interpolations. tree-sitter recomputes an
  invalidated column by re-reading from the start of the line. Opening a few-hundred-KB crafted `.run` file
  in Neovim/Helix/Zed costs tens of seconds to minutes of CPU (Helix drops highlighting at its 500 ms
  parse timeout; Neovim 0.11+ is async; older Neovim freezes). No memory-safety issue found.

## Checked and found clean

- **`catalog.ts` runs the catalog command with `execFile`, not a shell.** The setting is split on
  whitespace and executed with `cwd` = the workspace folder and an 8 MB `maxBuffer`, so shell metacharacters
  in the *output* or in the target data never reach a shell here. (The package.json description calls it a
  "Shell command"; it is not one, which is the safe direction.) No `timeout` is set; see notes.
- **The interactive task path (the default) spawns `run` with an argv and no shell**
  (`RunfileInteractivePty.open`). Target names arrive as single arguments; only VS Code's own variable
  resolution touches them first (finding A). Process-group kill on close is correct on Unix and falls
  back cleanly elsewhere.
- **LSP framing (`pure.ts` `MessageReader`, `frame`).** Lengths are counted in bytes on a `Buffer`, a
  header without a length is dropped rather than stalling, a malformed body loses one message, junk before
  a header is tolerated because the regex is unanchored, and `frame` writes the UTF-8 byte length of the
  same string it sends. The buffer has no cap, so a stray `Content-Length: <huge>` from the server would
  stall the client and buffer everything after it, but only the extension's own `run :lsp` writes to that
  pipe and serde-escaped JSON cannot produce a header. Requests time out after 2 s and remove themselves
  from `pending`.
- **How the server binary is chosen.** `lspPath` (default `run`) is spawned with no `cwd`. On Windows,
  VS Code chdirs every process to its install folder (`bootstrap-node.ts`), so a bare name is not found in
  the workspace there. The `runfile-lsp` fallback is a bare name resolved the same way, and it is tried only
  if `run :lsp` never answered. On Unix a bare name goes through PATH only. The workspace-settings angle is
  existing finding 2.
- **Markdown rendering.** Tree tooltips (`**label**\n\n<description>`), hover and completion docs are
  `new vscode.MarkdownString(text)` with `isTrusted` left false, so `command:` links in repository text
  (a target description, or a binding's source line, which the server wraps in a code fence that a line
  containing a triple backtick can close) are not clickable. Ordinary links and images may render, which
  costs at most an outbound request when the user hovers, in a trusted workspace. Not reported.
- **`definition` / diagnostics URIs** are parsed from the server's replies, and the server builds them
  from file paths. A non-`file:` URI would only fail to open.
- **The TextMate grammars** have no catastrophic-backtracking regex: the `let` name list, property
  path, `exec` openers and `function` / `capture-call` patterns all have unambiguous separators or a `^`/`\b`
  anchor, and measured linear at 20,000 characters (experiment 4). The superlinear cases come from
  begin/end nesting depth, are shared with VS Code's own shell grammar, and are capped at 500 ms per line
  by VS Code. The injection selector (`L:meta.embedded.*`) is the costly part and could be narrowed, but
  this is a performance note, not a finding.
- **`language-configuration.json`** has two trivial indentation regexes.
- **`package.json`.** No `scripts`, so no `vscode:prepublish` or install hooks. `activationEvents` is
  `onStartupFinished` (activates in every trusted window, starting `run :lsp` even in workspaces with no
  runfiles: hygiene, and the reason `lspPath` fires at startup). `devEngines.runtime` with
  `onFail: download` is pinned in `pnpm-lock.yaml` to `node@runtime:24.21.0` with a sha256 per platform
  tarball from nodejs.org, and pnpm itself is pinned through `packageManager` with integrity hashes.
- **`.vscodeignore`** excludes `src/**`, `node_modules/**`, `**/*.map`, compiled tests and the lockfile,
  so no source maps or dependencies ship. It does ship the extension's own `runfiles/*.run` (harmless build
  recipes). A locally built `.vsix` (`editors/vscode/runfile-vscode.vsix`, 2026-09-14) also contains a
  stale `out/runfileParser.js` from a source file deleted in 81f5f26, because nothing cleans `out/` before
  `tsc`. Release builds start from a clean checkout, so they are unaffected. Hygiene: `rm -rf out` in
  `compile.run`, or an `out/**` allowlist.
- **`scanner.c` memory safety** (review + experiment 5). `MAX_INDENT` = 64 and every write to
  `line_indent` is guarded by `< MAX_INDENT`. Every length is clamped to 64 before `at_terminator` indexes
  `exec_indent`, and both `memcpy`s copy exactly 64 between two 64-byte arrays. `serialize` writes
  `sizeof(Scanner)` = 131 bytes, within the 1024-byte `TREE_SITTER_SERIALIZATION_BUFFER_SIZE`. `deserialize`
  copies only on an exact length match and zeroes otherwise. Its input is only ever the scanner's own
  earlier output, never file bytes. The end-of-file zero-width newline is emitted once, guarded by
  `eof_newline_emitted`. Every other token consumes at least one character. `skip_interpolation` and the
  string loop inside it stop at end of line. `stdlib.h` (for `calloc`) comes in through `parser.h`.
  Logic-only nits: the `uint8_t` indent counters wrap at 256 blanks, so an `exec` opened at 256+k blanks of
  indentation closes on a k-blank `end` (a mis-highlight, no memory effect). `calloc` is unchecked (OOM
  only). With every symbol valid (error recovery), `scan_exec_content` claims ordinary text as body (parse
  quality only).
- **`queries/injections.scm`** takes `@injection.language` from the `structured_format` node, which the
  scanner only ever produces as `json`, so a file cannot pick an arbitrary injected language. The bash
  injections are fixed strings. `highlights.scm` is declarative.
- **Package and config files.** `tree-sitter.json`, `.gitattributes` and `.gitignore` are clean.
  `package.json` pins `tree-sitter-cli` exactly (0.25.6), and `pnpm-workspace.yaml` approves its install
  script. That script's unverified binary download is already recorded by the dependencies agent.
- **The editors' runfiles** (read, not run). `setup` is `pnpm install --frozen-lockfile` in both.
  `compile`/`test`/`watch` run the local `tsc`/`node`. `install` packages and then
  `code --install-extension runfile-vscode.vsix --force`, and `uninstall` removes it. `package` uses the
  unpinned `npx @vscode/vsce`, which is existing finding 4. tree-sitter `generate`/`test` run the
  lockfile-pinned CLI, and `test` parses every `.run` file under the repo with `find -exec`. `_shared.run`
  in both only adds `node_modules/.bin` to PATH.

## Live lookups (all retrieved 2026-10-03)

- https://code.visualstudio.com/api/extension-guides/workspace-trust: an extension that declares
  nothing "will be treated as not supporting Workspace Trust. It will be disabled when a workspace is in
  Restricted Mode"; `restrictedConfigurations` makes VS Code return only the user value in Restricted Mode;
  VS Code prevents tasks and debugging in Restricted Mode.
- https://code.visualstudio.com/docs/editing/workspaces/workspace-trust: a new folder opens in Restricted
  Mode with a banner; `security.workspace.trust.startupPrompt` defaults to `never`; Restricted Mode disables
  "AI agents, terminal, tasks, debugging, workspace settings, and extensions"; trusting a parent trusts all
  subfolders.
- https://code.visualstudio.com/api/references/contribution-points: default configuration scope is
  `window`; `machine` "can be set only in user settings or only in remote settings".
- https://raw.githubusercontent.com/microsoft/vscode/main/src/vs/workbench/contrib/tasks/browser/task.contribution.ts:
  `task.allowAutomaticTasks` now has `default: 'off'`, `scope: ConfigurationScope.APPLICATION`,
  `restricted: true`. VS Code core itself no longer lets a trusted repository's configuration run a
  command on folder open.
- VS Code `terminalTaskSystem.ts`, `mainThreadTask.ts`, `api/node/extHostTask.ts`,
  `baseConfigurationResolverService.ts`, `variableResolver.ts`, `vscode.d.ts`,
  `textMateTokenizationSupport.ts` (all at raw.githubusercontent.com/microsoft/vscode/main/...): quoting
  rules, variable collection and resolution, the 500 ms tokenization cap.
- https://raw.githubusercontent.com/libuv/libuv/v1.x/src/win/process.c: `search_path` checks the
  explicit `cwd` first for a bare filename, *if* `NeedCurrentDirectoryForExePathW(L"")` (that is, unless
  `NoDefaultCurrentDirectoryInExePath` is set), then PATH, trying `.com` then `.exe`.
- https://raw.githubusercontent.com/tree-sitter/tree-sitter/master/lib/src/lexer.c: `ts_lexer__get_column`
  goes back to the start of the line when its cache is invalid.
- https://raw.githubusercontent.com/neovim/neovim/release-0.11/runtime/doc/news.txt: "Treesitter
  highlighting is now asynchronous".
- https://raw.githubusercontent.com/helix-editor/helix/master/helix-core/src/syntax.rs:
  `PARSE_TIMEOUT` = 500 ms.

## Notes on existing findings

**Existing editors finding 1: target names reach the task shell unquoted when `runfile.interactive` is
false.** Holds; low, confirmed. Re-reproduced (experiment 2). One sentence in its attack path was wrong and
I corrected it in place: it said a name containing a space "was single-quoted and inert". It is not inert
when the name also contains a `'`, because VS Code's strong quoting does not escape an inner quote
(reproduced, `Q1`/`Q2`). So no character restriction short of "no `'` and no space-free metacharacters"
makes the plain-string path safe, and the recommended fix (ProcessExecution, or ShellQuotedString with
strong quoting) is still right. I also added a pointer to new finding A. Execution happens only when the
task is run (click), never on provision: `provideTasks` only constructs Task objects, and VS Code resolves
and builds the command line in `_executeCommand`. PowerShell and cmd quoting were not reproduced.

**Existing editors finding 2: `catalogCommand`/`lspPath` are window-scoped.** Holds; needs a trusted
workspace, per the live docs above, so it does not fire on merely opening an untrusted folder. Low is
defensible. Three points add weight, and the main agent may want medium:

- VS Code core has moved the comparable setting for its own tasks out of the repository's reach:
  `task.allowAutomaticTasks` defaults to `off` and is application-scoped and restricted. Trust alone no
  longer runs a repository-chosen command without a user action in core VS Code, but this extension's
  setting still does. `catalogCommand` runs when the Runfile view renders, including on window restore if
  the view was open, on Run Task / Run Build Task, and on any `runfile.*` config change while the view is
  visible. `lspPath` runs at activation (`onStartupFinished`).
- The carrier needs no file in the repository: `catalogCommand` is split on whitespace and `execFile`d
  with whatever arguments it names, so `sh -c <payload-without-spaces>` or `node -e <expr>` works with the
  system's own interpreters.
- On Windows `lspPath` can be an absolute UNC path to a remote executable (`\\host\share\run.exe`), which
  removes its dependence on the extension host's cwd. This is possible, not tested.

The fix stays `"scope": "machine"` on both, plus an explicit `capabilities.untrustedWorkspaces` and
`restrictedConfigurations`. The extension uses neither today; `machine` scope is what VS Code's `git.path`
uses.

**Existing editors finding 3: Windows bare `run` resolved from the workspace folder.** Holds as low,
likely. I re-read the current libuv source: the cwd-first search still applies, but only when
`NeedCurrentDirectoryForExePathW` says so, so a user with `NoDefaultCurrentDirectoryInExePath` set is not
affected. It adds little beyond finding 2 (trust is still required), but it needs no settings file, only a
PE at the repository root. The non-interactive `ShellExecution` path goes through the user's shell: the
default PowerShell does not search the current directory, cmd does.

**Existing editors finding 4: unpinned vsce.** Holds, but it duplicates
`dependencies.jsonl`'s "Published .vsix is packaged by an unpinned `npx @vscode/vsce`…" and
`supply-chain-ci.jsonl`'s medium of the same name. I left it for the main agent to merge, as instructed.
Its line references (package.run:7, release.yml:191-195, github release.yml:216-220, package.json) are
correct.

**Other agents' findings that the extension makes reachable.**

- `untrusted-repo`'s and `cli`'s `run :list` DoS findings (FIFO named `*.run`, symlink-loop traversal,
  unbounded `read_to_string`) are reached in VS Code, in a trusted workspace, whenever the Runfile view
  renders, and through `run :lsp` at startup.
- `catalog.ts` sets no `timeout` on `execFile`, so each refresh against a hanging `run :list` leaves one
  more process running forever and the tree spinning. Extension-side hardening: `timeout: 15_000` with
  `killSignal: "SIGKILL"`.
- The README tells Neovim/Helix/Zed users to run `run :lsp` as the language server. Those editors (Neovim
  and Helix at least) start it on opening a `.run` file with no trust prompt at all, so anything
  `runfile-lsp` does on `didOpen` (discovery, `_shared.run` chain, name resolution, checks) is reachable
  from opening an untrusted repository there.

## Open questions

1. Finding A and finding 3 were traced in source, not run in VS Code or on Windows. A manual check needs
   five minutes: make `runfiles/${command/workbench.action.reloadWindow}.run` in a trusted folder and click
   its Run button.
2. Does any commonly installed extension call `vscode.tasks.fetchTasks()` unfiltered at startup? If so,
   finding 2's `catalogCommand` fires on open with no click at all.
3. Zed: does it gate language servers or highlighting behind a project-trust prompt, and does it set a
   parse timeout? Not checked.
4. Should discovery refuse target names outside a conservative character set? That would close finding 1,
   finding A, `cli`'s bash-completion high and the JetBrains `:generate` injection at the source. It is the
   main agent's call, since it is a cross-component behaviour change.

FILES READ:
security-audit-2026-10-03/00-project-map.md
security-audit-2026-10-03/findings/editors.jsonl
CLAUDE.md
README.md
editors/vscode/package.json
editors/vscode/.vscodeignore
editors/vscode/tsconfig.json
editors/vscode/language-configuration.json
editors/vscode/pnpm-lock.yaml
editors/vscode/src/extension.ts
editors/vscode/src/catalog.ts
editors/vscode/src/codeLens.ts
editors/vscode/src/lsp.ts
editors/vscode/src/pure.ts
editors/vscode/src/pure.test.ts
editors/vscode/src/grammar.test.ts
editors/vscode/syntaxes/runfile.tmLanguage.json
editors/vscode/syntaxes/runfile-interpolation.injection.json
editors/vscode/runfiles/_shared.run
editors/vscode/runfiles/compile.run
editors/vscode/runfiles/install.run
editors/vscode/runfiles/package.run
editors/vscode/runfiles/setup.run
editors/vscode/runfiles/test.run
editors/vscode/runfiles/uninstall.run
editors/vscode/runfiles/watch.run
editors/vscode/runfile-vscode.vsix
editors/vscode/out/
editors/tree-sitter/.gitattributes
editors/tree-sitter/.gitignore
editors/tree-sitter/grammar.js
editors/tree-sitter/package.json
editors/tree-sitter/pnpm-workspace.yaml
editors/tree-sitter/tree-sitter.json
editors/tree-sitter/queries/highlights.scm
editors/tree-sitter/queries/injections.scm
editors/tree-sitter/runfiles/_shared.run
editors/tree-sitter/runfiles/generate.run
editors/tree-sitter/runfiles/setup.run
editors/tree-sitter/runfiles/test.run
editors/tree-sitter/src/scanner.c
editors/tree-sitter/src/tree_sitter/parser.h
editors/tree-sitter/src/parser.c
editors/tree-sitter/test/corpus/
crates/runfile-lsp/src/server.rs
crates/runfile-lsp/src/analysis.rs
.cicd/release.yml
.github/workflows/release.yml
runfiles/
