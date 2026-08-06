import * as cp from "node:child_process"
import * as vscode from "vscode"

/** The task type we register a provider for and stamp on every generated task. */
const TASK_TYPE = "runfile"

/**
 * Default command run in each workspace folder to emit the target descriptors on
 * stdout. `task-descriptors` always resolves `includes` (namespaces) and merges the
 * machine-wide global files — it needs no flags — and its JSON carries per-target
 * provenance (`local` / `included` / `global`) so the sidebar can bucket targets
 * without re-deriving anything from their names.
 */
const DEFAULT_COMMAND = "run :generate task-descriptors"

/** The `task-descriptors` schema version this extension understands. */
const SUPPORTED_FORMAT_VERSION = 1

/** `workspaceState` key holding the pinned targets (see [`PinStore`]). */
const PINNED_STATE_KEY = "runfile.pinnedTargets"

let output: vscode.OutputChannel
let pins: PinStore

export function activate(context: vscode.ExtensionContext): void {
	output = vscode.window.createOutputChannel("Runfile")
	context.subscriptions.push(output)
	// Pins are per-workspace: they record which of *this* project's targets you
	// reach for, so they have no meaning in another window.
	pins = new PinStore(context.workspaceState)

	const provider: vscode.TaskProvider = {
		provideTasks: () => provideRunfileTasks(),
		resolveTask: (task) => resolveRunfileTask(task)
	}
	context.subscriptions.push(vscode.tasks.registerTaskProvider(TASK_TYPE, provider))

	const targets = new RunfileTargetsProvider()
	const treeView = vscode.window.createTreeView("runfile.targets", { treeDataProvider: targets })
	context.subscriptions.push(treeView)

	context.subscriptions.push(
		vscode.commands.registerCommand("runfile.showLog", () => output.show()),
		vscode.commands.registerCommand("runfile.refresh", async () => {
			targets.refresh()
			// fetchTasks re-invokes providers, so this forces a fresh generation.
			const tasks = await vscode.tasks.fetchTasks({ type: TASK_TYPE })
			output.appendLine(`Refreshed — ${tasks.length} task(s) available.`)
		}),
		vscode.commands.registerCommand("runfile.runTarget", (arg?: TargetNode | TargetEntry) => {
			const entry = entryOf(arg)
			if (entry) {
				void vscode.tasks.executeTask(entry.task)
			}
		}),
		vscode.commands.registerCommand("runfile.pinTarget", (arg?: TargetNode | TargetEntry) =>
			setPinned(arg, true, targets)
		),
		vscode.commands.registerCommand("runfile.unpinTarget", (arg?: TargetNode | TargetEntry) =>
			setPinned(arg, false, targets)
		),
		// Keep the sidebar in sync when relevant settings change or folders come and go.
		vscode.workspace.onDidChangeConfiguration((e) => {
			if (e.affectsConfiguration("runfile")) {
				targets.refresh()
			}
		}),
		vscode.workspace.onDidChangeWorkspaceFolders(() => targets.refresh())
	)
}

export function deactivate(): void {
	/* nothing to clean up beyond the disposables above */
}

// ---------------------------------------------------------------------------
// Descriptor document (the `run :generate task-descriptors` contract)
// ---------------------------------------------------------------------------

/** Where a group of targets came from. */
type SourceKind = "local" | "included" | "global"

interface TaskDescriptors {
	formatVersion?: number
	sources?: DescriptorSource[]
}

interface DescriptorSource {
	filePath?: string
	kind?: string
	/**
	 * The `globals.onlyInDirectories` restriction that gated this source, when it
	 * had one. Only ever set on `global` sources. Absent from older CLIs.
	 */
	onlyInDirectories?: string[]
	targets?: DescriptorTarget[]
}

interface DescriptorTarget {
	/** Full canonical invocation name — what we pass to `run` (e.g. `api:build`). */
	name?: string
	/** The include-namespace this target belongs to, or absent when un-namespaced. */
	namespace?: string
	description?: string
}

// ---------------------------------------------------------------------------
// Task provider
// ---------------------------------------------------------------------------

/** A generated task paired with the folder and metadata the sidebar needs to render it. */
interface TargetEntry {
	task: vscode.Task
	folder: vscode.WorkspaceFolder
	/** The target's canonical name, e.g. `api:build`. */
	name: string
	/** The task label, e.g. `run api:build`. */
	label: string
	/** The target's description, when the generator emitted one. */
	detail?: string
	/** The include-namespace this target belongs to, or `undefined`. */
	namespace?: string
	/** Which source file kind contributed this target. */
	kind: SourceKind
}

