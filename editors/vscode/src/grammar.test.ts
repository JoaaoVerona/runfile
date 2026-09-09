// Tests for the TextMate grammar: that a `$` line is handed to the shell
// grammar, and that everything which is *not* shell is not.
//
// The point of these is one bug in particular. The grammar set `contentName`
// to an embedded-shell scope and stopped there, which tells VS Code which
// language's comments and brackets to use but nothing about how to tokenize --
// so a shell line came out one flat colour. Colour needs `include:
// source.shell`, and only a real tokenizer run can tell the two apart.
//
// `source.shell` here is a stub that scopes every word `test.shell`. The point
// is whether our grammar *delegates*, not how VS Code's shell grammar happens
// to break up a command.

import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { test } from "node:test"
import * as oniguruma from "vscode-oniguruma"
import * as vsctm from "vscode-textmate"

const RUN_GRAMMAR = path.join(__dirname, "..", "syntaxes", "runfile.tmLanguage.json")

/** Stands in for VS Code's shellscript grammar. */
const SHELL_STUB = {
	scopeName: "source.shell",
	patterns: [{ match: "\\S+", name: "test.shell" }]
}

const INJECTION = path.join(__dirname, "..", "syntaxes", "runfile-interpolation.injection.json")

/**
 * VS Code's own shell grammar, if this machine has VS Code.
 *
 * Worth reaching for: the anchoring inside it is the whole reason the
 * embedding needed care, and a stub cannot stand in for that. Tests that use
 * it skip when it is absent, the way the shellcheck ones do.
 */
function realJsonGrammar(): string | undefined {
	return [
		process.env.RUNFILE_JSON_GRAMMAR,
		"/usr/share/code/resources/app/extensions/json/syntaxes/JSON.tmLanguage.json",
		"/usr/lib/code/extensions/json/syntaxes/JSON.tmLanguage.json",
		"/Applications/Visual Studio Code.app/Contents/Resources/app/extensions/json/syntaxes/JSON.tmLanguage.json"
	].find((c): c is string => !!c && fs.existsSync(c))
}

function realShellGrammar(): string | undefined {
	const candidates = [
		process.env.RUNFILE_SHELL_GRAMMAR,
		"/usr/share/code/resources/app/extensions/shellscript/syntaxes/shell-unix-bash.tmLanguage.json",
		"/usr/lib/code/extensions/shellscript/syntaxes/shell-unix-bash.tmLanguage.json",
		"/snap/code/current/usr/share/code/resources/app/extensions/shellscript/syntaxes/shell-unix-bash.tmLanguage.json",
		"/Applications/Visual Studio Code.app/Contents/Resources/app/extensions/shellscript/syntaxes/shell-unix-bash.tmLanguage.json"
	]
	return candidates.find((c): c is string => !!c && fs.existsSync(c))
}

function load(file: string): vsctm.IRawGrammar {
	return vsctm.parseRawGrammar(fs.readFileSync(file, "utf8"), file)
}

async function registry(shellGrammar?: string): Promise<vsctm.Registry> {
	const wasm = fs.readFileSync(require.resolve("vscode-oniguruma/release/onig.wasm"))
	await oniguruma.loadWASM(wasm.buffer as ArrayBuffer)
	return new vsctm.Registry({
		onigLib: Promise.resolve({
			createOnigScanner: (s: string[]) => new oniguruma.OnigScanner(s),
			createOnigString: (s: string) => new oniguruma.OnigString(s)
		}),
		// What `injectTo` in package.json does at runtime.
		getInjections: (scope: string) =>
			scope === "source.run" ? ["runfile.injection.interpolation"] : undefined,
		loadGrammar: async (scope: string) => {
			if (scope === "source.shell") {
				return shellGrammar ? load(shellGrammar) : (SHELL_STUB as unknown as vsctm.IRawGrammar)
			}
			if (scope === "source.run") {
				return load(RUN_GRAMMAR)
			}
			if (scope === "runfile.injection.interpolation") {
				return load(INJECTION)
			}
			if (scope === "source.json") {
				const json = realJsonGrammar()
				return json ? load(json) : null
			}
			return null
		}
	})
}

interface Token {
	text: string
	scopes: string[]
}

/** Every token of every line, with the scopes it carries. */
async function tokensOf(source: string, shellGrammar?: string): Promise<Token[][]> {
	const grammar = await (await registry(shellGrammar)).loadGrammar("source.run")
	assert.ok(grammar, "the runfile grammar loaded")
	let state = vsctm.INITIAL
	const out: Token[][] = []
	for (const line of source.split("\n")) {
		const r = grammar.tokenizeLine(line, state)
		out.push(r.tokens.map((t) => ({ text: line.slice(t.startIndex, t.endIndex), scopes: t.scopes })))
		state = r.ruleStack
	}
	return out
}

