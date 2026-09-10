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
| `run <target> [args]` | Dispatch another target, in-process. |
| `expr` | Evaluated for effect, e.g. `write_file(…)`. |

**The one rule: the language is the default, the shell is marked.** `exec`, `end` and `$ ` were chosen because
they collide with none of the shell lines in the 1,897-line corpus this was designed against; a keyword-first
design would have collided with 69.

`$ run x` re-execs the binary. `run x` dispatches in-process — that is the normal form.

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
`ARG.x` out of the scope the classification is about to fill.

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

**`:format` lays a block out**, unlike an `exec` body. An `exec` body is somebody else's language and only its
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
exactly once, which is the least useful way for it to be wrong — and an `exit()` inside is re-raised rather
than retried. It is refused inside `.parallel`: several bodies sleeping and re-running against each other has
no useful reading of what "attempts" counted, so `RunError::RetryInParallel` says so rather than guessing.

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

Refused inside `.parallel`, like `retry`: a fan-out collects every branch before any of them runs, and a
conditional loop has nothing to collect until its body has run. `break` has the same problem from the other
end. A `for` is still expanded there, because its list is known before it starts.

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
re-execing the binary and not to mean something else: a target that calls `exit(3)` is scored 3, and every
other failure is the 1 the CLI reports (a target is not its last command, so there is no other status it could
honestly carry). The failure is printed where the re-exec's own `[runfile] error:` would have appeared —
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
line into or out of a body. The formatter re-attaches the comment one space out and never touches its text:
it is prose, and re-spacing an author's sentence is not what a formatter is for. Nothing in the tree records
a comment, so the fingerprint check cannot catch a dropped one — only a test can.

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
.github/actions/setup/         # The only shipped action: installs `run`, PATH, optional secret keys
runfiles/                      # This project's own targets (self-hosting); ci/ and wsl/ are namespaces
editors/vscode/                # The VS Code extension (TypeScript) + its own runfiles/
editors/tree-sitter/   # The tree-sitter grammar (Zed, Neovim, Helix) + its own runfiles/

crates/
  runfile-lang/                # Lexer, parser, evaluator, values, the function library
  runfile-discovery/           # Finding runfiles/ directories and building the catalog
  runfile-runtime/             # Properties, env building, process spawning, the walker, dispatch
  runfile-lsp/                 # Language server (a library; served by `run :lsp`)
  runfile-cli/                 # The `run` binary
  runfile-env/                 # .env parsing and env-map building
  runfile-crypto/              # AES-256-GCM for encrypted env values
  runfile-state/               # Prepare state, OS credential store access