/**
 * Regenerate the whole target set from scratch — the generator command is cheap and
 * we deliberately keep no cache so entries never go stale. Shared by the task provider
 * (which just needs the `vscode.Task`s) and the sidebar tree (which also needs the
 * per-folder grouping, namespaces, and the local/global split).
 */
async function collectEntries(): Promise<TargetEntry[]> {
	if (!isEnabled()) {
		return []
	}
	const folders = vscode.workspace.workspaceFolders ?? []
	const entries: TargetEntry[] = []
	for (const folder of folders) {
		for (const source of await loadDescriptors(folder)) {
			const kind = normalizeKind(source)
			for (const target of source.targets ?? []) {
				if (!target.name) {
					continue
				}
				entries.push({
					task: buildRunTask(target, folder),
					folder,
					name: target.name,
					label: `run ${target.name}`,
					detail: target.description,
					namespace: target.namespace || undefined,
					kind
				})
			}
		}
	}
	return entries
}

/**
 * Coerce a descriptor source's `kind` to a known [`SourceKind`], defaulting to `local`.
 *
 * A `global` source that carries `onlyInDirectories` is deliberately reported as
 * `local`. That restriction means the file merges in *only* while the working
 * directory sits inside one of those directories — so despite being registered
 * machine-wide it is, in practice, scoped to this project. Filing it under
 * **Globals** would misrepresent it and bury targets that belong in the main tree.
 */
function normalizeKind(source: DescriptorSource): SourceKind {
	if (source.kind === "global") {
		return source.onlyInDirectories && source.onlyInDirectories.length > 0 ? "local" : "global"
	}
	return source.kind === "included" ? "included" : "local"
}

// ---------------------------------------------------------------------------
// Pinning
// ---------------------------------------------------------------------------

/**
 * The set of targets the user pinned, persisted in `workspaceState`.
 *
 * Pinning **moves** a target: it is lifted out of its namespace / workspace / Globals
 * folder and listed as a loose leaf at the top of the tree, so it appears exactly
 * once. Unpinning drops it back where it belongs.
 */
class PinStore {
	constructor(private readonly memento: vscode.Memento) {}

	private get keys(): string[] {
		return this.memento.get<string[]>(PINNED_STATE_KEY, [])
	}

	has(entry: TargetEntry): boolean {
		return this.keys.includes(pinKey(entry))
	}

	/** Pin or unpin `entry`. Idempotent — pinning twice stores one key. */
	async set(entry: TargetEntry, pinned: boolean): Promise<void> {
		const key = pinKey(entry)
		const current = this.keys
		if (current.includes(key) === pinned) {
			return
		}
		const next = pinned ? [...current, key] : current.filter((k) => k !== key)
		await this.memento.update(PINNED_STATE_KEY, next)
	}
}

/**
 * Stable identity for a pin: the target name scoped to the workspace folder it runs
 * in, so the same target name in two roots of a multi-root workspace pins separately.
 * Keys for folders that are no longer open match nothing and render nothing — they
 * are kept rather than pruned so closing and reopening a folder does not drop pins.
 */
function pinKey(entry: TargetEntry): string {
	return `${entry.folder.uri.toString()} ${entry.name}`
}

/** Resolve the entry behind a tree node or a bare entry argument. */
function entryOf(arg: TargetNode | TargetEntry | undefined): TargetEntry | undefined {
	return arg && "entry" in arg ? arg.entry : arg
}

async function setPinned(
	arg: TargetNode | TargetEntry | undefined,
	pinned: boolean,
	tree: RunfileTargetsProvider
): Promise<void> {
	const entry = entryOf(arg)
	if (!entry) {
		return
	}
	await pins.set(entry, pinned)
	tree.refresh()
}

/**
 * Split the flat entry list into the main tree (workspace-local + included targets)
 * and the machine-wide globals, deduped by name — the same globals reappear once per
 * workspace folder, since every folder's descriptor merges in the machine-wide set.
 */
function partitionEntries(entries: TargetEntry[]): { main: TargetEntry[]; globals: TargetEntry[] } {
	const main = entries.filter((e) => e.kind !== "global")
	const seen = new Set<string>()
	const globals: TargetEntry[] = []
	for (const e of entries) {
		if (e.kind === "global" && !seen.has(e.name)) {
			seen.add(e.name)
			globals.push(e)
		}
	}
	return { main, globals }
}

