# Runfile

[Quick start](#quick-start) · [The language](#the-language) · [Why](#why-a-language) · [Editors](#editor-support) · [Platforms](#platform-support)

**One directory. One binary. Every OS.**

A command runner that replaces Makefiles, shell scripts and `npm run` — without the platform headaches.

```bash
$ run dev --port=4000
```

Tasks live in a `runfiles/` directory, one target per file:

```
runfiles/
  _shared.run      settings every target here inherits
  dev.run          → run dev
  build.run        → run build
  api/deploy.run   → run api:deploy
```

```sh
# runfiles/build.run
# Type-check and build

.parallel = true
.env-file = ".env"

run type-check
$ vite build
```

```sh
# runfiles/dev.run
# Start the dev server

.env.PORT = ARG.port ? "3000"

$ vite
```

That is the whole idea: **a target is a file**, its first comment block is its description, and its name is its
path.

## Quick start

```bash
curl -fsSL https://github.com/JoaaoVerona/runfile/releases/latest/download/install.sh | sh
run :init
run hello
```

```bash
run :list                 # every target, with descriptions
run build --env=prod      # arguments
run build --dry-run       # print what would run, without running it
```

## The language

Line-oriented, with one rule: **the language is the default, the shell is marked.**

| Line | Meaning |
| --- | --- |
| `# text` | A comment. The leading block is the target's description. |
| `.name = value` | A property. |
| `$ echo hi` | Hand this line to a shell. |
| `exec python3` … `end` | Run a command with the block as its stdin. |
| `let x = 1` | Bind a value. `x = 2` rebinds. |
| `if` / `else` / `end` | Branch. |
| `for x in list` / `end` | Loop. |
| `match` / `case` / `default` / `end` | Dispatch on a value. A `case` label is a quoted string: `case "linux"`. |
| `retry n [every s]` … `end` | Run the block again while it fails, up to `n` times. |
| `run other-target` | Run another target, in this process. |

```sh
# Deploy to an environment

let env = one_of(first(ARGS), "staging", "production")

if env == "production"
	.confirm = "Deploy to production?"
end

$ docker build -t app:{{ env }} .
$ docker push app:{{ env }}
```

### Values

Strings, numbers, booleans and lists — **strict, with no coercion.** `"a" + 1` is an error telling you to use
`concat`. `"1" == 1` is false. `number(ARG.count)` is required before arithmetic. `1 / 2` is `0.5`.

```sh
let parts = split("1.2.3", ".")
let major = number(parts[0])
$ echo "next is {{ major + 1 }}"
```

### Where values come from

| Source | From |
| --- | --- |
| `ARG.name` | `--name=value` |
| `FLAG.name` | `--name` (a boolean) |
| `ARGS` | positional arguments, as a list |
| `ENV.NAME` | the environment |
| `RUN.os` `RUN.arch` `RUN.cwd` `RUN.file` `RUN.parent` `RUN.namespaces` | the run itself |

`a ? b` means "`a`, or `b` if `a` is not there":

```sh
let port = ARG.port ? ENV.PORT ? "3000"
```

Arguments are `--key=value`. Writing `--key value` gives you a flag and a positional, because nothing declares
which names take values — and if you meant an argument, the error says so.

Everything after a bare `--` is passed through untouched, flags included. That is how a wrapper forwards a
command line it does not understand:

```sh
# runfiles/aws.run
$ docker run --rm amazon/aws-cli {{ ARGS }}
```

```bash
run aws -- s3api list-buckets --output json
```

Forget the `--` and `--output` would be read as a flag for the target instead. `run` warns when a target is
handed a flag it never reads, so that mistake does not pass silently.

### Interpolation quotes itself

`{{ … }}` in a shell line becomes **exactly one argument** — or, for a list, one argument per item. So this is
safe with any file name, spaces and quotes included:

```sh
$ cp {{ ARG.src }} {{ ARG.dest }}
```

**Never wrap an interpolation in shell quotes.** There is no `shell_quote` function because there is nothing to
quote: the substitution already did it.

### Asking whether a command worked

A `$` run may stand as a condition or as a `match` subject. The condition is true when the command exits 0;
the cases are exit codes.

```sh
if $ command -v docker
	$ docker info
else
	$ echo 'no docker here' >&2
end

match $ curl -fsS https://example.com
	case "0"
		$ echo up
	case "22"
		$ echo 'HTTP error'
	default
		$ echo 'could not reach it'
end
```

Neither stops the target: a non-zero exit is the answer, not a failure. `code_of($ cmd)` is the same thing as
a number, for when you want to keep it:

```sh
let made = code_of($ mkdir out)
```

A `$` run reaches to the end of its line, which is why a capture cannot nest in a call — `code_of` is the one
exception, and its command may not contain a `)`.

### Waiting for something

`retry` runs its block again while it fails, up to a number of attempts, waiting between them. `else` is what
to do when it never worked; without one, the last failure is the statement's.

```sh
retry 120 every 1
	$ docker exec db pg_isready -h 127.0.0.1 >/dev/null 2>&1
else
	$ echo 'ERROR: the database did not come up' >&2
	exit(1)
end
```

The block sees its own failures whatever `.ignore-errors` says around it — a retry that could not tell would
run exactly once. An `exit()` inside is not retried: it is an instruction to stop. And a `retry` cannot sit
inside a `.parallel` block, where several bodies sleeping against each other has no useful meaning.

### Stopping early

A line that is only a value — `exit`, `abc`, `35`, `"hi"` — is a parse error, since it computes something and
throws it away. Most often it is a call with the parentheses left off, and the message says so.

`exit()` ends the run with a status — no argument means 0, and any number is taken as given and truncated to
a byte the usual way, so `exit(-1)` is 255. Like every call it is written with parentheses; there is no
bare-word form.

```sh
if !file_exists(".env")
	$ echo 'no .env here' >&2
	exit(1)
end
```

Nothing catches it: not `try`, not a `?` fallback, not `.ignore-errors`. A target may forgive a command that
failed, but being told to stop is not that.

### Temporary files

`temp_file` and `temp_dir` make something in the OS temp directory and hand back the path. Both are deleted
when the run ends, however it ends, so a target that fails half-way does not leave a decoded secret behind:

```sh
.env.GOOGLE_APPLICATION_CREDENTIALS = temp_file(base64_decode(ENV.SERVICE_ACCOUNT_B64), "json")

$ fastlane upload
```

### Structured text

`json … end` is a block of JSON, as a value. An interpolation inside it renders as **one JSON value** — the
same rule as a shell line, one layer up — so nothing has to be escaped by hand:

```sh
let policy = json
	{
	  "Version": "2012-10-17",
	  "Statement": [{ "Effect": "Allow", "Resource": {{ buckets }} }],
	  "MaxKeys": {{ number(ARG.limit) }}
	}
end

run aws -- iam put-user-policy --policy-document {{ policy }}
```

A string becomes a quoted, escaped string; a number becomes a number; a list becomes an array. **Do not put
quotes around an interpolation** — `"{{ x }}"` is wrong here for the same reason it is wrong in a `$` line.

The block is checked while the file is read, with each interpolation standing in as a value, so a missing
brace is an error in the editor rather than one the far end reports later.

### Capturing output

```sh
let staged = $ git diff --cached --name-only
let files = lines(staged)

if length(files) != 0
	$ rustfmt {{ files }}
end
```

A list interpolates as separate arguments, so `{{ files }}` passes each file as its own.

### Blocks that are not shell

```sh
exec python3
	import json, sys
	print(json.dumps({"ok": True}))
end
```

The body is the command's stdin. `$ line` is shorthand for a one-line `exec` with the default shell.

## Properties

Set on the file, or inside a block where marked.

| Property | Effect |
| --- | --- |
| `.shell` | Which shell `$` lines use. |
| `.env.NAME` | Set an environment variable. |
| `.env-file` | Load a `.env` file (encrypted values are decrypted in memory). |
| `.add-path` | Prepend a directory to `PATH`. |
| `.workdir` | Where commands run. |
| `.parallel` | Run this block's commands at once. On a `for`, its iterations. |
| `.ignore-errors` | Keep going when a command fails. |
| `.logging` | Announce each command on stderr before it runs. Off unless set. |
| `.confirm` | Ask before running. |
| `.watch` | Re-run when matching files change. |
| `.alias` | Another name for this target. |
| `.hide` | Keep out of `run :list`. |
| `.detach` | Start the commands and do not wait. |
| `.only-in-directories` | For the machine-wide directory: only offer these targets inside these directories. |

`shell`, `env`, `workdir`, `parallel` and `ignore-errors` may be set inside an `if` / `for` / `match` block. The
rest belong at the top of the file.

`_shared.run` holds properties every target in its directory inherits. A nested one layers over the directory
above it, so `runfiles/api/_shared.run` adds to `runfiles/_shared.run`.

## How targets are found

`run` walks **up** from the working directory for the nearest `runfiles/`, then **down** one level for
subprojects. A nested directory becomes a namespace:

```
runfiles/build.run          → run build
web/runfiles/build.run      → run web:build
runfiles/api/deploy.run     → run api:deploy
```

`$HOME/.runfiles/` holds machine-wide targets, available in every project. If you would rather see the
directory than hide it, `$HOME/runfiles/` and `$HOME/Runfiles/` are read too — but only one of the three may
hold anything. Two populated ones is an error naming both, because a merge would let one target shadow
another with no way to see it.

Inside a subproject, `run build` means *that* subproject's `build` — so a file reads the same wherever you
invoke `run` from.

Everything relative — `.env-file`, `.add-path`, `glob`, `read_file`, `{{ RUN.parent }}`, the working directory
— resolves against **the parent of `runfiles/`**. One anchor, one rule.

## Setup targets

Name a target `setup` and every other target in its directory refuses to run until it has:

```
$ run build
error: `setup` has never been run

    run setup
```

It re-triggers when `setup.run` itself changes, so a new dependency is not silently missed. `--dry-run` is
exempt, CI is exempt, and `RUNFILE_SKIP_PREPARE=1` bypasses it.

## Commands

| Command | |
| --- | --- |
| `run <target> [args…]` | Run a target |
| `run :list` | List every target (`--names`, `--json` for tooling) |
| `run :init` | Create `runfiles/` with an example |
| `run :format [path…]` | Format runfiles in place (`--check`, `--stdout`) |
| `run :env <sub>` | Manage `.env` files: `init`, `get`, `set`, `encrypt`, `decrypt`, `rotate`, `inject`, `secret-keys` |
| `run :completions <command>` | `install`, `uninstall` or `output` a completion script |
| `run :generate <editor>` | Task files for `zed`, `jetbrains` or `vscode`, merged into what is there |
| `run :update` | Update the binary |

`run :completions install` puts bash and fish completions in the directory each shell loads on demand, and
adds a line to `.zshrc` or PowerShell's profile for the other two. Open a new shell afterwards.

`run :format` has no settings: one shape, everywhere. It reindents with tabs, spaces expressions, places
blank lines (after the description, around a run of `let`s, before a block, after an `end`), and leaves the
three things that are not the language's to touch — strings, the text after `$ `, and `exec` bodies — exactly
as written. It refuses a file that does not parse, and checks that its own output still means the same thing
before writing it. `--check` reports what would change and exits 1, which is what a CI step wants.

The language server offers the same formatting, so **format-on-save works in any editor with the extension or
the LSP configured** — in VS Code, `"editor.formatOnSave": true`. A file is never reformatted while it does
not parse.

| Flag | |
| --- | --- |
| `-y`, `--yes` | Skip confirmation prompts |
| `--stdin-args` | Prompt for anything a target needs but was not given |
| `--dry-run` | Print what would run, without running it |
| `--dir <path>` | Start discovery somewhere else |
| `-h`, `--help` | Show the help for `run` or any of its commands |
| `-v`, `--version` | Print the version |

Flags belong **before** the target name; everything after it is passed to the target.

## Encrypted environment variables

Values in a `.env` file can be encrypted individually, so the file stays readable and diffable:

```
DATABASE_URL=encrypted:AAAA...
PUBLIC_HOST=example.com
```

```bash
run :env init .env.production          # create, with a fresh key
run :env set .env.production KEY value # encrypts on write
run :env get .env.production KEY       # decrypts on read
run :env rotate .env.production        # re-encrypt under a new key
```

Keys live in the OS credential store — Keychain, Credential Manager, or Secret Service with a keyutils
fallback. In CI, pass them as `RUNFILE_PRIVATE_KEYS` (newline-separated) and no credential store is involved.

Decryption happens in memory; secrets never reach disk. The credential store is only touched when something
actually decrypts, so a locked keyring never gets in the way of an unrelated target.

## Why a language

The alternative is a config format plus an escape hatch into shell, and every version of that ends up encoding
control flow in JSON or YAML — or giving up and shipping shell scripts that only run on one platform.

A small language means `if`, `for` and `match` read as themselves; values have types, so `"a" + 1` is a mistake
rather than `"a1"`; and interpolation can be safe by construction, because the language knows a string is one
argument.

Shell is still there. It is just marked.

## How it compares

| | Runfile | Make | Just | Taskfile |
| --- | :-: | :-: | :-: | :-: |
| One target per file, so a shared task file never conflicts | ✅ | ❌ | ❌ | ❌ |
| Encrypted env vars, built-in (AES-256-GCM) | ✅ | ❌ | ❌ | ❌ |
| Inline OS / shell / cwd branching via `RUN.*` | ✅ | ❌ | ❌ | ❌ |
| Editor diagnostics from the runner's own parser | ✅ | ❌ | ❌ | ❌ |
| IDE task generation (VS Code / Zed / JetBrains) | ✅ | ❌ | ❌ | ❌ |
| Per-target shell override | ✅ | ❌ | ✅ | ❌ |
| Any interpreter for a block (`exec python3`) | ✅ | ❌ | ✅ | ❌ |
| Strict parsing (typos are errors) | ✅ | ❌ | ✅ | ❌ |
| Argument substitution with chained fallbacks | ✅ | ❌ | ✅ | ❌ |
| Watch mode, built-in | ✅ | ❌ | ❌ | ✅ |
| Shell completions | ✅ | ❌ | ✅ | ✅ |
| Hidden targets | ✅ | ❌ | ✅ | ✅ |
| Built-in functions (strings, regex, paths, JSON, hashes, time) | ✅ | ❌ | ✅ | ✅ |
| Parallel execution | ✅ | ✅ | ❌ | ✅ |
| Single static binary | ✅ | ✅ | ✅ | ✅ |
| Native Windows binary (`$` lines use Git Bash) | ✅ | ❌ | ✅ | ✅ |
| First-class PowerShell / cmd.exe | ❌ | ❌ | ✅ | ❌ |
| Output prefixing in parallel mode | ✅ | ❌ | ❌ | ✅ |
| Pattern rules (`%.o: %.c`) | ❌ | ✅ | ❌ | ❌ |
| Preconditions / status checks | ❌ | ❌ | ❌ | ✅ |
| Incremental builds (sources / timestamps / checksums) | ❌ | ✅ | ❌ | ✅ |

## Editor support

The VS Code extension gives you a Run button on every target, a task provider, a sidebar tree, and syntax
highlighting. Install the `.vsix` from the [latest release](https://github.com/JoaaoVerona/runfile/releases).

`runfile-lsp` provides diagnostics, completion, hover and go-to-target as you type — from the same parser `run` itself uses, so it never disagrees
with what will actually happen. It also hands `$` lines and shell `exec` bodies to
[shellcheck](https://www.shellcheck.net) when it is installed, mapping findings back to the lines you wrote.

**JetBrains IDEs** read the same grammar: *Settings → Editor → TextMate Bundles*, add the `editors/vscode`
directory from a checkout, and `.run` files highlight.

**Zed, Neovim and Helix** use the tree-sitter grammar in `editors/tree-sitter`, with highlight queries
included. For Neovim with nvim-treesitter, register the parser from this repository and copy `queries/` into
your runtime path as `queries/runfile/`:

```lua
require("nvim-treesitter.parsers").get_parser_configs().runfile = {
  install_info = {
    url = "https://github.com/JoaaoVerona/runfile",
    location = "editors/tree-sitter",
    files = { "src/parser.c", "src/scanner.c" },
  },
  filetype = "runfile",
}
vim.filetype.add({ extension = { run = "runfile" } })
``` `run :generate zed` writes every target into `.zed/tasks.json`, and `run :generate jetbrains` into
`.idea/runConfigurations/`; both leave entries you wrote yourself alone and replace only their own.

## Platform support

| | |
| --- | --- |
| Linux | x86-64, arm64 |
| macOS | Intel, Apple Silicon |
| Windows | x86-64, arm64 |

`$` lines use bash where it exists, Git Bash on Windows, and `sh` otherwise — so one file works everywhere. Set
`.shell` to pin something else.

## License

MIT
