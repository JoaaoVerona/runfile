// The language server client.
//
// Hand-rolled rather than pulled in through `vscode-languageclient`: the
// server speaks a small, fixed subset, and a full client library would be a
// large dependency to carry for it.
//
// Everything the server offers is asked for here. It used to be only the
// notifications -- open, change, close -- with the reply-carrying half of the
// protocol unimplemented, so completion, hover and go-to-definition were
// advertised by the server, wired to nothing, and simply did not happen.

import { type ChildProcess, spawn } from "node:child_process";
import * as vscode from "vscode";
import { MessageReader, completionKind, completionPrefixStart, frame, markdownOf, replyId } from "./pure";

/** A whole-document replacement, which is the only edit the server sends. */
interface TextEdit {
	range: {
		start: { line: number; character: number };
		end: { line: number; character: number };
	};
	newText: string;
}

interface CompletionItem {
	label: string;
	kind?: number;
	detail?: string;
	documentation?: unknown;
}

interface Diagnostic {
	range: {
		start: { line: number; character: number };
		end: { line: number; character: number };
	};
	severity?: number;
	message: string;
}

/** LSP severities are 1-4; VS Code's enum runs the other way. */
export function severityOf(n: number | undefined): vscode.DiagnosticSeverity {
	switch (n) {
		case 2:
			return vscode.DiagnosticSeverity.Warning;
		case 3:
			return vscode.DiagnosticSeverity.Information;
		case 4:
			return vscode.DiagnosticSeverity.Hint;
		default:
			return vscode.DiagnosticSeverity.Error;
	}
}

/**
 * The standalone server, as it was installed beside `run` before the runner
 * grew a `:lsp` of its own.
 *
 * Only a fallback, and only for the window where the two can disagree: the
 * marketplace updates this extension on its own, while `run` is updated by
 * hand, so a client asking for `run :lsp` can meet a runner that has never
 * heard of it. That runner came with a `runfile-lsp` next to it, which is
 * still on PATH and still answers. Removable once no supported `run` predates
 * the subcommand.
 */
const LEGACY_SERVER = "runfile-lsp";

export class LanguageClient implements vscode.Disposable {
	private child?: ChildProcess;
	private reader = new MessageReader();
	private readonly diagnostics = vscode.languages.createDiagnosticCollection("runfile");
	private readonly subs: vscode.Disposable[] = [];
	private nextId = 1;
	/** Requests waiting on a reply, by id. */
	private readonly pending = new Map<number, (result: unknown) => void>();
	/** Whether the server has said anything at all -- see [`LEGACY_SERVER`]. */
	private answered = false;
	private fellBack = false;
	private disposed = false;

	constructor(
		private readonly command: string,
		private readonly args: string[],
		private readonly log: vscode.OutputChannel,
	) {}

	start(): void {
		this.spawnServer(this.command, this.args);
		// Registered once, outside `spawnServer`: a fallback replaces the
		// process, not this client, and subscribing a second time would send
		// every notification twice.
		this.subs.push(
			vscode.workspace.onDidOpenTextDocument((d) => this.didOpen(d)),
			vscode.workspace.onDidChangeTextDocument((e) => this.didChange(e.document)),
			vscode.workspace.onDidCloseTextDocument((d) => this.didClose(d)),
		);
	}

