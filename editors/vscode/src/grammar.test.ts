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

async function registry(): Promise<vsctm.Registry> {
	const wasm = fs.readFileSync(require.resolve("vscode-oniguruma/release/onig.wasm"))
	await oniguruma.loadWASM(wasm.buffer as ArrayBuffer)
	return new vsctm.Registry({
		onigLib: Promise.resolve({
			createOnigScanner: (s: string[]) => new oniguruma.OnigScanner(s),
			createOnigString: (s: string) => new oniguruma.OnigString(s)
		}),
		loadGrammar: async (scope: string) => {
			if (scope === "source.shell") {
				return SHELL_STUB as unknown as vsctm.IRawGrammar
			}
			if (scope === "source.run") {
				return vsctm.parseRawGrammar(fs.readFileSync(RUN_GRAMMAR, "utf8"), RUN_GRAMMAR)
			}
			return null
		}
	})
}

/** Every scope applied to every line, as one array per line. */
async function scopesOf(source: string): Promise<string[][]> {
	const grammar = await (await registry()).loadGrammar("source.run")
	assert.ok(grammar, "the runfile grammar loaded")
	let state = vsctm.INITIAL
	const out: string[][] = []
	for (const line of source.split("\n")) {
		const r = grammar.tokenizeLine(line, state)
		out.push([...new Set(r.tokens.flatMap((t) => t.scopes))])
		state = r.ruleStack
	}
	return out
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
