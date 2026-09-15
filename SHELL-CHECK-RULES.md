# Shell check rules

`run` reads the shell in every runfile before it runs anything, and holds it to the rules below. A finding is
underlined in your editor, reported by `run :lint`, and **refuses the run** — `--dry-run` included — with one
`[runfile] error:` line per finding, each saying what to write instead when there is one thing to write:

```bash
$ run :lint
runfiles/ports.run:3:17: error: `'sport = :{{ ARG.port }}'` puts an interpolation inside `'…'`, but an interpolation is already one quoted word: a value with a space or a quote in it would end these quotes early and be split apart [quoted-interpolation]
  fix: write `'sport = :'{{ ARG.port }}`
1 file checked: 1 error in 1 file
```

`run :lint` never rewrites shell to fix a finding: the fix is a suggestion, and the shell is yours to edit.

## No false positives

A finding stops a run, so **a rule reports only what is wrong every time it appears.** Where the same text can
be right — an argument that is shell code for another shell to read, a quote that is part of a message — the
rule says nothing, and each rule below lists what it deliberately leaves alone and why. That makes the checker
narrower than ShellCheck on purpose: it does not report style, portability or "probably a mistake", only
mistakes.

A script the checker cannot follow completely — `coproc`, a heredoc delimiter built from an interpolation, a few
other rare constructs — is **left alone entirely** rather than read by a guess. Every syntax finding is one bash
reports too, which the tests hold it to with `bash -n`.

There is no comment that silences a rule. A rule that needs silencing is a rule that is wrong, and the fix
belongs in the rule.

## What is read

- **`$` lines**, the way the runner hands them over: adjacent `$` lines are one script, a `detach $` line is one of
  its own, and they run in bash — Git Bash on Windows — unless `.shell` names another shell.
- **`exec` blocks whose command is a shell** this checker reads: `sh`, `bash`, `dash`, `ash`, `brush` or
  `busybox sh`.
- **Captures of either**: `let x = $ …`, `if $ …`, `lines($ …)`, `code_of($ …)`, `let x = exec bash … end`.

Not read: an `exec` block for another program (`python3`, `node`, `pwsh`), a file whose `.shell` is `zsh`,
`ksh` or a value only known at run time, and an `exec` command line itself, which the runner splits into
words rather than handing to a shell.

**An interpolation is read as what the runner makes of it**: one shell word, already quoted, whatever it holds
— never as placeholder text the checker could mistake for a command. That is what lets a rule tell
`{{ ARG.dest }}/backup` from `"{{ ARG.dest }}/backup"`, and point at the column you wrote.

## Syntax

### `unclosed`

A quote, an expansion or a block that is never closed.

Bash reads a script as it runs it, so the lines above run and then the script stops at a syntax error — with
whatever those lines did half done. Covers `'`, `"`, `` ` ``, `$'`, `$(`, `${`, `$((`, `<(`, an array's `(`,
`if` without `fi`, `for`, `while` and `until` without `done`, `case` without `esac`, `{`, `(`, `[[`, a function
with no body, and a script ending on `|`, `&&` or `||`.

```sh
# flagged
$ echo it's done
```

```sh
# flagged
$ if [ -f .env ]; then
$ 	cp .env .env.backup
```

```sh
# not flagged
$ echo "it's done"
$ if [ -f .env ]; then
$ 	cp .env .env.backup
$ fi
```

Left alone: an extended glob like `@(a|b)`, which bash reads only once `shopt -s extglob` has run — and that may
be a line above.

### `unexpected`

A keyword or an operator where bash cannot take one.

`fi`, `done`, `esac`, `then`, `do`, `else`, `elif`, `in` or `}` with nothing open to take it; `;;` outside a
`case`; `;`, `&` or `|` with no command before it; a redirection with nothing to redirect to; and a block with
no command in it, which bash refuses at the word that ends it.

```sh
# flagged
$ if [ -f .env ]; then fi
```

```sh
# flagged
$ echo "done" >
```

```sh
# not flagged
$ if [ -f .env ]; then :; fi
```

Left alone: a keyword that is only a word, as in `echo fi` or `for x in do done`.

### `unterminated-heredoc`

A heredoc that no line ends.

Bash warns and reads every line after `<<END` into the heredoc, so the commands below it never run — and the
target does not fail. The usual cause is a delimiter spelled one way on each end, or indented where `<<` (rather
than `<<-`) wants it at the start of the line.

```sh
# flagged
$ cat > config.ini <<END
$ name = app
$ EOF
```

```sh
# not flagged
$ cat > config.ini <<END
$ name = app
$ END
```

## Interpolations and values

### `quoted-interpolation`

An interpolation wrapped in shell quotes.

`{{ … }}` becomes one shell word, already quoted. Inside `'…'` its quotes end yours early, so a value with a
space in it is split apart; inside `"…"` its quotes are kept as characters, so a value with a space arrives as
`'my dir'`, quotes and all. Either way it works until the first value that needs quoting.

