# Language check rules

`run` reads every runfile before it runs anything, and holds the language in it — every line that is not shell —
to the rules below. A finding is underlined in your editor, reported by `run :lint`, and **refuses the run** —
`--dry-run` included — with one `[runfile] error:` line per finding, each saying what to write instead when there is
one thing to write:

```bash
$ run :lint
runfiles/setup.run:4:4: error: `RUN.os == "darwin"` is always false: `RUN.os` is only ever `linux`, `mac` or `windows` [never-equal]
  fix: write `"mac"`
1 file checked: 1 error in 1 file
```

These sit beside the check that every name resolves — a call to a function the language does not have, a read of a
name nothing binds, a `RUN` key that does not exist — which the [README](README.md) describes, and beside the
[shell checker](SHELL-CHECK-RULES.md), which reads the `$` lines and shell `exec` blocks.

## No false positives

A finding stops a run, so **a rule reports only what goes wrong every time it is reached**: a value the place it
reaches refuses whatever it holds, or a comparison whose answer never changes. It is reported wherever it sits, a
branch that is never taken included — the month that branch runs is the wrong time to find out.

What a value can be is worked out from the text, and only as far as the text is sure:

- A literal is what it is. `ARG.x` and `ENV.X` are always strings, `FLAG.x` a bool, and `ARGS` a list of strings.
- An operator and a call answer with what they always answer with — `length(…)` a number, `lines(…)` a list of
  strings — and `a ? b` with either side.
- **A name holds whatever any line binds it to**, anywhere in the file or in a `_shared.run` above it, and not only
  the line just before it is read. `let n = ARG.n` followed by `n = number(n)` leaves `n` a string or a number, so
  `n + 1` below them is left alone.

A value that can be of the right type is never reported, even when it can also be of the wrong one: `ENV.PORT ?
3000` is a string when the variable is set and a number when it is not, so nothing is said about `+` on it.

There is no comment that silences a rule. A rule that needs silencing is a rule that is wrong, and the fix belongs in
the rule.

## Rules

### `arity`

A call with a number of arguments the function never takes.

The call is refused when it is reached, whatever its arguments hold.

```sh
# flagged
let parts = split("a,b")
```

```sh
# not flagged
let parts = split("a,b", ",")
```

Left alone: a call to a function the language does not have, which the name check reports.

### `wrong-type`

A value of a type the place it reaches always refuses.

Nothing is converted. `+` adds numbers and nothing else, an `if` needs `true` or `false`, and an argument or an
environment variable is always a string — so `ARG.port + 1` and `if ENV.CI` fail every time they run.

```sh
# flagged
let port = ARG.port ? "8080"

print(port + 1)
```

```sh
# flagged
if ENV.CI
	print("in CI")
end
```

```sh
# not flagged
let port = number(ARG.port ? "8080")

print(port + 1)

if ENV.CI == "true"
	print("in CI")
end
```

Covers the operators `+`, `-`, `*`, `/`, `%`, `<`, `<=`, `>`, `>=`, `!`, `&&` and `||`; an `if`, `while` or
`until` condition; `for`, which walks a list; `retry` and `every`, which count; an index and what it indexes;
unpacking several names; `code_of`, which takes a `$` run or a `run` dispatch; every argument of every function; and
the `%d` and `%f` values of a `printf` whose format is written out.

Left alone: the right side of `&&` or `||` when the left side is written as the answer that settles it, as in
`false && …`.

### `never-equal`

A `==` or `!=` whose two sides can never be equal.

`==` converts nothing and refuses nothing: `"1" == 1` is simply false. So a comparison across types is always false
— and `!=` always true — and the block behind it never runs, or always does, with nothing to say why. The same goes
for `RUN.os` and `RUN.arch` compared with a name they never have: `RUN.os` is only ever `linux`, `mac` or `windows`,
and `RUN.arch` only `x86-64`, `arm64`, `riscv64` or `unknown`.

```sh
# flagged
if RUN.os == "darwin"
	$ brew install jq
end
```

```sh
# flagged
let status = code_of($ make test)

if status != "0"
	exit(1)
end
```

