// Logic with no `vscode` import, so it can be exercised by a plain Node test
// run rather than only inside an extension host.

/** Bump alongside `FORMAT_VERSION` in crates/runfile-cli/src/list.rs. */
export const SUPPORTED_FORMAT_VERSION = 2;

export type Origin = "local" | "subprojects" | "global";

export interface Target {
	readonly name: string;
	readonly description: string;
	readonly origin: Origin;
	/** The file that defines it — one target, one file. */
	readonly path: string;
}

interface Document {
	readonly formatVersion: number;
	readonly targets: readonly Target[];
}

export interface Log {
	appendLine(line: string): void;
}

/** Read `run :list --json`. Any failure yields no targets and one log line. */
export function parseCatalog(stdout: string, label: string, log: Log): Target[] {
	// A BOM survives some shells on Windows and would fail `JSON.parse`.
	const text = stdout.replace(/^﻿/, "").trim();
	if (text === "") {
		return [];
	}
	let doc: Document;
	try {
		doc = JSON.parse(text) as Document;
	} catch (e) {
		log.appendLine(`[${label}] catalog is not valid JSON: ${String(e)}`);
		return [];
	}
	if (doc?.formatVersion !== SUPPORTED_FORMAT_VERSION) {
		// A warning, not a refusal: an added field is still readable, and
		// refusing would empty the view over a version bump alone.
		log.appendLine(
			`[${label}] catalog format ${doc?.formatVersion} != expected ` +
				`${SUPPORTED_FORMAT_VERSION}; reading it anyway`,
		);
	}
	const targets = doc?.targets;
	return Array.isArray(targets) ? targets.filter((t) => typeof t?.name === "string") : [];
}

/**
 * The namespace a target is filed under: the part before the first colon, and
 * only when there is one. A plain `build` belongs at the top level.
 */
export function namespaceOf(name: string): string | undefined {
	const i = name.indexOf(":");
	return i > 0 ? name.slice(0, i) : undefined;
}

/** `runfiles/api/build.run` -> `api:build`; `_shared.run` is not a target. */
export function targetNameFor(filePath: string): string | undefined {
	const parts = filePath.split(/[\\/]/);
	const at = parts.lastIndexOf("runfiles");
	if (at < 0 || at === parts.length - 1) {
		return undefined;
	}
	const rest = parts.slice(at + 1);
	const file = rest.pop();
	if (file === undefined || !file.endsWith(".run")) {
		return undefined;
	}
	const stem = file.slice(0, -".run".length);
	// Shared settings apply to a directory; they are not runnable themselves.
	if (stem === "_shared") {
		return undefined;
	}
	return [...rest, stem].join(":");
}

/** The directory `run` should discover from: the parent of `runfiles/`. */
export function anchorFor(filePath: string, sep = "/"): string | undefined {
	const parts = filePath.split(/[\\/]/);
	const at = parts.lastIndexOf("runfiles");
	if (at <= 0) {
		return undefined;
	}
	return parts.slice(0, at).join(sep) || sep;
}

/** Split a byte stream into LSP messages, holding partial reads over. */
export class MessageReader {
	private buffer = Buffer.alloc(0);

	push(chunk: Buffer): unknown[] {
		this.buffer = Buffer.concat([this.buffer, chunk]);
		const out: unknown[] = [];
		for (;;) {
			const headerEnd = this.buffer.indexOf("\r\n\r\n");
			if (headerEnd < 0) {
				return out;
			}
			const header = this.buffer.subarray(0, headerEnd).toString("ascii");
			const match = /Content-Length:\s*(\d+)/i.exec(header);
			if (!match) {
				// Unparseable header: drop it rather than stall forever.
				this.buffer = this.buffer.subarray(headerEnd + 4);
				continue;
			}
			const length = Number(match[1]);
			const start = headerEnd + 4;
			// The count is bytes, so the check must be on the buffer, not a string.
			if (this.buffer.length < start + length) {
				return out;
			}
			const body = this.buffer.subarray(start, start + length).toString("utf8");
			this.buffer = this.buffer.subarray(start + length);
			try {
				out.push(JSON.parse(body));
			} catch {
				// A malformed body loses one message, not the connection.
			}
		}
	}
}

