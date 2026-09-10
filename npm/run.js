#!/usr/bin/env node
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const KEY = process.platform + '-' + process.arch;
const EXE = process.platform === "win32" ? "run.exe" : "run";
const BINARIES = {
  "linux-x64": EXE,
  "linux-arm64": EXE,
  "darwin-arm64": EXE,
  "darwin-x64": EXE,
  "win32-x64": EXE,
  "win32-arm64": EXE,
};
const bin = BINARIES[KEY];

if (!bin) {
	console.error(`run: unsupported platform ${KEY}. Supported: ${Object.keys(BINARIES).join(', ')}`);
	process.exit(1);
}

const binPath = path.join(__dirname, KEY, bin);
const result = spawnSync(binPath, process.argv.slice(2), { stdio: 'inherit' });

if (result.error) {
	console.error(`run: failed to spawn ${binPath}: ${result.error.message}`);
	process.exit(1);
}

process.exit(result.status == null ? 1 : result.status);
