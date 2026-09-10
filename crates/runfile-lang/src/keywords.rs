//! What the line forms mean, for an editor to show on hover.
//!
//! `FUNCTIONS` and `PROPERTIES` already carry their own documentation, and a
//! person reading a `.run` file for the first time meets `#`, `$`, `exec` and
//! `run` long before either. Those are the words with no signature to read and
//! no completion entry to hover, so they are written down here instead.

/// One line form, as editor tooling sees it.
pub struct Keyword {
	/// The word itself. `"$"` and `"#"` are two, and are matched as characters
	/// rather than as words.
	pub name: &'static str,
	/// The shape of the construct, for the hover heading.
	pub syntax: &'static str,
	/// What it does, and the one thing about it worth knowing up front.
	pub doc: &'static str,
	pub example: &'static str,
}

pub const KEYWORDS: &[Keyword] = &[
	Keyword {
		name: "$",
		syntax: "$ <command line>",
		doc: "Hand the rest of the line to a shell — bash where it exists, Git Bash on Windows, `sh` \
		      otherwise. A run of `$` lines is **one** script, so a variable set on one is still set on \
		      the next. An interpolation becomes exactly one argument, so it never needs quoting.",
		example: "$ docker build -t app:{{ tag }} .\n$ docker push app:{{ tag }}",
	},
	Keyword {
		name: "exec",
		syntax: "exec <command> … end",
		doc: "Run `<command>` with the block's body as its **stdin**. The body is that command's \
		      language, not this one — only `{{ … }}` is read — so a Python or SQL block travels \
		      through untouched. In value position it captures what the command prints.",
		example: "exec sudo tee /etc/systemd/journald.conf.d/app.conf\n\t[Journal]\n\tStorage=persistent\nend",
	},
	Keyword {
		name: "run",
		syntax: "run <target> [arguments]",
		doc: "Run another target **in this process** — no second binary, no shell in between. Its \
		      arguments are values rather than shell text: one `{{ … }}` is one argument even with \
		      spaces in it, and a list becomes one argument per item. A bare `--` forwards the rest \
		      exactly as typed.",
		example: "run build --env=production\nrun _aws -- s3api put-bucket-encryption --bucket {{ name }}",
	},
	Keyword {
		name: "let",
		syntax: "let <name>[, <name>…] = <expression>",
		doc: "Bind a value. Assigning again without `let` rebinds it. Several names take a list \
		      apart in order — `_` for a position to skip, and a name with nothing to bind is an \
		      error rather than an empty string. A binding in `_shared.run` is visible to every \
		      target in that directory.",
		example: "let part = one_of(first(ARGS), \"major\", \"minor\", \"patch\")\nlet major, minor, patch = split(cur, \".\")\nlet owner, _, branch = split(ARG.ref, \"/\")\n\npart = \"patch\"",
	},
	Keyword {
		name: "do",
		syntax: "do … end",
		doc: "A block with no condition. Properties are block-scoped, so this is where `.workdir`, \
		      `.env-file`, `.shell` or `.ignore-errors` go when they should cover a few commands and \
		      not the whole target — without inventing an `if true` to hold them.",
		example: "do\n\t.workdir = \"web\"\n\n\t$ pnpm install\n\t$ pnpm exec vite build\nend",
	},
	Keyword {
		name: "if",
		syntax: "if <condition> … [else …] end",
		doc: "Branch. The condition is an ordinary expression — or a command: `if $ cmd` is true when \
		      the command succeeds, and neither captures its output nor stops the target.",
		example: "if RUN.os == \"windows\"\n\t$ ./build.ps1\nelse\n\t$ ./build.sh\nend\n\nif $ command -v shellcheck >/dev/null\n\t$ shellcheck scripts/*.sh\nend",
	},
	Keyword {
		name: "else",
		syntax: "if … [else if <condition> …] [else …] end",
		doc: "The other branch of an `if`, or — after `retry` — what runs when every attempt failed. \
		      `else if` continues the chain rather than opening a block of its own, so however many \
		      of them there are, one `end` closes the lot.",
		example: "if RUN.arch == \"aarch64\"\n\tlet target = \"arm64\"\nelse if RUN.arch == \"x86_64\"\n\tlet target = \"amd64\"\nelse\n\terror(\"unsupported: {{ RUN.arch }}\")\nend",
	},
	Keyword {
		name: "for",
		syntax: "for <name>[, <name>…] in <list> … end",
		doc: "Loop over a list. Lists nest, so a row can carry several fields — and several names \
		      take each row apart as it arrives, which is what `for name, owner in …` is for. \
		      `break` leaves the loop and `continue` starts the next pass.",
		example: "for name, owner in [\n\t[\"app-data\", \"999:999\"],\n\t[\"app-cache\", \"1000:1000\"],\n]\n\t$ docker run --rm -v {{ name }}:/v alpine chown -R {{ owner }} /v\nend",
	},
	Keyword {
		name: "in",
		syntax: "for <name> in <list>",
		doc: "Separates a loop's binding from the list it walks.",
		example: "for compose in glob(\"**/docker-compose.yml\")\n\t$ docker compose -f {{ compose }} config -q\nend",
	},
	Keyword {
		name: "while",
		syntax: "while <condition> … end",
		doc: "Run the block again while the condition holds, testing it before each pass. The \
		      condition is an ordinary expression — or a command, as in an `if`. Refused inside a \
		      `.parallel` block, which has to know its branches before any of them runs.",
		example: "let left = 5\n\nwhile left > 0\n\tprint(\"{{ left }} to go\")\n\tleft = left - 1\nend",
	},
	Keyword {
		name: "until",
		syntax: "until <condition> … end",
		doc: "`while` with the question the other way round: run again until the condition holds. \
		      `until $ cmd` is the wait loop — run the block again until the command succeeds.",
		example: "until $ curl -sf localhost:8080/health\n\tsleep(1)\nend",
	},
	Keyword {
		name: "loop",
		syntax: "loop … end",
		doc: "Run the block again forever. It takes no condition — `break` is how it ends, and \
		      `while` is the form that asks a question. Under `--dry-run` the body is walked once: a \
		      preview performs none of the effects the loop is waiting on.",
		example: "loop\n\tif file_exists(\"build/done\")\n\t\tbreak\n\tend\n\n\tsleep(1)\nend",
	},
	Keyword {
		name: "break",
		syntax: "break",
		doc: "Leave the innermost `for`, `while`, `until` or `loop`. Written anywhere else it is a \
		      parse error, so an editor says so rather than a run finding out.",
		example: "for f in glob(\"**/*.log\")\n\tif contains(read_file(f), \"PANIC\")\n\t\tprint(\"first panic in {{ f }}\")\n\t\tbreak\n\tend\nend",
	},
	Keyword {
		name: "continue",
		syntax: "continue",
		doc: "Start the innermost loop's next pass, skipping the rest of the body.",
		example: "for f in glob(\"src/**/*.rs\")\n\tif starts_with(basename(f), \"_\")\n\t\tcontinue\n\tend\n\n\t$ rustfmt --check {{ f }}\nend",
	},
	Keyword {
		name: "match",
		syntax: "match <value> … case \"…\" … [default …] end",
		doc: "Dispatch on a value. A label is a **quoted string**, because the subject is a value and \
		      the label is compared against it. `match $ cmd` dispatches on a command's exit code.",
		example: "match RUN.os\n\tcase \"linux\"\n\t\t$ sudo apt-get install -y build-essential\n\tcase \"macos\"\n\t\t$ xcode-select --install\n\tdefault\n\t\terror(\"unsupported: {{ RUN.os }}\")\nend",
	},
	Keyword {
		name: "case",
		syntax: "case \"<label>\"",
		doc: "One arm of a `match`. The label is a quoted string — `case linux` would be asking about \
		      a bare word, and everything else in the language spells a string with quotes.",
		example: "match $ curl -fsS https://example.com/health\n\tcase \"0\"\n\t\tprint(\"healthy\")\n\tcase \"22\"\n\t\terror(\"the endpoint answered 4xx or 5xx\")\nend",
	},
	Keyword {
		name: "default",
		syntax: "default",
		doc: "The arm a `match` takes when no `case` matched. Optional: without one, an unmatched \
		      value simply runs nothing.",
		example: "match RUN.os\n\tcase \"linux\"\n\t\trun flatpak:install\n\tdefault\n\t\tprint(\"No packaged install for {{ RUN.os }}.\")\nend",
	},
	Keyword {
		name: "retry",
		syntax: "retry <n> [every <seconds>] … [else …] end",
		doc: "Run the block again while it fails, up to `n` times, pausing `every` seconds between \
		      attempts. `else` runs once when every attempt has failed. Failure is always visible \
		      inside it, whatever `.ignore-errors` says — a retry that could not see failure would \
		      run exactly once.",
		example: "retry 120 every 1\n\t$ docker exec db pg_isready -h 127.0.0.1\nelse\n\terror(\"the database never became ready\")\nend",
	},
	Keyword {
		name: "every",
		syntax: "retry <n> every <seconds>",
		doc: "How long to wait between attempts of a `retry`. Left out, the attempts follow one \
		      another immediately.",
		example: "retry 150 every 2\n\t$ adb shell getprop sys.boot_completed | grep -q 1\nend",
	},
	Keyword {
		name: "json",
		syntax: "let <name> = json … end",
		doc: "A block of JSON as one value. An interpolation inside renders as **one JSON value** — a \
		      string quoted and escaped, a whole number written whole, a list as an array — so \
		      `\"{{ x }}\"` is as wrong here as it is on a `$` line. The document is checked when the \
		      file is parsed, so a missing brace underlines as you type, and `run :format` lays it out.",
		example: "let policy = json\n\t{\n\t\t\"Version\": \"2012-10-17\",\n\t\t\"Days\": {{ number(ARG.days) }},\n\t\t\"Resource\": {{ buckets }}\n\t}\nend\n\nrun _aws -- s3api put-bucket-policy --policy {{ policy }}",
	},
	Keyword {
		name: "code_of",
		syntax: "code_of($ <command line>) or code_of(run <target> [args])",
		doc: "The exit status of a command, or of another target, as a number. It never stops this \
		      target and never captures output, so it is the way to ask how something went rather \
		      than depend on it. This is the one call a `$` capture or a `run` may sit inside. A \
		      dispatched target is scored the way the binary would exit: `exit(3)` is 3, any other \
		      failure is 1.",
		example: "let rust = code_of(run coverage:rust)\nlet web = code_of(run web:coverage)\n\nif rust != 0 || web != 0\n\texit(1)\nend",
	},
	Keyword {
		name: "end",
		syntax: "end",
		doc: "Closes the nearest open block — `if`, `for`, `while`, `until`, `loop`, `match`, \
		      `retry`, `do`, `exec` or a structured block. A chain of `else if` is one block, so one \
		      `end` closes all of it. An `exec` body closes only on an `end` at its **opener's** \
		      indentation, so a body may contain the word freely.",
		example: "for f in glob(\"*.sh\")\n\t$ shellcheck {{ f }}\nend",
	},
	Keyword {
		name: "#",
		syntax: "# <text>",
		doc: "A comment, to the end of the line. It may follow code, and starts at a `#` that begins \
		      a **word** — the shell's own rule, so one sentence covers both halves of a file: the \
		      text after `$ ` is the shell's, `#` included, and `a#b` is one word on either side. A \
		      `#` inside a string or a `{{ … }}` is text. The block of comments a file opens with \
		      is the target's description, which `run <target> --help` prints.",
		example: "# Build every release binary.\n\nlet targets = [\n\t\"x86_64-unknown-linux-musl\", # the portable one\n\t\"aarch64-apple-darwin\",\n]",
	},
];
