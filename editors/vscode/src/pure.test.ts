// Tests for the extension logic that does not need an extension host.
// Run with `run vscode:test`.

import assert from "node:assert/strict";
import { test } from "node:test";

import {
	MessageReader,
	SUPPORTED_FORMAT_VERSION,
	anchorFor,
	frame,
	namespaceOf,
	parseCatalog,
	replyId,
	targetNameFor,
} from "./pure";

function collectingLog(): { lines: string[]; appendLine(l: string): void } {
	const lines: string[] = [];
	return { lines, appendLine: (l) => lines.push(l) };
}

function doc(targets: unknown[], version = SUPPORTED_FORMAT_VERSION): string {
	return JSON.stringify({ formatVersion: version, targets });
}

test("a catalog is read into targets", () => {
	const log = collectingLog();
	const out = parseCatalog(
		doc([{ name: "build", description: "Builds", origin: "local", path: "/p/runfiles/build.run" }]),
		"p",
		log,
	);
	assert.equal(out.length, 1);
	assert.equal(out[0]?.name, "build");
	assert.deepEqual(log.lines, [], "a clean read says nothing");
});

test("no output means no targets, not an error", () => {
	// A folder with no runfiles/ is the common case.
	const log = collectingLog();
	assert.deepEqual(parseCatalog("   ", "p", log), []);
	assert.deepEqual(log.lines, []);
});

test("invalid JSON is logged and yields nothing", () => {
	const log = collectingLog();
	assert.deepEqual(parseCatalog("not json", "p", log), []);
	assert.equal(log.lines.length, 1);
	assert.match(log.lines[0] ?? "", /not valid JSON/);
});

test("a leading BOM does not break the read", () => {
	// Some Windows shells prepend one, and JSON.parse rejects it.
	const log = collectingLog();
	const out = parseCatalog(`\ufeff${doc([{ name: "a" }])}`, "p", log);
	assert.equal(out.length, 1);
});

test("a different format version warns but still reads", () => {
	// Refusing would empty the view over a version bump alone.
	const log = collectingLog();
	const out = parseCatalog(doc([{ name: "a" }], 99), "p", log);
	assert.equal(out.length, 1);
	assert.match(log.lines[0] ?? "", /reading it anyway/);
});

test("entries without a name are dropped", () => {
	const log = collectingLog();
	assert.equal(parseCatalog(doc([{ name: "a" }, { description: "x" }]), "p", log).length, 1);
});

test("a namespace is the part before the first colon, when there is one", () => {
	assert.equal(namespaceOf("api:build"), "api");
	assert.equal(namespaceOf("api:web:build"), "api", "the first, so nesting groups by the outermost");
	assert.equal(namespaceOf("build"), undefined);
	assert.equal(namespaceOf(":odd"), undefined, "a leading colon names nothing");
});

test("a file under runfiles/ names a target by its path", () => {
	assert.equal(targetNameFor("/p/runfiles/build.run"), "build");
	assert.equal(targetNameFor("/p/runfiles/api/deploy.run"), "api:deploy");
	assert.equal(targetNameFor("/p/runfiles/a/b/c.run"), "a:b:c");
});

test("windows separators name the same target", () => {
	assert.equal(targetNameFor("C:\\p\\runfiles\\api\\deploy.run"), "api:deploy");
});

test("files that are not targets get no name", () => {
	assert.equal(targetNameFor("/p/runfiles/_shared.run"), undefined, "shared settings are not runnable");
	assert.equal(targetNameFor("/p/runfiles/notes.txt"), undefined);
	assert.equal(targetNameFor("/p/src/build.run"), undefined, "outside runfiles/");
});

test("the deepest runfiles/ wins", () => {
	// A nested subproject's file belongs to that subproject.
	assert.equal(targetNameFor("/p/runfiles/x/runfiles/build.run"), "build");
	assert.equal(anchorFor("/p/runfiles/x/runfiles/build.run"), "/p/runfiles/x");
});

test("the anchor is the parent of runfiles/", () => {
	assert.equal(anchorFor("/p/runfiles/build.run"), "/p");
	assert.equal(anchorFor("/p/sub/runfiles/build.run"), "/p/sub");
	assert.equal(anchorFor("/src/build.run"), undefined);
});

test("a message is framed with its byte length", () => {
	// `{"s":"héllo"}` is 13 characters but 14 bytes; a character count would
	// truncate the message on the way out.
	assert.match(frame({ s: "héllo" }), /^Content-Length: 14\r\n\r\n/);
});

test("messages are read back whole", () => {
	const r = new MessageReader();
	const out = r.push(Buffer.from(frame({ n: 1 }) + frame({ n: 2 }), "utf8"));
	assert.deepEqual(out, [{ n: 1 }, { n: 2 }]);
});

test("a message split across chunks is held until complete", () => {
	// The stream is a pipe: a message arriving in pieces is normal, not an error.
	const r = new MessageReader();
	const bytes = Buffer.from(frame({ hello: "world" }), "utf8");
	assert.deepEqual(r.push(bytes.subarray(0, 10)), []);
	assert.deepEqual(r.push(bytes.subarray(10, 25)), []);
	assert.deepEqual(r.push(bytes.subarray(25)), [{ hello: "world" }]);
});

test("a multibyte body is not cut short", () => {
	const r = new MessageReader();
	assert.deepEqual(r.push(Buffer.from(frame({ s: "→→→" }), "utf8")), [{ s: "→→→" }]);
});

test("a malformed body loses one message, not the stream", () => {
	const r = new MessageReader();
	const bad = "Content-Length: 3\r\n\r\n{[}";
	const out = r.push(Buffer.from(bad + frame({ ok: true }), "utf8"));
	assert.deepEqual(out, [{ ok: true }]);
});

test("a header without a length is skipped rather than stalling", () => {
	const r = new MessageReader();
	const out = r.push(Buffer.from(`X-Nonsense: 1\r\n\r\n${frame({ ok: true })}`, "utf8"));
	assert.deepEqual(out, [{ ok: true }]);
});

test("a reply is told from a notification by its id", () => {
	// Format-on-save waits on a reply; mistaking a notification for one would
	// resolve the wrong request, and mistaking a reply for a notification
	// would leave the save waiting until it timed out.
	assert.equal(replyId({ jsonrpc: "2.0", id: 9, result: [] }), 9);
	assert.equal(replyId({ jsonrpc: "2.0", id: 0, result: null }), 0);
	assert.equal(replyId({ jsonrpc: "2.0", method: "textDocument/publishDiagnostics", params: {} }), undefined);
	// A server-to-client *request* has both, and is not ours to resolve.
	assert.equal(replyId({ jsonrpc: "2.0", id: 1, method: "window/showMessageRequest" }), undefined);
	assert.equal(replyId({ jsonrpc: "2.0", id: "1", result: [] }), undefined);
})
