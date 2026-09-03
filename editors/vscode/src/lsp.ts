// The language server client.
//
// Hand-rolled over `vscode.languages.createDiagnosticCollection` rather than
// pulled in through `vscode-languageclient`: the server speaks a small, fixed
// subset -- open, change, close, publish -- and a full client library would be
// a large dependency to carry for four message types. Completion and
// go-to-definition are contributed by the extension directly, from the same
// catalog the tree uses, so nothing is lost by not routing them through LSP.

import { type ChildProcess, spawn } from "node:child_process";
import * as vscode from "vscode";
import { MessageReader, frame } from "./pure";

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

	private receive(message: unknown): void {
		const m = message as { method?: string; params?: { uri?: string; diagnostics?: Diagnostic[] } };
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