/** Every scope applied to every line, as one array per line. */
async function scopesOf(source: string, shellGrammar?: string): Promise<string[][]> {
	const lines = await tokensOf(source, shellGrammar)
	return lines.map((l) => [...new Set(l.flatMap((t) => t.scopes))])
}

test("a shell line is tokenized by the shell grammar", async () => {
	const [line] = await scopesOf("$ cp -r src dest\n")
	assert.ok(line?.includes("keyword.control.shell.run"), "the `$` is ours")
	assert.ok(line?.includes("test.shell"), "the command is the shell grammar's")
	assert.ok(line?.includes("meta.embedded.line.shell"), "and marked as embedded shell")
})

test("an interpolation inside a shell line stays ours", async () => {
	// `{{ … }}` is the language's, not the shell's; it must win over the
	// embedded grammar, which is why it is listed first.
	const [line] = await scopesOf("$ echo {{ ARG.name }}\n")
	assert.ok(line?.includes("meta.embedded.expression.run"), `got ${line?.join(" ")}`)
})

test("a shell exec body is shell and any other is not", async () => {
	const shell = await scopesOf("exec bash\n\tls -la\nend\n")
	assert.ok(shell[1]?.includes("test.shell"), "an `exec bash` body is shell")

	const python = await scopesOf("exec python3\n\timport sys\nend\n")
	assert.ok(!python[1]?.includes("test.shell"), `a python body is not: ${python[1]?.join(" ")}`)
	assert.ok(python[0]?.includes("keyword.control.exec.run"), "but `exec` is still a keyword")
})

test("an exec body closes only on an `end` at the opener's indentation", async () => {
	// A ruby or lua body carries its own `end`; closing on that one would spill
	// the rest of the file into the block.
	const lines = await scopesOf("\texec ruby\n\t\tif x\n\t\tend\n\tend\n$ after\n")
	assert.ok(lines[2]?.includes("meta.embedded.block.exec"), "the inner `end` is body text")
	assert.ok(lines[3]?.includes("keyword.control.exec.run"), "the outer one closes it")
	assert.ok(lines[4]?.includes("keyword.control.shell.run"), "and the file carries on")
})

test("a backslash continuation keeps the shell line open", async () => {
	const lines = await scopesOf("$ echo a \\\n\tb\n")
	assert.ok(lines[1]?.includes("test.shell"), `the continuation is still shell: ${lines[1]?.join(" ")}`)
})

test("a flag and an argument are told apart", async () => {
	const shell = realShellGrammar()
	if (!shell) {
		return
	}
	const [line] = await scopesOf("$ git commit -m 'x'\n", shell)
	assert.ok(line?.includes("constant.other.option.dash.shell"), "the flag")
	assert.ok(line?.includes("meta.argument.shell"), "the argument")
	assert.ok(line?.includes("string.quoted.single.shell"), "the quoted string")
})

/**
 * Scopes that cannot match, and need not.
 *
 * A `.sh` file's root is `source.shell` where ours is the embedded marker --
 * that is the embedding, not a difference in colour. And `meta.statement.shell`
 * wraps the *first* statement in a `.sh` file but not here: it comes from
 * `normal_statement`, the rule whose anchor we cannot satisfy. No theme
 * shipped with VS Code targets `meta.statement`, so nothing is coloured by it.
 */
// Embedding markers and the scope the anchored rule forces on us, none of
// which any shipped theme colours -- what is being compared is colour.
const STRUCTURAL = new Set([
	"source.shell",
	"meta.embedded.line.shell",
	"meta.embedded.block.shell",
	"meta.statement.shell"
])

/** The scopes that decide colour, in order. */
function coloured(scopes: string[]): string {
	return scopes.filter((s) => s.endsWith(".shell") && !STRUCTURAL.has(s)).join(" ")
}

/**
 * Assert that `$ <line>` is coloured exactly as `<line>` in a .sh file.
 *
 * This is the requirement stated plainly: the same theme, the same colours,
 * for the same command. Anything less specific would pass while the first
 * command on the line was left bare, which is what happened twice.
 */