```

## Crate Responsibilities

### runfile-lang

`ast.rs` carries `LoopTest` beside `Statement`. `eval.rs` holds `destructure`, the one description of what a
comma-separated left-hand side means.
`args.rs` classifies a command line against what the target reads — the one place the `--key value` rule is
spelled, so the runner and the `--stdin-args` prompt cannot read the same words two ways.
`format.rs` is the pretty-printer. `lexer.rs` is a hand-rolled scanner (`skip_interp`, `split_interp`, `scan_string`, `tokenize`). `parser.rs`
classifies lines, then climbs precedence for expressions. `eval.rs` holds `Scope` and evaluation; `functions.rs`
holds the pure standard library plus `call_io` for filesystem and regex; `value.rs` holds `Value` and shell
quoting.

- `Scope.private_keys` is a `Keys`: a **deferred, memoized** key pool. Loading is deferred because the pool
  comes from an OS credential store, and a locked keyring blocks on an interactive unlock prompt — an eager load
  turned every `run <target>` into a hang. Memoized so a run that decrypts twice still prompts once.
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
- **`exit()` ends the run with a status.** It leaves as `EvalError::Exit`, since an error is the only path
  out of an expression, and *every* catcher re-raises it: `try`, a `?` chain and `.ignore-errors` all let it
  through, the way an interrupt is not something a target gets to shrug off. `RunError::exit_code` is what the
  CLI reads to set its own status instead of printing an error.
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

### runfile-discovery

Walks **up** for the nearest `runfiles/`, then **down** for `*/runfiles/` (depth cap 3, skipping
`node_modules`, `target`, `dist`, `build`, `.git`, `vendor`). Nested directories become `:`-separated namespace
segments. The machine-wide directory is `$HOME/.runfiles/`, `$HOME/runfiles/` or `$HOME/Runfiles/` — a
**fixed set of names with no setting to add to it**, so a person can show the folder or hide it without
telling the runner. **None of them is read in CI**: `main::discovery_home` answers `None` there, which is the
whole gate — one call rather than an `is_ci` in the catalog, `:list`, `:format`, `:generate` and `:complete`
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
  machine-wide one has none, and `Catalog.shared` records a key whether or not the file is there, so keying
  by it meant that merely *having* a `~/.runfiles` silently disabled the root `_shared.run` of every project
  on the machine. Its properties *and* its `let` bindings apply (`run_block_bindings`), which is what makes
  it the `globals` analog.
- `resolve` is one hash lookup; `.alias` is only scanned on a miss, so aliases cost nothing in the common case.
  A real file name always wins over an alias, and two targets claiming one alias is an error naming both.
- **An alias carries its target's namespace.** `web/runfiles/setup.run` declaring `deps` answers to `web:deps`,
  never a bare `deps` — a subproject must not claim a name in the root.
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
  `:list` grouping, and what `:format` and `:generate` leave out without `--include-global`.

### runfile-runtime

`props.rs` (property resolution), `env.rs` (env building), `exec.rs` (spawning), `run.rs` (the walker),
`dispatch.rs` (`Host`, target resolution, cycle detection), `shell.rs` (shell selection).

- `PROPERTIES` is exported with a block-scoped flag per name, tested against `extend` the same way `FUNCTIONS`
  is. Block-scoped: `shell`, `parallel`, `ignore-errors`, `workdir`, `env`. The rest are header-only.
- **Shell resolution**: bash → Git Bash (four known Windows paths) → sh. `System32\bash.exe` is deliberately
  excluded: it is the WSL launcher, and a different filesystem.
- `is_shell()` matches `sh|bash|dash|ash|zsh|ksh|busybox|brush` on the **first word only**, and inserts `-e`.
  `brush` is there because it is a bash-compatible shell someone may name in `.shell`; without it such a
  target would run fine and silently stop stopping on failure. It is *not* a default candidate — the default
  has to supply the POSIX toolbox as well as the language, which a shell alone does not: 9% of the corpus's
  shell lines call one of 31 toolbox programs, and `sed`, `grep`, `awk`, `find`, `xargs` and `tar` are not
  coreutils at all. Git Bash ships both, which is why it is the Windows answer.
- **`Dispatch::run` returns the child's trace** rather than writing to shared state. A child finishes while its
  parent is still walking, so a shared buffer printed every dependency *before* the line that called it. The
  caller splices the trace in where the call appeared, which is what makes `--dry-run` order match execution
  order.
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
  files per run, one an orphan nothing referenced.
- **Ctrl+C** is caught so the run can stop between statements, delete its temp files, and exit 130. The flag
  is process-global because a signal handler has nowhere else to write, but the runtime reads an injected
  predicate (`Host::interrupted`), so it reaches for no process state of its own and one test cannot
  interrupt another. `.ignore-errors` does not apply to it.
- `Dispatch` is `Sync` with `&self` and an explicit `chain: &[String]`. Per-path rather than shared, so
  parallel siblings are not mistaken for a cycle.
- `.parallel`: bindings evaluate in source order, then executable leaves fan out via `std::thread::scope`.
  Control flow expands into the same batch, and every branch completes before a failure surfaces.
- **A block inside a fan-out layers its own properties, and the leaf carries them.** `collect` recursed into a
  nested `do` / `if` / `for` / `match` with the *enclosing* properties, so every block-scoped property inside a
  `.parallel` block was parsed, accepted, and then ignored -- a `.workdir` ran in the anchor, a `.env` reached
  nobody, a `.env-file` was never read. `collect_nested` is `nested` for the collecting walk: `extend` plus
  `with_block_env`, so the same `.env-file`/`.add-path` rebuild happens and is put back after. The rest had to
  move *into* `Leaf`, because a leaf is rendered where it is collected and spawned somewhere else: `.shell`
  resolved through `command_for`, `.logging` as `announce`, and `.ignore-errors` paired with its own branch --
  read off the batch, a block that asked to be forgiven either took its siblings with it or was not forgiven
  at all. `.detach` is header-only and `extend(nested)` clears it, which is what the sequential walk does too.
- **A fan-out's preamble runs through the process host, like every other statement.** `collect` reached the
  *pure* evaluator for an `if` condition, a `for` list and a bare call, so `if $ cmd`,
  `for f in lines($ git ls-files)` and `code_of($ cmd)` were an error inside a `.parallel` block -- *"capture
  needs a process host"*, an internal sentence about the walker, for a line that works one indent out. They
  are `cond_of` and `value_of` now, the two the sequential walk uses. Not a new decision: `collect`'s `let`
  arm already ran its captures this way, and these sit in the same place -- the preamble is what decides
  *what* fans out, so it is answered in source order before any leaf exists. `Statement::Match` was already
  right, since `subject_of` runs a capture itself; a test pins it so the four cannot drift apart again. What a
  preamble capture runs under is the *nested* block's properties, because `props` is what the collecting walk
  is holding -- which is the bullet above.
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
  reason a `.parallel` branch is not labelled `bash`: `` `docker compose up -d` exited with status 1 `` where
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
- **`.detach`** starts the commands and does not wait. Its streams go to null: inherited, they would hold the
  runner's own stdout and stderr open after it exits, so whoever is reading them waits for the very command
  that was meant to outlive the run. **On Windows that is not enough** -- `Stdio::null()` says what a child
  *uses*, not what it *holds*, and `CreateProcessW` is called with `bInheritHandles: TRUE`, so every
  inheritable handle in the runner is duplicated into the child anyway, the pipes its own stdout and stderr
  arrived on included. `exec::KeepHandles` clears `HANDLE_FLAG_INHERIT` on the three standard handles across a
  detached spawn and restores it after. Safe for whatever else is spawning at the time, `.parallel` included,
  because `Stdio::inherit()` does not depend on that flag: the standard library duplicates the handle it passes
  with `bInheritHandle` set regardless. Unix needs none of it -- everything but the three descriptors a child
  is handed is close-on-exec.
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
- **`keywords::KEYWORDS` documents the line forms** — `#`, `$`, `exec`, `run`, `let`, `if`, `else`, `for`,
  `in`, `match`, `case`, `default`, `retry`, `every`, `json`, `code_of`, `end`. These are what a person meets
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
- **Shellcheck delegation**: `$` runs and `exec sh|bash|dash|ash|ksh` bodies are handed to shellcheck. An
  interpolation renders as one quoted placeholder, because it resolves to exactly one shell word; leaving the
  braces in would have shellcheck reporting on a command nobody wrote. Since a placeholder is a different width
  from what it stands for, a line containing one is reported **whole** rather than with a confidently wrong
  column. Shellcheck's four levels map onto LSP's four. Checking is skipped when the document does not parse,
  and a missing shellcheck is silent.

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
`.run` files; every other editor gets the same thing straight from the LSP.

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
  is one pass over that whitespace answering for both the comment and the `exec` keyword, because the second
  is recognised at column 0 and would otherwise be answered by the blanks the first had skipped. The string rule needs no
  scanner: the interpolation's expression is parsed as an expression, quotes and all. A dispatch's word inside
  `code_of(…)` is a second token, because there the `)` closing the call ends it — unless a `(` in the word
  opened it, which is how the parser reads one too.
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
`cmd_env/`, `cmd_format.rs`, `cmd_update.rs`, `ci_detect.rs`.

