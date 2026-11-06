// Resolving the `run` executable to an absolute path before spawning it.
//
// On Windows a child is searched for in its working directory before PATH, so a
// `run.exe` dropped into a workspace folder would be run instead of the installed
// one when the catalog is fetched (or a task spawned) with that folder as cwd --
// audit SA-010. Resolving the name here, against PATH only, closes that -- as
// long as a name that is not on PATH is never spawned bare either (SA-011).

import { statSync } from "node:fs";
import * as path from "node:path";
import { pathCandidates } from "./pure";

function extensions(env: NodeJS.ProcessEnv): string[] {
	if (process.platform !== "win32") {
		return [];
	}
	return (env.PATHEXT ?? ".COM;.EXE;.BAT;.CMD").split(";").filter((e) => e.length > 0);
}

/**
 * The program as an absolute path when it is a bare name found on PATH, the
 * name unchanged when the user wrote it as a path, and `undefined` otherwise.
 *
 * A bare name not on PATH does not simply fail to spawn: handed to
 * `execFile`/`spawn`, it is looked for in the child's cwd -- the workspace
 * folder -- first on Windows, and VS Code's `ProcessExecution` ends its search
 * there on every platform. So the caller must decline rather than try it.
 */
export function resolveProgram(program: string, env: NodeJS.ProcessEnv = process.env): string | undefined {
	if (program.includes("/") || program.includes("\\")) {
		return program;
	}
	for (const candidate of pathCandidates(program, env.PATH, {
		delimiter: path.delimiter,
		sep: path.sep,
		extensions: extensions(env),
	})) {
		try {
			if (statSync(candidate).isFile()) {
				return candidate;
			}
		} catch {
			// Not there; try the next.
		}
	}
	return undefined;
}
