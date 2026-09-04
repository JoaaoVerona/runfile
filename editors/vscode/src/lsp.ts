// The language server client.
//
// Hand-rolled over `vscode.languages.createDiagnosticCollection` rather than
// pulled in through `vscode-languageclient`: the server speaks a small, fixed
// subset -- open, change, close, publish, format -- and a full client library
// would be a large dependency to carry for five message types. Completion and
// go-to-definition are contributed by the extension directly, from the same
// catalog the tree uses, so nothing is lost by not routing them through LSP.

import { type ChildProcess, spawn } from "node:child_process";
import * as vscode from "vscode";
import { MessageReader, frame, replyId } from "./pure";

/** A whole-document replacement, which is the only edit the server sends. */
interface TextEdit {
	range: {
		start: { line: number; character: number };
		end: { line: number; character: number };
	};
	newText: string;
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

export class LanguageClient implements vscode.Disposable {
	private child?: ChildProcess;
	private readonly reader = new MessageReader();
	private readonly diagnostics = vscode.languages.createDiagnosticCollection("runfile");
	private readonly subs: vscode.Disposable[] = [];
	private nextId = 1;
	/** Requests waiting on a reply, by id. */
	private readonly pending = new Map<number, (result: unknown) => void>();

	constructor(
		private readonly command: string,
		private readonly log: vscode.OutputChannel,
	) {}

	start(): void {
		try {
			this.child = spawn(this.command, [], { stdio: ["pipe", "pipe", "pipe"] });
		} catch (e) {
			this.log.appendLine(`language server did not start: ${String(e)}`);
			return;
		}
		this.child.on("error", (e) => {
			// Not installed is the common case, and it must not be a popup.
			this.log.appendLine(`language server unavailable (${this.command}): ${e.message}`);
			this.child = undefined;
		});
		this.child.stderr?.on("data", (b: Buffer) => this.log.append(b.toString()));
		this.child.stdout?.on("data", (b: Buffer) => {
			for (const message of this.reader.push(b)) {
				this.receive(message);
			}
		});

		this.send({ jsonrpc: "2.0", id: this.nextId++, method: "initialize", params: {} });
		this.send({ jsonrpc: "2.0", method: "initialized", params: {} });

		// Whatever is already open when the extension activates never fires
		// `onDidOpen`, so it is opened explicitly.
		for (const doc of vscode.workspace.textDocuments) {
			this.didOpen(doc);
		}
		this.subs.push(
			vscode.workspace.onDidOpenTextDocument((d) => this.didOpen(d)),
			vscode.workspace.onDidChangeTextDocument((e) => this.didChange(e.document)),
			vscode.workspace.onDidCloseTextDocument((d) => this.didClose(d)),
		);
	}

	dispose(): void {
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