/**
 * VS Code's `CompletionItemKind` for an LSP one.
 *
 * The two enumerations list the same kinds in the same order, LSP starting at
 * 1 and VS Code at 0, so the mapping is an offset -- but only where the value
 * is one the server actually sends. Anything else falls back to Text rather
 * than landing on a wrong icon.
 */
export function completionKind(lsp: number | undefined): number {
	// Function, Variable, Module, Property, Value, Keyword: what `server.rs`
	// sends, via `analysis::Kind::lsp`.
	if (lsp === undefined || ![3, 6, 9, 10, 12, 14].includes(lsp)) {
		return 0;
	}
	return lsp - 1;
}

/**
 * Where the word being completed starts on its line.
 *
 * Given explicitly rather than left to VS Code, whose idea of a word stops at
 * a `-` and at a `:` -- so `.env-file` would come out as `.env-env-file` and
 * `run vscode:test` as `run vscode:vscode:test`. A `.` is *not* part of it:
 * the dot stays, and the property name is written after it.
 */
export function completionPrefixStart(line: string, character: number): number {
	let i = Math.min(character, line.length);
	while (i > 0 && /[A-Za-z0-9_:-]/.test(line[i - 1] as string)) {
		i--;
	}
	return i;
}

/** The text of an LSP `MarkupContent`, a plain string, or a list of either. */
export function markdownOf(contents: unknown): string {
	if (typeof contents === "string") {
		return contents;
	}
	if (Array.isArray(contents)) {
		return contents.map(markdownOf).filter(Boolean).join("\n\n");
	}
	const m = contents as { value?: unknown } | null;
	return typeof m?.value === "string" ? m.value : "";
}

/**
 * Whether a message from the server is a reply to a request rather than a
 * notification.
 *
 * A reply carries an id and no method; a notification carries a method and no
 * id. Told apart here so the rule is one line with a test on it rather than a
 * condition buried in a class that cannot be loaded without VS Code.
 */
export function replyId(message: unknown): number | undefined {
	const m = message as { id?: unknown; method?: unknown };
	if (m.method !== undefined || typeof m.id !== "number") {
		return undefined;
	}
	return m.id;
}

export function frame(message: unknown): string {
	const body = Buffer.from(JSON.stringify(message), "utf8");
	return `Content-Length: ${body.length}\r\n\r\n${body.toString("utf8")}`;
}

/**
 * Absolute candidate paths for a program name, from a `PATH` string.
 *
 * `execFile` and `spawn` search the child's working directory before `PATH` on
 * Windows, so a `run.exe` committed to a workspace folder shadows the real
 * runner when a command is spawned with that folder as cwd (audit SA-010).
 * Resolving the name to an absolute path first means neither the cwd nor a
 * relative or empty `PATH` entry (which also means the cwd) is ever searched.
 *
 * Empty for a name that already holds a path separator -- the user gave a path,
 * so the caller uses it unchanged. Only absolute `PATH` directories are
 * considered, each tried bare and with every extension (Windows `PATHEXT`).
 */
export function pathCandidates(
	program: string,
	pathVar: string | undefined,
	opts: { delimiter: string; sep: string; extensions: readonly string[] },
): string[] {
	if (program.includes("/") || program.includes("\\")) {
		return [];
	}
	const absolute = (d: string) => /^(\/|[A-Za-z]:[\\/]|\\\\)/.test(d);
	const dirs = (pathVar ?? "").split(opts.delimiter).filter((d) => d.length > 0 && absolute(d));
	const names = ["", ...opts.extensions].map((ext) => program + ext);
	const out: string[] = [];
	for (const dir of dirs) {
		for (const name of names) {
			out.push(dir.endsWith(opts.sep) ? dir + name : dir + opts.sep + name);
		}
	}
	return out;
}

/**
 * Whether any of these strings carries VS Code's `${…}` task-variable syntax.
 *
 * A target name is a file name and `dir` is a directory path, both chosen by an
 * untrusted repository. VS Code resolves every `${…}` in a task's definition and
 * arguments before the task runs -- and `${command:<id>}` / `${input:<id>}` run
 * an editor command or read a workspace-defined input on Run -- with no way to
 * pass a literal `${`. So a name or dir that holds one cannot be turned into a
 * safe task and is skipped (audit SA-029).
 */
export function hasTaskVar(...parts: (string | undefined)[]): boolean {
	return parts.some((p) => p !== undefined && p.includes("${"));
}
