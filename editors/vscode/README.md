# Runfile

Contributes VS Code tasks that are **generated at runtime** instead of being
written to `tasks.json`.

When a workspace folder is open, the extension runs a shell command, reads a
**task descriptors** JSON document from its **stdout**, and surfaces each target
as a real task in **Terminal → Run Task…**. The tasks behave exactly like ones
authored in `tasks.json` — they can be bound to keybindings, used as a
`preLaunchTask`, and so on — but nothing is ever written to disk.

By default the command is:

```sh
run :generate task-descriptors
```

which asks the [`run`](https://github.com/JoaaoVerona/runfile) CLI to describe
every runnable target — the workspace's own, everything pulled in via `includes`,
and the machine-wide global targets — in one editor-agnostic JSON document.

## Installation

The extension is not on the Marketplace; every Runfile release ships a `.vsix`.

1. Install the [`run`](https://github.com/JoaaoVerona/runfile) CLI and make sure
   it is on your `PATH` (`run --version`).
2. Download `runfile-vscode-<version>.vsix` from the
   [latest release](https://github.com/JoaaoVerona/runfile/releases/latest).
3. Install it:

   ```sh
   code --install-extension runfile-vscode-<version>.vsix
   ```

   Or, from VS Code: **Extensions → … → Install from VSIX…**

To build and install it from a checkout instead:

```sh
run vscode:deps      # once, installs the dev dependencies (pnpm)
run vscode:install   # compile → package → code --install-extension --force
```

## How it works

- A [`TaskProvider`](https://code.visualstudio.com/api/extension-guides/task-provider)
  is registered for the `runfile` task type.
- Whenever VS Code fetches tasks (e.g. you open **Run Task**), the provider runs
  the configured command **once per workspace folder**, with that folder as the
  working directory.
- stdout is parsed as the task-descriptors JSON, and each target becomes a
  `vscode.Task` that runs `run --stdin-args <name>` in the workspace folder.
  The target's `description` becomes the task's hover/inline detail, and its
  `namespace` / source `kind` drive how the sidebar groups it.

## Targets sidebar

The extension contributes a **Runfile** view container to the activity bar with a
**Targets** tree that lists every target the provider would contribute — the same
generation, honoring the same settings, so the sidebar and the **Run Task** list
never disagree. Click a target (or its inline ▶ button) to spawn it as a task,
exactly as if you'd picked it from Run Task; each row shows the target's
description as hover/inline detail.

**Namespaced targets are grouped into a folder per namespace.** A target named
`run api:build` lands under an `api` folder shown as `build`, matching how
`run :list` presents included/namespaced targets. Grouping uses the generator's
real namespace list, so a target merely *named* with a colon (e.g. `all:package`)
stays a top-level target rather than inventing an `all` folder. Un-namespaced
targets stay at the top level, and in a multi-root workspace everything is first
grouped under its workspace folder.

The view's title bar has one button:

- **↻ Refresh** — re-runs generation.

## Pinned targets

Right-click any target (or use its inline 📌 button) and choose **Pin Target** to
keep it at hand. Pinning **moves** the target to the very top of the tree, listed
loose — no wrapper folder to expand, and no copy left behind further down.

Because a pin is lifted out of its namespace folder, the label becomes the full
canonical name (`api:build`, not `build`) — the folder that used to supply the
prefix is no longer there to do it. A namespace or workspace folder whose targets
are *all* pinned disappears entirely, which is the point: there's nothing left to
scroll past. **Unpin Target** drops the target back where it belongs.

Pins live in the workspace's own state, so they're per-project and travel with
neither your settings nor your repository. In a multi-root workspace a pin is
scoped to the folder it runs in, so the same target name in two roots pins
separately.

## Global targets

The **global** targets you've registered with `run :config global-files` — the
machine-wide targets that `run :list` folds in — are always surfaced under a
**Globals** folder, shown as the **last item at the tree root**. Its position is
fixed: the folder stays put even when nothing is registered (it just expands to
nothing).

Globals are told apart from a folder's own targets by the `kind` each descriptor
source carries (`local` / `included` / `global`), so a single generation is
enough — nothing is re-derived from target names. The same machine-wide targets
appear once per workspace folder, so they are deduplicated by name before the
folder is built.

**Directory-scoped globals are not treated as globals.** A global Runfile that
declares `globals.onlyInDirectories` only merges in while you're inside one of
those directories — registered machine-wide, but in practice scoped to certain
projects. Burying those targets in **Globals** would misrepresent them, so the
extension puts them in the main tree exactly as if they came from a local
Runfile. The descriptor reports the restriction via `onlyInDirectories` on the
source; this needs a `run` CLI new enough to emit it (older ones simply keep the
old behaviour of filing them under **Globals**).

## No caching by design

The generate command is cheap, so the extension keeps **no cache** — it is
re-run every time VS Code asks for tasks, and the results are always fresh. VS
Code itself re-invokes the provider when the Run Task list is opened; if you ever
need to force a regeneration, run **Runfile: Refresh Targets** from the Command
Palette (or click **↻** in the Targets view).

## Settings

| Setting | Default | Description |
| --- | --- | --- |
| `runfile.generateCommand` | `run :generate task-descriptors` | Shell command run in each workspace folder to produce the task-descriptors JSON on stdout. |
| `runfile.enabled` | `true` | Turn off to stop contributing tasks (and stop running the command). |
| `runfile.interactive` | `true` | Run `--stdin-args` tasks through an interactive pseudoterminal so `run` can prompt for missing arguments. |

## Commands

| Command | Description |
| --- | --- |
| **Runfile: Refresh Targets** | Force a fresh regeneration of the task list and sidebar. |
| **Runfile: Run Target** | Spawn a target as a task (used by the sidebar's rows and ▶ buttons). |
| **Runfile: Pin Target** | Move a target to the top of the tree. |
| **Runfile: Unpin Target** | Send a pinned target back to its normal place. |
| **Runfile: Show Log** | Open the output channel (useful when the command fails or emits invalid JSON). |

Pin/Run/Unpin act on a sidebar row, so they're only offered from the tree (not
the Command Palette).

## Expected stdout format

The task-descriptors document emitted by `run :generate task-descriptors`:

```jsonc
{
  "formatVersion": 1,
  "sources": [
    {
      "filePath": "/abs/path/Runfile.json",
      "kind": "local",              // "local" | "included" | "global"
      "onlyInDirectories": ["~/w"], // present only on directory-scoped globals
      "targets": [
        {
          "name": "api:build",      // full canonical invocation name
          "namespace": "api",       // omitted for un-namespaced targets
          "description": "Build the API"
        }
      ]
    }
  ]
}
```

`local`/`included` targets are grouped by namespace in the sidebar; `global`
targets land in the trailing **Globals** folder — unless the source carries
`onlyInDirectories`, in which case it is treated as local (see
[Global targets](#global-targets)). Log output should go to **stderr** so it does
not corrupt the JSON on stdout.