```sh
# flagged
$ sudo ss -ltnp 'sport = :{{ ARG.port }}'
```

```sh
# flagged
$ cp {{ ARG.src }} "{{ ARG.dest }}/backup"
```

```sh
# not flagged
$ sudo ss -ltnp 'sport = :'{{ ARG.port }}
$ cp {{ ARG.src }} {{ ARG.dest }}/backup
```

The fix closes the quotes around the interpolation and drops what that leaves empty or needlessly quoted:
`"{{ sdk }}/emulator/{{ bin }}"` becomes `{{ sdk }}/emulator/{{ bin }}`.

Inside single quotes it is always reported. **Inside double quotes it is reported only where no shell reads the
word again**: the program a command runs, the file a redirection opens, an operand of `[` or `test`, and the
arguments of commands that never run one as code — `cd`, `ls`, `cat`, `cp`, `mv`, `rm`, `mkdir`, `touch`, `ln`,
`chmod`, `chown`, `tee`, `head`, `tail`, `stat`, `find` outside `-exec`, `grep`, and the path options of
`docker` and `podman`. Everywhere else a double-quoted interpolation can be exactly right, and is left alone:

```sh
# not flagged
$ ssh deploy@host "cd {{ ARG.dir }} && ./restart"
$ sh -c "rm -rf {{ ARG.dir }}"
$ echo "export PATH={{ ARG.bin }}:\$PATH" >> ~/.profile
$ echo "Deploying {{ ARG.version }}"
```

There the string is shell code for another shell, and the interpolation's quoting is what makes it correct —
or it is a message, where a value shown in quotes is not wrong. The same goes for `[ "{{ a }}" = "{{ b }}" ]`,
where both sides carry the same quotes and still compare equal.

### `unexpanded-string`

A string that spells `$HOME` or `~`, used as a path.

A string is not shell: `"$HOME/Android/Sdk"` holds the characters `$HOME`, and a path starting with them names a
directory nobody has. Reported at the string, when a value it can reach is used as a path — in an
interpolation in a `$` line where `quoted-interpolation` would judge the word, or as the path of `read_file`,
`write_file`, `file_exists`, `directory_exists`, `is_executable`, `glob`, `.workdir`, `.env-file` or
`.add-path`.

```sh
# flagged
let sdk = ENV.ANDROID_HOME ? "$HOME/Android/Sdk"

$ {{ sdk }}/platform-tools/adb devices
```

```sh
# not flagged
let sdk = ENV.ANDROID_HOME ? "{{ ENV.HOME }}/Android/Sdk"

$ {{ sdk }}/platform-tools/adb devices
```

A value is followed through `let`s and reassignments as they reach the line, `?`, lists, and the calls that pass
a value along (`concat`, `join_path`, `dirname`, `first`, `last` and the like), including from the `_shared.run`
files above — where the finding is placed at the interpolation instead, since the string is in another file.

Left alone: a `$NAME` that is not a whole name in capitals followed by a separator (`$RECYCLE.BIN`, `$5`), a
string used anywhere that is not a path (`print`, an `echo`, a value written into a file), a `~` that does not
start the value, and a string replaced before the line that uses it.

### `unexpanded-glob`

An interpolation holding a `*` the shell never expands.

An interpolation is one quoted word, so `rm -f {{ logs }}` with `logs` holding `build/*.log` hands `rm` the
pattern itself, and removes nothing.

```sh
# flagged
let logs = "build/*.log"

$ rm -f {{ logs }}
```

```sh
# not flagged
for log in glob("build/*.log")
	$ rm -f {{ log }}
end
```

Reported where the value is a file the command opens: an operand of the file commands above, the file of a
redirection, and the operand of a file test. Left alone where a pattern is what the command wants — `find -name`,
`grep`, `git ls-files`, `rsync --exclude` — and a `*` written outside the interpolation, as in `{{ dir }}/*`,
which the shell does expand. Only `*` counts: a `[` or a `?` is ordinary in a file name.

## Commands

### `tilde-in-quotes`

A quoted `~`, which never means the home directory.

The shell expands `~` only outside quotes, at the start of a word. `"~/.aws"` is a directory named `~`.

```sh
# flagged
$ docker run -v "~/.aws:/root/.aws" amazon/aws-cli s3 ls
```

```sh
# not flagged
$ docker run -v "$HOME/.aws:/root/.aws" amazon/aws-cli s3 ls
```

Reported where the word names a file, as `quoted-interpolation` decides. Left alone in a message
(`echo "keys go in ~/.ssh"`), in a pattern (`grep "~/" notes`), and anywhere the word may be text.

### `positional-parameter`

`$1` or `$@` in a shell that is given no arguments.

A `$` line runs in a shell of its own with no positional parameters, so `$1` is always empty, `$@` expands to
nothing and `$#` is always 0. The target's arguments are `ARGS`.

