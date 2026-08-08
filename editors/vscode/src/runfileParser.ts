/**
 * A pragmatic, dependency-free JSON5 reader for Runfile documents.
 *
 * The `run` CLI parses every Runfile — `.json` and `.json5` alike — as JSON5, so
 * the file on screen may carry comments, trailing commas, unquoted keys and
 * single-quoted strings, all of which `JSON.parse` chokes on. This reads just
 * enough of that grammar to locate each target's key in the source text, which is
 * what the inline Run buttons anchor to.
 *
 * Pure module (no `vscode` import) so it can be exercised with plain Node.
 */

/** One entry of the Runfile's top-level `targets` object. */
export interface RunfileTarget {
	/** The target's canonical name, exactly as written as the object key. */
	name: string
	/** Offset of the key token's first character (its opening quote, when quoted). */
	keyStart: number
	/** Offset just past the key token. */
	keyEnd: number
	/** The target's `description`, when it declares one. */
	description?: string
	/** `metadata.excludeFromGenerateCommand` — opts the target out of editor integrations. */
	excluded: boolean
	/** Internal targets are only reachable as `@_name` from another target's commands. */
	internal: boolean
}

/**
 * Read the `targets` of a Runfile document, in source order.
 *
 * Throws a `SyntaxError` when the text is not parseable — which is the normal state
 * of a file being edited, so callers are expected to treat that as "no targets yet"
 * rather than as something to report.
 */
export function findTargets(text: string): RunfileTarget[] {
	const root = new Reader(tokenize(text)).parseDocument()
	if (root.kind !== "object") {
		return []
	}
	const targets = memberOf(root, "targets")
	if (targets?.kind !== "object") {
		return []
	}
	return targets.members.map(toTarget)
}

/**
 * Mirrors the CLI's internal-target rule: a target is internal when the **last**
 * `:`-separated segment of its name starts with `_`, so a namespaced `api:_helper`
 * keeps its internal status.
 */
export function isInternalTargetName(name: string): boolean {
	return name.slice(name.lastIndexOf(":") + 1).startsWith("_")
}

function toTarget(member: JsonMember): RunfileTarget {
	const metadata = memberOf(member.value, "metadata")
	return {
		name: member.key,
		keyStart: member.keyStart,
		keyEnd: member.keyEnd,
		description: stringOf(memberOf(member.value, "description")),
		excluded: wordOf(memberOf(metadata, "excludeFromGenerateCommand")) === "true",
		internal: isInternalTargetName(member.key)
	}
}

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

/**
 * Numbers, booleans and `null` all land in `word` — nothing here needs them typed,
 * and keeping them as their source text avoids a second layer of interpretation.
 */
type JsonValue =
	| { kind: "object"; members: JsonMember[] }
	| { kind: "array"; items: JsonValue[] }
	| { kind: "string"; value: string }
	| { kind: "word"; value: string }

interface JsonMember {
	key: string
	keyStart: number
	keyEnd: number
	value: JsonValue
}

/** The first member named `key`, or `undefined` — for anything that isn't an object. */
function memberOf(value: JsonValue | undefined, key: string): JsonValue | undefined {
	if (value?.kind !== "object") {
		return undefined
	}
	return value.members.find((m) => m.key === key)?.value
}

function stringOf(value: JsonValue | undefined): string | undefined {
	return value?.kind === "string" ? value.value : undefined
}

function wordOf(value: JsonValue | undefined): string | undefined {
	return value?.kind === "word" ? value.value : undefined
}

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------

interface Token {
	/** A punctuation character, a string's decoded contents, or a bare word's text. */
	kind: "punct" | "string" | "word"
	value: string
	start: number
	end: number
}

const PUNCTUATION = "{}[]:,"

