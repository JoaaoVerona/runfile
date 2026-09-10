import * as cp from "node:child_process"
import * as vscode from "vscode"
import { type Target, load, namespaceOf } from "./catalog"
import { RUNFILE_LANGUAGE, RUNFILE_SELECTOR, RunfileCodeLensProvider } from "./codeLens"
import { LanguageClient } from "./lsp"

/** The task type we register a provider for and stamp on every generated task. */
const TASK_TYPE = "runfile"

/**
 * Default command run in each workspace folder to list the targets on stdout.
 * Discovery is the CLI's job: it walks up for the nearest `runfiles/`, down for
 * subproject ones, and folds in the machine-wide directory, so this needs no flags.
 */
const DEFAULT_COMMAND = "run :list --json"

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

	const codeLens = new RunfileCodeLensProvider()
	context.subscriptions.push(codeLens, vscode.languages.registerCodeLensProvider(RUNFILE_SELECTOR, codeLens))

	const config = vscode.workspace.getConfiguration("runfile")
	if (config.get<boolean>("lsp", true)) {
		const client = new LanguageClient(config.get<string>("lspPath", "runfile-lsp"), output)
		client.start()
		context.subscriptions.push(client)
		// Registering this is what makes `editor.formatOnSave` work for `.run`
		// files, and Format Document too. The shape comes from the server, so
		// an editor cannot leave a file in one the CLI would then change.
		// The server advertises all of these; without a provider registered for
		// each, it is asked nothing and none of them happen.
		context.subscriptions.push(
			vscode.languages.registerDocumentFormattingEditProvider(RUNFILE_LANGUAGE, {
				provideDocumentFormattingEdits: (doc) => client.format(doc)
			}),
			// The trigger characters are the server's: `.` opens the property
			// list, and a space is what puts `run ` in front of a target name.
			vscode.languages.registerCompletionItemProvider(
				RUNFILE_LANGUAGE,
				{ provideCompletionItems: (doc, pos) => client.completion(doc, pos) },
				".",
				" "
			),
			vscode.languages.registerHoverProvider(RUNFILE_LANGUAGE, {
				provideHover: (doc, pos) => client.hover(doc, pos)
			}),
			vscode.languages.registerDefinitionProvider(RUNFILE_LANGUAGE, {
				provideDefinition: (doc, pos) => client.definition(doc, pos)
			})
		)
	}

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
		vscode.commands.registerCommand("runfile.runTargetInFile", (arg?: { name: string; anchor: string }) => {
			if (arg) {
				void vscode.tasks.executeTask(buildFileTargetTask(arg.name, arg.anchor))
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
				codeLens.refresh()
			}
		}),
		vscode.workspace.onDidChangeWorkspaceFolders(() => targets.refresh())
	)
}

export function deactivate(): void {
	/* nothing to clean up beyond the disposables above */
}

// ---------------------------------------------------------------------------
// Targets
// ---------------------------------------------------------------------------

/**
 * Where a target came from. The CLI's own three buckets, renamed only where the
 * tree reads better: `subprojects` is what `run :list` calls a nested
 * `runfiles/` directory found below the root.
 */
type SourceKind = "local" | "included" | "global"

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
	const entries: TargetEntry[] = []
	for (const folder of vscode.workspace.workspaceFolders ?? []) {
		const catalog = await load(folder, commandFor(folder), output)
		for (const target of catalog.targets) {
			entries.push(entryFor(target, folder))
		}
	}
	return entries
}

function entryFor(target: Target, folder: vscode.WorkspaceFolder): TargetEntry {
	return {
		task: buildRunTask(target, folder),
		folder,
		name: target.name,
		label: `run ${target.name}`,
		detail: target.description || undefined,
		namespace: namespaceOf(target.name),
		kind: kindOf(target)
	}
}