	private spawnServer(command: string, args: string[]): void {
		// A fresh reader: a half-written frame from a process that died is not
		// the start of the next one's first message.
		this.reader = new MessageReader();
		try {
			this.child = spawn(command, args, { stdio: ["pipe", "pipe", "pipe"] });
		} catch (e) {
			this.log.appendLine(`language server did not start: ${String(e)}`);
			this.child = undefined;
			return;
		}
		this.child.on("error", (e) => {
			// Not installed is the common case, and it must not be a popup.
			this.log.appendLine(`language server unavailable (${command}): ${e.message}`);
			this.child = undefined;
			this.fallBack("could not be started");
		});
		// A runner with no `:lsp` prints its refusal here and exits 1.
		this.child.on("exit", (code) => this.fallBack(`exited with code ${code}`));
		this.child.stderr?.on("data", (b: Buffer) => this.log.append(b.toString()));
		this.child.stdout?.on("data", (b: Buffer) => {
			for (const message of this.reader.push(b)) {
				this.answered = true;
				this.receive(message);
			}
		});

		this.send({ jsonrpc: "2.0", id: this.nextId++, method: "initialize", params: {} });
		this.send({ jsonrpc: "2.0", method: "initialized", params: {} });

		// Whatever is already open when the extension activates never fires
		// `onDidOpen`, so it is opened explicitly. Also what re-syncs the
		// documents onto a fallback server, which missed them the first time.
		for (const doc of vscode.workspace.textDocuments) {
			this.didOpen(doc);
		}
	}

	/**
	 * Try [`LEGACY_SERVER`] once, if the configured one never said anything.
	 *
	 * A server that answered and then died is a crash, not a version
	 * mismatch, and reaching for a different binary would hide it.
	 */
	private fallBack(why: string): void {
		if (this.answered || this.fellBack || this.disposed) {
			return;
		}
		this.fellBack = true;
		this.log.appendLine(`${[this.command, ...this.args].join(" ")} ${why}; trying ${LEGACY_SERVER}`);
		this.spawnServer(LEGACY_SERVER, []);
	}

	dispose(): void {
		this.disposed = true;
		for (const s of this.subs) {
			s.dispose();
		}
		this.diagnostics.dispose();
		this.child?.kill();
	}

	private send(message: unknown): void {
		this.child?.stdin?.write(frame(message));
	}

	/**
	 * Send a request and wait for its reply.
	 *
	 * Times out rather than waiting forever: this runs on save, and a server
	 * that has wedged must not take the editor's save with it.
	 */
	private request(method: string, params: unknown, timeoutMs = 2000): Promise<unknown> {
		if (!this.child) {
			return Promise.resolve(null);
		}
		const id = this.nextId++;
		return new Promise((resolve) => {
			const done = (result: unknown) => {
				clearTimeout(timer);
				this.pending.delete(id);
				resolve(result);
			};
			const timer = setTimeout(() => done(null), timeoutMs);
			this.pending.set(id, done);
			this.send({ jsonrpc: "2.0", id, method, params });
		});
	}

	/**
	 * What may be written at this position: properties after a `.`, functions
	 * and sources, `RUN.` keys, target names after `run `.
	 *
	 * The range is given explicitly. VS Code's own idea of a word ends at a
	 * `-` and a `:`, which would make `.env-file` complete to `.env-env-file`.
	 */
	async completion(doc: vscode.TextDocument, pos: vscode.Position): Promise<vscode.CompletionItem[]> {
		const result = (await this.position("textDocument/completion", doc, pos)) as {
			items?: CompletionItem[];
		} | null;
		const items = result?.items;
		if (!Array.isArray(items)) {
			return [];
		}
		const line = doc.lineAt(pos.line).text;
		const range = new vscode.Range(
			pos.line,
			completionPrefixStart(line, pos.character),
			pos.line,
			pos.character,
		);
		return items.map((i) => {
			const item = new vscode.CompletionItem(i.label, completionKind(i.kind) as vscode.CompletionItemKind);
			item.detail = i.detail;
			const doc = markdownOf(i.documentation);
			if (doc) {
				item.documentation = new vscode.MarkdownString(doc);
			}
			item.range = range;
			return item;
		});
	}

	async hover(doc: vscode.TextDocument, pos: vscode.Position): Promise<vscode.Hover | undefined> {
		const result = (await this.position("textDocument/hover", doc, pos)) as { contents?: unknown } | null;
		const text = markdownOf(result?.contents);
		return text ? new vscode.Hover(new vscode.MarkdownString(text)) : undefined;
	}