/**
 * VS Code invokes this each time the task list is fetched (e.g. opening Run Task).
 */
async function provideRunfileTasks(): Promise<vscode.Task[]> {
	const { main, globals } = partitionEntries(await collectEntries())
	return [...main, ...globals].map((e) => e.task)
}

/**
 * Build the `vscode.Task` that runs a descriptor target. Every task invokes
 * `run --stdin-args <name>` so `run` can prompt for any missing `{{ ARG.x }}` /
 * `{{ FLAG.x }}` / `{{ ENV.X }}` values. When the `interactive` setting is on those
 * prompts are served by a pseudoterminal we control (see [`RunfileInteractivePty`]);
 * otherwise the task runs as a plain shell task (matching VS Code's default, where
 * stdin prompts fail).
 */
function buildRunTask(target: DescriptorTarget, folder: vscode.WorkspaceFolder): vscode.Task {
	const name = target.name as string
	const args = ["--stdin-args", name]
	const cwd = folder.uri.fsPath

	const execution = isInteractive()
		? new vscode.CustomExecution(async () => new RunfileInteractivePty("run", args, cwd))
		: new vscode.ShellExecution("run", args, { cwd })

	const definition: RunfileTaskDefinition = { type: TASK_TYPE, task: name }
	const task = new vscode.Task(definition, folder, `run ${name}`, TASK_TYPE, execution)
	if (target.description) {
		task.detail = target.description
	}
	task.presentationOptions = {
		reveal: vscode.TaskRevealKind.Always,
		panel: vscode.TaskPanelKind.Shared
	}
	return task
}

/**
 * Resolve a task referenced by definition only (e.g. a user writing
 * `{ "type": "runfile", "task": "..." }` in their tasks.json). We re-generate and
 * match by name so the reference gets a real execution attached.
 */
async function resolveRunfileTask(task: vscode.Task): Promise<vscode.Task | undefined> {
	const wanted = (task.definition as RunfileTaskDefinition).task
	if (typeof wanted !== "string") {
		return undefined
	}
	const folder = folderOfScope(task.scope)
	if (!folder) {
		return undefined
	}
	for (const source of await loadDescriptors(folder)) {
		for (const target of source.targets ?? []) {
			if (target.name === wanted) {
				return buildRunTask(target, folder)
			}
		}
	}
	return undefined
}

// ---------------------------------------------------------------------------
// Sidebar tree
// ---------------------------------------------------------------------------

/** A grouping node with children — a workspace folder, a namespace bucket, or Globals. */
interface GroupNode {
	kind: "group"
	item: vscode.TreeItem
	children: TreeNode[]
}

/** A single runnable target. */
interface TargetNode {
	kind: "target"
	item: vscode.TreeItem
	entry: TargetEntry
}

type TreeNode = GroupNode | TargetNode

/**
 * Backs the **Runfile → Targets** activity-bar view. It lists exactly the targets the
 * task provider would contribute (same generation), so the sidebar and the Run Task
 * list never disagree. Namespaced targets are grouped into a folder per namespace, and
 * the machine-wide globals always hang off a trailing **Globals** folder. Selecting a
 * target — or clicking its inline ▶ — spawns it as a task, just like picking it from
 * Run Task.
 */
class RunfileTargetsProvider implements vscode.TreeDataProvider<TreeNode> {
	private readonly changed = new vscode.EventEmitter<TreeNode | undefined>()
	readonly onDidChangeTreeData = this.changed.event

	/** Re-run generation and repaint. Cheap by design — there is no cache to bust. */
	refresh(): void {
		this.changed.fire(undefined)
	}

	getTreeItem(node: TreeNode): vscode.TreeItem {
		return node.item
	}