async function sameAsShellFile(line: string, shell: string): Promise<void> {
	const reg = await registry(shell)
	const sh = await reg.loadGrammar("source.shell")
	const run = await reg.loadGrammar("source.run")
	assert.ok(sh && run, "both grammars loaded")

	const real = sh
		.tokenizeLine(line, vsctm.INITIAL)
		.tokens.map((t) => ({ text: line.slice(t.startIndex, t.endIndex), scopes: coloured(t.scopes) }))
		.filter((t) => t.text.trim())

	const wrapped = `$ ${line}`
	const ours = run
		.tokenizeLine(wrapped, vsctm.INITIAL)
		.tokens.map((t) => ({ text: wrapped.slice(t.startIndex, t.endIndex), scopes: coloured(t.scopes) }))
		// Drop the `$` marker itself, which is ours and has no counterpart.
		.slice(1)
		.filter((t) => t.text.trim())

	assert.deepEqual(ours, real, `\`${line}\` is not coloured the way a .sh file colours it`)
}

/**
 * Assert that `exec <cmd>` colours `<cmd>` the way a .sh file colours it.
 *
 * The body of an `exec` is that command's input and stays uncoloured, but the
 * command itself is a program with flags and arguments, exactly like the text
 * after `$ ` -- and it came out flat white, because the alternation inside the
 * rule's lookahead was silently taking the capture number the command needed.
 */
async function execSameAsShellFile(line: string, shell: string): Promise<void> {
	const reg = await registry(shell)
	const sh = await reg.loadGrammar("source.shell")
	const run = await reg.loadGrammar("source.run")
	assert.ok(sh && run, "both grammars loaded")

	const real = sh
		.tokenizeLine(line, vsctm.INITIAL)
		.tokens.map((t) => ({ text: line.slice(t.startIndex, t.endIndex), scopes: coloured(t.scopes) }))
		.filter((t) => t.text.trim())

	const wrapped = `exec ${line}`
	const ours = run
		.tokenizeLine(wrapped, vsctm.INITIAL)
		.tokens.map((t) => ({ text: wrapped.slice(t.startIndex, t.endIndex), scopes: coloured(t.scopes) }))
		// Drop the `exec` keyword itself, which is ours and has no counterpart
		// -- by position, since a command may well take `exec` as an argument.
		.slice(1)
		.filter((t) => t.text.trim())

	assert.deepEqual(ours, real, `\`${line}\` after \`exec\` is not coloured the way a .sh file colours it`)
}

test("an `exec` command is coloured exactly as the same command in a .sh file", async () => {
	const shell = realShellGrammar()
	if (!shell) {
		return
	}
	for (const line of [
		"sudo tee /etc/systemd/journald.conf.d/skiley.conf",
		"python3",
		"tee -a 'a file.txt'",
		"docker exec -i db psql -U postgres"
	]) {
		await execSameAsShellFile(line, shell)
	}
})

test("an `exec sh` command is coloured too, and its body stays shell", async () => {
	const shell = realShellGrammar()
	if (!shell) {
		return
	}
	await execSameAsShellFile("bash -eu", shell)
	const scopes = await scopesOf("exec bash -eu\n\techo hi\nend\n", shell)
	assert.ok(
		scopes[1].some((s) => s.includes("meta.embedded.block.shell")),
		"the body of an `exec bash` is still shell"
	)
})

test("an interpolation in an `exec` command is still ours", async () => {
	// It sits inside the region now coloured as shell, so the injection has to
	// win there the way it does on a `$` line.
	const scopes = await scopesOf("exec tee {{ ARG.path }}\n\tx\nend\n")
	assert.ok(
		scopes[0].some((s) => s === "meta.embedded.expression.run"),
		`no interpolation scope in ${JSON.stringify(scopes[0])}`
	)
})

test("a shell line is coloured exactly as the same command in a .sh file", async () => {
	const shell = realShellGrammar()
	if (!shell) {
		return
	}
	for (const line of [
		// Two commands: the first used to be left bare, because the shell
		// grammar starts a statement only after `^`, `;`, `|`, `&`, `!`, `(`,
		// `{` or a backtick -- and `$ ` is none of those.
		"echo 'abc'; echo 'abc'",
		"git commit -m 'x' && git push",
		"ls -la | grep x",
		// An assignment, a `for`, a function definition and a subshell: the
		// first fix reached for `command_statement`, which knows about none of
		// them, and flattened lines like these.
		'd=$(mktemp -d); trap \'rm -rf "$d"\' EXIT',
		'for f in *.sh; do bash -n "$f"; done',
		'r() { sed -e \'s|a|b|g\' "$1"; }; r x > "$d/out"'
	]) {
		await sameAsShellFile(line, shell)
	}
})

