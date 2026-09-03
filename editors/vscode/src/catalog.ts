// The target catalog, as the CLI reports it.
//
// `run :list --json` is the only source: the extension never parses `.run`
// files itself, so it cannot disagree with the runner about what exists.

import { execFile } from "node:child_process";
import * as vscode from "vscode";
import { type Target, parseCatalog } from "./pure";

export { type Origin, type Target, SUPPORTED_FORMAT_VERSION, namespaceOf } from "./pure";

export interface FolderCatalog {
	readonly folder: vscode.WorkspaceFolder;
	readonly targets: readonly Target[];
}

/** Run the catalog command in one folder. Failure yields no targets. */
export async function load(
	folder: vscode.WorkspaceFolder,
	command: string,
	log: vscode.OutputChannel,
): Promise<FolderCatalog> {
	const [program, ...args] = command.split(/\s+/).filter((s) => s.length > 0);
	if (!program) {
		return { folder, targets: [] };
	}
	const stdout = await new Promise<string>((resolve) => {
		execFile(
			program,
			args,
			{ cwd: folder.uri.fsPath, maxBuffer: 8 * 1024 * 1024 },
			(err, out, errOut) => {
				// A folder with no runfiles/ is the common case, not a fault, so
				// this is logged rather than shown.
				if (err) {
					log.appendLine(`[${folder.name}] ${command}: ${errOut.trim() || err.message}`);
					resolve("");
					return;
				}
				resolve(out);
			},
		);
	});
	return { folder, targets: parseCatalog(stdout, folder.name, log) };
}