```sh
# flagged
$ cargo test "$@"
```

```sh
# not flagged
$ cargo test {{ ARGS }}
```

Left alone: inside a function, which has parameters of its own; a script that runs `set` with arguments; inside
single quotes, where another command expands it (`xargs sh -c 'echo "$1"' _`); `$0`; and an `exec` block whose
shell is handed arguments, as in `exec bash -s one two`.

### `lost-effect`

`cd`, `export` or `set` as the last command of its shell.

Each `$` run is a shell of its own, and what `cd` changes, `export` sets or `set` turns on ends with it. As the
last command of its shell, it does nothing — while the lines below it read as if it had.

```sh
# flagged
$ cd web

print("installing")
$ npm install
```

```sh
# not flagged
$ cd web
$ npm install
```

Covers `cd`, `export`, `unset`, `set` and `shopt` with options only, `umask` with a mask, `alias` with a
definition, and a bare `NAME=value`. Adjacent `$` lines are one shell — a blank line or a comment between them
does not end it — so a `cd` followed by more `$` lines is fine. Set `.workdir` on a `do` block, or `.env.NAME`,
to reach the lines below. Left alone: a capture or a condition (`if $ cd web` asks whether it can), `cd -`,
anything with a redirection or a `$(…)` in it, and a command run in the background.

### `dropped-backslash`

A backslash bash removes, in front of a letter or a digit.

Outside quotes a backslash only stops the next character being special, and a letter never is — so bash drops
it: `\r` is `r`, and `frontend\target` is `frontendtarget`.

```sh
# flagged
$ tr -d \r < input.txt > output.txt
```

```sh
# not flagged
$ tr -d '\r' < input.txt > output.txt
$ find . -name \*.log -delete
```

Left alone: a backslash in front of a character it does protect (`\*`, `\;`, `\$`, `\ `), anything quoted, a
command name (`\rm` skips an alias), a heredoc delimiter (`<<\EOF`), and the inside of `[[ … ]]` and a `case`
pattern.

### `windows-command`

cmd.exe's syntax, in a shell that is not cmd.exe.

A `$` line runs in a POSIX shell on every system — Git Bash on Windows — so `rmdir /S /Q build` is `rmdir` asked
to remove directories called `/S`, `/Q` and `build`.

```sh
# flagged
$ rmdir /S /Q build
```

```sh
# not flagged
$ rm -rf build
```

Covers `del`, `erase`, `rd`, `rmdir`, `copy`, `xcopy`, `move` and `ren`, and only with a `/X` switch, which is
cmd.exe's and never a path anyone means.

### `bracket-spacing`

A `[` or `]` written against the word beside it.

`[` is a command and `]` its last argument, so each needs a blank around it: `[ -f .env]` is `[` without its
`]`, and `[-f` is a command named `[-f`. Both fail every time they run.

```sh
# flagged
$ if [ -f .env]; then cp .env .env.backup; fi
```

```sh
# not flagged
$ if [ -f .env ]; then cp .env .env.backup; fi
```

### `test-redirect`

`>` or `<` inside `[ … ]`, where it redirects.

Inside `[ … ]`, `>` is not a comparison: `[ "$count" > 100 ]` writes a file named `100` and tests whether
`$count` is empty.

```sh
# flagged
$ [ "$(wc -l < todo.txt)" > 100 ] && echo "too many"
```

```sh
# not flagged
$ [ "$(wc -l < todo.txt)" -gt 100 ] && echo "too many"
```

Left alone: a redirection with a file descriptor (`2>/dev/null`) and one after the closing `]`.

### `outside-function`

`local` or `return` outside a shell function.

Bash fails the script at either one outside a function.

```sh
# flagged
$ local tag=$(git describe --tags)
```

```sh
# not flagged
$ tag=$(git describe --tags)
```

### `spaced-assignment`

`NAME = value`, which runs a command called `NAME`.

An assignment has no blanks around its `=`. With them, the shell runs `VERSION` with `=` as its first argument.

```sh
# flagged
$ VERSION = $(git describe --tags)
$ echo "$VERSION"
```

```sh
# not flagged
$ VERSION=$(git describe --tags)
$ echo "$VERSION"
```

Reported for a name in capitals, since no command is spelled that way, and never for a name defined as a function
in the same script. `NAME= value` — an empty variable for one command — is left alone.

### `sudo-builtin`

`sudo cd` and the like, which have no program to run.

`cd`, `export`, `source` and the other builtins are part of the shell, so `sudo` finds no program to run — and
could not change this shell if it did.

```sh
# flagged
$ sudo cd /var/www
$ ls
```

```sh
# not flagged
$ sudo ls /var/www
```

Covers `cd`, `pushd`, `popd`, `export`, `unset`, `source`, `.`, `alias`, `set`, `shopt`, `ulimit`, `umask`,
`exit`, `declare`, `typeset`, `readonly` and `local`.