test("a capture in value position is coloured as the same `$` line", async () => {
	// `let x = $ cmd` had no rule at all: the shell text was read as a runfile
	// expression, so `--name-only` came out two operators and `"*.rs"` a
	// runfile string.
	const shell = realShellGrammar()
	if (!shell) {
		return
	}
	const reg = await registry(shell)
	const run = await reg.loadGrammar("source.run")
	assert.ok(run)
	const line = 'git diff --cached --name-only -- "*.rs" | wc -l'
	const scopes = (src: string, skip: number) =>
		run
			.tokenizeLine(src, vsctm.INITIAL)
			.tokens.map((t) => ({ text: src.slice(t.startIndex, t.endIndex), scopes: coloured(t.scopes) }))
			.filter((t) => t.text.trim())
			.slice(skip)

	// `$ <line>` is the shape already known to be right, so it is the standard.
	assert.deepEqual(scopes(`let rs = $ ${line}`, 4), scopes(`$ ${line}`, 1))
	assert.deepEqual(scopes(`rs = $ ${line}`, 3), scopes(`$ ${line}`, 1), "a reassignment too")
})

test("an exec bound to a name is still an exec block", async () => {
	// `let x = exec sh` … `end` matched nothing: the body was not shell, and
	// the block was not a block.
	const shell = realShellGrammar()
	if (!shell) {
		return
	}
	const bound = await scopesOf("let out = exec sh\n\techo hi\nend\n", shell)
	assert.ok(bound[0].includes("keyword.control.exec.run"), `${bound[0].join(" ")}`)
	assert.ok(bound[1].some((s) => s.includes("meta.embedded.block.shell")), `${bound[1].join(" ")}`)
	assert.ok(bound[2].includes("keyword.control.exec.run"), `end closes it: ${bound[2].join(" ")}`)

	// A body that is not shell stays the command's own.
	const other = await scopesOf("let out = exec python3\n\tprint(1)\nend\n", shell)
	assert.ok(
		!other[1].some((s) => s.includes("embedded.block.shell")),
		`a python body is not shell: ${other[1].join(" ")}`
	)
})

test("an embedded region never outlives its line", async () => {
	// The failure this catches: the shell grammar runs a rule to the end of a
	// line, so `code_of($ xcode-select --install)` had its `)` swallowed by the
	// option rule. With nothing to close the region, every line below it was
	// coloured as shell -- `default` stopped being a keyword and a string
	// stopped being a string. A stub shell grammar cannot show this; only the
	// real one over-consumes.
	const shell = realShellGrammar()
	if (!shell) {
		return
	}
	const reg = await registry(shell)
	const run = await reg.loadGrammar("source.run")
	assert.ok(run)
	const lines = [
		"match RUN.os",
		'\tcase "macos"',
		"\t\tcode_of($ xcode-select --install)",
		"\tdefault",
		'\t\tprint("no setup for {{ RUN.os }}")',
		"end"
	]
	let rules = vsctm.INITIAL
	const scopesOf: Record<string, string[]> = {}
	for (const line of lines) {
		const r = run.tokenizeLine(line, rules)
		rules = r.ruleStack
		for (const tok of r.tokens) {
			const text = line.slice(tok.startIndex, tok.endIndex).trim()
			if (text && !(text in scopesOf)) {
				scopesOf[text] = tok.scopes
			}
		}
	}
	for (const word of ["default", "match", "end"]) {
		assert.ok(
			scopesOf[word]?.includes("keyword.control.run"),
			`${word} is not a keyword: ${scopesOf[word]?.join(" ")}`
		)
	}
	assert.ok(
		scopesOf["print"]?.includes("support.function.run"),
		`print: ${scopesOf["print"]?.join(" ")}`
	)
	for (const [text, scopes] of Object.entries(scopesOf)) {
		if (text === "xcode-select" || text.startsWith("-") || text === ")") {
			continue
		}
		assert.ok(
			!scopes.some((s) => s.includes("embedded.line.shell")),
			`${text} leaked into the shell region: ${scopes.join(" ")}`
		)
	}
})