- **Runner flags are recognised only before the target name**; everything after it belongs to the target. So
  `run echoes --dry-run` passes `--dry-run` through as `FLAG.dry-run`.
- `:list` shows the aliases a target answers to. Without that a documented alias is undiscoverable, since
  the listing otherwise reports only the file name -- which the corpus inventory diff is what surfaced.
- **`:list` shows the machine-wide targets first**, under `global:`. They are reachable from every directory
  and appear in no file a reader of the project can see, so they are the group worth meeting first; the local
  ones follow. `local:` is the unlabelled default only while nothing precedes it, which is every project
  without a machine-wide directory -- an unheaded run of names under `global:` would read as more global ones.
  The order is the human listing's; `--names` and `--json` stay in the catalog's own (sorted) order, since
  their readers work by name.
- `:list` has three forms: human, `--names` (for completion scripts), `--json` (for tooling). The JSON is
  serialized by hand — four string fields do not justify a serde dependency in the CLI — and carries a
  `formatVersion` that CI checks against the extension's constant.
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
- `:format` writes every runfile into the one shape there is, `_shared.run` included — it is not a target, so
  nothing that walks the catalog by name would reach it. Global files are left out unless `--include-global`,
  as with `:generate`. `--check` reports and exits 1; `--stdout` prints. Explicit paths override the catalog,
  and a directory argument is walked.