	async definition(doc: vscode.TextDocument, pos: vscode.Position): Promise<vscode.Location | undefined> {
		const result = (await this.position("textDocument/definition", doc, pos)) as {
			uri?: string;
			range?: TextEdit["range"];
		} | null;
		if (!result?.uri || !result.range) {
			return undefined;
		}
		const r = result.range;
		return new vscode.Location(
			vscode.Uri.parse(result.uri),
			new vscode.Range(r.start.line, r.start.character, r.end.line, r.end.character),
		);
	}

	/** A request about a place in a document, which is most of them. */
	private position(method: string, doc: vscode.TextDocument, pos: vscode.Position): Promise<unknown> {
		if (!this.isRunfile(doc)) {
			return Promise.resolve(null);
		}
		return this.request(method, {
			textDocument: { uri: doc.uri.toString() },
			position: { line: pos.line, character: pos.character },
		});
	}

	/**
	 * The edits that put a document into the one shape there is -- the same
	 * `run :format` produces, since both call the same formatter.
	 *
	 * An empty list when nothing needs changing, so saving a clean file marks
	 * nothing dirty; nothing at all when the document does not parse, because
	 * a file is unfinished for most of the time it is being written.
	 */
	async format(doc: vscode.TextDocument): Promise<vscode.TextEdit[]> {
		if (!this.isRunfile(doc)) {
			return [];
		}
		const result = (await this.request("textDocument/formatting", {
			textDocument: { uri: doc.uri.toString() },
			options: { tabSize: 4, insertSpaces: false },
		})) as TextEdit[] | null;
		if (!Array.isArray(result)) {
			return [];
		}
		// The server sends one edit past the last line; clamp it to what the
		// document actually has, which is what VS Code will accept.
		const full = new vscode.Range(0, 0, doc.lineCount, 0);
		return result.map((e) => vscode.TextEdit.replace(full, e.newText));
	}

	private receive(message: unknown): void {
		const m = message as {
			id?: number;
			result?: unknown;
			method?: string;
			params?: { uri?: string; diagnostics?: Diagnostic[] };
		};
		const id = replyId(message);
		if (id !== undefined) {
			this.pending.get(id)?.(m.result ?? null);
			return;
		}
		if (m.method !== "textDocument/publishDiagnostics" || !m.params?.uri) {
			return;
		}
		const uri = vscode.Uri.parse(m.params.uri);
		this.diagnostics.set(
			uri,
			(m.params.diagnostics ?? []).map((d) => {
				const range = new vscode.Range(
					d.range.start.line,
					d.range.start.character,
					d.range.end.line,
					d.range.end.character,
				);
				const out = new vscode.Diagnostic(range, d.message, severityOf(d.severity));
				out.source = "runfile";
				return out;
			}),
		);
	}

	private isRunfile(doc: vscode.TextDocument): boolean {
		return doc.uri.scheme === "file" && doc.uri.fsPath.endsWith(".run");
	}

	private didOpen(doc: vscode.TextDocument): void {
		if (!this.isRunfile(doc)) {
			return;
		}
		this.send({
			jsonrpc: "2.0",
			method: "textDocument/didOpen",
			params: {
				textDocument: {
					uri: doc.uri.toString(),
					languageId: "runfile",
					version: doc.version,
					text: doc.getText(),
				},
			},
		});
	}

	private didChange(doc: vscode.TextDocument): void {
		if (!this.isRunfile(doc)) {
			return;
		}
		// Full sync, matching what the server advertises.
		this.send({
			jsonrpc: "2.0",
			method: "textDocument/didChange",
			params: {
				textDocument: { uri: doc.uri.toString(), version: doc.version },
				contentChanges: [{ text: doc.getText() }],
			},
		});
	}

	private didClose(doc: vscode.TextDocument): void {
		if (!this.isRunfile(doc)) {
			return;
		}
		this.send({
			jsonrpc: "2.0",
			method: "textDocument/didClose",
			params: { textDocument: { uri: doc.uri.toString() } },
		});
	}
}