	async getChildren(node?: TreeNode): Promise<TreeNode[]> {
		if (node) {
			return node.kind === "group" ? node.children : []
		}
		const { main, globals } = partitionEntries(await collectEntries())
		// Pinning *moves* a target to the top: it is pulled out of the tree below, so
		// it is listed exactly once. A namespace (or workspace) folder whose targets
		// are all pinned therefore disappears, which is the point — nothing is left
		// behind to scroll past.
		const isPinned = (e: TargetEntry): boolean => pins.has(e)
		const pinned = [...main, ...globals].filter(isPinned)
		const restMain = main.filter((e) => !isPinned(e))
		const restGlobals = globals.filter((e) => !isPinned(e))

		const folders = vscode.workspace.workspaceFolders ?? []
		let roots: TreeNode[]
		// In a multi-root workspace, group by workspace folder first, then by
		// namespace within each. A single folder skips that redundant outer layer.
		if (folders.length > 1) {
			roots = folders
				.map((folder) => {
					const own = restMain.filter((e) => e.folder === folder)
					return own.length > 0 ? makeFolderNode(folder, own) : undefined
				})
				.filter((n): n is GroupNode => n !== undefined)
		} else {
			roots = groupByNamespace(restMain, "")
		}
		// The machine-wide globals always hang off a trailing folder, regardless of how
		// many are registered (it stays put even when empty, so its place never shifts).
		roots.push(makeGlobalsNode(restGlobals))
		// Pinned targets lead the root as loose leaves — no wrapper folder, so they cost
		// no expand click. Labels keep the full canonical name (`api:build`, not
		// `build`): a pin is lifted out of the namespace folder that supplied the prefix.
		// The id prefix carries the workspace folder so two roots of a multi-root
		// workspace can pin the same target name without colliding.
		roots.unshift(
			...pinned
				.sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0))
				.map((entry) => makeTargetNode(entry, entry.name, `pinned:${entry.folder.uri.toString()}::`))
		)
		return roots
	}
}

function makeFolderNode(folder: vscode.WorkspaceFolder, entries: TargetEntry[]): GroupNode {
	const item = new vscode.TreeItem(folder.name, vscode.TreeItemCollapsibleState.Expanded)
	item.id = `wsfolder:${folder.uri.toString()}`
	item.iconPath = new vscode.ThemeIcon("folder")
	item.contextValue = "runfileFolder"
	return { kind: "group", item, children: groupByNamespace(entries, `${folder.uri.toString()}::`) }
}

/**
 * The trailing **Globals** folder — the machine-wide targets registered via
 * `run :config global-files`. Always present as the last tree-root item so its position
 * is stable; when no globals are registered it simply expands to nothing.
 */
function makeGlobalsNode(entries: TargetEntry[]): GroupNode {
	const item = new vscode.TreeItem("Globals", vscode.TreeItemCollapsibleState.Collapsed)
	item.id = "runfile:globals"
	item.iconPath = new vscode.ThemeIcon("globe")
	item.contextValue = "runfileGlobals"
	item.tooltip = new vscode.MarkdownString(
		entries.length > 0
			? `${entries.length} machine-wide global target${entries.length === 1 ? "" : "s"} (\`run :config global-files\`)`
			: "No global targets registered (`run :config global-files add`)"
	)
	return { kind: "group", item, children: groupByNamespace(entries, "globals::") }
}

/**
 * Split `entries` into a namespace folder per namespace (sorted), followed by the
 * un-namespaced targets as loose leaves. `idPrefix` keeps tree-item ids unique across
 * workspace folders (and the Globals bucket) so VS Code preserves expansion state
 * across refreshes.
 *
 * Everything is sorted by canonical name here rather than trusting the descriptor's
 * order: the generator only sorts targets *within* each source file, but this bucket can
 * merge several sources (local + un-namespaced includes into the loose leaves; multiple
 * global files into the Globals folder), so the merged result must be re-sorted. Sorting
 * by full name also orders each namespace's children by their stripped name, since they
 * all share the `<ns>:` prefix.
 */
function groupByNamespace(entries: TargetEntry[], idPrefix: string): TreeNode[] {
	const sorted = [...entries].sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0))
	const groups = new Map<string, TargetEntry[]>()
	const loose: TargetEntry[] = []
	for (const entry of sorted) {
		const ns = entry.namespace
		if (ns === undefined) {
			loose.push(entry)
		} else {
			const bucket = groups.get(ns)
			if (bucket) {
				bucket.push(entry)
			} else {
				groups.set(ns, [entry])
			}
		}
	}
	const namespaceNodes = [...groups.keys()]
		.sort()
		.map((ns) => makeNamespaceNode(ns, groups.get(ns) as TargetEntry[], idPrefix))
	const looseNodes = loose.map((entry) => makeTargetNode(entry, entry.name, idPrefix))
	return [...namespaceNodes, ...looseNodes]
}