- `:generate zed|jetbrains|vscode` is a lean port of the old generators: an entry is recognised as ours by its
  shape (command `run`, label `run <target>`), so a rerun replaces exactly those and keeps a person's own; a
  file is rewritten with the indentation it already uses. The 661-line `.editorconfig` reader did not come
  back. Global targets are left out unless `--include-global`, since a task file is committed and
  the machine-wide directory is one person's. `Catalog.root` (the parent of the nearest `runfiles/`) is where the files go.

## Properties

Header-only: `alias`, `watch`, `only-in-directories` (machine-wide files only), `detach`.
Block-scoped (may also appear inside `if` / `for` / `match`): `shell`, `parallel`, `ignore-errors`, `logging`,
`workdir`, `env` (addressed by sub-key, `.env.NAME = "value"`), `env-file`, `add-path`.

**`env-file` and `add-path` are block-scoped, and both append.** A block that names one has a longer list than
the block around it, which is how `with_block_env` tells it has to rebuild -- reading and decrypting the files
again for every `if` would be work nothing asked for. The environment the block inherited is put back when it
closes, which is what stops a loop carrying one iteration's files into the next, and is why **every** block
form goes through that one function: `if` and `match` via `nested`, and `for` and `retry` via their own arms,
whose bodies were extracted into `for_body` and `retry_attempts` so the wrapper has something to wrap. A
`retry`'s `else` runs *outside* the body's environment -- it is what to do when the block never worked, not
part of it. What this buys over the header form is that a block's file can be named by something the body
computed, since a header property resolves before any statement runs. A `.env-file` naming a file that is not
there is skipped, the same as a header one: a project whose `.env` is git-ignored still has to run.

**`.confirm` and `.hide` are gone**, each replaced by something that could not disagree with itself.

`confirm(question)` is a function, so it can be called anywhere -- inside an `if`, after the value it asks
about has been worked out. Declining raises `EvalError::Cancelled`, which travels exactly like `Exit`: every
catcher re-raises it, because someone who said no has not asked to be second-guessed by a `?` fallback or by
`.ignore-errors`. `RunError::is_stop()` is what those catchers now consult -- `exit_code()` still answers only
the status question, so a cancel keeps printing `cancelled` rather than exiting silently. The prompt reaches
it as `Scope::confirm`, a plain `fn(&str) -> bool` like `Scope::ask`, which is why `prompt::confirmer()`
became `prompt::confirm`: the closure captured nothing. It is skipped by `-y`, in CI, and under `--dry-run`,
where there is nothing to approve -- previously a guarded target could not be previewed at all.

**A target is hidden when its file name starts with `_`** (`discovery::is_hidden`, which reads the last `:`
segment so a namespace is not part of the question). Fifteen of the sixteen targets that set `.hide` were
already named that way, and every `_`-prefixed target set it -- the property only let the two disagree. The
sixteenth, `kico/runfiles/test.run`, is now visible.