test("a `$` condition and a code_of body are coloured as shell", async () => {
	// Both used to be read as runfile *expressions* -- a quoted argument came
	// out a runfile string, a redirection came out an operator.
	const shell = realShellGrammar()
	if (!shell) {
		return
	}
	for (const line of [
		'if $ launchctl print "gui/$(id -u)/x" >/dev/null 2>&1',
		"match $ grep -q a b",
		"let c = code_of($ mkdir out)",
		// Any call may hold one, not just `code_of`. Matched by shape rather
		// than by name: while the rule spelled `code_of` out, every other call
		// had its command read as a runfile *expression*, so `--cached` came
		// out as two operators and `"*.rs"` as a string.
		"let files = lines($ git diff --cached --name-only)"
	]) {
		const [tokens] = await tokensOf(`${line}\n`, shell)
		const marker = (tokens ?? []).findIndex((t) => t.scopes.includes("keyword.control.shell.run"))
		assert.ok(marker >= 0, `${line}: the $ is not marked as ours`)
		// Everything past the marker belongs to the shell grammar.
		const after = (tokens ?? []).slice(marker + 1).filter((t) => t.text.trim() && t.text !== ")")
		assert.ok(
			after.some((t) => t.scopes.includes("entity.name.command.shell")),
			`${line}: no command scope after the $`
		)
		for (const t of after) {
			assert.ok(
				!t.scopes.some((s) => s.endsWith(".run") && s !== "source.run"),
				`${line}: ${JSON.stringify(t.text)} was read as runfile, not shell: ${t.scopes.join(" ")}`
			)
		}
	}
})

test("the keyword before a `$` condition is still the language's", async () => {
	const [scopes] = await scopesOf("if $ true\n")
	assert.ok(scopes?.includes("keyword.control.run"), `got ${scopes?.join(" ")}`)
})

test("retry and every are keywords, and only inside a retry header", async () => {
	const header = await tokensOf("retry 120 every 1\n")
	const words = (header[0] ?? []).filter((t) => t.text.trim())
	const kind = (text: string) =>
		words.find((t) => t.text === text)?.scopes.find((s) => s.endsWith(".run") && s !== "source.run")
	assert.equal(kind("retry"), "keyword.control.run")
	assert.equal(kind("every"), "keyword.control.run")
	assert.equal(kind("120"), "constant.numeric.run", "the count is still a number")
	assert.equal(kind("1"), "constant.numeric.run")

	// A binding may be called `every`; the keyword lives in the header only.
	const [binding] = await tokensOf("let every = 3\n")
	const name = (binding ?? []).find((t) => t.text === "every")
	assert.ok(!name?.scopes.includes("keyword.control.run"), `got ${name?.scopes.join(" ")}`)
})

test("a json block is coloured as JSON, and its interpolations stay ours", async () => {
	if (!realJsonGrammar()) {
		return
	}
	const lines = await tokensOf('let doc = json\n\t{ "b": {{ ARG.x }} }\nend\n$ echo after\n')

	const opener = (lines[0] ?? []).find((t) => t.text === "json")
	assert.ok(opener?.scopes.includes("keyword.control.structured.run"), `${opener?.scopes.join(" ")}`)

	const body = lines[1] ?? []
	assert.ok(
		body.some((t) => t.scopes.some((s) => s.endsWith(".json") && s.includes("property-name"))),
		"the key is not JSON: " + body.flatMap((t) => t.scopes).join(" ")
	)
	// A JSON object covers its own braces, so `{{` would be read as one of
	// them without the injection putting the interpolation ahead of it.
	const interp = body.find((t) => t.text === "{{")
	assert.ok(interp?.scopes.includes("meta.embedded.expression.run"), `${interp?.scopes.join(" ")}`)

	// And `end` closes it: the line after is the language's again.
	assert.ok(
		(lines[3] ?? []).some((t) => t.scopes.includes("keyword.control.shell.run")),
		"the block did not close"
	)
})

test("a dispatch inside code_of is coloured like a run statement", async () => {
	// `code_of(run build)` is the same dispatch the statement spells, so the
	// keyword and the target read the same way. Without a rule for it both
	// came out plain identifiers, since the statement's is anchored to `^`.
	const lines = await tokensOf("let c = code_of(run web:build --env=prod)\n")
	const tokens = lines[0] ?? []
	const scopeOf = (text: string) => tokens.find((t) => t.text === text)?.scopes ?? []
	assert.ok(scopeOf("code_of").includes("support.function.run"), `code_of: ${scopeOf("code_of").join(" ")}`)
	assert.ok(scopeOf("run").includes("keyword.control.run.run"), `run: ${scopeOf("run").join(" ")}`)
	assert.ok(
		scopeOf("web:build").includes("entity.name.function.target.run"),
		`web:build: ${scopeOf("web:build").join(" ")}`
	)
	// And nothing was mistaken for shell: there is no `$` here.
	assert.ok(
		!tokens.some((t) => t.scopes.some((s) => s.includes("embedded.line.shell"))),
		"a dispatch is not shell"
	)
})
