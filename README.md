# Runfile

[Quick start](#quick-start) · [Examples](#what-a-runfile-looks-like) · [Language](#the-language) · [Properties](#properties) · [Commands](#commands) · [Editors](#editor-support)

**One directory. One binary. Every OS.**

A command runner that replaces Makefiles, shell scripts and `npm run` — without the platform headaches.

```bash
run dev --port=4000
```

Your project's tasks live in a `runfiles/` directory, **one target per file**:

```
runfiles/
  _shared.run          settings every target here inherits
  dev.run              → run dev
  build.run            → run build
  check.run            → run check
  api/deploy.run       → run api:deploy
web/runfiles/build.run → run web:build
```

A target is a file, its name is its path, and its first comment block is its description. There is no central
file to merge, so two people adding a task never conflict.

```sh
# runfiles/dev.run
# Start the dev server

.env-file = ".env.local"
.env.PORT = ARG.port ? "3000"

$ vite
```

```bash
$ run :list
  build    Type-check and build
  check    Non-mutating gate
  dev      Start the dev server
```

## Quick start

```bash
curl -fsSL https://github.com/JoaaoVerona/runfile/releases/latest/download/install.sh | sh
run :init             # creates runfiles/ with an example
run :list             # every target, with descriptions
run <target> --help   # what one target does, and what it reads
```

```bash
run build --env=prod      # an argument for the target
run --dry-run build       # print what would run, without running it
run :completions install bash   # tab-completion (or zsh, fish, powershell)
```

Runner flags go **before** the target name; everything after it belongs to the target. `run build --dry-run`
passes `--dry-run` to `build` as `FLAG.dry-run` and runs it for real — so the position is the whole meaning.
`run` warns when a target is handed a flag it never reads, which is what catches that.

In CI, the setup action installs `run` and puts it on PATH, so every step after it is a target name:

```yaml
- name: Setup Runfile
  uses: JoaaoVerona/runfile/.github/actions/setup@v1

- run: run check
- run: run test
```

`@v1` is the major alias, moved to each release as it ships — so a fix arrives without editing every
workflow, and a new major never arrives unannounced. Pin `@v1.0.0` instead to hold one exact version.

## What a runfile looks like

These are complete files. Nothing is elided.

### One command that works on three operating systems

`match` on `RUN.os` instead of shipping three scripts and a wrapper.

```sh
# runfiles/setup/system.run
# Install the OS-level build prerequisites

match RUN.os
	case "linux"
		$ sudo apt-get update
		$ sudo apt-get install -y build-essential libssl-dev pkg-config
	case "mac"
		$ xcode-select --install || true
		$ brew install openssl pkg-config
	default
		print("No system setup for {{ RUN.os }} — see docs/prerequisites.md")
end
```

### A gate that names the file that failed

The loop is the language's, so a failure reports the one command that broke rather than the whole script.

```sh
# runfiles/check.run
# Non-mutating gate: every compose file is valid

for compose in glob("**/docker-compose.yml")
	$ docker compose -f {{ compose }} config -q
end

print("check: every docker-compose.yml is valid")
```

```
[runfile] error: `docker compose -f observability/loki/docker-compose.yml config -q` exited with status 1
```

### Waiting for something to come up

`retry` replaces the `until … do sleep … done` loop, and counts the attempts for you.

```sh
# runfiles/db/wait.run
# Block until Postgres answers, then load the seed data

retry 120 every 1
	$ docker exec db pg_isready -U app -h 127.0.0.1
else
	error("the database never became ready")
end

$ docker exec -i db psql -U app < seed.sql
```

### Doing several things at once

`.parallel` on a `for` fans out its iterations, and labels every line so you can tell them apart.

It is block-scoped, so **where it sits is what it covers**: inside the loop it parallelises the iterations and
nothing else, and at the top of the file it would cover every command in the target.

```sh
# runfiles/pull.run
# Pull every service image

for compose in glob("**/docker-compose.yml")
	.parallel
	.ignore-errors

	$ docker compose -f {{ compose }} pull
end
```

```
[docker] api      Pulling
[docker] web      Pulling
[docker] postgres Pull complete
```

### JSON without the backslashes

`json … end` is a block of JSON as a value. An interpolation inside becomes **one JSON value** — quoted and
escaped if it is a string, an array if it is a list — so nothing is escaped by hand.

```sh
# runfiles/bucket/policy.run
# Apply the storage policy to a bucket

let bucket = one_of(ARG.bucket, "app-uploads", "app-backups")

let policy = json
	{
		"Version": "2012-10-17",
		"Statement": [
			{
				"Effect": "Allow",
				"Action": [
					"s3:PutObject",
					"s3:GetObject"
				],
				"Resource": {{ concat("arn:aws:s3:::", bucket, "/*") }},
				"MaxKeys": {{ number(ARG.limit ? "1000") }}
			}
		]
	}
end

run _aws -- s3api put-bucket-policy --bucket {{ bucket }} --policy {{ policy }}
```

`run :format` lays the block out, and a missing brace is underlined in your editor as you type — not reported
by the far end an hour later.

### Reading JSON without jq

`json_query` walks a path and answers with a **list**, where `[]` descends into every element the way jq's
`.[]` does. What comes back is one of the language's own lists — something to loop over and count, not text to
parse a second time.

```sh
# runfiles/audit/gate.run
# Fail when a dependency has a high or critical vulnerability

let report = ARG.report ? "osv.json"
let severities = json_query(read_file(report), "results[].packages[].groups[].max_severity")
let high = 0

for severity in severities
	if severity != "" && number(severity) >= 7
		high = high + 1
	end
end

print("High or critical:", high)

if high != 0
	exit(1)
end
```

`json_get` reads one value at a path, and `json_keys` opens an object up — there is no map type, so without it
an object is text and nothing more. `json_type` says what is at a path, which is the one question `json_get`
cannot answer: it hands back `""` for a null and the compact text for an object. `json_format` is `jq .`, and
`json_encode` goes the other way, turning a list into an array. Nothing here is a dependency your machine has
to already have.

### Any language you have installed

`exec <command>` runs that command with the block as its **stdin**, so a target can be a Python script, a Node
script, a SQL file — whatever is already on the machine. The body is that command's language, not this one.

```sh
# runfiles/i18n/check.run
# Check that every translation catalogue has the same keys as the reference

.env.LOCALES = "src/locales"

exec python3
	import json, os, pathlib, sys

	files = sorted(pathlib.Path(os.environ["LOCALES"]).glob("*.json"))
	catalogues = {f.stem: set(json.loads(f.read_text())) for f in files}
	reference = catalogues.pop("en")

	for name, keys in catalogues.items():
		missing = reference - keys
		if missing:
			sys.exit(f"{name}: missing {', '.join(sorted(missing))}")

	print(f"i18n: {len(files)} catalogues agree on {len(reference)} keys")
end
```

```sh
# runfiles/deps/list.run
# Print the first few runtime dependencies

.env.LIMIT = ARG.limit ? "10"

exec node
	const { readFileSync } = require("node:fs")

	const pkg = JSON.parse(readFileSync("package.json", "utf8"))
	const deps = Object.keys(pkg.dependencies ?? {})

	for (const name of deps.slice(0, Number(process.env.LIMIT))) {
		console.log(`  ${name}`)
	}
end
```

```sh
# runfiles/db/report.run
# Row counts for the tables that matter

exec psql --quiet {{ ENV.DATABASE_URL }}
	select 'accounts' as table, count(*) from accounts
	union all
	select 'orders', count(*) from orders
	order by 1;
end
```

In value position it captures what the command printed, so another language can answer a question this one
then acts on:

```sh
# runfiles/coverage/gate.run
# Fail if line coverage dropped below the floor

let percent = exec python3
	import pathlib, re

	text = pathlib.Path("lcov.info").read_text()
	hit = len(re.findall(r"^DA:\d+,[1-9]", text, re.M))
	total = len(re.findall(r"^DA:", text, re.M))

	print(round(100 * hit / total))
end

if number(percent) < 85
	error("coverage is {{ percent }}%, below the 85% floor")
end
```

One thing to know: the body **is** the command's stdin, so a script cannot also read data from stdin — give it
what it needs through `.env`, a file, or the command's own arguments.

An interpolation inside the body arrives **as itself**, unquoted, when the command is not a shell — shell
quoting is the wrong quoting for anybody else's language, so quote it the way that language wants. A shell
body still self-quotes, and `$ cp {{ src }} {{ dst }}` is as safe as it ever was.

### Secrets that never touch the disk

```sh
# runfiles/deploy.run
# Deploy to production

.env-file = ".env.production"
.env.GOOGLE_APPLICATION_CREDENTIALS = temp_file(base64_decode(ENV.SERVICE_ACCOUNT_B64), "json")

confirm("Deploy to production?")
$ terraform apply -auto-approve
```

The temp file is deleted when the run ends, however it ends — including a failure half-way, which is exactly
when a decoded credential must not be left behind.

### A pre-commit hook

```sh
# runfiles/precommit.run
# Format staged files and re-stage them

let staged = $ git diff --cached --name-only --diff-filter=ACMR -- "*.rs"
let files = lines(staged)

if length(files) > 0
	$ rustfmt {{ files }}
	$ git add {{ files }}
end
```

`files` is a list, so `{{ files }}` becomes one argument per file — spaces in names and all.

### Re-running on change

```sh
# runfiles/watch.run
# Recompile whenever a source file changes

.watch = "src/**/*.rs"
.watch = "!src/generated/**"

run build
```

There is no `--watch` flag: the file already said what it wants, so `run watch` watches.

## The language

Line-oriented, with one rule: **the language is the default, the shell is marked.**

| Line | Meaning |
| --- | --- |
| `# text` | A comment, to the end of the line. It may follow code. The leading block is the target's description. |
| `.name = value` | A property. |
| `$ echo hi` | Hand this line to a shell. |
| `exec python3` … `end` | Run a command with the block as its stdin. |
| `detach $ npm run dev` | Start this one command and do not wait for it. Also `detach exec …`. |
| `json` … `end` | A block of JSON, as one value. |
| `let x = 1` | Bind a value. `x = 2` rebinds. `let a, b = pair` takes a list apart. |
| `if` / `else if` / `else` / `end` | Branch. However many `else if`s, one `end`. |
| `for x in list` / `end` | Loop over a list. `for k, v in pairs` unpacks each item. |
| `while c` / `until c` / `loop` / `end` | Loop on a condition, on its negation, or forever. |
| `break` / `continue` | Leave the innermost loop, or start its next pass. |
| `match` / `case` / `default` / `end` | Dispatch on a value. A label is a quoted string: `case "linux"`. |
| `retry n [every s]` … `end` | Run the block again while it fails, up to `n` times. |
| `do` … `end` | A block with no condition, so a property can cover a few commands. |
| `run other-target` | Run another target, in this process. |
| `print(…)` | Anything else is an expression, evaluated for its effect. |

A `#` opens a comment where it begins a word — the shell's own rule, so it reads the same on both sides of the
marker, and `a#b` is one word in either half. The text after `$ ` is the shell's, `#` and all, and so is an
`exec` block's body; everything else this language reads takes a comment at the end of the line.

### Values

Strings, numbers, booleans and lists — **strict, with no coercion.** `"a" + 1` is an error telling you to use
`concat`. `"1" == 1` is false. `number(ARG.count)` is required before arithmetic. `1 / 2` is `0.5`.

Lists nest as deep as you like, which beats packing several fields into one string and splitting it back out:

```sh
for volume, owner in [
	["prometheus-data", "65534:65534"], # nobody
	# the rest run as themselves
	["kvrocks-data", "999:999"],
]
	$ docker run --rm -v {{ volume }}:/v alpine chown -R {{ owner }} /v
end
```

Several names on the left of a `let`, a reassignment or a `for` take a list apart in order —
`let major, minor, patch = split(version, ".")` — with `_` for a position you have no use for. A name with
nothing to bind is an error rather than an empty string, since the names are a claim about the shape of the
value.

`print` writes a line to stdout; `printf` writes exactly what you give it, with `%s`, `%d`, `%f`, `%.Nf` and
`%%`, and no newline of its own.

Lists are values, and every list function answers with a **new** list, so nothing changes behind another name:

```sh
let recent = slice(reverse(sort(tags)), 0, 5)
let targets = without(RUN.namespaces, "docs")

for name, owner in zip(names, owners)
	$ chown {{ owner }} /srv/{{ name }}
end
```

`append`, `prepend`, `concat_lists`, `sort`, `reverse`, `unique`, `slice`, `flatten`, `zip`, `index_of` and
`without`, beside `first`, `last`, `length` and `join`.

There are 85 built-in functions — strings, lists, regex, paths, JSON, hashes, time, files. Your editor lists
them all with a description and an example; `run :list` is for targets, and the language server is for the
language.

### Loops

`for` walks a list. `while` and `until` ask before each pass — `until` is the shape a wait loop wants — and
`loop` never asks at all. `break` leaves the innermost one, `continue` starts its next pass, and both are a
parse error anywhere else, so your editor says so rather than a run finding out.

```sh
until $ curl -sf http://localhost:8080/health
	sleep(1)
end

for path in glob("**/*.log")
	if !contains(read_file(path), "PANIC")
		continue
	end

	print("first panic in {{ path }}")
	break
end
```

`range(n)` counts from zero and stops short of `n`; `range(a, b)` is every number from `a` to `b`, both
included. Under `--dry-run` a conditional loop walks its body once: a preview performs none of the effects the
condition is waiting on, so how often is not a thing it can honestly answer.

### Where values come from

| Source | From |
| --- | --- |
| `ARG.name` | `--name=value`, or `--name value` |
| `FLAG.name` | `--name` (a boolean) |
| `ARGS` | positional arguments, as a list |
| `ENV.NAME` | the environment |
| `RUN.os` `RUN.arch` `RUN.cwd` `RUN.file` `RUN.parent` `RUN.namespaces` `RUN.user` | the run itself |

`a ? b` means "`a`, or `b` if `a` is not there", and chains:

```sh
let port = ARG.port ? ENV.PORT ? "3000"
```

An argument takes its value two ways, `--key=value` and `--key value`, and nothing is declared to make the
second one work: `run` walks the target's parsed tree, so it already knows `ARG.target` takes a value and
`FLAG.force` does not. `--target aarch64` is the triple; `--force x` is a flag and a positional. Where a name
is read both ways the bare `--name` is the flag, and `--name=value` is the argument. A value starting with a
dash needs the `=` form — `--target --release` is refused rather than quietly setting the triple to
`--release`.

**A flag a target cannot read is an error**, not a typo you find out about later. `run` knows exactly what a
target reads by walking its parsed tree, so `--forse` stops the run and points at `--help`:

```bash
$ run deploy --help
run deploy [--env=<value>] --token=<value> [--force]

  Deploy to an environment.

Arguments
  --env=<value>                 defaults to staging
  --token=<value>               required
  --force                       off unless passed

Environment
  DEPLOY_KEY                    required
```

Nothing is declared for that — a `?` chain is what makes a value optional, and the literal it ends in is the
default. `run --stdin-args deploy` asks for the same list, in the same order, **before anything runs**.

That refusal has one exception, and it is the useful one: **a target that reads `ARGS`**. A wrapper can read
any word, so it gets any word — a `--flag` it does not claim for itself joins the positionals where it was
written, and forwarding a whole command line needs nothing special.

```sh
# runfiles/aws.run
# The AWS CLI, in a container

$ docker run --rm amazon/aws-cli {{ ARGS }}
```

```bash
run aws s3api list-buckets --output json
```

Everything after a bare `--` is passed through untouched, whatever the target reads. That is how a wrapper
forwards a word the target would otherwise claim — its own `--output`, or a `--help` meant for the command
inside:

```bash
run aws -- s3api list-buckets --help
```

### Interpolation quotes itself

`{{ … }}` in a shell line becomes **exactly one argument** — or, for a list, one argument per item. So this is
safe with any file name, spaces and quotes included:

```sh
$ cp {{ ARG.src }} {{ ARG.dest }}
```

**Never wrap an interpolation in shell quotes.** There is no `shell_quote` function because there is nothing
left to quote: the substitution already did it. The same rule holds one layer up inside a `json` block, where
an interpolation becomes one JSON value.

### Asking whether a command worked

A `$` run may stand as a condition or as a `match` subject. The condition is true when the command exits 0;
the cases are exit codes. Neither stops the target — a non-zero exit is the answer, not a failure.

```sh
if $ command -v docker
	$ docker info
else
	error("docker is not on PATH")
end

match $ curl -fsS https://example.com
	case "0"
		print("up")
	case "22"
		error("the endpoint answered 4xx or 5xx")
	default
		error("could not reach it")
end
```

`code_of($ cmd)` is the same status as a number, for when you want to keep it — and `code_of(run other)`
scores another **target** the same way. That is how one target runs both halves of a check and gives a single
verdict at the end, instead of stopping at the first thing that failed:

```sh
# runfiles/coverage.run
# Coverage for both halves, with one verdict

let rust = code_of(run coverage:rust)
let web = code_of(run web:coverage)

run _verdict --name=Rust --code={{ rust }}
run _verdict --name=Web --code={{ web }}

if rust != 0 || web != 0
	exit(1)
end
```

A dispatched target is scored the way `run` itself would exit: `exit(3)` is 3, any other failure is 1, and the
error is printed where it happened. An interrupt, or answering no to a `confirm()`, is not a status — that
stops the caller too.

### Capturing output

```sh
let branch = $ git rev-parse --abbrev-ref HEAD

if branch != "main"
	error("release from main, not {{ branch }}")
end
```

A `$` capture runs to the end of its line, so it can only be a call's **last** argument, and the `)` has to be
the last character of the line:

```sh
let files = lines($ git diff --cached --name-only)

for f in lines($ git ls-files '*.sh')
	$ shellcheck {{ f }}
end
```

`code_of($ cmd)` and `code_of(run other)` are the same shape: run it and take the status, ignoring how it went
— which is what `|| true` was doing.

### Blocks that are not shell

`$ line` is shorthand for a one-line `exec` with the default shell. Name any other command and the block
becomes its stdin — see [Any language you have installed](#any-language-you-have-installed).

```sh
exec python3
	import json, sys

	print(json.dumps({"ok": True}))
end
```

### Stopping early

`exit()` ends the run with a status; `error("…")` fails with a message; `confirm("…")` asks, and stops if the
answer is no. Nothing catches any of them: not a `?` fallback, not `.ignore-errors`. A target may forgive a
command that failed, but being told to stop is not that.

`confirm` is a function rather than a property, so the question can depend on what is about to happen — and is
skipped by `-y`, in CI, and under `--dry-run`, where there is nothing to approve.

A line that is only a value — `exit`, `abc`, `35` — is a parse error, since it computes something and throws
it away. Most often it is a call with the parentheses left off, and the message says so.

## Properties

Set at the top of the file, or inside a block where marked.

| Property | Effect | In a block? | Flag? |
| --- | --- | :-: | :-: |
| `.shell` | Which POSIX shell `$` lines use. | | |
| `.env.NAME` | Set an environment variable. | ✅ | |
| `.workdir` | Where commands run. | ✅ | |
| `.parallel` | Run this block's commands at once. On a `for`, its iterations. | ✅ | ✅ |
| `.ignore-errors` | Keep going when a command fails. | ✅ | ✅ |
| `.logging` | Announce each command on stderr before it runs. Off unless set. | | ✅ |
| `.env-file` | Load a `.env` file (encrypted values are decrypted in memory). Appends. | ✅ | |
| `.add-path` | Prepend a directory to `PATH`. Appends. | ✅ | |
| `.watch` | Re-run when matching files change. A `!` prefix excludes. | | |
| `.only-in-directories` | Machine-wide targets only: offer this one inside these directories. Appends. | | |

A **flag** is written bare for `= true`, or given a bool. A constant that can never be one is refused where it
is written — `.parallel = 23` and `.parallel = "true"` are both errors, and the second says to drop the
quotes. A value the run works out is left to the run: `.parallel = ENV.CI` and
`.parallel = ARG.p ? ENV.CI ? "false"` are how a flag is decided from outside the file, and there `true`,
`false`, `1` and `0` are all understood. Anything else stops the run rather than reading as `false`, because
a flag that quietly did not take effect is found out much later.

A property is applied **where it is written**: it can read a binding above it, and it takes effect from there
down.

```sh
# runfiles/build.run
# Build into a directory the command line picks

let out = concat("target-", ARG.profile ? "debug")

.workdir = out

$ ls
```

Five of them describe the whole block rather than the commands under it — `.parallel`, `.shell`, `.logging`,
`.watch` and `.only-in-directories` — and those have to be written above the block's first statement.

`.shell` names one of the eight shells `$` knows how to drive: `sh`, `bash`, `dash`, `ash`, `zsh`, `ksh`,
`busybox` or `brush`. Any other interpreter is named on the line instead, with `exec` — which is the same
mechanism, and says so where it can be read:

```sh
# runfiles/list.run
# List the current directory, whatever the platform

if RUN.os == "windows"
	exec pwsh
		Get-ChildItem
	end
else
	$ ls -la
end
```

`.env-file` and `.add-path` **append**, so a block adds to what it inherited rather than replacing it, and a
block's file can be named by something that block worked out:

```sh
# runfiles/deploy.run
# Deploy to an environment

let env = one_of(first(ARGS), "staging", "production")

if env == "production"
	.env-file = ".env.production"
	.add-path = "vendor/prod-tools"

	confirm("Deploy to production?")
end

$ terraform apply -auto-approve
```

**A target whose file name starts with `_` is hidden** from `run :list`, from completion and from generated
editor tasks — and still runs when something calls it. That is the whole of it; there is no property to
disagree with the name.

```
runfiles/_aws.run       → run _aws     (a helper other targets call)
runfiles/deploy.run     → run deploy
```

`do … end` is a block with no condition — somewhere for a property to go when it should cover a few commands
and not the whole target:

```sh
# runfiles/setup.run
# One-time per clone: hooks, then the web dependencies

$ git config core.hooksPath .githooks

do
	.workdir = "web"

	$ pnpm install
	$ pnpm exec playwright install chromium
end
```

That is what replaced writing `cd web &&` on every line, which the shell would have needed once per command.

`_shared.run` holds properties — and `let` bindings — that every target in its directory inherits. A nested
one layers over the directory above it, so `runfiles/api/_shared.run` adds to `runfiles/_shared.run`.

```sh
# runfiles/_shared.run
# Settings every target here inherits

.add-path = "node_modules/.bin"
.env.CARGO_TERM_COLOR = "always"

let image = "ghcr.io/example/builder:v2"
```

## How targets are found

`run` walks **up** from the working directory for the nearest `runfiles/`, then **down** one level for
subprojects. A nested directory becomes a namespace:

```
runfiles/build.run          → run build
web/runfiles/build.run      → run web:build
runfiles/api/deploy.run     → run api:deploy
```

Inside a subproject, `run build` means *that* subproject's `build` — so a file reads the same wherever you
invoke `run` from.

`$HOME/.runfiles/` holds machine-wide targets, available in every project. If you would rather see the
directory than hide it, `$HOME/runfiles/` and `$HOME/Runfiles/` are read too — but only one of the three may
hold anything. `run :list` puts them first, under `global:`: they are the part of the listing you cannot see
by looking at the project.

A machine-wide target can say where it belongs, so one directory can hold work for several places at once:

```sh
# ~/.runfiles/deploy.run
# Ship the current branch

.only-in-directories = ["~/work/acme", "~/work/zed"]

$ ./scripts/ship
```

It is offered inside those directories and nowhere else. A `_shared.run` says the same thing for every target
below it, and each level narrows the one above — a target cannot name its way back out of a directory that
excluded it. Relative entries are relative to your home. It is the one property a project's own files may not
set: their targets are visible to anyone reading the repository, so hiding some by working directory would
bring back the very invisibility this exists to fix.

**Not in CI.** A runner's home directory is nobody's, so `run` reads none of the three there: what runs is
what is checked in and reviewable. This is also why nothing has to be cleaned up after a job.

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

The record lives in `state.json` in the platform state directory. In CI there is none: a runner is built and
thrown away, so there is no earlier session whose `setup` this one could be relying on — the gate is not
enforced and the file is not written. `RUNFILE_SKIP_PREPARE=1` is different: it turns the gate off on a machine
whose state is still worth keeping, so a `setup` run under it is still recorded.

## Commands

| Command | |
| --- | --- |
| `run <target> [args…]` | Run a target |
| `run :list` | List every target (`--names`, `--json` for tooling) |
| `run :init` | Create `runfiles/` with an example |
| `run :format [path…]` | Format runfiles in place (`--check`, `--stdout`) |
| `run :env <sub>` | Manage `.env` files: `init`, `get`, `set`, `encrypt`, `decrypt`, `rotate`, `inject`, `secret-keys` |
| `run :completions <command> <shell>` | `install`, `uninstall` or `output` a completion script for `bash`, `zsh`, `fish` or `powershell` |
| `run :generate <editor>` | Task files for `zed`, `jetbrains` or `vscode`, merged into what is there |
| `run :update` | Update the binary |

| Flag | |
| --- | --- |
| `-y`, `--yes` | Skip confirmation prompts |
| `--stdin-args` | Prompt for every input the target reads, before it runs |
| `--dry-run` | Print what would run, without running it |
| `--dir <path>` | Start discovery somewhere else |
| `-h`, `--help` | Show the help for `run` or any of its commands |
| `-v`, `--version` | Print the version |

Flags belong **before** the target name; everything after it is passed to the target.

`run :format` has no settings: one shape, everywhere. It reindents with tabs, spaces expressions, places blank
lines, lays out `json` blocks, and leaves the three things that are not the language's to touch — strings, the
text after `$ `, and `exec` bodies — exactly as written. It refuses a file that does not parse, and checks that
its own output still means the same thing before writing it. `--check` reports what would change and exits 1,
which is what a CI step wants.

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

## Editor support

`run :lsp` is a language server, so the binary you already have gives you diagnostics, completion, formatting,
go-to-definition and documentation on hover — from the same parser `run` itself uses, so your editor never
disagrees with what will actually happen. There is nothing extra to install.

Hover anything: a function shows its signature, what it does and a worked example; a property adds whether it
may sit inside a block; `$`, `exec`, `run`, `retry`, `match` and the rest explain the line form itself.

Ctrl+click a `run <target>` to open that target's file, or a variable to jump to where it was bound — including
into the `_shared.run` above it, which is the one definition you cannot find by reading the file in front of
you.

It also hands `$` lines and shell `exec` bodies to [shellcheck](https://www.shellcheck.net) when it is
installed, mapping findings back to the lines you wrote.

**VS Code** — install the `.vsix` from the [latest release](https://github.com/JoaaoVerona/runfile/releases).
You get a Run button on every target, a task provider, a sidebar tree, and shell lines coloured exactly as the
same command in a `.sh` file. Set `"editor.formatOnSave": true` and saving formats.

**JetBrains IDEs** read the same grammar: *Settings → Editor → TextMate Bundles*, add the `editors/vscode`
directory from a checkout.

**Zed, Neovim and Helix** point their language-server configuration at `run` with the argument `:lsp` — in
Neovim, `vim.lsp.config("runfile", { cmd = { "run", ":lsp" }, filetypes = { "runfile" } })` — and take the
tree-sitter grammar in `editors/tree-sitter` for highlighting, with highlight queries included. For Neovim
with nvim-treesitter:

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
```

`run :generate zed` writes every target into `.zed/tasks.json`, and `run :generate jetbrains` into
`.idea/runConfigurations/`; both leave entries you wrote yourself alone and replace only their own.

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
| Hover documentation for every function, property and line form | ✅ | ❌ | ❌ | ❌ |
| One canonical format, in the CLI and on save | ✅ | ❌ | ❌ | ❌ |
| IDE task generation (VS Code / Zed / JetBrains) | ✅ | ❌ | ❌ | ❌ |
| Per-target shell override | ✅ | ❌ | ✅ | ❌ |
| Any interpreter for a block (`exec python3`) | ✅ | ❌ | ✅ | ❌ |
| Strict parsing (typos are errors) | ✅ | ❌ | ✅ | ❌ |
| `--help` for one target, built from its own comments | ✅ | ❌ | ✅ | ❌ |
| Argument substitution with chained fallbacks | ✅ | ❌ | ✅ | ❌ |
| Watch mode, built-in | ✅ | ❌ | ❌ | ✅ |
| Shell completions | ✅ | ❌ | ✅ | ✅ |
| Hidden targets | ✅ | ❌ | ✅ | ✅ |
| Built-in functions (strings, regex, paths, JSON, hashes, time) | ✅ | ❌ | ✅ | ✅ |
| Parallel execution | ✅ | ✅ | ❌ | ✅ |
| Single static binary | ✅ | ✅ | ✅ | ✅ |
| Native Windows binary (`$` lines use Git Bash) | ✅ | ❌ | ✅ | ✅ |
| Output prefixing in parallel mode | ✅ | ❌ | ❌ | ✅ |
| First-class PowerShell / cmd.exe | ❌ | ❌ | ✅ | ❌ |
| Pattern rules (`%.o: %.c`) | ❌ | ✅ | ❌ | ❌ |
| Preconditions / status checks | ❌ | ❌ | ❌ | ✅ |
| Incremental builds (sources / timestamps / checksums) | ❌ | ✅ | ❌ | ✅ |

## Platform support

| | |
| --- | --- |
| Linux | x86-64, arm64 |
| macOS | Intel, Apple Silicon |
| Windows | x86-64, arm64 |

`$` lines use bash where it exists, Git Bash on Windows, and `sh` otherwise — so one file works everywhere. Set
`.shell` to pin one of the other POSIX shells, or name any other interpreter with `exec`.

## License

MIT
