//! The `task-descriptors` generator: a stable, editor-agnostic JSON description of
//! every runnable target, grouped by the source file it came from.
//!
//! Unlike the `vscode-tasks` / `zed-tasks` / `jetbrains-run-configurations`
//! generators — which emit editor-specific config shaped to land on disk — this one
//! is consumed by external tooling (the Runfile VS Code extension) that builds its
//! own editor integration from the semantic facts here. It always prints to stdout,
//! always includes namespaced (`includes`) and global targets, and carries per-target
//! provenance so a client can bucket local vs. included vs. global targets however it
//! likes — without re-deriving any of it from target names.

use runfile_parser::{MergeResult, SourceKind, is_internal_target_name};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

/// On-the-wire format version. Bump when the shape changes incompatibly so a
/// consumer pinned to an older CLI can detect the mismatch instead of mis-parsing.
pub const TASK_DESCRIPTORS_FORMAT_VERSION: u32 = 1;

/// Top-level `task-descriptors` document.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TaskDescriptors {
	#[serde(rename = "formatVersion")]
	pub format_version: u32,
	/// One entry per source file that contributes at least one runnable target,
	/// ordered local → included → global, then by path.
	pub sources: Vec<DescriptorSource>,
}

/// Where a group of targets came from. Mirrors [`SourceKind`], serialized as a
/// lowercase string (`"local"` / `"included"` / `"global"`).
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum DescriptorKind {
	/// The workspace's own Runfile (auto-discovered or `-f`/`--file`).
	Local,
	/// A file pulled in via `includes` (its targets may be namespaced).
	Included,
	/// A machine-wide global Runfile registered via `run :config global-files`.
	Global,
}

impl From<SourceKind> for DescriptorKind {
	fn from(k: SourceKind) -> Self {
		match k {
			SourceKind::Local => DescriptorKind::Local,
			SourceKind::Included => DescriptorKind::Included,
			SourceKind::Global => DescriptorKind::Global,
		}
	}
}

/// One source file and the runnable targets it contributes.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DescriptorSource {
	#[serde(rename = "filePath")]
	pub file_path: String,
	pub kind: DescriptorKind,
	pub targets: Vec<DescriptorTarget>,
}

/// A single runnable target.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DescriptorTarget {
	/// Full canonical invocation name — exactly what you pass to `run` (e.g.
	/// `build` or the namespaced `api:build`).
	pub name: String,
	/// The include-namespace this target belongs to, or omitted for un-namespaced
	/// targets. Derived from the real namespace list, so a name that merely
	/// *contains* a colon (e.g. `all:package`) stays un-namespaced.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub namespace: Option<String>,
	/// The target's `description`, when it has one.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub description: Option<String>,
}

/// Build the `task-descriptors` document from a merged Runfile.
///
/// Applies the same visibility filter as the editor generators — internal
/// (`_`-prefixed) targets and those with `excludeFromGenerateCommand: true` are
/// omitted — then groups the survivors by source file (and kind), with both the
/// groups and the targets within them sorted for deterministic output.
pub fn generate_task_descriptors(merge: &MergeResult) -> TaskDescriptors {
	// Namespace roots: the first `:`-segment of every real include-namespace. A
	// target's namespace is its first segment when that segment is a known root —
	// matching how `run :list` and the sidebar bucket namespaced targets, and
	// leaving colon-in-the-name-but-not-namespaced targets (e.g. `all:package`)
	// un-namespaced.
	let roots: HashSet<&str> = merge
		.runfile
		.namespaces
		.iter()
		.map(|ns| ns.split(':').next().unwrap_or(ns))
		.collect();

	// Group by (kind, path) so the output is ordered local → included → global,
	// then by path. A file included twice under different namespaces collapses to
	// one group (its targets carry their own `namespace`), which is what we want.
	let mut groups: BTreeMap<(DescriptorKind, PathBuf), Vec<DescriptorTarget>> = BTreeMap::new();

	for (name, spec) in &merge.runfile.targets {
		if is_internal_target_name(name) || spec.is_excluded_from_generate() {
			continue;
		}
		// Every merged target has a source entry; skip defensively if not.
		let Some((path, kind)) = merge.target_sources.get(name) else {
			continue;
		};
		groups
			.entry(((*kind).into(), path.clone()))
			.or_default()
			.push(DescriptorTarget {
				name: name.clone(),
				namespace: namespace_of(name, &roots),
				description: spec.description.clone(),
			});
	}

	let sources = groups
		.into_iter()
		.map(|((kind, path), mut targets)| {
			targets.sort_by(|a, b| a.name.cmp(&b.name));
			DescriptorSource {
				file_path: path.to_string_lossy().into_owned(),
				kind,
				targets,
			}
		})
		.collect();

	TaskDescriptors {
		format_version: TASK_DESCRIPTORS_FORMAT_VERSION,
		sources,
	}
}

/// The include-namespace a target `name` belongs to: its first `:`-segment when
/// that segment is a known namespace root, else `None`.
fn namespace_of(name: &str, roots: &HashSet<&str>) -> Option<String> {
	let colon = name.find(':')?;
	if colon == 0 {
		return None;
	}
	let first = &name[..colon];
	roots.contains(first).then(|| first.to_string())
}

/// Serialize a [`TaskDescriptors`] document to pretty JSON bytes for stdout.
pub fn render_task_descriptors(descriptors: &TaskDescriptors) -> Result<Vec<u8>, serde_json::Error> {
	serde_json::to_vec_pretty(descriptors)
}
