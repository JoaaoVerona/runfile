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

test("the first command on a line is coloured like every later one", async () => {
	// `source.shell` starts a statement only after `^`, `;`, `|`, `&`, `!`,
	// `(`, `{` or a backtick. The text after `$ ` is none of those, so the
	// first command came out bare while every later one was fine.
	const shell = realShellGrammar()
	if (!shell) {
		return
	}
	const [line] = await tokensOf("$ echo 'abc'; echo 'abc'\n", shell)
	const echoes = (line ?? []).filter((t) => t.text === "echo")
	assert.equal(echoes.length, 2, "the line has two of them")
	for (const [i, e] of echoes.entries()) {
		assert.ok(
			e.scopes.includes("entity.name.command.shell"),
			`echo #${i + 1} is not a command: ${e.scopes.join(" ")}`
		)
	}
	assert.deepEqual(echoes[0]?.scopes, echoes[1]?.scopes, "and they are coloured identically")
})

test("an interpolation survives the shell grammar's own rules", async () => {
	// A command statement covers its arguments and a quoted string covers its
	// contents, so a pattern listed beside them can never win -- TextMate takes
	// the earliest match, not the first listed. The injection is what puts
	// `{{ … }}` ahead of both.
	const shell = realShellGrammar()
	if (!shell) {
		return
	}
	for (const line of ["$ cp {{ ARG.src }} dest\n", '$ echo "{{ ARG.x }}"\n']) {
		const [scopes] = await scopesOf(line, shell)
		assert.ok(scopes?.includes("meta.embedded.expression.run"), `${line}: ${scopes?.join(" ")}`)
		assert.ok(scopes?.includes("variable.other.member.run"), `${line}: the expression is parsed too`)
	}
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