```sh
# not flagged
if RUN.os == "mac"
	$ brew install jq
end

let status = code_of($ make test)

if status != 0
	exit(1)
end
```

`RUN.os` and `RUN.arch` are followed through `?`, and through a name every line binds to one of them. Left alone: a
string compared through a name — `let mode = "dev"` above `if mode == "prod"` — which is how a setting edited by hand
is written.

### `unreachable-case`

A `case` its `match` can never take.

A `match` takes the first `case` whose label is its subject written out. A label written twice is never reached the
second time, and neither is a label the subject is never written as: `match $ …` takes an exit status, which is a
whole number; a bool is `true` or `false`; a number is written the way `print` writes it; and `RUN.os` and `RUN.arch`
have the values above.

```sh
# flagged
match RUN.os
	case "linux"
		$ sudo apt-get install -y jq
	case "macos"
		$ brew install jq
end
```

```sh
# flagged
match ARG.env ? "dev"
	case "dev"
		$ docker compose up
	case "prod"
		$ ./deploy.sh
	case "dev"
		$ npm run dev
end
```

```sh
# not flagged
match RUN.os
	case "linux"
		$ sudo apt-get install -y jq
	case "mac"
		$ brew install jq
end
```

Left alone: any label under a subject that is a string or a list, which can be written as anything.

### `invalid-literal`

A value written out that the place it reaches always refuses.

```sh
# flagged
if regex_matches(ARG.version, r"^v(\d+\.\d+$")
	print("a release")
end
```

```sh
# flagged
let hosts = split(ARG.hosts, ",")

print(hosts[-1])
```

```sh
# not flagged
if regex_matches(ARG.version, r"^v(\d+)\.\d+$")
	print("a release")
end

let hosts = split(ARG.hosts, ",")

print(last(hosts))
```

Covers a regex that does not compile, in any `regex_…` call; a glob that does not compile; a `printf` format that is
not one, or that substitutes more or fewer values than it is given; a `now` format that is not one; an index or a
count that is negative or not whole, and a `sleep` that is negative; dividing by zero; and a list written out with
fewer items than the names unpacking it.

Left alone: any of these worked out during the run rather than written out.

### `capture-position`

A `$` run or a `run` dispatch where the runner has nothing to run it in.

The runner runs a `$` run as the whole value of a `let`, a reassignment, a line of its own or a `for` — or as the
last argument of a call that is — and as the whole of what an `if`, `while`, `until` or `match` asks. Anywhere else
there is no process to run it in, and the line fails every time: inside a call in a condition, and in a
`_shared.run`'s own `let`s, which are worked out before any target runs.

```sh
# flagged
if file_exists($ git rev-parse --git-path MERGE_HEAD)
	error("finish the merge first")
end
```

```sh
# flagged
if code_of($ make test)
	print("passed")
end
```

```sh
# not flagged
let merging = $ git rev-parse --git-path MERGE_HEAD

if file_exists(merging)
	error("finish the merge first")
end

if $ make test
	print("passed")
end
```

Left alone: a `let` inside a block of a `_shared.run`, which never runs at all.

### `glued-list`

A list interpolated with no space between it and the text beside it, in a `$` line.

An interpolation is one shell word, or — for a list — one word per item, so a list rendered next to literal text
with no space is wrong whatever it holds: an empty list leaves the text standing on its own, and a list of
several splits into words with the text stuck to the last. The silent case is the dangerous one —
`rm -rf {{ dirs }}/cache` is `rm -rf /cache` when the glob matched nothing.

Reported only for a list that is **its own word with a literal glued onto its end**: a space (or the line start)
before it, and a non-space literal after it.

```sh
# flagged
let dirs = glob("build/*")
$ rm -rf {{ dirs }}/cache
```

```sh
# not flagged
let dirs = glob("build/*")
$ rm -rf {{ dirs }}
```

Left alone: a list built *into* a word on purpose, with a non-space prefix (`-Dexec.args={{ ARGS }}`,
`inst={{ ARGS }}; …`), which is the common "zero or one positional" idiom; a list with a space after it
(`{{ ARGS }} -- --flag`); and a value that is only sometimes a list (`ARG.x ? glob("…")`), whose string form is fine.
