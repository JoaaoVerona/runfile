// Logic with no `vscode` import, so it can be exercised by a plain Node test
// run rather than only inside an extension host.

/** Bump alongside `FORMAT_VERSION` in crates/runfile-cli/src/list.rs. */
export const SUPPORTED_FORMAT_VERSION = 1;

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
