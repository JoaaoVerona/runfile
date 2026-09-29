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
run check                  # Non-mutating gate: fmt --check + clippy (deny warnings) + every release target compiles
run lint                   # Formats, then lints
run test                   # All workspace tests
run install                # Builds release and installs BOTH binaries to ~/.local/bin

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
| `# text` | Comment, to end of line; it may follow code. The leading block is the target's description. |
| `.name = value` | A property. |
| `$ <line>` | Hand this line to a shell. |
| `exec <cmd>` … `end` | Run `<cmd>`, with the block's body as its stdin. |
| `json` … `end` | A block of structured text, as one value of that format. |
| `let x = expr`, `x = expr` | Bind and rebind. `let a, _, c = xs` unpacks a list. |
| `if` / `else if` / `else` / `end`, `for x in …`, `while c`, `until c`, `loop`, `break`, `continue`, `match` / `case` / `default`, `retry n [every s]`, `do` | Control flow. |
| `parallel do` … `end`, `parallel for x in …` … `end` | Run each statement inside, or each iteration, at once, and wait for all of them. |
| `detach $ <line>`, `detach exec <cmd>` … `end` | Start one command and do not wait for it. |
| `run <target> [args]` | Dispatch another target, in-process. |
| `expr` | Evaluated for effect, e.g. `write_file(…)`. |

**The one rule: the language is the default, the shell is marked.** `exec`, `end` and `$ ` were chosen because
they collide with none of the shell lines in the 1,897-line corpus this was designed against; a keyword-first
design would have collided with 69.

`$ run x` re-execs the binary. `run x` dispatches in-process — that is the normal form — and hands `x` the
environment its own commands have at that line (see *runfile-runtime*).

### Types

String, number (one `f64`), bool, list. **Strict, with no coercion**: `"a" + 1` is an error naming `concat`,
`"1" == 1` is false, `number(ARG.x)` is required before arithmetic, and `1 / 2 == 0.5`.

Lists index with `[n]` and work with `first`, `last`, `length`, `join` — and with `append`, `prepend`,
`concat_lists`, `sort`, `reverse`, `unique`, `slice`, `flatten`, `zip`, `index_of` and `without`, **every one
of which answers with a new list**. A list used to be read-only: it could be received from `split`, `lines`,
`glob` or a literal and then only looked at, so anything that wanted to *collect* had to go out to a shell and
come back. Answering with a new list rather than mutating is what keeps a binding from changing behind another
name. `sort` compares numbers by value and everything else by its text, and puts numbers first in a mixed list
rather than refusing — `sort(ARGS)` should not be a type puzzle. `slice` clamps rather than erroring, the way
every language with one does.

**A list nests to any depth**: `[[1, 2], [3, [4]], "a"]`. An element is a whole expression, so nothing about a
list restricts what may sit in one at any level, and `xs[1][2]` is an index of an index. `length`, `first` and
`last` answer about the level they are asked about, since a nested list is *one* element. This replaced the
`"a b c"` + `split()` idiom the corpus used to pack several fields into a loop's list — four files carried it,
and one of them then had to `number()` its way back out of the strings it had just made.

Nesting needs the bracket count to be a count of *real* brackets, which is what `lexer::brackets` is for: it
says how a line moves the depth, and how many of the brackets it closes were opened before it. Counted from
the tokens, so `["x[y"]` is one complete line rather than the start of one — it used to swallow the line after
it and the file stopped parsing somewhere else entirely. The whole line often will not tokenise (a bare `=` is
not an operator this language has), so the right-hand side is tried next and the characters last. The second
number is what indents a spilled list by depth: a `]` sits with the `[` it answers, not with what was inside
it.

### Sources

`ARG.x` (from `--x=value`), `ENV.X`, `FLAG.x` (bool, from `--x`), `ARGS` (a list of positionals), and
`RUN.os` / `RUN.arch` / `RUN.cwd` / `RUN.file` / `RUN.parent` / `RUN.namespaces` / `RUN.user`.

**`--key value` works, and nothing declares that it does.** It used to be a flag plus a positional -- the
argument was `--key=value` only, because nothing said which names take values, so it could not be
disambiguated. But `inputs::of` already answers exactly that question, and the answer is exact: a name read as
`ARG.key` takes a value, a name read as `FLAG.key` does not. `args::parse` is the one place the rule is
spelled, and both the runner and the `--stdin-args` prompt read it, so the two cannot disagree about whether
`--token given` supplied a token.

**No command line that worked before means something different now.** `--key=value` is untouched, a bare
`--key` read as `FLAG.key` is still a flag, and a bare `--key` that is *not* read as a flag was a hard error
-- so every word this newly claims came from a command line that already failed. That is what made the rule
safe to change rather than a break. It is also why a name read **both** ways is a flag in its bare form:
`run x --verbose hello` is a flag and a positional today, and preferring the flag is what keeps it one. The
`=` form still reaches the argument, so both readings stay available, and `EvalError::MissingArgSawFlag` --
"it was passed as a flag" -- is now exactly the case it describes.

A value beginning with `-` is **refused rather than swallowed**: `run build --target --release` would
otherwise set the triple to `--release` and fail inside `cargo`, where the mistake is no longer visible.
`--target=--release` is how to mean it, and `RunError::MissingArgValue` names the word it declined to take,
since "put the value after it" describes what the person just did.

**A bare `--` ends parsing**: everything after it is a positional exactly as typed, flags included. It is no
longer how a wrapper forwards an ordinary command line -- it is how one forwards a word the target *would*
claim (its own `--output`, or a `--help` meant for the command inside). Chosen over an `ARGV` source, so
`ARGS` keeps one meaning.

**An input no name reads goes to `ARGS` when the target reads `ARGS`, and is refused when it does not.** A
wrapper can read the word, so it gets the word, in the position it was written -- which is what makes
`run build --target aarch64-…` reach `cargo` intact with no `--` in front of it. A target with no positionals
has nowhere to put it and nothing to forward it to, so a mistyped `--forse` is still an error there rather
than a flag that quietly did not take effect. The cost is honest: a wrapper now forwards `--targt` to the
command it wraps instead of catching it, and that command is the only thing that knows its own flags. What a
target reads is answered by **`inputs::of`, which walks the tree** — `ARG.x`, `FLAG.x`, `ENV.X` and `ARGS`,
from every position one can sit in, and from every `_shared.run` above it, since a name a shared file reads is
read for every target under it. That chain is walked for what it reads **before** the command line is
classified and evaluated afterwards -- two passes over one parse, because evaluating a shared file reads
`ARG.x` out of the scope the classification is about to fill. **`inputs::of_chain(target, shared)` is the one
description of the chain**: `prepare` and `target_help::inputs` each used to fold it by hand with
`Inputs::extend`, and both ask it now, so the command line, `--help` and `--stdin-args` cannot disagree.

**A name a `.env` property has already set where it is read is not an input.** `--help` said `PORT required`
for `.env.PORT = "3000"` above a `print(ENV.PORT)`, and `--stdin-args` asked for it -- telling someone to
pass a value that the property then overrides. The walk carries the set of names a `.env` has supplied (in
`At`, beside the guard and the default, since it is another fact about where a read sits) and skips a read of
one. It follows the runner exactly, which is the only way it can be trusted: a property covers what is below
it and inside its block; its own value reads what was there before it, so `.env.P = concat(ENV.P, ":x")`
still lists `P`; a `?`-guarded default like `.env.PORT = ENV.PORT ? "3000"` is listed as `defaults to 3000`,
which is how a target says the caller may override. Since a value reads the environment as it stands at its
own line (see *Properties*), the walk is one rule with no exceptions: the set runs forward through the
`_shared.run` chain, into the target's header and down through its body, and a nested block adds to it only
for itself -- `walk` hands back what is set by the end of a block, which `of_chain` carries from one shared
file to the next. It had two regions that saw nothing, while the runner built the environment only after a
header; they went when the runner stopped doing that. A `.env-file` supplies
nothing here, since which names a file sets is only known once it is read and it may well not exist; and a
name is matched exactly as the property sets it, because `ENV.X` tries the exact name before any other case,
so a caller's `PORT` still reaches `ENV.PORT` under a `.env.port`. One place is deliberately loose: a loop's
condition is read under the enclosing environment, though the runner asks it inside the body's, which can
only list a name the caller need not pass, never hide one they must.

It scanned the **text** for `ARG.` until it did not. The keys have no dynamic form — `ARG[k]`, `ARG.{{ k }}`
and `ARG."k"` are all refused by the parser — so every use is spelled out, which made scanning look exact. But
text is not only the program: a name in a comment, in a string, or in the literal half of a `$` line counted
as a use. So `run <target> --help` listed inputs the target never reads, and, worse, the check *suppressed
itself* — a comment saying `ARG.legacy` was enough for a mistyped `--legacy` to pass without a word. A tree has
no comments in it and no strings to confuse.

Because it is exact, **an input a target cannot read is an error** rather than a warning: a flag that warns is
a flag that did not take effect, found out later. The message is one line — everything the runner says while a
target runs carries the `[runfile]` prefix, and a continuation line would not — so it points at
`run <target> --help` rather than listing what the target does read. It quotes the word **as typed**, so a
message about `--region=eu` does not name a `--region` nobody wrote.

`Use` carries the other two things the tree knows: **whether a name can fail** (read bare, with no `?` chain
or `try` around it) and **what it falls back to** (the literal a chain ends in). `a ? b ? c` is
left-associative, so the outermost fallback is what is reached once everything before it is missing, and that
is what is shown. One guarded use does not excuse a bare one. This is what `--help` prints and what
`--stdin-args` asks from — **no declaration syntax was added, and none is needed**: a `?` chain already says
"optional, and here is the default", in the place a reader is already looking.

**`run` statement arguments are values, not shell text.** A `{{ x }}` in `run w {{ x }}` arrives at `w` as one
positional even with spaces, and a list expands to one positional per item — there is no shell in between to
quote for.

`a ? b` takes `a`, or `b` if `a` does not resolve. It is not a ternary; branch with `if`.

### Interpolation self-quotes

`{{ … }}` inside a `$` line or `exec` body becomes **one shell argument** for a string, or **N arguments** for a
list. **Never wrap an interpolation in shell quotes.** This is why there is no `shell_quote` function: of 49
interpolation sites in the corpus, 48 would have been unsafe under manual quoting.

Quotes inside `{{ }}` need no escaping — an interpolation is opaque to the string containing it.
`confirm("Greet {{ ARG.name ? "world" }}?")` is correct as written.

### Structured blocks

`json … end` is a block of JSON in value position. **An interpolation inside renders as one value of the
format** — a string quoted and escaped, a whole number written whole, a list as an array — which is the shell
rule one layer up, and the reason the block is worth having rather than a multi-line string: `"{{ x }}"` is as
wrong here as it is in a `$` line. It is validated at **parse** time with each interpolation replaced by a
quoted-string placeholder (valid as a key *and* as a value, which a bare `null` is not), so a missing brace
underlines in the editor and the values cannot change the shape; and again after rendering, since shipping a
malformed document is the failure worth paying a parse for.

**The formatter lays a block out** -- `run :lint`, and format-on-save -- unlike an `exec` body. An `exec` body is somebody else's language and only its
base indent moves; a structured block is a format this runner knows, so it gets the same treatment as
everything else in the file — one member to a line, nested a level in, tabs like the language around it, and
an empty `{}` or `[]` left on its line. It is laid out **from tokens copied out of the source**, not by
re-serializing a parsed document: a string keeps its own escapes (`\u0041` is not rewritten as `A`), key order
holds by construction rather than by a `preserve_order` feature flag, and `{{ … }}` survives whole as one
token. A body that will not tokenise is left exactly as written, the same fallback the shell tracing takes.

That layout is only possible because **a block's fingerprint is its tokens, not its text**. `ast::StructuredBody`
carries the format and hand-writes `Debug` as the canonical token stream, which is what `fingerprint` hashes —
so reformatting is not an edit, and neither the formatter's own before/after check nor the prepare gate on
`setup.run` sees one. Tokens are separated, so `[1, 2]` and `[12]` still differ. It is the same reasoning that
strips spans: a change of layout is not a change of meaning, and a change of content still is.

`structured.rs` holds one enum: a format answers what its keyword is, how a value is written, what stands in
while checking, whether a document is well-formed, how it is laid out, what its canonical form is, and what an
editor should highlight the body as. A format whose whitespace is significant would answer the last two with
the text unchanged. Adding YAML or TOML is a variant and those arms, plus a word in the tree-sitter scanner's
format list and one alternation in the TextMate rule. `create-buckets.run` was the case for it — 674 characters with 54 `\"` —
and converting it found the escaping had been hiding a bug: `run` arguments are values, so a `"…"` word keeps
its quotes, and the AWS CLI was being handed a JSON *string* where an object was required.

### `retry`

`retry n [every s]` … `[else …]` `end` runs its block again while it fails. Four wait loops in the corpus were
shell `until … do sleep … done`, three of them re-implementing an attempt counter and an error message by
hand. The block is run with **`ignore_errors` forced off** — a retry that could not see failure would run
exactly once, which is the least useful way for it to be wrong — and the target's own `exit()` is re-raised
rather than retried (a target the body *runs* that ends with one is a failure, and is retried as a `$ run` of it
would be). It may be a parallel branch or sit inside one: a branch is ordinary code walked in order, so its
attempts are its own. (`.parallel` had to refuse it, having no way to collect what a body would run before
it had run.)

### Loops

`for` walks a list; **`while`, `until` and `loop` ask a question instead**, and all four take `break` and
`continue`. One `Statement::Loop` carries a `LoopTest` of `While(expr)` / `Until(expr)` / `Forever`, so every
walker that handles one handles the three -- `until` exists because the reverse question is the one a wait
loop asks, and four in the corpus were shell `until … do sleep … done`. The condition may be a capture, the
same rule an `if` follows, which is what makes `until $ curl -sf health` read as what it is.

**`break` and `continue` are refused at parse time outside a loop.** Whether a line sits inside one is a
question about the text, so `P.loops` counts the enclosing loop bodies and an editor underlines a stray one
rather than a run finding out half-way. They travel out as `RunError::Break` / `Continue` -- an error is the
only path back out of a walk, the same reason `exit()` is one -- so **`is_stop` covers them**: a `retry` would
otherwise read a `break` as a failed attempt and run the body again, and `.ignore-errors` would forgive it
into a statement that plainly did nothing. `retry` is *not* a loop for this purpose; a `break` inside one
leaves the `for` around it.

**Under `--dry-run` a conditional loop walks its body once.** A preview performs none of the effects the
condition is asking about, so the answer it gets back never changes: `while` ran forever or not at all, and
neither is what would have happened. One pass is the honest thing a preview has to say. A `loop` with an empty
body also checks the interrupt itself, since `walk` only looks *between* statements and there are none.

There is no `parallel while`, `until` or `loop`: an iteration of one exists only once the iteration before it
has answered the question. Inside a branch any of them is ordinary code. `break` and `continue` meet a
parallel block as a wall -- see below.

### Parallel blocks

**`parallel do … end` makes every statement directly inside it a branch, and `parallel for` every
iteration.** The branches start together and the block waits for the last. A branch is a `$` line, a `run`, a
call, or a whole block -- an `if` with its `else`, a `for`, a `retry` -- and inside one everything runs in
order, like any other code: only what is marked `parallel` fans out, so a `parallel do` inside a branch fans
out again. `Statement::Do` and `For` carry `parallel: bool`; the walker's two arms hand a `true` to
`run::parallel_do` and `run::parallel_for`. `parallel` is claimed only in front of a keyword, so
`parallel = 5` is still a binding, and in front of any keyword but `do` or `for` it is refused with what it
takes.

It replaced the `.parallel` property, which was a **second walker**. `collect` went through a block ahead of
time and gathered its commands into one batch of `Leaf`s, so everything that was not a command ran first, in
order: every `let`, `print` and `sleep` before anything was parallel at all. Two adjacent `$` lines were
folded into one process and so were one branch, run one after the other under a block that said otherwise; a
`for` became one flat batch, so a service's `push` could finish before its own `build`; and it had to refuse
`retry` and every conditional loop, having no way to know what they would run. It also drifted from the real
walker three times -- over a nested block's properties, over the captures in its preamble, and over what a
leaf had to carry -- and each drift had cost a paragraph here. A branch is walked by the ordinary walker, so
there is no second description of anything left to drift.

- **A branch is a `Runner::fork`**: a runner with a copy of the scope, the chain, the environment and a label,
  walked by the ordinary `statement` / `walk` on a thread of its own under `std::thread::scope`
  (`run_branches`). `Scope` is `Clone`, and what must stay one thing across branches is a shared handle already
  -- the temp-file registry and the key pool -- so a branch's temp files are still cleaned up and the keyring
  still prompts once.
- **Nothing a branch binds reaches its siblings or the code after the block**, and so **assigning a name bound
  outside the branch is a parse error**: the value would be gone when the block ends -- a lost write rather
  than a race, but lost all the same, and an editor can say so. `P.branch` holds the names the branch being
  parsed bound itself: a `parallel for`'s names, each `let`, and a plain `for`'s names for the extent of its
  body only, since the runner puts a loop variable back when the loop ends. `check_reassign` refuses the rest.
  A `let` directly inside `parallel do` is refused too: it would be a branch of its own, binding a name nobody
  could read.
- **The `$` fold is broken inside `parallel do`**, so each `$` line is its own branch. `P.unfold_next` is set
  for the one statement parsed as a branch and taken by it, so nothing nested inside that statement is
  affected. Folded, `$ lint` and `$ test` were one process run in turn -- which is what `.parallel` did.
- **Every branch runs to completion before a failure is reported.** Stopping the others would leave a
  half-started set of services behind, and one failing says nothing about whether the rest should. A stop --
  Ctrl+C, `exit()` -- is reported ahead of any failure, since the first command to fail may be one the signal
  killed; among equals the first in source order wins. `.ignore-errors` on the block forgives each branch, the
  way it forgives each statement of a block walked in order.