function makeNamespaceNode(ns: string, entries: TargetEntry[], idPrefix: string): GroupNode {
	const item = new vscode.TreeItem(ns, vscode.TreeItemCollapsibleState.Collapsed)
	item.id = `${idPrefix}ns:${ns}`
	item.iconPath = new vscode.ThemeIcon("symbol-namespace")
	item.contextValue = "runfileNamespace"
	item.tooltip = `${entries.length} target${entries.length === 1 ? "" : "s"} in the “${ns}” namespace`
	// Strip the `<ns>:` prefix from each child's label — the namespace is the folder.
	const children = entries.map((entry) => makeTargetNode(entry, displayName(entry, ns), idPrefix))
	return { kind: "group", item, children }
}

/** A namespaced target's leaf label — its name with the owning `<ns>:` prefix removed. */
function displayName(entry: TargetEntry, ns: string): string {
	return entry.name.startsWith(`${ns}:`) ? entry.name.slice(ns.length + 1) : entry.name
}

function makeTargetNode(entry: TargetEntry, display: string, idPrefix: string): TargetNode {
	const item = new vscode.TreeItem(display, vscode.TreeItemCollapsibleState.None)
	const pinned = pins.has(entry)
	item.id = `${idPrefix}target:${entry.name}`
	item.iconPath = new vscode.ThemeIcon(pinned ? "pinned" : "play")
	// The two values drive which of Pin / Unpin the menus offer (see package.json).
	item.contextValue = pinned ? "runfileTargetPinned" : "runfileTarget"
	if (entry.detail) {
		item.description = entry.detail
		item.tooltip = new vscode.MarkdownString(`**${entry.label}**\n\n${entry.detail}`)
	} else {
		item.tooltip = entry.label
	}
	const node: TargetNode = { kind: "target", item, entry }
	// Clicking the row spawns the task; the inline ▶ button reuses the same command.
	item.command = { command: "runfile.runTarget", title: "Run Target", arguments: [node] }
	return node
}

// ---------------------------------------------------------------------------
// Generation
// ---------------------------------------------------------------------------

/** Run the descriptor command in `folder` and return its `sources`, or `[]` on any failure. */
async function loadDescriptors(folder: vscode.WorkspaceFolder): Promise<DescriptorSource[]> {
	const command = commandFor(folder)
	let stdout: string
	try {
		stdout = await runShell(command, folder.uri.fsPath)
	} catch {
		// runShell already logged the failure; treat as "no targets".
		return []
	}
	const text = stdout.replace(/^﻿/, "").trim()
	if (!text) {
		return []
	}
	let doc: TaskDescriptors
	try {
		doc = JSON.parse(text) as TaskDescriptors
	} catch (err) {
		output.appendLine(`Could not parse task descriptors from \`${command}\`: ${(err as Error).message}`)
		return []
	}
	if (!doc || typeof doc !== "object") {
		return []
	}
	if (typeof doc.formatVersion === "number" && doc.formatVersion !== SUPPORTED_FORMAT_VERSION) {
		output.appendLine(
			`Runfile: task-descriptors formatVersion ${doc.formatVersion} differs from the supported ` +
				`${SUPPORTED_FORMAT_VERSION}; parsing anyway. Update the extension or the \`run\` CLI if targets look wrong.`
		)
	}
	return Array.isArray(doc.sources) ? doc.sources : []
}

function runShell(command: string, cwd: string): Promise<string> {
	return new Promise((resolve, reject) => {
		cp.exec(
			command,
			{ cwd, env: process.env, maxBuffer: 16 * 1024 * 1024, windowsHide: true },
			(err, stdout, stderr) => {
				if (err) {
					output.appendLine(`$ ${command}  (cwd: ${cwd})`)
					output.appendLine(`  ✗ ${err.message}`)
					const trimmed = stderr?.trim()
					if (trimmed) {
						output.appendLine(trimmed)
					}
					reject(err)
					return
				}
				resolve(stdout)
			}
		)
	})
}

function commandFor(folder: vscode.WorkspaceFolder): string {
	return vscode.workspace.getConfiguration("runfile", folder.uri).get<string>("generateCommand", DEFAULT_COMMAND)
}

function folderOfScope(
	scope: vscode.WorkspaceFolder | vscode.TaskScope | undefined
): vscode.WorkspaceFolder | undefined {
	if (scope && typeof scope === "object") {
		return scope
	}
	return vscode.workspace.workspaceFolders?.[0]
}

// ---------------------------------------------------------------------------
// Interactive execution
// ---------------------------------------------------------------------------