/** Characters that end a bare word: structure, quotes, whitespace, and comment starts. */
const WORD_END = /[\s{}[\]:,"'/]/

const ESCAPES: Record<string, string> = {
	b: "\b",
	f: "\f",
	n: "\n",
	r: "\r",
	t: "\t",
	v: "\v",
	"0": "\0"
}

function tokenize(text: string): Token[] {
	const tokens: Token[] = []
	let i = 0
	while (i < text.length) {
		const ch = text[i]
		if (/\s/.test(ch)) {
			i++
		} else if (ch === "/" && text[i + 1] === "/") {
			const end = text.indexOf("\n", i)
			i = end === -1 ? text.length : end + 1
		} else if (ch === "/" && text[i + 1] === "*") {
			const end = text.indexOf("*/", i + 2)
			i = end === -1 ? text.length : end + 2
		} else if (PUNCTUATION.includes(ch)) {
			tokens.push({ kind: "punct", value: ch, start: i, end: i + 1 })
			i++
		} else if (ch === '"' || ch === "'") {
			const token = readString(text, i)
			tokens.push(token)
			i = token.end
		} else {
			const start = i
			while (i < text.length && !WORD_END.test(text[i])) {
				i++
			}
			if (i === start) {
				throw new SyntaxError(`unexpected "${ch}" at offset ${start}`)
			}
			tokens.push({ kind: "word", value: text.slice(start, i), start, end: i })
		}
	}
	return tokens
}

function readString(text: string, start: number): Token {
	const quote = text[start]
	let value = ""
	let i = start + 1
	while (i < text.length) {
		const ch = text[i]
		if (ch === quote) {
			return { kind: "string", value, start, end: i + 1 }
		}
		if (ch !== "\\") {
			value += ch
			i++
			continue
		}
		const escape = text[i + 1]
		if (escape === undefined) {
			break
		}
		i += 2
		if (escape === "\n") {
			continue // line continuation
		}
		if (escape === "\r") {
			if (text[i] === "\n") {
				i++
			}
			continue
		}
		if (escape === "u" || escape === "x") {
			const width = escape === "u" ? 4 : 2
			const decoded = decodeHex(text.slice(i, i + width), width)
			if (decoded !== undefined) {
				value += decoded
				i += width
				continue
			}
		}
		value += ESCAPES[escape] ?? escape
	}
	throw new SyntaxError(`unterminated string starting at offset ${start}`)
}

function decodeHex(raw: string, width: number): string | undefined {
	if (raw.length !== width || !/^[0-9a-fA-F]+$/.test(raw)) {
		return undefined
	}
	return String.fromCharCode(Number.parseInt(raw, 16))
}

// ---------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------

class Reader {
	private i = 0

	constructor(private readonly tokens: Token[]) {}

	parseDocument(): JsonValue {
		const value = this.parseValue()
		const trailing = this.peek()
		if (trailing) {
			throw new SyntaxError(`unexpected "${trailing.value}" after the document at offset ${trailing.start}`)
		}
		return value
	}

	private peek(): Token | undefined {
		return this.tokens[this.i]
	}

	private next(): Token {
		const token = this.tokens[this.i]
		if (!token) {
			throw new SyntaxError("unexpected end of file")
		}
		this.i++
		return token
	}

	/** Consume `punct` if it is next. */
	private eat(punct: string): boolean {
		const token = this.peek()
		if (token?.kind === "punct" && token.value === punct) {
			this.i++
			return true
		}
		return false
	}

	private expect(punct: string): void {
		if (!this.eat(punct)) {
			const token = this.peek()
			const where = token ? `"${token.value}" at offset ${token.start}` : "end of file"
			throw new SyntaxError(`expected "${punct}" but found ${where}`)
		}
	}

	private parseValue(): JsonValue {
		const token = this.peek()
		if (!token) {
			throw new SyntaxError("unexpected end of file")
		}
		if (token.kind === "punct") {
			if (token.value === "{") {
				return this.parseObject()
			}
			if (token.value === "[") {
				return this.parseArray()
			}
			throw new SyntaxError(`unexpected "${token.value}" at offset ${token.start}`)
		}
		this.i++
		return token.kind === "string" ? { kind: "string", value: token.value } : { kind: "word", value: token.value }
	}

	private parseObject(): JsonValue {
		this.expect("{")
		const members: JsonMember[] = []
		while (!this.eat("}")) {
			const key = this.next()
			if (key.kind === "punct") {
				throw new SyntaxError(`expected a key but found "${key.value}" at offset ${key.start}`)
			}
			this.expect(":")
			members.push({ key: key.value, keyStart: key.start, keyEnd: key.end, value: this.parseValue() })
			// No comma ends the object — which also covers the trailing-comma form, since
			// the `}` is then consumed by the loop condition on the next pass.
			if (!this.eat(",")) {
				this.expect("}")
				break
			}
		}
		return { kind: "object", members }
	}

	private parseArray(): JsonValue {
		this.expect("[")
		const items: JsonValue[] = []
		while (!this.eat("]")) {
			items.push(this.parseValue())
			if (!this.eat(",")) {
				this.expect("]")
				break
			}
		}
		return { kind: "array", items }
	}
}