- **`break` and `continue` cannot leave a parallel block**, whose branches run at once and so have no order to
  leave in. `P.fence` records the loop depth the innermost one opened at, and a `break` or `continue` may only
  reach a loop opened inside it. A `parallel for` is one of those loops for `continue`, which ends the
  iteration it is in -- that iteration is the branch -- and not for `break`, since every iteration is already
  running.
- **A branch reads nothing from the terminal.** A shell's stdin is null in one -- a label is what says it is
  a branch -- a missing input is an error rather than a prompt, since `fork` clears `Scope::ask`, and
  `confirm()` refuses unless `-y`, CI or `--dry-run` answers without asking. Two questions at once on one
  terminal cannot be answered. `Scope.branch` is how the pure library knows: it holds the branch's label.
- **The block's properties are every branch's**, and one written between two branches is the ones below it:
  `branches_of` sets the branches up in order before any starts, applying each trailing property where it
  sits. A `parallel for`'s list and its body's properties are worked out once, before any iteration starts, as
  a sequential `for`'s are.
- **Concurrency is unbounded**: a branch is a thread, and so is every iteration of a `parallel for`. A limit is
  a question for later, with no syntax yet.
- **Under `--dry-run` the branches run in turn**, in source order, so a preview reads the same way every time.

The formatter prints `parallel do` and `parallel for …` the way it prints the plain forms. In the tree-sitter
grammar `parallel` is an optional field on `do_statement` and `for_statement`, recognised by the scanner the
way `detach` is (see *editors/tree-sitter*); in TextMate it is a keyword only in front of `do` or `for`.

### Unpacking a list

`let`, reassignment and `for` all take **several names**: `Statement::Let`, `Assign` and `For` carry
`names: Vec<String>`, and `eval::destructure` is the one place the rule is spelled. **One** name binds the
value whole -- a list stays a list, because unpacking is what the commas ask for and nothing else should
change behaviour because a value happened to be a list. Several take the elements in order.

Too **few** elements is a hard error (`EvalError::Unpack`), because the names are a claim about the shape of
the value and a claim that does not hold is a mistake every time; extra elements are simply not asked for.
`_` is a position matched and thrown away, may be repeated, and cannot collide with a name or shadow one --
an identifier starts with a letter, so `_` is not one. `let _ = f()` therefore evaluates and discards.

**`else if` chains rather than nests in the source and nests rather than chains in the tree.** The parser's
`if_tail` builds the inner `If` itself, so one `end` closes the whole chain, and nothing downstream has to
tell a chain from a nest -- the formatter is line-oriented and prints back whichever was written. Trailing
text after `else` used to be dropped without a word, so `else x > 1` ran its block unconditionally; it is an
error now.

### A command's exit status

`if $ cmd` is true when the command succeeds, and `match $ cmd` dispatches on the exit code with `case "0"`,
`case "3"` and so on. Neither stops the target — a non-zero exit is the answer being asked for, not a failure —
and neither captures stdout, so output reaches the terminal as it would from a `$` line. `code_of($ cmd)` is
the same status as a number, usable as a `let` value or as a statement of its own (run it, ignore the result).

`code_of` is the **one** call a capture may sit inside. A capture runs to end of line, so the closing `)` is
found from the right and a command containing a `)` is refused with a message saying to split it out. An
`exec` block is never a condition: it closes on an `end` at its opener's indentation, which is the same `end`
the `if` around it would want.

**`code_of(run <target> [args])` scores a target**, and is the only call that may hold a `run` — a dispatched
target writes to the terminal like any other, so its status is the only value it has to give, and
`lines(run x)` would have nothing to read. It is refused at *parse* time, so an editor says so while it is
being written. The number is the one `$ run <target>` yields, because dispatching in-process is meant to stop
re-execing the binary and not to mean something else: a target that ends with `exit(3)` is scored 3 -- as is
one that ran such a target and let the status through, since a status nothing handles keeps its number on the
way up -- and every other failure is the 1 the CLI reports (a target is not its last command, so there is no
other status it could honestly carry). The failure is printed where the re-exec's own `[runfile] error:` would have appeared —
a status is an answer, so nothing further up will say what went wrong. A **refusal** is not a status and is
passed on: `RunError::is_refusal` covers Ctrl+C and a declined `confirm()`, which are a person stopping the
run rather than a target reporting how it went, and a re-exec propagates them too since Ctrl+C reaches the
whole process group. This is what `coverage.run` is for — run both halves, report one verdict — and what
`$ run x` was standing in for.

### `$` and `exec` in value position

`let files = $ git diff --cached --name-only` captures stdout. This replaced a `capture()` function.