/**
 * A pseudoterminal that runs the command itself so the child's stdin is a pipe we
 * own. Keystrokes typed into the task terminal are line-buffered here and written
 * to the child on Enter — which is what lets `run --stdin-args` prompt the user for
 * missing arguments, something a plain shell/process task terminal can't do.
 *
 * The command is spawned directly (no shell): every `--stdin-args` task the
 * extension builds is a clean `run <flags...> <target>` argv with no shell
 * metacharacters, and `run` is resolved via PATH just as a shell would.
 */
class RunfileInteractivePty implements vscode.Pseudoterminal {
	private readonly writeEmitter = new vscode.EventEmitter<string>()
	private readonly closeEmitter = new vscode.EventEmitter<number>()
	readonly onDidWrite = this.writeEmitter.event
	readonly onDidClose = this.closeEmitter.event

	private child?: cp.ChildProcess
	private line = ""

	constructor(
		private readonly command: string,
		private readonly args: string[],
		private readonly cwd: string,
		private readonly env?: { [key: string]: string }
	) {}

	open(): void {
		const env = this.env ? { ...process.env, ...this.env } : process.env
		const child = cp.spawn(this.command, this.args, {
			cwd: this.cwd,
			env,
			stdio: ["pipe", "pipe", "pipe"],
			// Put `run` in its own process group (as leader, its pgid equals its pid).
			// `run` spawns the real work — tsc, vsce, code — as children; signalling
			// only `run`, which is all `child.kill()` does, would orphan them and leave
			// them running after the terminal is gone. Grouping lets us kill the lot.
			detached: true
		})
		this.child = child

		// Terminals expect CRLF; the child emits bare LF.
		const emit = (buf: Buffer): void => this.writeEmitter.fire(buf.toString().replace(/\r?\n/g, "\r\n"))
		child.stdout?.on("data", emit)
		child.stderr?.on("data", emit)
		child.on("error", (err) => {
			this.child = undefined
			this.writeEmitter.fire(`\r\n\x1b[31m${err.message}\x1b[0m\r\n`)
			this.closeEmitter.fire(1)
		})
		child.on("close", (code) => {
			this.child = undefined
			this.closeEmitter.fire(code ?? 0)
		})
	}

	close(): void {
		// The terminal is being torn down: ask the whole group to stop, then hard-kill
		// any stragglers that ignore SIGTERM so nothing outlives the terminal.
		const child = this.child
		this.signal("SIGTERM")
		if (child) {
			setTimeout(() => {
				if (this.child === child) {
					this.signal("SIGKILL")
				}
			}, 2000)
		}
	}

	/**
	 * Signal `run`'s entire process group, not just `run` itself, so the commands it
	 * spawned die with it. `detached: true` made `run` the group leader, so its pgid
	 * equals its pid and the negated pid targets the group. Falls back to signalling
	 * the child alone if the group send fails (already exited, or no process groups).
	 */
	private signal(sig: NodeJS.Signals): void {
		const child = this.child
		if (!child?.pid) {
			return
		}
		try {
			process.kill(-child.pid, sig)
		} catch {
			try {
				child.kill(sig)
			} catch {
				/* already gone */
			}
		}
	}

	handleInput(data: string): void {
		const stdin = this.child?.stdin
		if (!stdin) {
			return
		}
		for (const ch of data) {
			if (ch === "\r") {
				// Enter: commit the buffered line to the child as a newline.
				this.writeEmitter.fire("\r\n")
				stdin.write(`${this.line}\n`)
				this.line = ""
			} else if (ch === "\x7f" || ch === "\b") {
				// Backspace: erase one char from the buffer and the display.
				if (this.line.length > 0) {
					this.line = this.line.slice(0, -1)
					this.writeEmitter.fire("\b \b")
				}
			} else if (ch === "\x03") {
				this.signal("SIGINT") // Ctrl+C — reaches the whole group, not just `run`
			} else if (ch === "\x04") {
				stdin.end() // Ctrl+D closes stdin (EOF)
			} else if (ch >= " ") {
				// Printable: buffer it and echo (the pipe gives no terminal echo).
				this.line += ch
				this.writeEmitter.fire(ch)
			}
		}
	}
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

function isEnabled(): boolean {
	return vscode.workspace.getConfiguration("runfile").get<boolean>("enabled", true)
}

function isInteractive(): boolean {
	return vscode.workspace.getConfiguration("runfile").get<boolean>("interactive", true)
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface RunfileTaskDefinition extends vscode.TaskDefinition {
	task: string
}
