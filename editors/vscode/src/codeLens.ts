// The inline Run button.
//
// One target is one file, so there is exactly one button per file and it goes
// at the top. Nothing is parsed to place it: a `.run` file under a `runfiles/`
// directory *is* a target, which is the whole point of the layout.

import * as path from "node:path";
import * as vscode from "vscode";
import { anchorFor as anchorOf, targetNameFor } from "./pure";

export { targetNameFor };

export const RUNFILE_SELECTOR: vscode.DocumentSelector = {
	scheme: "file",
	pattern: "**/runfiles/**/*.run",
};

/**
 * Every `.run` file, wherever it is.
 *
 * The language features go by language rather than by location: completion
 * and formatting are about the file, not about whether it sits in a project.
 * The machine-wide directory is `~/.runfiles`, which the pattern above does
 * not match -- a leading dot is a different directory name -- so keying those
 * off it would leave a person's own global targets with no editor support at
 * all. The code lens still uses the pattern, since running a target needs a
 * project to run it in.
 */
export const RUNFILE_LANGUAGE: vscode.DocumentSelector = { scheme: "file", language: "runfile" };

/** The directory `run` should discover from, using this platform's separator. */
export function anchorFor(filePath: string): string | undefined {
	return anchorOf(filePath, path.sep);
}

export class RunfileCodeLensProvider implements vscode.CodeLensProvider, vscode.Disposable {
	private readonly changed = new vscode.EventEmitter<void>();
	readonly onDidChangeCodeLenses = this.changed.event;

	/** Re-ask for lenses, e.g. after the `codeLens` setting is toggled. */
	refresh(): void {
		this.changed.fire();
	}

	dispose(): void {
		this.changed.dispose();
	}

	provideCodeLenses(doc: vscode.TextDocument): vscode.CodeLens[] {
		if (!vscode.workspace.getConfiguration("runfile").get<boolean>("codeLens", true)) {
			return [];
		}
		const name = targetNameFor(doc.uri.fsPath);
		const anchor = anchorFor(doc.uri.fsPath);
		if (name === undefined || anchor === undefined) {
			return [];
		}
		const range = new vscode.Range(0, 0, 0, 0);
		return [
			new vscode.CodeLens(range, {
				title: "$(play) Run",
				command: "runfile.runTargetInFile",
				arguments: [{ name, anchor }],
			}),
		];
	}
}