**A capture may be a call's last argument** — `lines($ git ls-files)`, `for f in lines($ git ls-files '*.sh')`
— because it runs to end of line, so the `)` that closes the call has to be the last character of the line.
Anything after it would be part of the command. A `$` in any earlier argument says so (*"a `$` run has to be
the last argument"*), and a `)` inside the command is refused rather than guessed at. `code_of` was the only
call allowed to hold one; the rule is general now and `code_of` is an ordinary call, which cost it its
parse-time check — `code_of("x")` is caught by `exit_code` instead, because answering 0 would be a silent
wrong number. Three places had to learn the rule together: `capture_rhs` and the `for` header (parsing),
`value_of` (running it, since the pure evaluator has no process host — hence `functions::call_with`, the same
call with its arguments already evaluated), and `format::rhs` via `parser::is_capture_call`, since everything
from the `$` on is the shell's text and must not be re-spaced.

**An interpolation in a non-shell `exec` body is not shell-quoted.** `exec::body_is_shell` decides, and
`render` picks `interpolate_shell` or `interpolate_plain` accordingly. Shell quoting is the wrong quoting for
anybody else's language: it made `'it'\''s'` out of an apostrophe, flattened a list to bare words, and left a
path unquoted because nothing in it needed quoting — so a Python body read `/tmp/x.json` as division. Left
alone, the value arrives as itself and the author quotes it the way that language wants, which is the only
thing that can be right for every language. A shell body still self-quotes, which is what keeps `$ cp
{{ src }} {{ dst }}` safe.

### Comments

**A `#` opens a comment where it begins a word** — at the start of a line, or after a blank — and runs to the
end of it. That is the shell's own rule, and half of a runfile is shell: the text after `$ ` is handed over
whole, so the shell applies the rule there and `lexer::comment_at` applies it here, which is what makes one
sentence true on both sides of the marker. `a#b` is one word in either half, and `1#c` is a mistake worth its
own message (`LexError::TightComment`) rather than a silently truncated line.

A comment used to be a whole line and nothing else, so the three places most worth annotating — an element of
a list that spans lines, a condition, a call — could not carry one at all.

**Three regions answer `None` and keep their `#`**: a string (scanned by the same `scan_string` the tokenizer
uses), a `{{ … }}`, and **everything from a `$` on**. The last is the important one: a capture runs to the end
of its line, so `$ sed 's/#//' f` and `$ curl "$u#frag"` are the shell's to read, and no lexer of this
language can quote-scan another's. It also means `f($ cmd) # note` is refused — the `)` has to be the last
character of the line, which is the rule that was already there. An `exec` **body** keeps its `#` for the same
reason, being somebody else's language; its `end` does not, being ours.

An `exec` **command** does take one, unlike a `$` line. The two look alike and are not: the runner reads that
line itself — `split_command` splits it into words and spawns the program directly — so a `#` left in would
arrive as an argument nobody meant, while a `$` line's reaches a shell that applies the same rule we do. The
escape hatch is quoting, which `split_command` honours; a `run` argument keeps its quotes, so there it is
`{{ "#general" }}`.

`lexer::code` is the one place a line is stripped, and it is called from `logical` (so a spilled list's lines
are stripped before they are joined), from `property`, `case_label` and the `exec` header, and from
`parser::closes_body` — which the formatter shares, since a closer rule the two disagreed about would move a
line into or out of a body, and which the language server asks to find a body in a document that does not
parse. The formatter re-attaches the comment one space out and never touches its text:
it is prose, and re-spacing an author's sentence is not what a formatter is for. Nothing in the tree records
a comment, so the fingerprint check cannot catch a dropped one — only a test can.

### Names resolve before anything runs

**A call to a function the language does not have, and a read of a name no line before it can have bound, are
refused before a target's first statement runs** -- by `runfile_lang::resolve`, which the runner
(`Host::load`) and the language server (`analysis::diagnose`) both ask, and `run :lint` through the server's,
so an editor underlines exactly what a run refuses. `exists("x")` used to parse, sit in a branch that runs once a month, and fail the month it did; a
typo read under a `?` never failed at all, since the fallback was taken every time.

**What is in scope follows the runner exactly**, because a check that disagrees with it either refuses a file
that runs or passes one that fails -- and `runfile-runtime/src/tests/names.rs` holds the runner to each rule the
check relies on. Bindings are flat: a `let` inside an `if` outlives its `end`, so a name bound on *any* path to
a line counts. A loop's own names end with the loop. A pass of a loop, or an attempt of a `retry`, sees what the
one before it bound, so a name bound anywhere in the body counts anywhere in it, a condition included. A
parallel branch binds on a copy. A sequential `for` puts its names aside **before** its body's properties are
worked out -- once, before the first item is bound -- so `.workdir = dir` at the top of `for dir in …` was
always `dir is not defined` at run time, and is now said where it is written (a `parallel for` leaves an outer
binding in place). A `_shared.run` binds with its top-level `let`s only: `of_shared` checks one the way
`fold_shared` applies it, still checking what never runs, and letting nothing it binds out.

So a name is unresolved **where no binding of it can have run before it on any path** -- exact in the direction
that matters, since a file whose every read can be reached bound is never refused. A read under `?` is checked
like any other: a name nothing binds is not a failure that comes and goes. A reassignment of a name nothing
bound is refused too (`Kind::Rebind`), though the runner would bind it: `x = 2` *rebinds*, and a rebinding
that binds is how a slip in the name being updated goes unseen while the name meant keeps its value.

Every problem is reported at once: `HostError::Unresolved` prints one `[runfile] error:` line each, the prefix
included on every line after the first, since an editor underlines them all together and a run reporting one
per attempt is the slow way to the same list. A shared file is checked ahead of its target. A dispatched target
is checked when it is loaded, as a parse error in one is. `run --stdin-args` asks `Host::check` before it
prompts, so nobody answers questions about a file that is then refused.

**A `RUN` key `RUN` does not have is a name that does not resolve too** (`Kind::RunKey`): `RUN.oss` failed only
when reached, as `unknown RUN.oss`. Which keys exist never depends on the chain, so it is reported under
`Chain::Unknown` as well. `runfile_lang::eval::RUN_KEYS` is the list; `populate_run_context` is held to it in both
directions by a runtime test (`user` being the one key the environment may leave out), and the language server's
documented `RUN_KEYS` by another.

`functions::exists` is the one answer to whether a name is a function: `FUNCTIONS`, `code_of` (the runner's),
and `try` (the evaluator's -- unlisted since `?` replaced it, but still answered for the files that call it, so
refusing it would refuse files that run). The "did you mean" is `resolve::suggest`, in tiers, best first: the
same word in another case; one letter out; one `_`-word of a longer name, which is how `file_exists` gets
misremembered as `exists` and which no small edit count reaches; two letters out. Only the best tier with
anything in it is offered, so `exit` is never suggested beside `file_exists`.

**The check needed exact positions, and the tree did not have them.** A span was wrong in most places a name
can sit: an indented line counted from column zero, a trimmed value from before the blanks it lost, an
interpolation from its `{{`, a string's interpolation from the start of its expression rather than the file,
and every name on a spilled list's later lines was placed on its first. `parser::offset_in` works an offset out
from pointers rather than lengths, since a length cannot say how much a `trim` took off the front. `logical`
joins a spilled list's lines with blanks of the same length and keeps the newline, and `tokenize` and
`split_interp` count newlines, so a token is on its own line at its own offset. A continued `$` line is split a
line at a time and its literals rejoined (`joined`), because a `\r\n` gives the joined text no single offset.
`tests/spans.rs` asserts, for every name in every position and every `.run` file in the repository, that the
source at its span spells it, on the line it says. Spans are stripped from fingerprints and golden trees, so
none of this moved a prepare gate -- except `split_words`, which now slices a word out of its text: rebuilding
one a byte at a time read `ñ` as `Ã±`.

In the language server, a span becomes a range once the source at it spells the name; a `Rebind`'s names carry
no positions, so its name is found as a whole word on its line. While a `_shared.run` above the document does
not parse -- mid-edit, usually -- what the chain binds is not known, and `Chain::Unknown` checks calls only.

**The editors mark an unknown call from a list**, since neither grammar can ask the runner. The TextMate
`function` and `capture-call` rules colour a known name `support.function.run` and any other
`invalid.illegal.unknown-function.run`, which every theme draws as an error. The tree-sitter call patterns carry
an `#any-of?` list, so an unknown name keeps the plain identifier's colour instead of looking like a call that
works -- no capture is drawn as an error by every editor, so the error itself is `run :lsp`'s. Unresolved
*names* cannot be marked by either grammar, needing scopes; they are the server's alone.
`runfile-lang/tests/editor_grammars.rs` holds both lists to the runner's in both directions.

### What fails every time is refused too

**A line that fails whenever it is reached, or that can never do what it says, is refused before a target's first
statement runs** -- by `runfile_lang::check`, which `Host::load` asks after the name check, the language server
asks for every document, and `run :lint` asks through the server's function. `ARG.port + 1`, `if ENV.CI`,
`sleep(ARG.seconds)`, `split("a,b")`, `xs[-1]`, a regex that does not compile, `if code_of($ make)`: each used to wait
for a run to reach it. `RUN.os == "darwin"` failed nothing and was worse for it -- false everywhere, with nothing to
say why. `LANGUAGE-CHECK-RULES.md` lists the rules (`arity`, `wrong-type`, `never-equal`, `unreachable-case`,
`invalid-literal`, `capture-position`), gated like the shell's document. A finding has the shell checker's shape --
`check::Finding` and `check::Rule` are runfile-lang's, and runfile-shell re-exports them -- so `HostError::Findings`
reports both in one sorted list, and one function turns either into a diagnostic.

**The rule is the shell checker's: report only what is wrong every time.** A value's type is what `types.rs` can be
sure of: a literal's, a source's (`ARG` and `ENV` strings, `FLAG` a bool, `ARGS` a list of strings), an operator's,
and a call's, from `SIGNATURES`. **A name holds the union of everything any line binds it to, anywhere** in the file
and the `_shared.run` chain -- flow is ignored on purpose. A failure can leave a `let` undone under `.ignore-errors`,
in a `retry` that gave up, or in a loop that went round again, so a flow-sensitive answer has to model every place a
run carries on after one, and a place it missed is a report about a value the name can hold. The union has nothing to
miss; what it gives up is `let n = ARG.n` then `n = number(n)`, the file that already remembered to convert. Under
`Chain::Unknown` every name is anything. A report needs the type to be disjoint from what the place accepts -- `ENV.PORT
? 3000` is never reported, being sometimes right -- and `Ty::NONE`, what `exit()` and `error()` answer, is never
reported at all. What a value that always fails is then asked is not reported either: the run never gets that far.

**`SIGNATURES` is held to the functions, both ways, by calling them**: `every_count_of_arguments_the_table_refuses_is_refused_by_the_function`
calls every function with zero to five arguments, and `every_type_the_table_refuses_is_refused_by_the_function_and_what_it_answers_is_listed`
with every combination of the four types, asserting a `TypeError::Expected` exactly where the table refuses and a
return type the table lists. The second direction is what the rest rests on: a type worked out from a call that
leaves out what the call can really answer makes a later check wrong. `contains` is the one function whose accepted
type depends on another argument (a list is searched for anything, a string only for a string). Two functions accept
more under `--dry-run` than for real -- `write_file` and `temp_file` do not read their contents there -- and the table
follows the real run: a file that dry-runs clean and fails for real is still refused.

- **`never-equal` follows literals only for `RUN.os` and `RUN.arch`.** `OS_NAMES` and `ARCH_NAMES` are the only
  values they take -- `os_named` and `arch_named` in the runtime are held to them -- and a name is followed only
  when every line binds it to one of those. A string literal is not followed through a name: `let mode = "dev"`
  above `if mode == "prod"` is a setting edited by hand, and calling it a mistake would be the mistake. A fix names
  the platform's own name for an alias (`darwin` → `mac`, `x86_64` → `x86-64`). This found `RUN.arch == "x86_64"`
  in this repository's golden fixtures and `notes == "false"` (a bool) in a real release target.
- **`unreachable-case` asks how a `match` writes its subject**: `to_string`, so a bool is `true` or `false`, a
  number is `format_num`'s text, and `match $ …` an `i32` exit status; a string or a list can be written as anything.
  A label written twice is unreachable whatever the subject.
- **`invalid-literal` compiles what is written out with what the run uses**: `regex::Regex::new`, the `globset`
  builder `glob` uses, `now_formatted`, and `functions::parse_format` -- split out of `render_format` so a `printf`
  format is read one way before a run and during one. The count message is `format_count`'s, which is why a static
  refusal still says `more substitutions`.
- **`capture-position` is where `value_of` does not run.** The runner runs a capture as the whole value of a `let`,
  a reassignment, a call statement or a `for` (and as the last argument of a call that is), and as the whole of an
  `if`, `while`, `until` or `match`; everywhere else the pure evaluator meets it and fails with *"capture needs a
  process host"*. That is a condition's call (`if contains("a", $ cmd)`, `if code_of($ cmd)`) and every
  `_shared.run` `let`, which `fold_shared` evaluates with `eval_boundary`. A runtime test runs each and holds the
  runner to the rule. `code_of` given anything but a capture or a dispatch is `wrong-type`.

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
.cicd/                         # Gitea Actions (git.joaoverona.com reads only this): CI, audit, every release
.cicd/release-assets/          # install.sh / install.ps1 -- RUNFILE_CHANNEL picks gitea (default) or github
.github/workflows/             # GitHub Actions for the mirror: run on the `github` branch `run mirror` pushes
.github/actions/setup/         # The only shipped action: installs `run`, PATH, optional secret keys
runfiles/                      # This project's own targets (self-hosting); ci/ and wsl/ are namespaces
editors/vscode/                # The VS Code extension (TypeScript) + its own runfiles/
editors/tree-sitter/   # The tree-sitter grammar (Zed, Neovim, Helix) + its own runfiles/

crates/
  runfile-lang/                # Lexer, parser, evaluator, values, the function library
  runfile-discovery/           # Finding runfiles/ directories and building the catalog
  runfile-runtime/             # Properties, env building, process spawning, the walker, dispatch
  runfile-lsp/                 # Language server (a library; served by `run :lsp`)
  runfile-shell/               # The shell checker: `$` lines and shell `exec` bodies, read the way bash reads them
  runfile-cli/                 # The `run` binary
  runfile-env/                 # .env parsing and env-map building
  runfile-crypto/              # AES-256-GCM for encrypted env values
  runfile-state/               # Prepare state, OS credential store access
```

## Crate Responsibilities

### runfile-lang

`ast.rs` carries `LoopTest` beside `Statement`. `eval.rs` holds `destructure`, the one description of what a
comma-separated left-hand side means.
`ast.rs` also holds `Constant` and `Expr::constant_non_bool` -- the one description of "a value the parser can
read straight off the page", which the flag rule is about -- plus `Statement::span` and the
`Block::declaration()` / `Block::trailing()` split that gives a block's two property regions.
`args.rs` classifies a command line against what the target reads — the one place the `--key value` rule is
spelled, so the runner and the `--stdin-args` prompt cannot read the same words two ways.
`format.rs` is the pretty-printer. `lexer.rs` is a hand-rolled scanner (`skip_interp`, `split_interp`, `scan_string`, `tokenize`). `parser.rs`
classifies lines, then climbs precedence for expressions. `eval.rs` holds `Scope` and evaluation; `functions.rs`
holds the pure standard library plus `call_io` for filesystem and regex; `value.rs` holds `Value` and shell
quoting.

- `Scope.private_keys` is a `Keys`: a **deferred, memoized** key pool. Loading is deferred because the pool
  comes from an OS credential store, and a locked keyring blocks on an interactive unlock prompt — an eager load
  turned every `run <target>` into a hang. Memoized so a run that decrypts twice still prompts once — **across
  its targets, not only within one**. `Host::key_pool` makes one pool per run and hands a clone to every target
  the run dispatches, `fork` shares it with every parallel branch, and the clones share one cache. It used to
  be a `Keys::new` per target in `Host::prepare`, so two targets reading one encrypted `.env-file` through the
  `_shared.run` above both unlocked the keyring twice in one run, and the test that said otherwise never saw
  it: its pool could decrypt nothing, so the first target failed before it ran the second. The watch probe
  takes the pool too, and the run after it has what the probe loaded. **`Host::run` drops the pool on the way
  out, however the run ended**, so each watch iteration is a run of its own and asks again. That is the
  decision, and it is `watch.rs`'s own promise that the next save is the retry: a pool kept for the session
  would remember a keyring that was locked, or a key not yet added, until someone restarted it. The cost is
  one credential-store read for each iteration that decrypts, which an unlocked keyring answers without a
  prompt. `tests/keys.rs` holds both halves -- one load across targets, branches and the probe, and a second
  run on the same host that finds a key the first could not.
- `Scope.dry_run` exists so `write_file` and `decrypt` can refuse to write, and so `confirm` does not ask. A
  preview that edits the working tree is worse than no preview, and one that stops to ask permission for what
  it is not going to do is not a preview at all.
- `FUNCTIONS` is exported and driven into editor completion, with tests in **both** directions: every listed
  name must dispatch, and every dispatch arm must be listed. Only the first existed at one point, and `min`
  sat implemented but unlisted, so completion never offered it.
- `Scope.temps` is a `TempFiles`: a shared handle, not a process-global, holding what `temp_file` and
  `temp_dir` made. `Host::cleanup_temps` drains it however the run ended, which is the point — a target that
  fails half-way is exactly when a decoded credential must not be left in the temp directory. Watch mode
  drains after every iteration.
- Binding names are validated in both `let` and reassignment; a block closer (`end`/`else`/`case`/`default`)
  with nothing open is a parse error rather than an expression statement.
- **`exit()` ends the target it is written in, with a status.** It leaves as `EvalError::Exit`, since an
  error is the only path out of an expression, and *every* catcher inside the target re-raises it: `try`, a
  `?` chain, `.ignore-errors` and `retry` all let it through, the way an interrupt is not something a target
  gets to shrug off. **Where the target ends, it becomes how the target went** (`dispatch::ended`, asked of
  `run_inner` whole, so a `_shared.run` `let` counts too): 0 is `Ok`, and the line that ran the target carries
  on; anything else is `RunError::Exited`, a failure of that `run` line and *not* a stop -- a caller's
  `.ignore-errors` forgives it, `retry` has another go and `code_of` scores it, exactly as at a `$ run` of the
  same target. It used to be a stop all the way up, so a helper's early `exit()` ended the whole run and skipped
  the rest of its caller -- and `exit(0)` reported that run as a success, a build that never happened called
  done -- while `$ run` and `code_of(run …)` handed back: the in-process form, the normal one, was the one that
  meant something else. A status nothing handles keeps its number on the way up, and the run ends with it; that
  is the one number a re-exec cannot match, since its caller knows only that a `$` line failed, and reports 1.
  A status is a byte, as a process's is: `exit(-1)` is 255 and `exit(256)` is 0. `RunError::exit_code` answers
  for both forms, and is what the CLI reads to set its own status instead of printing an error. An `exit()` in a
  property's value is the same `exit()`: `From<PropError> for RunError` unwraps an evaluation error rather than
  keeping it as the property's own, which had made one neither a stop nor a status -- `.ignore-errors` forgave
  `.env.X = exit(3)` in a block while refusing to forgive `exit(3)` on the line below it. Ctrl+C and a
  declined `confirm()` are not statuses, and still stop everything (`is_refusal`). Of the 21 `exit()` calls in
  the author's 1,224 runfiles, three were already this early-return idiom, and none sat where the change moves an
  outcome: every caller that dispatched one in-process had nothing after the call, forgave nothing and retried
  nothing.
- **Every call is written with parentheses**, `exit()` included — there is no bare-word form.
- **A statement that computes a value and discards it is a parse error.** `exit`, `abc`, `35`, `"hi"`,
  `ARG.x`, `x + 1` — a line that is only a value is always a mistake, most often a call with the parentheses
  left off, so the message says which: *"`exit` is a function; call it as `exit()`"*, else *"this line does
  nothing: …"*. `has_effect` looks for a `Call` or `Capture` anywhere in the tree, so `f() ? "x"` and
  `a() && b()` are statements while `x[0]` is not; deliberately conservative, since being stricter would mean
  deciding which functions are pure. At **statement** level this is safe to check while parsing — the line is
  inert whether or not the name is bound — so an editor underlines it. Inside an *expression* a bare name may
  well be a binding, so the same "is a function" hint lives in `eval` too, where `sc.vars` is consulted first
  and `let first = …` stays legal.
- **A `case` label is a quoted string.** A subject is a value and a label is compared against it, so
  `case linux` asked about a string while looking like a bare word; `RUN.os` is a string like any other. One
  rule holds instead: a string is in quotes wherever it appears. The grammar and `GRAMMAR.ebnf` say the same.
- **The formatter is line-oriented, not an AST printer.** The tree keeps no comments and folds a run of `$`
  lines into one statement, so printing from it would delete what a person wrote. Tokens are rendered from
  their **source spans**, so a string — raw prefix, escapes, interpolations — is copied out verbatim and
  nothing has to be reconstructed. Three regions are never touched: strings, the text after `$ ` (a
  continuation line reaches the shell *raw*, indentation included), and `exec` bodies (only the base indent
  moves, which the parser strips anyway). An `exec` **command** is not re-split on whitespace the way a `run`
  argument is, so its internal spacing is left alone too. `format` re-parses its own output and compares
  `fingerprint`s before returning: it cannot change what a file means. It refuses source that does not parse,
  since reindenting unclosed blocks is guesswork. `case`/`default` sit at the `match`'s own level, and a
  `for x in [` spills its list at the header's level rather than the body's — a `case` is indented *inside*
  its `match`, since three keywords at one level read as three separate things. Blank lines are **added, never
  removed**: after the description, between properties and the body, around a run of `let`s (a run is one
  group), before a block opener and after an `end` — but never between a comment and the statement it is
  about, and never pushing the first line of a body away from what opened it. Removing an author's own blank
  lines would be arguing with them; adding the missing ones is not. Gated by
  `runfile-lang/tests/repo_format.rs`, the `.run` equivalent of `cargo fmt --check`.
- `Statement::Exec` carries `lines: Vec<usize>`, the source line of each body line. The two are not derivable
  from each other: a `$` run skips blank and comment lines, and a backslash continuation folds several source
  lines into one.
- **A statement's span reaches its last line.** `block` stretches every statement's `span.end` to the end of
  the last line it read — a block's `end`, a spilled list's `]`, a run's last `$` line — which an `exec`
  block's already did. Where a block *ends* was otherwise nowhere in the tree, and completion has to know
  whether the cursor is still inside the `for` that binds a name. Nothing else notices: every other reader of
  a statement's span reads `.line`, and spans are stripped from the fingerprint and the golden trees. An
  `else if` is the one exception — an `If` of its own inside its chain's `else`, closed by the chain's one
  `end` — so only the `if` that opened the chain reaches it.

### runfile-discovery

Walks **up** for the nearest `runfiles/`, then **down** for `*/runfiles/` (depth cap 3, skipping
`node_modules`, `target`, `dist`, `build`, `.git`, `vendor`). Nested directories become `:`-separated namespace
segments. The machine-wide directory is `$HOME/.runfiles/`, `$HOME/runfiles/` or `$HOME/Runfiles/` — a
**fixed set of names with no setting to add to it**, so a person can show the folder or hide it without
telling the runner. **None of them is read in CI**: `main::discovery_home` answers `None` there, which is the
whole gate — one call rather than an `is_ci` in the catalog, `:list`, `:lint`, `:generate` and `:complete`
each. A runner's home directory is nobody's (a hosted one holds whatever the image shipped, a self-hosted one
belongs to the machine's owner), so a target no reader of the repository can see must not join the run or
shadow a checked-in one of the same name. It is also why nothing *cleans* `$HOME/.runfiles` on a runner: a
directory that is never read is not a leak, and deleting a self-hosted runner's own would be pure destruction. **Exactly one of the three may hold anything**: two populated ones is `AmbiguousGlobal`
naming both, since merging them would let one target shadow another invisibly. An *empty* one never clashes
(a leftover `mkdir` must not stop a run), and `$HOME/runfiles/` found by the upward walk is not collected a
second time as the global. This replaced `includes` entirely.

- **The anchor rule**: the parent of `runfiles/` is the single anchor for cwd, `.env-file`, `.add-path`,
  `glob`, `read_file` and `{{ RUN.parent }}`.
- **`_shared.run` layers by directory.** `Catalog::shared_chain` returns every one that applies, outermost
  first, so `runfiles/api/_shared.run` adds to `runfiles/_shared.run` rather than replacing it. Only the top
  one used to be registered at all, so a nested one was read by nothing. The walk stops at the **anchor**: a
  subproject does not inherit the root's, for the same reason its targets are namespaced. It is walked from
  the target's own **path**, never looked up by namespace — a namespace is not unique across trees, the
  machine-wide one has none, and `Catalog.shared` recorded a key whether or not the file was there, so keying
  by it meant that merely *having* a `~/.runfiles` silently disabled the root `_shared.run` of every project
  on the machine -- and hid it from `:lint`, until that map was keyed by path too. Its properties *and* its
  `let` bindings apply, which is what makes it the `globals` analog
  -- **folded in source order** (`run::fold_shared`), so a property below a `let` reads it. Shared files are
  never walked, so when properties became applied-where-written the fold had to learn it separately: it kept
  applying only the region above the first statement, and `let d = "x"` then `.env.DIR = d` dropped the
  property **without a word**, where it had at least been an error before. Unlike a walk, a property below the
  last `let` still applies -- in a shared file, after the value it is computed from is exactly where a
  setting sits. A `let` there reads the environment the lines above it set, through `keep_current`, the same
  as a header value does. Its other statements never run: a `print` in a `_shared.run` is not an action taken
  once per target, it is nothing.
- **`resolve` is one hash lookup, and a target is reached by its file name only.** `.alias` is gone: a target
  used to answer to a second name declared in its own file, found by scanning every *other* file's declaration
  region on a miss. The property was the last thing in the language read from the **text** rather than from a
  value — only a literal string could be, since discovery runs before any scope exists — and it enforced that
  in the loosest possible way: a list literal registered nothing, `"k{{ x }}"` silently registered `k`, and
  `"{{ x }}k"` silently registered nothing. Meanwhile `Props.aliases` was filled by the *evaluator* and read
  by nobody, so `.alias = concat("a","b")` ran, succeeded, and registered nothing while `.alias = nope` failed
  the run. One name per target instead, and `.alias` is refused with that sentence (`PropError::Replaced`).
- **`.only-in-directories` scopes a machine-wide target, per file.** Registered everywhere, active only
  inside the paths it names. Compared on path components, so `work/acme` does not admit `work/acme-other`;
  `~` expands, and a relative entry anchors to **home** at every level, so one spelling means one directory
  however deep the file naming it sits. It was read from `$HOME/.runfiles/_shared.run` and nowhere else,
  while `Props.only_in_directories` sat filled and read by nothing -- so a *target* file naming its
  directories was accepted, offered by completion, and did nothing, and so was a nested `_shared.run`. It is
  judged as the tree is walked now: every level that names directories has to cover the working directory, so
  a `_shared.run` scopes its whole subtree and a target **narrows** within it and can never name its way back
  out. A subtree that excludes us is pruned whole, which keeps the common case to one file read rather than
  one parse per target; elsewhere the property has to appear in the text before a file is parsed at all. A
  **list literal used to read as no scope**, which failed *open* -- the directory became active everywhere,
  the opposite of what was written -- so a value that cannot be read this early is now
  `DiscoverError::UnreadableScope` rather than silence. A file that does not *parse* is still left alone: it
  is broken whatever it says, and refusing would take every other target on the machine with it. **A project
  file may not set it at all** (`PropError::NotMachineWide`): its targets are visible to anyone reading the
  repository, so hiding some by working directory recreates the invisibility the machine-wide rule exists to
  fix. `Props` carries `machine_wide` -- where the file was found, not a property -- because that is the one
  fact that makes the property mean anything, and it is `discovery::is_machine_wide` that answers, **not
  `Origin`**. The two ask different questions: `Origin` says how discovery *reached* a file, and
  `$HOME/runfiles` found by the upward walk is reached as `Local`, so standing at or below the home directory
  made every machine-wide target refuse its own scope with *"this target is part of the project"*. It is also
  the function the language server asks, and one question with two answers is how an editor and the runner
  come to disagree. For the same reason the scope is now applied to that directory **whichever walk found
  it**: the three names are what make a directory machine-wide, so a file in one gets to say where it belongs
  even while it is also the nearest `runfiles/`. Being reached as `Local` still decides everything else --
  `:list` grouping, and what `:lint` and `:generate` leave out without `--include-global`.
- **A scope says where a target is offered, so only what runs asks it.** `discover` collects what is offered
  where `from` stands; `discover_unscoped` collects every file, reading no scope and so refusing none, and is
  what `run :lint` and the language server use. Checking a file is not running it, and a scoped file is
  checked from outside its scope as a rule -- an editor opens it where it sits, which is not a directory it
  names. Collected the scoped way, `:lint --include-global` skipped every scoped file unless run inside the
  directories they named, and a file checked anyway -- by path, or in an editor -- was checked without its
  `_shared.run` or its siblings, so `` `region` is not defined `` and `` no target named `build` `` were
  reported about a file that ran clean where it belonged. The cost is exact and small: a *project* file's `run`
  of a machine-wide target scoped somewhere else is no longer reported, which was a question about the working
  directory rather than about the file. A scope that cannot be read is `UNREADABLE_SCOPE`, underlined on its
  line by `analysis` through `scope_entries` -- the one account of what a scope may be -- since nothing that
  checks files reaches discovery's refusal any more. `:generate`, `:list`, `:complete` and dispatch keep the
  scoped catalog: each is about what can run here.
- **`Catalog.shared` is keyed by path, with the `Origin` of the tree it was found in.** It was keyed by
  namespace prefix, which is not unique across trees: both roots have the empty one, and the machine-wide walk
  wrote it whether or not it had a file, so any `~/.runfiles` at all hid a project's root `_shared.run` from
  `:lint` -- the collision `shared_chain` had been rid of, still alive in the one reader left. Only files that
  exist are recorded, and `:lint` filters them by `Origin` exactly as it filters targets.

### runfile-runtime

`props.rs` (property resolution), `env.rs` (env building), `exec.rs` (spawning), `run.rs` (the walker),
`dispatch.rs` (`Host`, target resolution, cycle detection), `shell.rs` (shell selection), `term.rs` (the
terminal's width, and how wide text is on it).

- **`Host::load` refuses a file for the language's findings and the shell's together** (`HostError::Findings`),
  once its names resolve, in the order they are written. `os_named` and `arch_named` are what `RUN.os` and
  `RUN.arch` say, and a test holds them to `runfile_lang::eval::OS_NAMES` and `ARCH_NAMES`, which the checker takes
  to be every value there is.
- **`PROPERTIES` carries the whole taxonomy, and is the only copy of it.** Three booleans per name --
  `block_scoped`, `declaration_only`, `flag` -- each tested against `extend`'s actual behaviour the way
  `FUNCTIONS` is, in both directions, so a mislabelled column fails rather than misinforming an editor. There
  used to be a second copy: a `BLOCK_SCOPED` const beside the list, and the language server describing the
  same rules again in `check_properties`, which is how the scope rule had already drifted once. `props::check`
  is the one function that answers everything about a property line without running it, and `analysis.rs`
  calls **it** rather than restating it.
- **Shell resolution**: bash → Git Bash (four known Windows paths) → sh, each step through `shell::locate`, which
  never answers with WSL's launcher -- a different filesystem, and on a machine with no distribution nothing at
  all. WSL puts a `bash.exe` in two places, `System32` (the Windows feature's) and `WindowsApps` (the Store
  package's App Execution Alias, on the user PATH by default), and one found in either is passed over for the
  next on PATH. `is_wsl_launcher` judges by the directory the file is in: the substring test before it
  (`\system32\`) knew only the first place, so `$` lines reached the Store's alias wherever it was the first bash
  on PATH, missed a PATH entry spelled with `/`, and took a `system32` further up for the launcher.
- **A shell the file names is located the same way.** `exec bash` and `.shell = "bash"` -- any shell `is_shell`
  knows, written without a path -- go through `exec::executable` to `locate`. Handed to the standard library as a
  bare word, a named `bash` was simply the first on PATH, which on a default Windows install is the launcher,
  while a `$` line beside it ran Git Bash. It is looked up on the PATH the command runs with -- the standard
  library's choice too, so `.add-path` still decides which shell runs -- while the default shell stays the
  runner's, from the runner's PATH. A path is handed over as written. A shell found nowhere is refused -- *could
  not start `bash`: not on PATH* -- rather than left to the standard library, whose search would reach the
  launcher; and `$0` in `exec bash` is now the full path, as it always was for `$` lines.
- `is_shell()` matches `sh|bash|dash|ash|zsh|ksh|busybox|brush` on the **first word only**, and adds `-e`
  **after** the rest of the words, in `program_and_args`. In front of them it went to the wrong thing:
  `busybox -e sh` asked for an applet called `-e`, and bash refuses `--posix` once it has read a short option.
  `brush` is there because it is a bash-compatible shell someone may name in `.shell`; without it such a
  target would run fine and silently stop stopping on failure. It is *not* a default candidate — the default
  has to supply the POSIX toolbox as well as the language, which a shell alone does not: 9% of the corpus's
  shell lines call one of 31 toolbox programs, and `sed`, `grep`, `awk`, `find`, `xargs` and `tar` are not
  coreutils at all. Git Bash ships both, which is why it is the Windows answer.
- **`Dispatch::run` writes the child's trace into the caller's own** rather than into shared state. A child
  finishes while its parent is still walking, so a shared buffer printed every dependency *before* the line that
  called it; the caller's buffer puts the trace where the call appeared, which is what makes `--dry-run` order
  match execution order. **It is written however the child ended**, and `Host::run` banks the top-level trace
  the same way: a target that stopped part-way -- at an `exit()`, or at a failure a caller forgives or scores --
  still ran everything above the stop. The trace used to come back only with `Ok`, so a preview dropped those
  lines, showed a caller carrying on after a call that seemed to do nothing, and printed nothing at all for a
  target that ended with `exit()`, 0 included. The CLI prints a preview however it ended, unless it stopped
  before anything was traced, when there is nothing to show but why.
- **A subproject calls its own siblings.** `run compile` inside `web/runfiles/` resolves `web:compile` first,
  falling through to a root `compile` when there is no sibling — so a file spells its neighbours the same way
  wherever `run` was invoked from.
- **`--stdin-args` asks before anything runs** (`stdin_args.rs`), from that same list. It asked lazily, when
  an `ARG.x` resolved to nothing — so it could only ask about inputs a run happened to *reach*, it asked after
  earlier statements had already done their work, and it never asked about anything with a fallback at all,
  because a chain that resolves raises no error to catch. A flag was never asked about either: an absent one
  is `false`, not a failure. Answers for arguments and flags are appended to the command line, which is where
  the runner reads them from; an environment answer is set in this process, which the target's environment is
  built on top of. The lazy prompt stays as a backstop.
- `Host::header_props` **probes**: it evaluates the declaration region only to read `.watch`, so it neither
  refuses an unread input -- a probe rejecting the command line would report the failure before the run that
  owns it -- nor lets a writing function write. Without the second half, `.env.X = temp_file(...)` made two
  files per run, one an orphan nothing referenced. An `exit()` it meets, in a header value or a `_shared.run`
  `let`, is the run's to carry out too: the CLI reads one as "no `.watch`" and runs, where it used to report it
  as `error: exit 0`, status 1.
- **Ctrl+C** is caught so the run can stop between statements, delete its temp files, and exit 130. The flag
  is process-global because a signal handler has nowhere else to write, but the runtime reads an injected
  predicate (`Host::interrupted`), so it reaches for no process state of its own and one test cannot
  interrupt another. `.ignore-errors` does not apply to it.
- `Dispatch` is `Sync` with `&self` and an explicit `chain: &[String]`. Per-path rather than shared, so
  parallel siblings are not mistaken for a cycle.
- **A parallel branch is a `Runner::fork`**, walked by the same `statement` and `walk` as everything else --
  see *Parallel blocks* above.
- **Everything the runner says while a target runs carries `[runfile]`** — the announcement, `error:`, the
  `.confirm` question, the `--stdin-args` prompt, watch-mode notices. A person reading a terminal is watching
  two things talk at once, and without the prefix `error: …` could as easily be the target's own output.
  `exec::tag()` is the one place it is spelled; the credential-store warning in `runfile-state` writes a plain
  one because that crate sits below the one that owns it, and is the **only** `warning:` left — the runtime
  had a `Host::warn` hook for non-fatal advice, and it went dead the day an unread input became a refusal
  rather than a warning. Nothing had called it since. A runner that warns about something is a runner that
  did it anyway, so the hook is gone rather than kept for a case that has not appeared. `run :env`'s own
  chatter is not target execution and is left alone.
- **`.logging` announces each command on stderr, and is off unless asked for** — the same default the old
  `logging` field had (`unwrap_or(false)`), and for the same reason: a target is run for its output, and a
  runner talking over it is noise. Block-scoped, so a `_shared.run` covers a directory the way `globals` used
  to. stderr, so a pipeline reading stdout is unaffected, and never under `--dry-run`, which already prints
  the commands to stdout. **Each command announces itself as it runs**, from *inside* the script: a block of `$` lines is one process, so the runner cannot observe when
  each line starts and could only ever print all of them before any of them ran — which put a failure at the
  end, under nothing. `exec::traced` puts a `printf … >&2` before each line instead. It returns `None`, and
  the block is announced whole as before, when that would be unsafe: a `for` or an `if` spread over several
  `$` lines is one command to the shell, and so is a quote or a heredoc that spans them, so anything ending
  in a continuation and anything opening a quote falls back. Measured over the corpus: 238 of 242 multi-line
  blocks are traced, 4 fall back, and none is broken by it. A failure **never names the shell**, for the same
  reason a parallel branch is not labelled `bash`: `` `docker compose up -d` exited with status 1 `` where
  the runner knows the command, and *"the command above"* where several share a shell and only the
  announcements can say which one stopped — but only when the block was traced. A block announced whole has
  no single line above to point at, so it is named instead (*"a command in `for f in a b; do …`"*): pointing
  at output that says something else would be worse than saying less.
- **On Windows that argument has to be quoted by hand when it holds no space.** The standard library quotes
  an argument containing a space or a tab and nothing else -- a newline does not count -- so a block whose
  every line is a bare word (`true`, then `false`) reached the command line bare, and the shell's own parser,
  which *does* treat a newline as a separator, saw several arguments and ran only the first. Everything below
  line one was dropped and a block that should have failed succeeded. `exec::windows_quoted` applies
  `CommandLineToArgvW`'s rules, and only where the standard library would leave the script bare: a block with
  a space anywhere in it -- almost every real one, which is why this went unseen -- was already quoted and
  keeps the path it had.
- **A shell gets its script as an argument (`-c`), so stdin stays the terminal.** Handed over on stdin
  instead — which is how `exec <command>` works, and how this did — every interactive command inside it read
  a pipe the runner had already written and closed: `ssh` announced *"Pseudo-terminal will not be
  allocated"* and then sat there unusable, and so would `vim`, a REPL, or anything asking for a password.
  `exec <command>` keeps the body-as-stdin contract, since that is what `exec tee file` and `exec python3`
  are for. A detached shell gets `/dev/null`: a background process must not hold the terminal's input after
  the run that started it is over.
- **`detach`** starts the command and does not wait. Its streams go to null: inherited, they would hold the
  runner's own stdout and stderr open after it exits, so whoever is reading them waits for the very command
  that was meant to outlive the run. **On Windows that is not enough** -- `Stdio::null()` says what a child
  *uses*, not what it *holds*, and `CreateProcessW` is called with `bInheritHandles: TRUE`, so every
  inheritable handle in the runner is duplicated into the child anyway, the pipes its own stdout and stderr
  arrived on included. `exec::KeepHandles` clears `HANDLE_FLAG_INHERIT` on the three standard handles across a
  detached spawn and restores it after. Safe for whatever else is spawning at the time, parallel branches included,
  because `Stdio::inherit()` does not depend on that flag: the standard library duplicates the handle it passes
  with `bInheritHandle` set regardless. Unix needs none of it -- everything but the three descriptors a child
  is handed is close-on-exec. **What it cannot guard is a copy `run` never knew it had**, and no process in a
  chain can guard one for the process below it. An inherited handle arrives *inheritable*, and
  `CreateProcessW` is called with `bInheritHandles: TRUE`, so every process hands its own standard handles to
  every child a second time -- beside the ones it names in `STARTUPINFO`, at values the child cannot ask
  about. `Stdio::inherit()` duplicates afresh, so the copy a child receives as *its* standard handle is never
  the same one: clearing `HANDLE_FLAG_INHERIT` on `GetStdHandle`'s answer -- which is what `KeepHandles` does,
  correctly, for the copy `run` was given -- can only ever reach one of them. That is why the `detach` tests no
  longer leave a `sleep 30` running: five processes deep (`run` -> shell -> cargo -> the test binary -> `run`)
  the Gitea step's output pipe had arrived several times over, the detached command held it half a minute past
  the last test, and act_runner failed the step ten seconds in with "WaitDelay expired before I/O complete". A
  `cli.rs` guard over its own standard handles (`keep_our_std_handles`) was written for this and could not
  work; a detached command that waits for a file the test writes proves the same thing and leaves nothing
  behind (see *Testing Requirements*). The realistic one-level case is the one `KeepHandles` is for and does
  cover: a step's shell spawns `run` directly, so the pipe `run` holds *is* its standard handle.
- **Parallel output is labelled per line**, since several branches write at once: `label | line`, whole
  lines, stderr as well as stdout, and a `print` too (`functions::emit`, which reads `Scope.branch`). A
  `parallel for` iteration is labelled by its value, fitted to 40 columns -- the path out of a `glob` is
  exactly what tells two apart, where the command's first word, `docker`, told none of them apart. A
  `parallel do` branch is labelled by what it runs, **in as few words as tell it from its siblings**
  (`branch_labels`), so `cargo test` and `cargo clippy` rather than `cargo` twice: a dispatch is its target
  and arguments, a command its words -- the `exec` header's unless that names a shell, since every `$` branch
  would otherwise be called `bash` -- and a call its function. A block is its keyword and line (`if:12`), and
  so is anything no number of words tells apart (`print:3` beside `print:5`). The words are read off the tree
  and stop at the first interpolation: rendering would run what is interpolated a second time, and a
  `{{ temp_file(…) }}` would make two files. A nested `parallel` block gets an empty label and adds no segment
  -- its branches are what get named, so `web` rather than `parallel:9/web` -- while other nested branches
  join theirs, `web/cargo test`. An empty label is still a branch (`relay` and `emit` print its lines bare).
  The label is threaded through `Dispatch::run`, so a branch's dependencies carry the branch's name rather
  than their own. A sequential run inherits the terminal and adds no prefix: nothing to disambiguate, and a
  pipeline reading `run`'s output keeps working.
- **Each branch's label is painted a colour of its own**, cycling cyan, yellow, magenta, green, blue, red in
  the order the branches were made. A label is a name to read and a colour is not, which is what makes six
  interleaved dev servers legible at a glance; it was the one thing the `.parallel` rewrite dropped, having
  moved the prefix out of a module that painted it (`format_parallel_prefix`). **Siblings take consecutive
  colours**, so the branches whose output actually interleaves are exactly the ones guaranteed to differ -- a
  colour hashed from the label needs no plumbing at all and was rejected for this: three branches would have
  collided 44% of the time, which is the whole point missed. A nested set starts **past** its parent's colour,
  so neither half of an `outer/inner` label repeats the other. A seventh branch repeats the first, which is all
  a fixed palette can promise; the six are the codes a terminal themes for itself, so they read on a light
  background and a dark one alike.
  `Runner.colour` is what carries it, set by `fork` from the branch's position among its siblings and read only
  to offset the branches made inside it; a dispatched target starts the palette fresh, since a colour says
  which of the branches made *together* this is and a dispatch is not one of them.
  **`exec::paint_branch` is called where the branch is made, not where a line is printed**: the colour is a
  fact about the branch, so the label already carries it by the time it reaches `relay`, `emit` and
  `Dispatch::run` -- none of which had to learn a palette, and none of which can disagree about one. An empty
  label stays empty, or a `parallel` block that is itself a branch would be wearing a bare `|`.
- **A sibling set pads its labels to the widest of them, so the `|` stands in a column.** Eyes follow a
  straight edge, and a gutter that moves with the length of each name is read one line at a time. The width is
  exact rather than a guess because it is known before any branch starts: `branch_labels` works a `parallel
  do`'s labels out from the statements and `parallel for`'s come from the list, both up front, which is what
  `pad_width` is handed. Capped at 24 columns, the `LABEL_PAD_MAX_WIDTH` the deleted module settled on: one
  40-column path out of a `glob` beside three short names would otherwise leave every one of them trailing
  twenty-odd spaces to line up with a line they are nowhere near, so past the cap a label overruns its own
  lines and leaves the column where the rest of the set can reach it.
  Measured in display columns with `term::width_of`, not characters -- an escape code occupies none and an
  emoji occupies two -- and applied **after** painting, so the colour wraps the name and the spaces that line
  the gutter up are plain text. Because each segment is padded when its own branch is forked, the `/` of a
  nested `outer/inner` label stands in a column too, and two cousins' lines line up with each other; that is
  as far as it can reach, since one branch's set is made before another's list is known. An empty label is
  never padded -- a nested `parallel` block adds no segment, and a column of blank gutter in front of the names
  that do say something is worse than none.
- **`exec::paints` is the one answer to whether the runner colours anything**, per stream, and `tags()` and
  `help::colour()` both ask it. It honours `NO_COLOR` whatever its value, treats `TERM=dumb` as a terminal
  that would print the codes rather than act on them, and gives a pipe none. There were two copies of this
  rule and they had already drifted: `NO_COLOR` reached the help page and nothing the runner said while a
  target ran, so `[runfile]` stayed bold cyan for someone who had asked for no colour at all. `FORCE_COLOR`
  (anything but `0`) is the other direction, for a pager or a CI log viewer that renders what is, to us, a
  pipe; `NO_COLOR` beats it, since both set can only mean one was set long ago and forgotten. It is also what
  makes the painting testable at all -- a test's stdout is a pipe, so the shipped path is otherwise
  unreachable, which is why `cli.rs` strips `FORCE_COLOR` like the CI variables and the one test that wants
  colour sets it back. A branch's label is keyed on **stdout**, the stream a pipeline reads: `run dev > log`
  keeps the escapes out of the file, and the cost is a terminal's stderr going unpainted when stdout alone is
  redirected.
- **A `run` hands the target it runs its environment.** Whatever a command on that line would be given -- the
  caller's `.env`, `.env-file` and `.add-path`, a block's and a trailing property's, its `_shared.run` chain's,
  and what the caller was itself run with -- is where the called target starts, in place of the process's
  environment. `Dispatch::run` carries it (`env::handed_over` works it out from the line's `props` and
  `r.env`, for `run` and `code_of(run …)` alike), `Host::prepare` seeds `Scope.env` with it so a `_shared.run`
  `let` or a header value reads it before anything has been built, and `Props.inherited` carries it into every
  build after that -- like `machine_wide`, a fact about the target that survives every `extend`. Before this,
  `Dispatch::run` carried no environment at all: every target started from the process's, so a helper like
  `_aws` that expected its caller's credentials got none, while `$ run _aws` got them.
- **It is handed over in two halves, and that is the design.** A value only a `.env-file` supplied is a
  *default*, laid beneath the called target's own files (`EnvBuildParams::defaults`); everything else is
  *exported* and stands where a shell's variables stand, above its files and below its `.env`
  (`EnvBuildParams::base_env`, which replaces the process's environment outright, overlay included). Handed
  over whole -- all `$ run x` can do, since a new process is given only variables -- a caller's `.env` would
  beat the called target's own `.env.test`: `ci` running `test` would test against the development database.
  A value is a file's when neither layer that beats a file put it there: not what the caller was itself run
  with, and not the caller's `.env`. PATH is always exported, so a called target's own `.add-path` goes in
  front of its caller's -- a subproject's `node_modules/.bin` ahead of the root's that called it. An
  `.add-path` entry already on PATH is moved to the front rather than added again (`apply_add_to_path`), or a
  `_shared.run` both levels read would stack a copy per level; a PATH none of the entries is on comes back
  byte for byte. Nothing flows back: what a called target sets is gone when it returns. The caller's values
  were decrypted before they were handed over, so a called target never asks the key pool for them again, and
  `tests/keys.rs` counts that. `RUN.user` and `RUN.cwd` stay the process's. `runfile-runtime/src/tests/inherited.rs`
  holds each of these rules, `runfile-env/src/tests/called.rs` the layering beneath them, and `cli.rs` the
  whole of it against `$ run`, which must agree about everything but a caller's file values.
- `env::build` receives the same deferred key pool the `decrypt` function uses. It was previously passed `None`,
  which meant an encrypted `.env-file` value could never be decrypted at all.
- **Precedence is decided in one place, `runfile_env::build_env`: `.env-file` < the caller's shell < the
  target's `.env`.** A file is a default -- the dotenv convention, so a checked-in `.env` does not clobber what
  someone exported -- and the target's own assignment beats both, since it is the one way a target can force
  a value; a default the caller may override is written `.env.PORT = ENV.PORT ? "3000"`. For a target another
  target `run`s, "the caller's shell" is what that caller exported, and its file values sit below this
  target's files: `defaults` < `.env-file` < `base_env` < `.env`. It used to be
  described twice, with opposite answers: `build_env` put the shell on top, while `merged_env` laid the
  block's `.env` back over the built environment at spawn. So `.env.PORT = "3000"` under `PORT=4000 run t`
  printed `4000` for `{{ ENV.PORT }}` and `3000` for `$PORT`. The overlay existed because a block that set
  only `.env` was never rebuilt; `with_block_env` now rebuilds when `.env` differs too, so `merged_env` is
  `r.env` and nothing else. **What commands receive did not change** -- they always got the property; only
  `ENV.X` moved to agree -- with one exception: an `.add-path` beside a `.env.PATH` used to vanish, because
  the overlay replaced the whole value, and now it is prepended onto the assignment. That is also why three
  `runfile-env` tests that pinned "the shell's PATH beats `.env.PATH`" were rewritten rather than kept: they
  held for `build_env`'s output and for no command that ever ran.

### runfile-shell

`script.rs` (the text a statement or a capture hands its shell, each character placed at its byte), `syntax.rs`
and `words.rs` (reading it the way bash does), `rules.rs` (the rules that read commands), `walk.rs` (the walk
over a file, with what each name can hold where). `SHELL-CHECK-RULES.md` is the list of rules, gated.

- **It reports only what is wrong every time**, because a finding refuses a run: `Host::load` asks it after the
  name check, for every shared file and then the target, and `HostError::Shell` prints one `[runfile] error:`
  line per finding. Where the same text can be right, a rule says nothing. A double-quoted interpolation is
  judged only where no shell reads the word again -- a program, a redirection's file, an operand of `cd`, `cp`,
  `[` and the like -- since `ssh host "cd {{ dir }}"` and `echo "export X={{ v }}" >> rc` are correct *because*
  of the quoting the interpolation brings. There is no suppression comment: a rule that needs one is wrong.
- **It replaced ShellCheck**, which saw each script with a placeholder where an interpolation was: it could not
  tell the placeholder from a word nobody wrote, so its findings on those lines were noise and its columns there
  wrong, and it had to be installed -- so a gate built on it passed on one machine and failed on another. Here an
  interpolation is one opaque character (`script::HOLE`) standing for its whole `{{ … }}`, which is exactly what
  the runner makes of it: one quoted word.
- **A script it cannot follow is left alone** (`Stop::Lost`): `coproc`, a heredoc delimiter that expands, a `(`
  after a word that is neither an extended glob nor an array. Giving up costs a check; guessing costs a false
  report. Every script in the author's 1,194-file corpus is read without giving up.
- **The reading follows bash where bash is surprising**, each case pinned by a test against `bash -n`:
  `${x:-{a}` closes on the first `}` (no braces are counted), a `'` inside a double-quoted `${…}` still opens a
  quote, `$((` is arithmetic only when its last two parentheses close together, and a heredoc's body starts at
  the next newline token, never at one inside quotes.
- **`$` lines are read only when the shell they run in is known**: bash by default, or whatever `.shell` names
  across the chain (`script::dollar`). `zsh`, `ksh` and a computed `.shell` are not read, and neither is any `$`
  line while a `_shared.run` above does not parse.
- **Values are followed flow-sensitively, and no further than the text says** (`walk.rs`): a `let` replaces
  what a name held, branches join, a loop's body is walked twice so a pass sees what the one before bound, and a
  parallel branch binds on a copy. Only strings, names, `?`, lists and the calls that pass a value through
  (`concat`, `dirname`, …) carry anything; `ENV.X` and `ARG.x` hold nothing to report.
- **Rules that read a command's own grammar take bash's answer, and give up where it is not certain.**
  `conditional` holds `[[ … ]]` to bash's `cond_term` -- `-a` and `-o` are not operators there, two words need one
  between them, `-f` and `==` need a word after them -- and leaves alone a test holding an interpolation, which could
  render as an operator, a word `[[` splits at `(`, `<`, `&` or `|` where the checker did not, and a test over several
  lines, where bash skips some newlines and refuses others. It runs only in bash (`Script.bash`: `$` lines no
  `.shell` points elsewhere, or a block for `bash`), since busybox reads `[[` as `test`, where `-a` joins two tests.
  **A command the script defines as a function is skipped by every command rule** (`defines`): `test()`, `sort()`
  and `break()` are names bash lets a function have, and calling one runs the function. `truncated-input` reads options with a table per command
  (`READERS`), getopt's way; an option missing from it gives the reading up, so a flag that takes a value on one
  platform cannot turn that value into a file name. `exact` is a word's literal text, and refuses a `$'…'` escape,
  which `Word::literal` reads wrong.
- **What is not syntax is held to what bash does when it runs.** `bash_does_what_each_rule_says_it_does` runs a
  script for each such rule and asserts the failure, or the wrong answer, its message describes -- and the corpus
  sweep reports `REFUSED` for a file the checker refuses whose every script `bash -n` reads, beside the `MISSED` it
  already reported the other way. The sweep prints the language's findings too.

### runfile-lsp

`analysis.rs` (diagnostics and completion, pure), `document.rs` (a file's diagnostics where it sits), `rpc.rs`
(framing), `server.rs` (dispatch).

- Diagnostics come from the **real parser**, so an editor and the runner cannot disagree about validity -- and
  from the runner's own name check (`resolve`), handed the `_shared.run` chain as the editor has it, unsaved
  edits and all, from what the runner refuses as failing every time (`runfile_lang::check`), and from its shell checker
  (`runfile-shell`). See *Names resolve before anything runs* and *What fails every time is refused too*. `document::diagnostics` composes them for a file
  where it sits -- its catalog's target names, its chain, whether it is machine-wide -- and `run :lint` asks
  the same function, so the command line and the editor cannot say different things about one file. **Both
  hand it the catalog `discover_unscoped` builds**, and have to: an editor opens a scoped machine-wide file
  from outside its scope as a rule, and a catalog of what is offered there has neither its `_shared.run` nor
  its siblings in it.
- **Property diagnostics come from the runner's own `props::check`**, for the same reason. `check_properties`
  used to describe the rules a second time, and the two had already drifted: it knew the scope rule and not
  the flag one. It now calls `check` per property -- with the region the property sits in -- and renders
  whatever `PropError` comes back, minus the `line N: ` prefix a diagnostic's range already carries. Only
  `only-in-directories` is asked separately, since that one depends on where the *file* was found rather than
  on anything in the line.
- **It is a library, and `run :lsp` is how it is served.** There is no `runfile-lsp` binary: the crate has no
  `[[bin]]`, `serve()` is its entry point, and the CLI dispatches to it from an arm that returns before the
  catalog is built. Everything it links -- `runfile-lang`, `runfile-discovery`, `runfile-runtime`,
  `serde_json` -- was already inside `run`, so it costs `run` about 200 KB and saves 1.1 MB from every
  archive, six of them in the npm package. The bytes are not the reason. **Two copies of one parser could
  skew, and did**: a `run` newer than the `runfile-lsp` beside it underlines valid files in red while the
  runner accepts them, which happened three times during the rewrite -- `retry`, `.logging`, a namespaced
  `run` call. Ten sites existed only to keep the two in step (both installers, two blocks of `release.yml`,
  the npm `bin` map and its name-yourself-from-`__filename` launcher, the setup action's `run*` glob, a CI
  step, `install.run`, `runfile.lspPath`), and the glob had already broken every consumer's job once by
  expanding to two words. One file cannot skew against itself.
- **`:lsp` answers before anything can print.** LSP framing owns stdout, and one stray line desynchronises the
  client for the rest of the session -- so the arm sits with the other `:` commands, all of which return
  before `catalog()`, and errors go to stderr where `main` already writes them.
- **The extension falls back to a `runfile-lsp` on PATH, once, and only if `run :lsp` never answers.** The
  marketplace updates the extension on its own while `run` is updated by hand, so a client can meet a runner
  that predates the subcommand -- and that runner shipped a standalone server next to it. A server that
  answered and *then* died is a crash, not a mismatch, and is not retried. Removable once no supported `run`
  predates `:lsp`.
- The transport is hand-rolled. LSP framing is a header and a byte count; a framework would reintroduce the
  async runtime this rewrite removed, for a server that answers one client, one message at a time.
- Full document sync, deliberately: these files are small, and an incremental applier is a source of drift.
- Every request is answered — an unanswered one hangs the client — and every notification is silent.
- `FUNCTIONS` and `PROPERTIES` carry a signature, a one-sentence doc **and a worked example**; `SOURCES` and
  `RUN_KEYS` carry the same three. The example is the part that earns the popup: a signature says the shape
  of a call and a sentence says its purpose, and neither answers *"what do I type here"*, which is what
  someone hovering a name is asking. `analysis::card` renders all four the same way — a fenced heading, the
  prose, an **Example** block, and for a property a rule and whether it may sit inside a block — fenced as
  `runfile`, which is the extension's own language id, so an editor colours the example with the same grammar
  as the file. Completion sends the example too, in its markdown `documentation`.
- **`keywords::KEYWORDS` documents the line forms** — `#`, `$`, `exec`, `detach`, `run`, `let`, `do`,
  `parallel`, `if`, `else`, `for`, `in`, `while`, `until`, `loop`, `break`, `continue`, `match`, `case`,
  `default`, `retry`, `every`, `json`, `code_of`, `end`. These are what a person meets
  first and the only things in the language with no signature to read and no completion entry to hover. `$`
  and `#` are matched as *characters* rather than words, and only where they are the marker: `is_shell_marker`
  accepts a `$` at the start of a line or after `if` / `match`, so `$HOME` and `$(date)` inside a command are
  left alone, and `lexer::comment_at` answers for the `#`, so an editor cannot think one in a string or in
  shell text is a comment.
- **Anything at or past where a comment starts hovers as the comment, and completes as nothing.** A `print`
  written in a sentence about printing is prose, and offering its card there is the same mistake as scanning
  the text for `ARG.` -- which is why both ask the runner's own rule rather than looking for a `#`.
- Hover reads the word under the cursor rather than the tree, so it keeps working
  while the document does not parse. `RUN.` is the only source whose keys are known ahead of time; `ARG`,
  `ENV` and `FLAG` are whatever the caller passed, so there is nothing to offer for them.
- **Completion reads the line for what kind of thing goes there, and the tree for what is in scope.** It used
  to read only the line, and so offered the same ninety functions and sources everywhere: no keyword, and no
  name the file had bound. The line so far says the kind — a keyword or a call at the start of a line, a
  value after an `=` (where `json` and `exec` may open one too), `in` after a loop's names, `every` after a
  retry's count, a property after a leading `.`, a target after `run ` or `code_of(run `, and nothing in a
  string or in shell text except inside a `{{ … }}`. The tree says which names are in scope and which blocks
  are open: a `let` from the end of its statement down (the runner's variables are flat, so one inside an `if`
  outlives its `end`), a loop's names only inside its body (the runner puts them back as the loop exits), and
  the top-level `let`s of every `_shared.run` above the file, which a binding in the file shadows. The nearest
  binding of a name is the one offered, with the line that binds it. `else`, `case`, `default`, `break`,
  `continue` and `end` are offered only where the blocks around the cursor take them, since anywhere else
  each is a parse error.
- **The tree is of a repaired copy, because the document is mid-edit.** `print(re` does not parse, and a
  block opened a moment ago has no `end`. `parse_around` blanks the line the parser stops at and tries again,
  as often as it takes (up to sixteen lines), and gives a block left open up to three `end`s. Every parse
  error names its line, which is what makes the repair exact rather than a guess. Blanking keeps every other
  line where it was, so a position in the repaired tree is a position in the document. That is also why
  there is no "last version that parsed" to fall back on: its lines have moved since.
- **A body is found from the lines, not the tree.** An `exec` body, a structured block and a `$` line's
  backslash continuation are somebody else's text, where only `{{ … }}` completes. `bodies` walks the lines
  with the parser's own `closes_body`, because a body is where a document spends most of its time not
  parsing: a `json` block is validated as it is read, and is not valid JSON halfway through a line. With the
  cursor in one, its whole statement is set aside before the repair, so the code around it still parses.
- **The `_shared.run` chain is asked only where a name could go.** `complete` takes it as a closure, and a
  test hands it one that panics, because answering means discovery and reading files. A target's chain is
  the catalog's; a `_shared.run` is not a target, so its chain is that of any target at or below its
  directory, cut off at its own. An open shared file is read as the editor has it, unsaved edits and all —
  for go-to-definition too.
- **Every item carries its own kind and a rank**, sent as `sortText`. An editor sorts by how well the typed
  prefix fits and only then by `sortText`, so the rank decides exactly the ties it should: `region`, bound
  three lines up, above `read_file` for `re`, and a keyword above a binding at the start of a line. A source is
  a `Module` (read through a dot), except `ARGS`, and is not offered at the start of a line, where a line
  that is only a value is an error.
- **Go to definition** answers for a `run <target>` *and* for a binding. A `run` target is matched by
  **position** rather than by word, because `word_at` stops at `:` and a namespaced `build:release` is two
  words to it -- clicking either half, the `:` between them, or the keyword, means the same thing. The span
  is checked **before** `word_at`, which is what makes the `:` work: it is no part of a word, so reading the
  word first made the one character in the middle of a namespaced target answer nothing. `dispatch_on` covers
  **both
  spellings**, since `code_of(run build)` is the same dispatch and so the same jump: the statement's span
  reaches back to column zero, because clicking an indented line's indentation means that line, while a
  `code_of`'s starts at its own `run` -- otherwise clicking the call, or the binding being assigned, would
  jump to the target instead of explaining itself. A binding is found in the tree
  (`analysis::binding_in`), so a name inside a comment or a string is not mistaken for one, and the nearest
  binding **at or above** the cursor wins, which is what shadowing and rebinding mean. A name the document
  does not bind comes back as `Ref::Shared`, and the server walks the `_shared.run` chain innermost-first --
  that is the case worth having: a shared binding applies to every target in its directory and appears
  nowhere in the file using it, so it is the one definition a reader cannot find by looking. A shared file is
  read as a whole rather than relative to a cursor, so its last binding wins. The extension already forwarded
  `textDocument/definition`; nothing there had to change.
- **`textDocument/formatting`** returns one edit covering the whole document, because sync is whole-document:
  a minimal diff would be a second description of the same change and a chance for the two to disagree. A
  document that does not parse is answered with `null` rather than an error — a file is unfinished for most of
  the time it is being written, and format-on-save must not put a dialog in the way of that. This is what
  makes format-on-save work in every editor, the CLI and the editor sharing one formatter.
- **Shell diagnostics are `runfile-shell`'s**, asked from `analysis::diagnose` with the chain the names are
  checked under, so they arrive with every other diagnostic and `run :lint` gets them too. Each is an error --
  the runner refuses the file for it -- and carries its fix as a second line of the message (`fix: …`), which an
  editor shows under the problem and `:lint` indents beneath it. ShellCheck used to be handed each script with a
  placeholder for every interpolation; *runfile-shell* says why that was replaced.

### editors/vscode

**A `$` line is coloured by VS Code's own shell grammar.** The TextMate rule sets `contentName` to an
embedded-shell scope *and* includes `source.shell`; the first alone only tells VS Code which language's
comments and brackets to use, so for a long time a shell line came out one flat colour. Interpolation is
listed before the include, so `{{ … }}` stays the language's. `exec` bodies are split in two rules —
`exec sh|bash|…` delegates, anything else does not, since a Python body is not shell — and both close on an
`end` at the opener's own indentation via a `\1` backreference to the captured indent.

**The comment rule needs no region of its own, and one word of care.** It matches `(?:^|(?<=[ \t]))#.*$` --
the word-start rule, spelled the way Oniguruma can see it -- and is listed first at the root, which is enough:
TextMate takes the *earliest* match, so a `$` line, a string or a capture, each of which begins further left,
takes the line before the comment rule is reached. That is also why a shell line's `#` comes out the shell
grammar's own comment rather than ours, which is exactly right: the rule it applies there is the rule we
apply here. Only two rules had to learn anything -- `structured-block`, whose `begin` ended at `$` and now
captures a trailing comment as one, and `retry-header`, which reads its own line to the end.

**A capture call is matched by shape, not by name.** The rule spelled `code_of` out, so every other call had
its command read as a runfile *expression*: `--cached` came out two operators and `"*.rs"` a string. It takes
any function name now, which is the same generalisation `capture_call` is in the tree-sitter grammar.

**A capture in value position is coloured too.** `shell-line` is anchored at `^\s*(\$)`, so `let rs = $ git
diff … -- "*.rs"` matched no rule at all and the shell text was read as a runfile *expression*: `--name-only`
came out two operators and `"*.rs"` a runfile string. `shell-capture` is `shell-condition` one line form over,
and the exec rules grew an optional assignment in front of the keyword — `let out = exec sh … end` was not a
block either, so its body was not shell and its `end` closed nothing. All three share the left-hand side
`structured-block` already spelled out (`(?:(let)\s+(name)|(name))\s*(=)\s*`), so the four capture forms are
written one way. The test is the sharpest one available: `let x = $ <line>` must colour `<line>` **exactly** as
`$ <line>` does, token for token.

**The command after `exec` is coloured as a command line**, like the text after `$ `. It could not be done in
a capture: `\G` has no anchor to reach for there, so the patterns never fired and the command came out flat
white. It is a region instead — `#exec-command`, `\G`-anchored to the position just after `exec` and ending
at the line's end, which is also what keeps it off the body below. The rules' lookaheads were made
non-capturing at the same time: the alternation inside one was silently taking group 3, the number the
command needed, so a `{{ … }}` in an `exec` command had never been coloured either.

**Every embedded region ends at the end of its line**, and that bound is not decoration. The shell grammar
happily runs a rule to end of line, so in `code_of($ xcode-select --install)` its option rule swallowed the
`)` — and with nothing left to match `end`, the region never closed and *every line below it* was coloured as
shell: `default` stopped being a keyword, a string stopped being a string. `shell-line`, `shell-condition` and
`exec-command` were already bound that way; `code-of` ended only on `\)` and was the one that leaked. Each of
these is a single line by construction — the parser requires `code_of(`'s `)` on the same line — so the bound
is correct rather than a fallback. The regression test uses **VS Code's real shell grammar**: a stub does not
over-consume, so nothing weaker can see this.

**A capture's region also has to end at its `)`, and the end-of-line bound hid that it did not.** The paren
closed the shell grammar's *own* command statement without closing anything of ours, because
`#first-shell-statement` was still open — a helper with no scope of its own, so nothing showed, and while it
is open the enclosing rule's `end` is not in the scanner at all. Its end is `;`, `|`, `&` or end of line, and
a `)` is none of those. So the region ran on and the shell grammar, which starts a fresh statement right
after a `)`, read the rest of the line as a command: `if code_of($ true) == 0` came out with `==` an unquoted
shell string and `0` a shell numeric, and every `)` closing a capture lost its punctuation scope.
`#capture-shell-statement` is the same helper with `\)` in its end, used only by `#capture-call` — a `)`
inside the command is refused by the parser, so stopping at the first one is the rule the runner already
enforces, while a `)` in a plain `$` line is ordinary shell and `#first-shell-statement` is left alone. Only
the real shell grammar starts a statement after a paren, so this is the second bug in this file a stub could
not see.

Two things the shell grammar's own anchoring forces. It starts a statement only after `^`, `;`, `|`, `&`,
`!`, `(`, `{` or a backtick — and the text after `$ ` is none of those, so the **first** command on a line
came out bare while every later one was coloured. `#first-shell-statement` fixes it: `source.shell`'s
`typical_statements`, which is exactly what the anchored rule delegates to once its lookbehind has passed —
assignments, `for`, function definitions and plain commands alike — wrapped in a `\G`-anchored rule so it
applies **only at the start of the embedded region**. The `\G` is the whole point: reaching for the narrower
`command_statement` instead, or letting either take every position, flattens a line like
`d=$(mktemp -d); trap …; r() { sed … }` into three tokens, because those rules know nothing of assignments,
subshells or function bodies. Everything past the first statement is left to the shell grammar's own rules
and comes out identical, token for token. And an interpolation cannot simply be listed beside the include: a command statement
covers its arguments and a quoted string covers its contents, and TextMate takes the **earliest** match, not
the first listed. So `{{ … }}` is a grammar **injection** (`syntaxes/runfile-interpolation.injection.json`,
`injectTo: source.run`), which is the mechanism for putting a pattern ahead of a host grammar's at every
position — and it works inside a shell string, which it never did before. A `$` line ends on
`(?<![\\\n])$`: the tokenizer scans `line + "\n"`, so a backslash continuation would otherwise end the rule
at the position after that newline. `grammar.test.ts` runs the real tokenizer over these — with a stub
`source.shell` for the structural checks, and with **VS Code's own shell grammar** (skipped where VS Code is
absent) for the one that matters: that `$ <line>` is coloured exactly as `<line>` in a `.sh` file, token by
token. Nothing weaker catches this; both bugs here passed tests that only asked whether *something* was
coloured. Two scopes are excluded from that comparison and named in the test: the root scope, which is the
embedding rather than a colour, and `meta.statement.shell`, which comes from the anchored rule we cannot
satisfy and which no shipped theme targets.

The LSP client is hand-rolled rather than `vscode-languageclient`: the server speaks a small, fixed subset,
and a full client library is a large dependency for five message types. It was notification-only until
format-on-save needed a reply, so it now correlates requests by id with a **timeout** — this runs on save, and
a wedged server must not take the editor's save with it. Telling a reply from a notification is `replyId` in
`pure.ts`, extracted so the rule has a test on it rather than sitting in a class that cannot be loaded outside
an extension host. Registering `DocumentFormattingEditProvider` is what makes `editor.formatOnSave` apply to
`.run` files; every other editor gets the same thing straight from the LSP. Completion forwards the server's
`sortText`, without which every list is alphabetical, and maps its kinds through `completionKind` — a list of
the kinds the server sends, so a new one has to be added there or it is drawn as plain text.

**A call to a function the language does not have is `invalid.illegal.unknown-function.run`**, from a list of
the runner's names in the `function` and `capture-call` rules; see *Names resolve before anything runs*. The
extension needs nothing else for the rest: an unresolved name is a diagnostic like any other.

### editors/tree-sitter

`grammar.js` mirrors `GRAMMAR.ebnf`. Newlines are tokens rather than extras, so every line form ends in one;
a file without a trailing newline gets a zero-width one from the scanner, exactly once.

- **A comment is not an extra; `_eol` is where one may go.** An extra is offered at every token position, and
  three of those are positions where the `#` is not ours: inside a string, inside a `$` line, and inside an
  `exec` body. As an extra it took all three -- `"a {{ x }} # b"` ended its string at the `#` and ran on to
  the next line. `_eol` is `optional($.comment), $._newline` and replaces `$._newline` on every line form the
  language reads, which is every one of them but `shell_line`. A line whose last value is *shell text* keeps
  the bare newline, since a capture runs to the end of its line: `endsLine` is the one place that split is
  spelled, and it is why `shell_capture` and `exec_capture` are named apart rather than wrapped in one
  `capture` node.
- **An `exec` command is a line of words, which is how the runner splits it too.** Its parts are the only
  non-immediate text tokens here, so a word ends at the blank after it and `_eol` has a comment left to find.
  A `$` line's text is one immediate run for the opposite reason: it runs to the end of the line because all
  of it is the shell's.
- **A capture is a call's last argument, whatever the call is called.** `capture_call` takes any name, so
  `lines($ git ls-files)` parses the way `code_of($ cmd)` always did -- both grammars had only the second,
  since `code_of` was once the only call a capture could sit inside, and no `.run` file in this repository
  used the general form until `check.run` did. The `[$.capture_call, $.arguments]` conflict is what lets GLR
  carry `name(a, b, …` as both an argument list and a capture call until a `$` settles it. Only `code_of` may
  hold a `run` dispatch, and that stays the *runner's* rule: it is about what a call means rather than what it
  looks like, so an editor colours `lines(run x)` and the parser is what refuses it.
- `src/scanner.c` carries the rules an EBNF cannot: an `exec` body closes only on an `end` at the opener's
  indentation, a `$` or `exec` body stops at `{{` so an interpolation is a node the grammar parses, a `run`
  argument is one whitespace-delimited word with its interpolations kept whole, and a `#` opens a comment only
  where it begins a word -- a rule about the character *before* it, which is why no regex here can hold it: by
  the time one matched, the whitespace in front would already have been skipped. The scanner still has it to
  look at. `at_line_tail` is the shared answer to "nothing after this but blanks and perhaps a comment", which
  both the block terminator and the `json` keyword ask.
- **A scanner that skips blanks and then fails has moved the position for whoever runs next.** `scan_line_start`
  is one pass over that whitespace answering for the comment, the `exec` keyword and the two markers below,
  because the rest are recognised at column 0 and would otherwise be answered by the blanks the first had
  skipped -- and it tells those three apart by the line's first letter, since a check that read part of a
  word and failed would leave the next one looking at the rest of it. The string rule needs no
  scanner: the interpolation's expression is parsed as an expression, quotes and all. A dispatch's word inside
  `code_of(…)` is a second token, because there the `)` closing the call ends it — unless a `(` in the word
  opened it, which is how the parser reads one too.
- **`parallel` and `detach` are scanner tokens, not keywords** (`scan_marker`). Each is a marker only in
  front of what it marks -- `do` or `for`, `$` or `exec` -- and an ordinary name everywhere else, as the
  runner reads them. As plain tokens they were keywords wherever a statement may start, because the grammar's
  lexer takes the keyword whenever one is allowed and cannot look past it to see the `=`: `parallel = 5` and
  `detach = 5`, both bindings to the runner, did not parse. The scanner reads the next word to decide, and
  ends the token at the marker itself.
- **An escape is exactly the runner's**: `\"`, `\\`, `\n`, `\t`, `\r` and `\e`. The grammar lacked `\e` --
  ESC, which `printf` colours with -- so a string holding one failed the whole file, and it accepted a `\{`
  the runner refuses.
- **Scanner state is carried forward only by a successful token**; what a false return records is discarded.
  The indentation a capture `exec` needs is therefore recorded on the newline token that precedes its line, by
  looking past the token's marked end — not in the column-0 check, which says no to every non-`exec` line.
- `conflicts: [[list]]`: a newline between the last element and `]` can belong to the separator or the closer.
  Both readings produce the same tree, since newlines are hidden, so GLR may pick either.
- Tested by `test/corpus/` (tree shapes) and a sweep that parses every `.run` file in the repository — the
  check that the grammar accepts what the runner accepts.
- pnpm 11 blocks tree-sitter-cli's install script, which downloads the binary; `pnpm-workspace.yaml`
  (`allowBuilds`) approves it. The old `pnpm` field in `package.json` is no longer read.
- **A call is `@function.call` only when it names a function the runner has** -- an `#any-of?` list in
  `highlights.scm`, so an unknown one keeps the identifier's colour. The grammar itself accepts any name, as
  the rule about what a call *means* is the runner's; see *Names resolve before anything runs*.

### runfile-cli

`main.rs` (flags and dispatch), `list.rs`, `prepare.rs`, `prompt.rs`, `watch.rs`, `completions.rs`, `init.rs`,
`cmd_env/`, `cmd_lint.rs`, `cmd_update.rs`, `ci_detect.rs`.

- **Runner flags are recognised only before the target name**; everything after it belongs to the target. So
  `run echoes --dry-run` passes `--dry-run` through as `FLAG.dry-run`.
- **`:update` is installing again, and it decides before it downloads.** A `cmd_update::Channel` is one URL,
  a repository's releases: `gitea` (the default, where every release is cut) or `github` (the mirror, which
  lags until `run mirror`). Two things are the same on both hosts and nothing else is used: `/releases/latest`
  redirects to the newest release's `/releases/tag/<tag>`, so following it with `curl -w '%{url_effective}'`
  names the version with no JSON to parse; and `/releases/download/<tag>/<asset>` serves a named release. Each
  host's own `latest` *download* alias is a different shape (Gitea `/releases/download/latest/…`, GitHub
  `/releases/latest/download/…`), so a URL with its host swapped 404s -- which is why the installers resolve
  `latest` to a tag the same way instead. Knowing the version first is what the output is for: `Plan` says up to
  date, **ahead** (this binary is newer than the channel's newest -- a source build or an unpushed mirror, and
  only naming a version or `--force` goes back), upgrade, reinstall or downgrade. The installer that runs is
  the one *that release* shipped, given the tag, `RUNFILE_CHANNEL` and `RUNFILE_INSTALL_DIR`; its stdout is kept
  back unless it fails, since the updater says the same with both versions in hand. Afterwards the new file's
  own `--version` is what gets reported, and a mismatch with the tag is an error -- it is the one account that
  cannot disagree with what was installed. On Unix the script is **downloaded first and then piped to `sh -s
  --`**, not run as `curl … | sh`: a pipeline's status is its last command's, and `sh` given an empty script
  succeeds, so a failed download used to be reported as a successful update that had changed nothing. `1.2.0`
  is taken as `v1.2.0`; a version starting with `-` is refused, and `:update --help` prints its usage -- both
  were read as a release name and sent to the server. `tests/update.rs` walks every outcome against a fake
  `curl` serving the repository's real `install.sh`; its tests take turns, because each writes an executable
  and runs it, and a fork from a parallel test in between holds the file open for writing ("Text file busy").
- **`:list` shows the machine-wide targets first**, under `global:`. They are reachable from every directory
  and appear in no file a reader of the project can see, so they are the group worth meeting first; the local
  ones follow. `local:` is the unlabelled default only while nothing precedes it, which is every project
  without a machine-wide directory -- an unheaded run of names under `global:` would read as more global ones.
  The order is the human listing's; `--names` and `--json` stay in the catalog's own (sorted) order, since
  their readers work by name.
- **`:list` prints one line per target, whatever the terminal.** The description is its first line only --
  `take_description` keeps the whole comment block and `facts` takes `lines().next()`, and a test with a
  description broken across comment lines pins it, since the one that existed used a single-line description
  and so pinned nothing -- and on a terminal it is cut to the width with `…` (`term::fit`). A description that
  wrapped put its tail at the start of the next line, where it read as the next target, and a listing is
  scanned down its left edge. Only the description gives way: the name is what gets typed. **A pipe is never
  cut**, `COLUMNS` or not, because `run :list | grep deploy` wants whole lines and a pipe has no width. The
  width is `COLUMNS` when it is a positive number, then `TIOCGWINSZ` or the console's visible window, from
  `runfile-runtime::term` -- which already makes the `libc` and console calls for Ctrl+C, so asking the width
  added no dependency and no lockfile change. It is measured in **display columns**, not characters: a small
  `wcwidth`-style table counts East Asian wide characters and emoji as two and combining marks as none,
  because counting characters would let a description holding a `🚀` run past the edge -- which the first
  cut of that table did, until a test asked about the rocket.
- `:list` has three forms: human, `--names` (for completion scripts), `--json` (for tooling). The JSON is
  serialized by hand — four string fields do not justify a serde dependency in the CLI — and carries a
  `formatVersion` that CI checks against the extension's constant. **Version 2** dropped the `aliases` array
  with the property; the extension never read it, but a field removed is a shape change and the constant says
  so rather than letting an old reader guess.
- `--dry-run` is **not** gated by prepare: it changes nothing, and reading what a target would do is a
  reasonable thing to want before setting a project up.
- Watch mode is entered automatically by any target declaring `.watch`; there is no flag, because the file
  already said what it wants. Patterns interpolate, so they resolve through `Host::header_props` rather than
  being read off the source text. A failing run is reported and watching continues.
- Completion scripts are hand-written per shell, but carry no knowledge of their own: they ask the binary
  what may follow what (`run :complete <cword> <word>...`, hidden, absent from the help and from the tree it
  serves). `completions::ROOT` is the single description of the command tree, so a subcommand of a subcommand
  completes without a fifth copy of the walk in a language that cannot share one. The binary answers with
  words, or with a `<files>` / `<dirs>` marker where only the shell can do the job well — a word list cannot
  append a `/` instead of a space. **A bare Tab lists the commands and the targets together**, as it always
  has: a command nobody can see without first guessing its `:` is a command nobody finds. Flags are the one
  list that waits for a `-`, and past a target name nothing is offered at all: the arguments there are the
  target's, and guessing would be confident nonsense. The bash script takes `:` out of `COMP_WORDBREAKS` at
  load, as npm and nvm do and as the old one did: readline otherwise splits `:env` and `vscode:test` before
  the function sees them, and inserts a candidate's own colon after the one already typed. It is tested by
  sourcing it and driving `_run` the way the shell does — **splitting the line on the script's own
  `COMP_WORDBREAKS`, not on spaces**, since a harness that splits by hand cannot see that class of bug at
  all. The tree is tested directly, and in both directions against the help.
  `:completions` takes `install`, `uninstall` or `output`, the shape the old CLI had. **bash and fish get a
  file in the directory their completion system reads on demand**, not a line in a profile: Ubuntu's
  `~/.profile` sources `.bashrc` *before* it puts `~/.local/bin` on PATH, so a startup hook that shells out to
  `run` found nothing and `eval` registered nothing, silently — completions simply never appeared in a login
  shell. zsh and PowerShell still take a profile line, so that line names the binary by its full path rather
  than trusting PATH at startup. `uninstall` also removes the older `.bashrc` hook, so upgrading leaves no
  dead line behind. **The zsh script loads the completion system when nothing else has.** `compdef` is
  `compinit`'s, not zsh's, so the bare `compdef _run run` the script ended in was `command not found` on every
  startup of a shell that had never run one — and `install` *creates* `~/.zshrc` where there is none, which
  makes the hook the only line in the file and leaves nothing there to load it. `compinit -i` rather than a
  bare `compinit`, because the insecure-directory check is a *question*, and one asked at every shell startup
  would be worse than the error it replaced; the `compdef` after it is guarded too, since a zsh with no
  completion system at all has nothing to register with and going quiet beats greeting every new terminal
  with an error. The profile line calls the binary rather than carrying a copy of the script, so an
  already-installed hook picks this up on upgrade with no reinstall. Tested against a real zsh started with
  `-f` — the same shell a fresh install meets — asserting that `_comps[run]` is set and that nothing reaches
  stderr, and skipped where zsh is absent, which is most Linux boxes and no macOS one.
- Help is data, not a string literal: `help.rs` renders `Section`/`Row` tables, bold headings and cyan names
  when stdout is a terminal and plainly into a pipe, honouring `NO_COLOR` and `TERM=dumb`. Every command
  renders through it, so they cannot drift into different shapes, and a `--help` never needs a project to
  exist. The version is flags only (`-v`, `-V`, `--version`): nothing else about the binary itself is a
  command.
- **`:lint` puts every runfile into the one shape there is and reports everything the runner refuses, and exits
  0 only when both hold** -- the promise that a file which passes runs with no parse-time error and opens in an
  editor with nothing underlined. It was `:format`, which passed files that failed the moment they ran. The
  errors are `runfile_lsp::document::diagnostics`, the function the server publishes from, so the command line
  cannot find more or less than an editor does: syntax, properties (`props::check`), a `run` of a target that
  is not there, unknown functions and unresolved names under the `_shared.run` chain, what fails every time it runs
  (`runfile_lang::check`), and the shell. Each file is formatted
  first and checked as it then stands, so a position is one in the file on disk, printed as
  `path:line:column` from the working directory, which a terminal makes a link. `--check` writes nothing and
  fails on a file that needs formatting too; `--stdout` prints the files and moves the report to stderr.
  **The shell checker is part of it**, since a shell finding refuses a run like any other error; a finding's
  `fix:` line is indented under it, and `:lint` never applies one -- only layout is ever rewritten.
  A flag it does not have is refused rather than ignored, since a mistyped `--check` that went unread would
  write. `_shared.run` is included -- it is not a target, so nothing that walks the catalog by name would reach
  it. The machine-wide directory is left out unless `--include-global`, as with `:generate` -- or unless
  `:lint` is standing inside it, where it *is* the project, and leaving it out said `0 files checked: no
  errors` about a directory full of runfiles. Whichever brings it in, every file in it is linted, scoped or
  not, and what leaves a file out is its `Origin`, a `_shared.run` the same as a target. Explicit paths
  override the catalog, a directory argument is walked, and each named file's project is discovered where it
  sits, the way an editor finds it -- all of it through `cmd_lint::catalog`, which is `discover_unscoped` (see
  *runfile-discovery*). `run :format` is refused with `:lint` named -- not a typo, but a script written for an
  older runner. `cli.rs`'s `this_repository_lints_clean` holds this repository to the promise.
- `:generate zed|jetbrains|vscode` is a lean port of the old generators: an entry is recognised as ours by its
  shape (command `run`, label `run <target>`), so a rerun replaces exactly those and keeps a person's own; a
  file is rewritten with the indentation it already uses. The 661-line `.editorconfig` reader did not come
  back. Global targets are left out unless `--include-global`, since a task file is committed and
  the machine-wide directory is one person's. `Catalog.root` (the parent of the nearest `runfiles/`) is where the files go.

## Properties

Four axes, all of them `PROPERTIES` columns:

| Axis | Yes | No |
| --- | --- | --- |
| Block-scoped (may sit inside `if` / `for` / `match`) | `ignore-errors`, `workdir`, `env`, `env-file`, `add-path` | `shell`, `logging`, `watch`, `only-in-directories` |
| Declaration-only (must be above the block's first statement) | `shell`, `logging`, `watch`, `only-in-directories` | the rest |
| Flag (bare means `= true`, takes a bool) | `ignore-errors`, `logging` | the rest |
| Machine-wide files only | `only-in-directories` | the rest |

`env` is addressed by sub-key, `.env.NAME = "value"`, and exactly two segments: `.env` and `.env.A.B` are
both "unknown property". Every value is a full expression, so a `?` chain, a call or a `"{{ … }}"` string is
legal in any of them -- `only-in-directories` is the one exception, read by discovery before any evaluator
exists and so a literal string or a list of them, refused with `DiscoverError::UnreadableScope` otherwise.

**`env-file` and `add-path` are block-scoped, and both append.** A block that names one has a longer list than
the block around it, which is how `with_block_env` tells it has to rebuild -- reading and decrypting the files
again for every `if` would be work nothing asked for. A block whose `.env` differs from the enclosing one is
rebuilt too; it was not, and `ENV.X` inside it read the enclosing value while `$X` read the block's. The environment the block inherited is put back when it
closes, which is what stops a loop carrying one iteration's files into the next, and is why **every** block
form goes through that one function: `if` and `match` via `nested`, and `for` and `retry` via their own arms,
whose bodies were extracted into `for_body` and `retry_attempts` so the wrapper has something to wrap. A
`retry`'s `else` runs *outside* the body's environment -- it is what to do when the block never worked, not
part of it. What this buys over the header form is that a block's file can be named by something the body
computed. A `.env-file` naming a file that is not there is skipped, the same as a header one: a project whose
`.env` is git-ignored still has to run.

**A property is applied where it is written.** It used to be applied before every statement of its block
whatever the order, because `ast::Block` keeps `properties` and `statements` in two lists and the walker read
one and then the other -- so `let x = "sub"` followed by `.workdir = x` was `` `x` is not defined ``, in a
file that reads top to bottom. `Block::declaration()` and `Block::trailing()` split the list at the first
statement's line: the declaration region is what `Props::extend` applies at block entry, exactly as before,
and anything below a statement is applied by `walk` when it reaches that line. So a property reads the
bindings above it, and takes effect from there down -- including one below the last statement, which has
nothing left to affect but is still evaluated, so a broken one says so. Nothing existing moved -- not one `.run` file in the
repository or on the author's machine had a property below a statement -- which is what made the ordering
safe to define rather than a break. `Cow<Props>` is what keeps the common path free: a block with no trailing
property walks on the borrowed properties it was handed and clones nothing.

**A value reads the environment as it stands at its own line, in a header too.** A declaration region is
applied one property at a time, but the walker builds the environment only once it is done, so `.env.B =
ENV.A` below `.env.A = "a"` read the caller's `A`, a value composed from what an `.env-file` above it had
loaded found nothing, and neither a target's header nor a shared `let` saw anything the `_shared.run` chain
set -- while the same lines below a statement, where each property rebuilds as it goes, worked. `extend`
and `fold_shared` now call `props::keep_current` before each value: it rebuilds, through the walker's own
`env::for_props`, **only when a property above has changed the environment and this value reads it**
(`Expr::reads_env`). The laziness is the point rather than an optimisation. `Host::header_props` evaluates
every header a second time, on every run that is not `--dry-run`, to learn whether it declares `.watch`, and
a rebuild reads and decrypts every `.env-file` -- so rebuilding after every property would have unlocked the
keyring in the probe as well as in the run for any target with an encrypted file. A header that never reads
`ENV` costs exactly what it did, and `tests/keys.rs` counts the loads to hold it there: none in the probe for
such a header, one for a header that does read -- and none more in the run after the probe, which shares its
key pool. What `extend` rebuilt it puts back before returning, because
the block's own environment is the walker's to build and to undo -- left in place, it is what
`with_block_env` would save, and it would outlive the block. A nested block starts current, since the one
around it was built; the top of a target starts stale whenever the chain folded into it touches the
environment.

The environment is the part that has to be put back. A trailing `.env`, `.env-file`, `.add-path` or
`.workdir` rebuilds it mid-walk, so `walk` saves the block's own environment and restores it on the way out
-- `with_block_env` cannot, because it decides whether to save by comparing list *lengths* at block entry,
before a trailing property has been applied.

**Four properties describe the whole file and have to be written above its first statement**
(`PropError::NotInDeclaration`): `shell`, `logging`, `watch`, `only-in-directories`. They are exactly the
header-only ones now. `.parallel` was the one block-scoped property that was declaration-only too, and it is
a keyword now, so the two columns agree -- and are kept apart all the same, because they answer different
questions and the next property may split them again. Refused by `extend` at block entry
rather than by `walk` where the line sits, so the message arrives whether or not the line would have been
reached -- after a `break`, or in an `if` that went the other way.

**A flag takes a bool, and a constant that can never be one is refused where it is written.**
`.ignore-errors` and `.logging` are the two. `matches!(v, Value::Bool(true))` used to
answer every other value with `false` and say nothing, so `.ignore-errors = "true"` was *off* -- in a language
that refuses `"a" + 1` and `"1" == 1`. Now `Expr::constant_non_bool` answers whether the right-hand side is
something the parser can read straight off the page and is not a bool -- a number, a list, or a string with no
interpolation in it -- and `PropError::NotABool` names what it found, with the hint the near miss earns
(*"a string -- write `true` without the quotes"*). Nothing is const-folded: `1 + 1` is left to the run,
because a flag written as arithmetic is not the mistake this is looking for.

The rule is about **constants, not types**, because a flag decided from outside the file has no other shape to
arrive in: `ENV.CI` is a string on every platform there is, and so is `ARG.p`. So `.ignore-errors = ENV.CI`,
`.ignore-errors = "{{ ENV.CI }}"` and `.ignore-errors = ARG.p ? ENV.CI ? "false"` all pass the static check and are
resolved at run time by `as_bool`, which takes a bool, or `true`/`1`/`false`/`0` case-insensitively and
trimmed -- the words the places a flag is read from spell one with. Anything else is
`PropError::FlagValue`, naming the value: silently off is the failure this replaced, and a run that stops is
strictly better than a line that quietly did nothing.

**`.shell` names one of the eight shells `is_shell` knows**, and `PropError::NotAShell` refuses the rest --
statically for a constant, and at apply time for a value the run works out, against the same predicate
(`exec::body_is_shell`, which splits the command so `busybox sh` and `/usr/bin/zsh` both pass). It looked like
a way to choose any interpreter and was not: `is_shell` gates `-e` *and* the script-as-`-c` handover, and it
is asked of the resolved **program**, with no memory of whether that program came from `.shell` or from
`exec`. So `.shell = "pwsh"` and `exec pwsh` were already the same mechanism -- body on stdin, no `-e` -- and
only the first of the two hid it: under `.shell = "pwsh"` a `$ ssh host` was broken exactly as it was before
scripts moved to `-c`, and the line said nothing. The message is the fix, spelled with the value that was
written: *"for another interpreter write `exec pwsh`"*.

That is also why `.shell` is header-only now, and `.logging` with it. Block scope for `.shell` was redundant
with `exec`, which says the same thing per block and names the program where a reader can see it; `.logging`
is the runner narrating itself, and a trace with gaps is worse than none, since absence stops meaning
anything. Both are declaration-only too -- header-only implies it, since a property describing the whole file
cannot be applied part-way through one.

**`.detach` is gone; `detach` marks the command.** `detach $ npm run dev` and `detach exec node … end` start
one process and do not wait. As a property it described a *file*, and got both halves wrong at once: every
top-level command in the file was detached, so setup-then-serve was unwritable, while `extend(nested)` cleared
it, so the same command inside an `if` quietly waited -- 5ms, 2007ms and 5ms for three targets that all said
`.detach = true`. A `_shared.run` setting it would have fire-and-forgotten every target in the directory.
`Statement::Exec` carries `detach` instead, so the marker travels with the one process it is about and there
is no question of where it applies. **It breaks the fold**: contiguous `$` lines are one process, so a
`detach $` line is its own statement and the `$` lines below it are not part of it -- folding on would
silently detach what came after, which is the failure being removed. Several lines as one detached process is
what `detach exec sh` is for. It is claimed only in front of `$` and `exec`, so `detach = 5` is still a
reassignment, and `detach run` is refused at parse time: a dispatch happens in this process. In the
tree-sitter grammar it takes the *capture* `exec` token, since behind `detach` the keyword is no longer at
column 0; in TextMate it joins the optional left-hand side the `exec` rules already carried for `let x = exec`.

**`.confirm` and `.hide` are gone**, each replaced by something that could not disagree with itself.

`confirm(question)` is a function, so it can be called anywhere -- inside an `if`, after the value it asks
about has been worked out. Declining raises `EvalError::Cancelled`, which travels like `Exit` inside a target
-- every catcher re-raises it, because someone who said no has not asked to be second-guessed by a `?` fallback
or by `.ignore-errors` -- and, unlike `Exit`, travels on past it: an `exit()` is a target reporting how it went,
for its caller to act on, while a person who declined has decided for the whole run. `RunError::is_stop()` is what those catchers now consult -- `exit_code()` still answers only
the status question, so a cancel keeps printing `cancelled` rather than exiting silently. The prompt reaches
it as `Scope::confirm`, a plain `fn(&str) -> bool` like `Scope::ask`, which is why `prompt::confirmer()`
became `prompt::confirm`: the closure captured nothing. It is skipped by `-y`, in CI, and under `--dry-run`,
where there is nothing to approve -- previously a guarded target could not be previewed at all.

**A target is hidden when its file name starts with `_`** (`discovery::is_hidden`, which reads the last `:`
segment so a namespace is not part of the question). Fifteen of the sixteen targets that set `.hide` were
already named that way, and every `_`-prefixed target set it -- the property only let the two disagree. The
sixteenth, `kico/runfiles/test.run`, is now visible.

A nested block inherits behaviour but never a parent's one-shot header state.

**`do … end` is a block with no condition** (`Statement::Do`, one arm in `walk`). Every
block form until it asked a question, so a property covering two commands meant inventing an `if true` — or
writing `cd web &&` on every line, which is what 35 sites in the corpus did. It takes nothing after the
keyword, and says so: anything there would read as a condition it does not have.

## The GitHub Action

`.github/actions/setup` is the only one, and **everything it leaves on a runner is job-scoped**: the binary
under `$RUNNER_TEMP/runfile-bin`, a `$GITHUB_PATH` entry for it, and — when `secret-keys` is passed —
`RUNFILE_PRIVATE_KEYS` in `$GITHUB_ENV`. Nothing is written to `$HOME`.

That invariant is what removed the companion `cleanup` action, its `clean-runner-state` twin and the ten call
sites of the two. Each of the three things they cleaned had stopped existing: `global-targets` and
`runfile-source` (which copied `.run` files into `$HOME/.runfiles/`), `env-file-source` (which wrote a
secret-supplied `.env` under `$RUNNER_TEMP` and exported `RUNFILE_ENV_FILE_TARGET` at it), and `state.json`,
which CI no longer writes. A workflow that wants a materialized env file writes it and points `run :env inject`
at the path — one step in the workflow rather than a capability in the action, and the CLI keeps
`RUNFILE_ENV_FILE_TARGET` for whoever sets it. Deleting `$HOME/.runfiles` had also turned actively wrong once
the CLI stopped reading it in CI: on a self-hosted runner it destroyed a directory the run was already ignoring.

## Gitea, and GitHub as a mirror

The repository lives on git.joaoverona.com, and **GitHub is a mirror, pushed when wanted rather than on every
push.** Master goes to Gitea only: its CI runs on every push, `run release` tags and pushes to master's
upstream there, and the tag cuts the release (`.cicd/release.yml`). Gitea is configured with
`WORKFLOW_DIRS=.cicd` and reads nothing else; GitHub reads `.github/workflows/` and nothing else, so neither
sees the other's files.

**`run mirror` is the one thing that sends anything to GitHub.** It fetches Gitea's master, fast-forwards the
local `github` branch to it -- Gitea's, not the local master, since a commit that has not reached Gitea has not
been tested or released there -- and pushes that branch to the remote it tracks, with `--follow-tags`. On
GitHub the `github` branch runs CI, and a release workflow that mirrors **one** release: the version Cargo.toml
names at the pushed commit, built from that version's tag. So a mirror pushed long after a release, with
commits past it, still gets exactly what Gitea released; several releases between two pushes arrive as the
newest, since npm's `latest` and the `v1` alias only want that one and would race if each were built; and a push
with no new release is CI and nothing more. Tags were the trigger until the mirror, and would have built every
intermediate release at once. A version whose tag did not arrive fails the release rather than skipping it.

- **Fast-forward only.** A `github` that Gitea's master does not contain means something landed on GitHub first
  -- a pull request merged there -- and it has to come back through master; `run mirror` says so and stops. It
  moves the branch with `git fetch . <src>:refs/heads/github`, never `git branch -f`: forced onto a
  remote-tracking branch, `branch` also makes it the upstream, so the next mirror read Gitea as the remote to
  push to. The one-time setup is `git branch github master && git push -u <github-remote> github`.
- **`run release` refuses to run off master**, which is what Gitea builds and what the mirror follows; a tag made
  anywhere else is one the mirror could never reach.
- **GitHub's default branch should be `github`**: it is what the repository page, a clone, a pull request and the
  scheduled audit all read. Nothing here can set that; it is a repository setting.

What each side publishes is decided by what it can do:

| | Gitea (`.cicd/release.yml`) | GitHub (`.github/workflows/release.yml`) |
| --- | --- | --- |
| When | every `v*.*.*` tag | the newest release, when `run mirror` pushes |
| Six archives, `.vsix`, installers | yes | yes |
| What `run :update` and the installers download by default | **yes** | with `--channel=github` |
| What the setup action downloads, and its `v1` alias | no | **yes** |
| npm | no | **yes** |

- **The setup action stays on GitHub.** Consumers write `JoaaoVerona/runfile/.github/actions/setup@v1`, and
  Gitea resolves a bare `owner/repo` against `DEFAULT_ACTIONS_URL`, which is github.com on this instance -- so a
  Gitea job already fetches the action, and the archive it installs, from GitHub. It moves when the mirror
  does. Pointing it at Gitea is a change to every consumer's supply chain and was not made in passing.
- **npm stays on GitHub** because trusted publishing accepts only GitHub's OIDC issuer, the provenance it
  generates requires `repository` in `npm/package.json` to be that GitHub repository, and a version can be
  published once -- so exactly one side does it.
- **Both releases ship the same installers**, Gitea by default and GitHub under `RUNFILE_CHANNEL=github`. A
  binary built before `:update` moved runs it against GitHub's `latest/download/install.sh`; that copy is what
  moves it over. Anonymous reads need the Gitea repository to be **public** (the instance has
  `REQUIRE_SIGNIN_VIEW=false`, so visibility is what decides); while it is private the installer, the default
  `:update` and the Gitea leg of CI's `update` job all 404.
- **A tag with a `-` is published as a prerelease on Gitea**, since Gitea's `latest` leaves prereleases out and
  `latest` is what every update installs. `v*.*.*` matches `v1.3.0-rc.1`. **GitHub marks the same versions**,
  for the same reason: its `latest` is what the setup action and the github channel install. npm takes one under
  `next`, since npm 11 refuses to publish a prerelease without a `--tag`. `run release` only ever makes
  `vX.Y.Z`, so a prerelease is a tag made by hand -- and both sides have to agree about one when it is.

What differs on Gitea's side is forced by the fleet (`gitea-easy-runners` documents it):

- **The toolchain is `shared-actions/rust-toolchain-and-cache` with `cache: false`.** The cache exports
  `CARGO_TARGET_DIR` to a path outside the workspace, and `runfiles/ci/_shared.run` pins its own -- a target's
  `.env` beats the caller's shell, by design -- so turned on it would send the from-source build somewhere the
  setup action does not look, and cache a directory the tests never build into. Making it work would mean
  `_shared.run` honouring a caller's `CARGO_TARGET_DIR`, which changes local builds too.
- **`macos-26` is an Intel Mac**, so `aarch64-apple-darwin` is cross-compiled there and nothing on Gitea runs
  aarch64 code; GitHub's `macos-15` is Apple Silicon and covers it. `ubuntu-24.04-arm` is qemu emulating the
  compiler, so `aarch64-unknown-linux-musl` builds on `ubuntu-24.04`, **linked by `rust-lld`**
  (`CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER`): it ships with the toolchain and links against the musl
  libc and crt objects the target's own rust-std carries, which is a whole cross toolchain with nothing to
  install while nothing compiles C for the target. `shared-actions/rust-cross-linux` is what a C dependency
  would need, and is not used: it also installs `qemu-user-static` to run tests, and that package is only
  virtual on the Ubuntu 26.04 image the `ubuntu-24.04` label now lands on, so its `apt-get` failed the first
  release before anything was built.
- **Windows builds natively on `windows-2025`**, both targets, with the MSVC host named outright (read from
  `rust-toolchain.toml`, since rustup settles a machine's host ABI once and settles on GNU if Visual Studio
  arrived later). `rust-cross-windows` would free the fleet's one Windows runner, but `+crt-static` lives in
  `.cargo/config.toml` and cargo's own MSVC build is the path known to carry it into the binary. The runner has
  no 7-Zip, so it zips with PowerShell 7's `Compress-Archive` -- 5.1's writes `\` into entry names.
- **Artifacts go through `shared-actions/gitea-upload-artifact` and `gitea-download-artifact`**: upstream's
  refuse any server that is not github.com.
- **CI's `update` job walks `:update` against the real hosts**, once per channel: it copies the from-source
  binary aside, runs `:update --force`, and asserts the file changed -- an update that downloaded nothing and
  exited 0 is the failure it exists for. `--force` because a build of master is usually the newest release's
  version already, and would rightly do nothing. Like `action`, it reports on the newest published releases as
  well as the commit.

## Preparation targets

A target named `setup` gates every other target in its directory, fingerprinted by its parsed tree, so editing the
setup re-triggers the requirement and editing a comment in it does not.

**`--dry-run` never records the gate, and neither does a setup that did not go well.** `main` recorded it after
any run that ended well -- a plain `Ok`, or an `exit(code)` of *any* code -- and `prepare::record` writes only
for a setup target, so `run --dry-run setup` stored setup's fingerprint without running one of its commands, and
every target after it walked through the gate. A preview changes nothing, and that includes what the gate
believes. A setup that says `exit(1)` did not go well either, and opened the gate all the same. Only `Ok`
records now: `exit(0)` becomes one where its target ends (see `exit()` under *runfile-lang*), so the arm that
returns an `exit()` status is left with statuses that are not 0, and records nothing. The watch loop records
too, but watch mode is never entered under `--dry-run`.

**A field added to the tree at a default that means "as before" goes in `UNASKED`**, in `runfile-lang`'s
`lib.rs`, in the same change. The fingerprint hashes the tree's `Debug` rendering, so a new field moves the
fingerprint of every file whether or not it uses the field -- and since a runner upgrade changes the rendering
for every project at once, the gate then asks all of them to run `setup` again for a change none of them made.
Adding `detach` to `Statement::Exec` did exactly that to 83 of the author's 88 projects. `UNASKED` lists fields
as they render at their default, separator included, and the fingerprint drops them; a file that does use the
field renders differently and still counts as changed. A test pins a real `setup.run` to the hash the runner
recorded for it before `detach` existed, so a field added without an entry fails there rather than on every
machine. `parallel` on `Statement::Do` and `For` is the second entry, added with the field. The five projects set up with the one build that shipped the field had their stored hashes
rewritten to the new rendering rather than being asked again: each one's stored value was exactly the old
rendering of its current file, which is proof setup had run for it. State lives in `state.json` in the platform state directory — **except in
CI, where there is none**. A runner is built from scratch and thrown away, so asking whether an earlier `setup`
happened is asking about a machine that did not exist: `prepare::enforce` returns before reading the file and
`prepare::record` returns before creating one. It used to be written on every CI run purely for a cleanup step
to delete afterwards. `RUNFILE_SKIP_PREPARE` is deliberately *not* the same predicate — it turns the gate off
on a machine whose state is still worth keeping, so a `setup` run under it is still recorded, or unsetting the
variable would report a setup that plainly ran as never having run. There is no
settings file: global registrations, path aliases and custom shell paths were all replaced by conventions
(the machine-wide directory, discovery, shell detection).

## Removed, and not coming back

MCP server, ShellCheck delegation (now `runfile-shell`), `.alias` (a target is its file name), `.detach` (now the `detach` marker), `.parallel` (now
`parallel do` and `parallel for`), `:convert`, `:config` (all subcommands), `:format` (now `:lint`, which
formats and also checks; the old name is refused with the new one), the user settings
file, `-p` / target globs, `capture()`
(now `$` in value position), `shell_quote()` (interpolation self-quotes), `set_cwd()` (now `.workdir`),
`define()` (now `let`), `nth()` / `count_parts()` (now `split()` and indexing), the arithmetic and comparison
functions (now operators), `when:` blocks, `sameShell`, `extendStdio`, `forceKillOnSigInt`, the JSON schema,
`VAR.` (replaced by `let`), and `RUNFILE_TARGET`. Dropped dependencies: `rmcp`, `tokio`, `json5`, `md-5`,
`shlex`.

The three properties that became something else are refused with what replaced them
(`PropError::Replaced`, from `props::REPLACED`) rather than as unknown: *"`.parallel` is gone -- write
`parallel do … end`, or `parallel for` to run a loop's iterations at once"*. An unknown property is most
often a typo, and these three are not -- they are a file written for an older runner, and the person reading
the message needs to know what to write instead.

Every other function from the old surface is present. Seventeen were missing at one point, dropped by
oversight rather than decision, and all are back. `try` is the one exception, replaced by `a ? b`.

**`run <target> --help`** prints the target's whole description, every input it reads — with what happens
without each one — and its path (`target_help.rs`). Two things made it necessary: `--help` after a target name is the
target's own argument, so `run deploy --help` warned about an unread flag and then **deployed**; and a
description was only ever visible as its first line in `:list`, which is why forty of them in the corpus had
grown past three hundred characters and one to sixteen hundred, with nowhere to be read. It is checked before
the prepare gate, since asking what a target does must not require the project to be set up, and only before a
`--` — `run w -- --help` still forwards it, which is how a wrapper hands `--help` to what it wraps. The inputs
are `inputs::of`'s, over the target and every `_shared.run` above it — walked from the tree, so a name in a
comment or a string is not mistaken for one the target reads.

`file_exists` is **files only** and `directory_exists` is directories only; `is_executable` is a file this
user may execute (the mode bits on Unix, being a file on Windows, where the question has no equivalent).
`file_exists` used to answer `exists()`, which is neither question — a `.git` is a directory in a normal
clone and a *file* in a worktree, so a check for one wants `directory_exists(p) || file_exists(p)`.

`\e` is ESC, beside `\n`, `\t`, `\r`, `\"` and `\\`. Colour is what `printf` is for, and without it every
coloured line had to stay a shell line — twelve of them in the corpus did.

`now` and `uuid` are read-only, so a preview shows a real value rather than a placeholder: `--dry-run` is
about not changing anything. `json_get` returns a number, bool or string directly, and an object or array as
its compact JSON text, since the language has no map type. That last part was a dead end: an array came back
as *text*, so `length` counted its characters, and an object could not be looked into at all. **`json_query`
answers with a list instead** — a path where `[]` descends into every element of an array, or every value of
an object, which is jq's `.[]` and the whole point. `results[].packages[].groups[].max_severity` collects one
value per group across every result, and what comes back is one of the language's own lists, so `for`, `if`
and `length` do the rest of what a jq pipeline was doing. No lambdas were added: `select(. >= 7) | length` is
a `for` with an `if` in it, which is the language being the default.

**A query cannot fail.** A segment matching nothing narrows the answer to the empty list, so a path over
ragged data is usable — a result with no packages contributes nothing rather than stopping the run — and a
scalar met by a `[]` drops out rather than erroring. That is the split from `json_get`, which asks for *one*
value and fails without it: finding none is an answer to a query and a failure to a lookup. An empty segment
(`a..b`) is the one typo worth catching, and answers with nothing rather than silently reading as `a.b`.

The other three close the gaps that leaves. **`json_keys`** is the only way to ask what is *in* an object,
in document order (`preserve_order` is on, so that is the order the file was written in); an array answers
with its indices, so both can be indexed the same way. **`json_type`** answers `object`, `array`, `string`,
`number`, `bool` or `null` — named the way the language names its own types, `bool` rather than `boolean`, so
a `match` on it reads like any other. It is the question `json_get` cannot answer, having flattened a null to
`""` and a container to text; a missing path fails exactly as `json_get`'s does, so `?` says the rest rather
than a `"missing"` type being invented. **`json_format`** is `jq .`, reusing `Structured::pretty` so a
document is laid out the way `run :lint` lays out a `json` block — validated first, since tokenising is
laxer than JSON and handing back a malformed document would be worse than refusing it. **`json_encode`** is
the write direction, reusing `Structured::render`, which is also what **`json_set` now accepts a number, bool
or list through**: building an array meant hand-writing JSON text with the escapes the `json` block exists to
avoid. One encoder, so a value cannot look like one thing in a block and another in a call.

**`range` builds a list of numbers**, which the language had no way to do: "do this N times" meant a list
literal or a trip out to `seq`. `range(n)` counts from zero and stops short, which is what an index wants;
`range(a, b)` includes both bounds, which is what a run of ports or versions wants. An end below the start is
empty rather than a count down, so `range(1, length(xs))` over an empty list is nothing rather than an error
or a backwards list. Bounded the way `repeat` is, because `range(1e9)` is a typo rather than a plan.

**`sleep(seconds)` takes a float**, and does nothing under `--dry-run` -- waiting changes nothing, but a
preview that takes the full minute a real run takes is not a preview. It is the gap `retry … every` left: a
pause on its own was still `$ sleep 5`.

**`print` and `printf` write to stdout**, and replaced 142 `$ echo` and `$ printf` lines across the corpus —
each of which was a shell process started to say one sentence. `print` takes one or more values, separates
them with a space the way `echo` does, and ends the line with `\n`, or `\r\n` on Windows: it is the one place
the language emits a line ending of its own. `printf` writes exactly what it is given, with `%s`, `%d`, `%f`,
`%.Nf` and `%%` — no widths or flags. The count of substitutions has to match the count of values **in both
directions**, since a `%s` with nothing to put in it, and a value with no `%` to go to, are each a typo every
time. Types are not coerced: `%d` refuses a string and refuses 2.5.

Both write under `--dry-run`, for the same reason `now` and `uuid` answer with real values there — printing
changes nothing, and a preview that hides what a run would say is a worse preview. Both take a locked handle
and flush, because a parallel branch may be printing at the same moment, and because the next thing to write
is usually a child process holding the same descriptor. Inside a branch a `print` is labelled like the
commands around it, and a `printf` without a newline ends its line there rather than waiting for the rest
while another branch writes in between -- a line with no label, or half of one, could belong to any of them.

What did *not* convert is as much the point: a pipe, a redirect, `>&2`, a `~` the shell would expand, and the
ANSI-coloured `printf`s in `~/.runfiles/check-git-status.run` — the language has `\n`, `\t`, `\r`, `\"` and
`\\`, and no escape for ESC. Those stayed shell lines.

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
   `RUNFILE_SKIP_PREPARE`, `RUNFILE_PRIVATE_KEYS` and **every one of the ten variables `ci_detect` looks at**
   stripped — not just `CI` and `GITHUB_ACTIONS`, because CI mode now decides whether the machine-wide
   directory is read and whether `state.json` is written, so a stray `BUILDKITE` in someone's shell would
   quietly turn off the tests that check both and they would pass. The first three
   matter on Windows, where the Known Folder API ignores `HOME` and `APPDATA`; the CLI reads `HOME` before
   asking the platform for exactly this reason.
4. LSP behaviour is tested by scripting a whole client conversation through the real transport
   (`crates/runfile-lsp/tests/protocol.rs`), and `run :lsp` is started as a subprocess
   (`crates/runfile-cli/tests/lsp.rs`) — nothing in the former would notice an arm that never dispatched, and
   the subcommand is what ships. One of those tests asserts that stdout **opens with a frame**, in a
   directory with a project in it: `run` is a program that talks, and anything of its own ahead of the first
   header would desynchronise the client.
5. **`run check` cross-checks the six release triples**, so a break that only shows on another platform is
   found before CI. `windows-sys` 0.61 moving `BOOL` out of `Win32::Foundation` cost a release run: nothing
   local compiled for Windows, and `cargo check --target` needs no linker, so nothing had to. Targets `rustup`
   does not have are skipped and **named** -- a gate that passes is worth less when you cannot tell what it
   did not look at.
6. **The shell checker is held to bash.** `runfile-shell`'s tests hand the script of every syntax finding to
   `bash -n`, which has to refuse it too, and skip cleanly where bash is absent. That bash is found by path
   (`bash_program`), never spawned as a bare `bash`: on Windows the standard library looks in `System32` before
   PATH, and `System32\bash.exe` is WSL's launcher -- which GitHub's Windows images ship with no distribution, so
   its "no installed distributions" was read as bash's verdict on every script. The fleet's Windows runner is
   provisioned without WSL, which is why only the mirror's CI saw it. The helper passes over both of the
   launcher's places, as `shell::locate` does; it cannot call it, the runtime being above this crate. Every
   example in
   `SHELL-CHECK-RULES.md` is run through the checker (`tests/rules_doc.rs`), whose rule list must match `RULES`
   in order. For the author's corpus, `tests::corpus` reads runfile paths from `RUNFILE_CORPUS_LIST`, prints every
   finding and every script it gave up on, and reports any script bash refuses that the checker read. A rule
   change is not done until that sweep has been read by hand, since a report on a line that works is the one
   failure this checker promises not to have. This repository's own shell is gated by the runtime's "every
   target here would be let run" test, which asks the checker too.
7. **Every documented example is parsed.** `FUNCTIONS` and `KEYWORDS` carry an `example`, which is what hover
   and completion show and what a person copies out. `code_of` shipped one that could not parse at all
   (`if code_of($ …) != 0` — a capture's `)` has to be the last character of the line), and four others opened
   an `if` they never closed, because nothing had ever fed one back through the parser. A trailing result
   annotation — two spaces, then `#` — is dropped first: a comment is a whole line here, so `abs(-4)   # 4`
   says what the value is the way a REPL transcript does rather than being code.
8. `runfile-lang/tests/golden/` holds one file per AST shape beside the tree it parses to, with source
   positions stripped so a diff is about structure rather than whitespace. Regenerate a deliberate change
   with `UPDATE_GOLDEN=1 cargo test -p runfile-lang --test golden`.
9. Watch tests poll rather than sleep; a fixed sleep is either flaky or slow. They **settle first and then
   assert a delta**, never an exact run count: a watcher starts moments after its fixture wrote the project,
   and on macOS it hears about those writes -- FSEvents gives an event its id when the daemon flushes rather
   than when the write lands, so a stream created "from now" still reports writes from just before it
   existed. `cli.rs`'s `settled` waits for the counter to hold still, which is what makes the assertion
   below it about the write the test just made. Asserting `== b"x"` blamed a stale event about a *watched*
   file on the unwatched write that happened to follow it, and failed on macOS CI.
10. Cross-platform: normalize backslashes in path assertions. A test that can only hold on one platform should
   be `#[cfg]`-gated there rather than weakened.
11. **No test leaves a process running, and none of them waits on a clock to prove one is.** The `detach`
   tests said `sleep 30` and asserted that the run came back in under ten seconds -- a margin against a
   number, and a command still running when `cargo test` exited. On Windows that failed the Gitea step
   outright (see *runfile-runtime*, `detach`); everywhere it made the assertion a guess about how slow a
   loaded runner may be. `cli.rs`'s `UNTIL_RELEASED` waits for a file `release` writes instead, so "the run
   did not wait for it" is a fact rather than a measurement -- the command cannot have finished, because it
   had not been let go -- and the test ends with nothing of its own still running. The loop is bounded at
   twenty seconds so a test that fails before releasing does not leave one spinning.
12. **A position the tree reports is checked against the text.** `runfile-lang/src/tests/spans.rs` asserts that
   every name, in every position one can sit in and in every `.run` file here, is spelled at its span on the
   line it says -- nothing read spans closely until diagnostics underlined names, and most were wrong. The
   name check has two repository gates beside it: every target here would be let run
   (`runfile-runtime/src/tests/names.rs`), and every README example calls only functions that exist, with a
   whole-file example naming only what it binds (`readme.rs`). A change to scoping belongs in the runtime tests
   there that hold the runner to each rule the check assumes. For the author's corpus, feed a `find` list
   rather than walking `~/Workspace` from a test: flatpak build directories there hold `var/run -> /run`.

## Documentation

`README.md` is the public documentation, and is written for someone deciding whether to use this rather than
for someone working on it: it opens with a gallery of **complete** runfiles — cross-platform setup, a gate
that loops, `retry`, `parallel for`, a `json` block, secrets, a pre-commit hook, watch mode — each captioned with
the one capability it shows. Rationale lives at the end, under *Why a language*.

**Every ```sh block in it is a runfile, and is gated**: `runfile-lang/tests/readme.rs` parses each one and
re-formats it, so an example cannot go stale against the language and cannot show a shape `run :lint` would
immediately undo. Documentation that has drifted is worse than none — a reader copies it, it does not parse,
and they conclude the tool is broken. Shell transcripts are ```bash and output is untagged, so neither is
swept up by the gate.

**After any change to behaviour, ask whether `README.md` needs it too — before calling the work done.** It is
the public documentation, so a change that lands in the code and not in it is a change that silently makes the
docs wrong. Anything a reader could act on counts: a new function, property, keyword or CLI flag; a changed
default; a renamed thing; a new capability worth an example. The `readme.rs` gate catches an example that
stops *parsing*, and cannot catch one that still parses and is now merely untrue — which is the more common
way documentation rots.

**`SHELL-CHECK-RULES.md` and `LANGUAGE-CHECK-RULES.md` list every rule of the two checkers, and are gated the same
way** (`tests/rules_doc.rs` in `runfile-shell` and in `runfile-lang`): the same rules in the same order, each opening
with the summary its checker gives it, and every example flagged by that rule alone or by nothing -- and, for the
language's, resolving every name it reads. A rule added without its section fails there.

`GRAMMAR.ebnf` is the normative grammar. Update this file with any new design decision, crate, or behaviour
change.