A nested block inherits behaviour but never a parent's one-shot header state.

**`do … end` is a block with no condition** (`Statement::Do`, one arm in `walk` and one in `collect`). Every
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

## Preparation targets

A target named `setup` gates every other target in its directory, fingerprinted by its own text, so editing the
setup re-triggers the requirement. State lives in `state.json` in the platform state directory — **except in
CI, where there is none**. A runner is built from scratch and thrown away, so asking whether an earlier `setup`
happened is asking about a machine that did not exist: `prepare::enforce` returns before reading the file and
`prepare::record` returns before creating one. It used to be written on every CI run purely for a cleanup step
to delete afterwards. `RUNFILE_SKIP_PREPARE` is deliberately *not* the same predicate — it turns the gate off
on a machine whose state is still worth keeping, so a `setup` run under it is still recorded, or unsetting the
variable would report a setup that plainly ran as never having run. There is no
settings file: global registrations, path aliases and custom shell paths were all replaced by conventions
(the machine-wide directory, discovery, shell detection).

## Removed, and not coming back

MCP server, `:convert`, `:config` (all subcommands), the user settings file, `-p` / target globs, `capture()`
(now `$` in value position), `shell_quote()` (interpolation self-quotes), `set_cwd()` (now `.workdir`),
`define()` (now `let`), `nth()` / `count_parts()` (now `split()` and indexing), the arithmetic and comparison
functions (now operators), `when:` blocks, `sameShell`, `extendStdio`, `forceKillOnSigInt`, the JSON schema,
`VAR.` (replaced by `let`), and `RUNFILE_TARGET`. Dropped dependencies: `rmcp`, `tokio`, `json5`, `md-5`,
`shlex`.

Every other function from the old surface is present. Seventeen were missing at one point, dropped by
oversight rather than decision, and all are back. `try` is the one exception, replaced by `a ? b`.

**`run <target> --help`** prints the target's whole description, every input it reads — with what happens
without each one — its aliases and its path (`target_help.rs`). Two things made it necessary: `--help` after a target name is the
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
document is laid out the way `run :format` lays out a `json` block — validated first, since tokenising is
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
and flush, because a target dispatched into a `.parallel` branch may be printing at the same moment, and
because the next thing to write is usually a child process holding the same descriptor. A `print` is never
itself a parallel branch: `collect` evaluates `Statement::Call` in source order before any leaf fans out.

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
6. Tests that need an external tool (shellcheck) skip cleanly when it is absent, so a contributor without it
   does not see a broken build. One of them is a gate: `runfile-lsp/tests/repo_shell.rs` shellchecks every
   `.run` file in this repository through the same extraction an editor uses, so the repo's own shell cannot
   rot. It caught an unbalanced `if` in a golden fixture the first time it ran.
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

## Documentation

`README.md` is the public documentation, and is written for someone deciding whether to use this rather than
for someone working on it: it opens with a gallery of **complete** runfiles — cross-platform setup, a gate
that loops, `retry`, `.parallel`, a `json` block, secrets, a pre-commit hook, watch mode — each captioned with
the one capability it shows. Rationale lives at the end, under *Why a language*.

**Every ```sh block in it is a runfile, and is gated**: `runfile-lang/tests/readme.rs` parses each one and
re-formats it, so an example cannot go stale against the language and cannot show a shape `run :format` would
immediately undo. Documentation that has drifted is worse than none — a reader copies it, it does not parse,
and they conclude the tool is broken. Shell transcripts are ```bash and output is untagged, so neither is
swept up by the gate.

**After any change to behaviour, ask whether `README.md` needs it too — before calling the work done.** It is
the public documentation, so a change that lands in the code and not in it is a change that silently makes the
docs wrong. Anything a reader could act on counts: a new function, property, keyword or CLI flag; a changed
default; a renamed thing; a new capability worth an example. The `readme.rs` gate catches an example that
stops *parsing*, and cannot catch one that still parses and is now merely untrue — which is the more common
way documentation rots.

`GRAMMAR.ebnf` is the normative grammar. Update this file with any new design decision, crate, or behaviour
change.
