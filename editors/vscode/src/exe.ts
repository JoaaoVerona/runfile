// Resolving the `run` executable to an absolute path before spawning it.
//
// On Windows a child is searched for in its working directory before PATH, so a
// `run.exe` dropped into a workspace folder would be run instead of the installed
// one when the catalog is fetched (or a task spawned) with that folder as cwd --
// audit SA-010. Resolving the name here, against PATH only, closes that.

import { statSync } from "node:fs";
import * as path from "node:path";
import { pathCandidates } from "./pure";

function extensions(): string[] {
	if (process.platform !== "win32") {
		return [];
	}
	return (process.env.PATHEXT ?? ".COM;.EXE;.BAT;.CMD").split(";").filter((e) => e.length > 0);
}

/**
 * The program as an absolute path when it is a bare name found on PATH, else
 * unchanged: a name the user wrote as a path, or one not on PATH (which then
 * fails to spawn exactly as before), is returned as given.
 */
export function resolveProgram(program: string): string {
	for (const candidate of pathCandidates(program, process.env.PATH, {
		delimiter: path.delimiter,
		sep: path.sep,
		extensions: extensions(),
	})) {
		try {
			if (statSync(candidate).isFile()) {
				return candidate;
			}
		} catch {
			// Not there; try the next.
		}
	}
	return program;
}
