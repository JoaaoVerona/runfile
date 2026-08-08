import * as vscode from "vscode"
import { findTargets, type RunfileTarget } from "./runfileParser"

/**
 * An inline **Run** button above every target in a `Runfile.json` / `Runfile.json5`,
 * so a target can be launched from the file that defines it without leaving the
 * editor — the JetBrains-style run gutter.
 *
 * Implemented as CodeLens: VS Code exposes no clickable gutter outside its own
 * debug and testing surfaces, and a `gutterIconPath` decoration cannot carry a
 * command. The provider is registered against a file-name pattern rather than a
 * language id, so the buttons appear whether the document is treated as `json`,
 * `jsonc` or plain text.
 */
export class RunfileCodeLensProvider implements vscode.CodeLensProvider {
	private readonly changed = new vscode.EventEmitter<void>()
	readonly onDidChangeCodeLenses = this.changed.event

	constructor(private readonly log: (message: string) => void) {}

	/** Repaint the buttons — VS Code re-queries on edits, this is for setting changes. */
	refresh(): void {
		this.changed.fire()
	}

	dispose(): void {
		this.changed.dispose()
	}

	provideCodeLenses(doc: vscode.TextDocument): vscode.CodeLens[] {
		if (!isCodeLensEnabled(doc.uri)) {
			return []
		}
		let targets: RunfileTarget[]
		try {
			targets = findTargets(doc.getText())
		} catch (err) {
			// A file mid-edit is unparseable more often than not, so this stays out of the
			// editor: the buttons simply disappear until the document parses again.
			this.log(`Could not read targets from ${doc.uri.fsPath}: ${(err as Error).message}`)
			return []
		}
		const lenses: vscode.CodeLens[] = []
		for (const target of targets) {
			// Internal targets are reachable only as `@_name` from another target, and
			// `excludeFromGenerateCommand` opts a target out of every editor integration.
			if (target.internal || target.excluded) {
				continue
			}
			const line = doc.positionAt(target.keyStart).line
			lenses.push(
				new vscode.CodeLens(new vscode.Range(line, 0, line, 0), {
					title: "$(play) Run",
					tooltip: target.description ? `run ${target.name} — ${target.description}` : `run ${target.name}`,
					command: "runfile.runTargetInFile",
					arguments: [doc.uri, target.name]
				})
			)
		}
		return lenses
	}
}

function isCodeLensEnabled(resource: vscode.Uri): boolean {
	return vscode.workspace.getConfiguration("runfile", resource).get<boolean>("codeLens", true)
}
