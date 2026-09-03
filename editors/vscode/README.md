# Runfile for VS Code

Run buttons, tasks and syntax highlighting for [Runfile](https://github.com/JoaaoVerona/runfile) projects.

## What it does

**A Run button on every target.** A `.run` file inside a `runfiles/` directory *is* a target, so the button is
placed from the path alone — no parsing, and it works whether or not `run` is on your `PATH`.

**Real VS Code tasks.** Every target appears in *Run Task*, without anything being written to `tasks.json`.

**A sidebar tree.** Targets grouped by namespace, with subproject and machine-wide targets in their own
sections. Pin the ones you reach for and they move to the top.

**Syntax highlighting**, with `$` lines and `exec` bodies highlighted as the shell they are.

**Diagnostics as you type**, when [`runfile-lsp`](https://github.com/JoaaoVerona/runfile) is installed — from
the same parser `run` itself uses, so the editor never disagrees with what will actually happen. Shell lines are
handed to [shellcheck](https://www.shellcheck.net) when it is available.

## How targets are found

The extension asks the CLI:

```
run :list --json
```

It never reads `.run` files itself, so the tree, the tasks and the run buttons cannot drift from what `run`
would actually do. The command runs in each workspace folder every time VS Code fetches tasks — there is no
cache, deliberately.

Discovery is the CLI's job: it walks up for the nearest `runfiles/`, down for subprojects, and folds in
`~/.runfiles/`. Hidden targets (`.hide`) are left out, the same as in `run :list`.

### Anchoring

A run button passes `--dir`, set to the directory that owns the file's `runfiles/`. That is what makes it
correct inside a subproject: `compile` in `web/runfiles/compile.run` is `web:compile` from the repository root,
so the name as the file spells it only means something against that directory.

`_shared.run` gets no button — it configures a directory rather than being a target.

## Interactive prompts

Tasks run as `run --stdin-args <target>`, so a target missing an `{{ ARG.x }}` asks for it instead of failing.
Answering needs a terminal that can accept input, so tasks run inside a pseudoterminal the extension controls.
Turn `runfile.interactive` off to use a plain shell task instead.

## Settings

| Setting | Default | |
| --- | --- | --- |
| `runfile.enabled` | `true` | Contribute tasks at all. |
| `runfile.catalogCommand` | `run :list --json` | How the target list is obtained. |
| `runfile.codeLens` | `true` | Show the inline Run button. |
| `runfile.interactive` | `true` | Run tasks in a pseudoterminal so prompts work. |
| `runfile.lsp` | `true` | Report errors as you type. |
| `runfile.lspPath` | `runfile-lsp` | Path to the language server. |

## Commands

*Runfile: Refresh Targets*, *Run Target*, *Pin Target*, *Unpin Target*, *Show Log*.

If targets are missing, *Show Log* has the failing command and its stderr.

## Requirements

The `run` CLI on your `PATH`. `runfile-lsp` as well, for diagnostics.

## License

MIT