/** Map the CLI's origins onto the tree's buckets. */
function kindOf(target: Target): SourceKind {
	switch (target.origin) {
		case "global":
			return "global"
		case "subprojects":
			return "included"
		default:
			return "local"
	}
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

/** Build the task that runs a listed target, from the folder it was listed in. */
function buildRunTask(target: Target, folder: vscode.WorkspaceFolder): vscode.Task {
	return buildTask(target.name, folder, folder.uri.fsPath, undefined, target.description)
}

/**
 * Build the task behind an inline **Run** button.
 *
 * The button is anchored with `--dir` to the directory that owns the file's
 * `runfiles/`, so a target inside a subproject runs under the name that
 * directory gives it rather than the name the workspace root would.
 */
function buildFileTargetTask(name: string, anchor: string): vscode.Task {
	const scope = vscode.workspace.getWorkspaceFolder(vscode.Uri.file(anchor)) ?? vscode.TaskScope.Workspace
	return buildTask(name, scope, anchor, anchor)
}

/**
 * The one place a `vscode.Task` is built. Every task invokes `run --stdin-args <name>`
 * so `run` can prompt for any missing `{{ ARG.x }}` / `{{ FLAG.x }}` / `{{ ENV.X }}`
 * values. When the `interactive` setting is on those prompts are served by a
 * pseudoterminal we control (see [`RunfileInteractivePty`]); otherwise the task runs as
 * a plain shell task (matching VS Code's default, where stdin prompts fail).
 */
function buildTask(
	name: string,
	scope: vscode.WorkspaceFolder | vscode.TaskScope,
	cwd: string,
	dir?: string,
	description?: string
): vscode.Task {
	// Every flag has to precede the target name: `run` collects the target and
	// everything after it as trailing arguments to pass through to the target itself.
	const args = ["--stdin-args", ...(dir ? ["--dir", dir] : []), name]

	const execution = isInteractive()
		? new vscode.CustomExecution(async () => new RunfileInteractivePty("run", args, cwd))
		: new vscode.ShellExecution("run", args, { cwd })

	const definition: RunfileTaskDefinition = { type: TASK_TYPE, task: name }
	if (dir) {
		definition.dir = dir
	}
	const task = new vscode.Task(definition, scope, `run ${name}`, TASK_TYPE, execution)
	if (description) {
		task.detail = description
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
	const definition = task.definition as RunfileTaskDefinition
	const wanted = definition.task
	if (typeof wanted !== "string") {
		return undefined
	}
	// A `dir` pins discovery, so there is nothing to look up.
	if (typeof definition.dir === "string") {
		return buildFileTargetTask(wanted, definition.dir)
	}
	const folder = folderOfScope(task.scope)
	if (!folder) {
		return undefined
	}
	const catalog = await load(folder, commandFor(folder), output)
	const target = catalog.targets.find((t) => t.name === wanted)
	return target ? buildRunTask(target, folder) : undefined
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
 * The trailing **Globals** folder — the machine-wide targets, which are nothing more
 * than the `.run` files in `~/.runfiles/` (or `~/runfiles/`, or `~/Runfiles/`; only one
 * of the three may hold anything). There is no registry to add to, so the tooltip names
 * the directory rather than a command. Always present as the last tree-root item so its
 * position is stable; when the directory holds nothing it simply expands to nothing.
 */
function makeGlobalsNode(entries: TargetEntry[]): GroupNode {
	const item = new vscode.TreeItem("Globals", vscode.TreeItemCollapsibleState.Collapsed)
	item.id = "runfile:globals"
	item.iconPath = new vscode.ThemeIcon("globe")
	item.contextValue = "runfileGlobals"
	item.tooltip = new vscode.MarkdownString(
		entries.length > 0
			? `${entries.length} machine-wide target${entries.length === 1 ? "" : "s"} from \`~/.runfiles/\``
			: "No machine-wide targets — put a `.run` file in `~/.runfiles/`"
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
// Configuration
// ---------------------------------------------------------------------------

function commandFor(folder: vscode.WorkspaceFolder): string {
	return vscode.workspace.getConfiguration("runfile", folder.uri).get<string>("catalogCommand", DEFAULT_COMMAND)
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
	/**
	 * Directory discovery starts from (`run --dir`), when pinned. A target in a
	 * subproject's `runfiles/` is named differently from the workspace root, so
	 * the anchor is what makes the name mean what the file meant.
	 */
	dir?: string
}
