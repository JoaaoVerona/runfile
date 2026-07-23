use crate::{DescriptorKind, generate_task_descriptors};
use runfile_parser::{CommandSpec, MergeResult, Metadata, Runfile, SourceKind};
use std::collections::HashMap;
use std::path::PathBuf;

/// A target spec with an optional description and the given shell commands.
fn spec(description: Option<&str>, commands: Vec<&str>) -> CommandSpec {
	let mut s = CommandSpec::new_shell(commands.into_iter().map(String::from).collect());
	s.description = description.map(String::from);
	s
}

/// Build a [`MergeResult`] from `(name, kind, file, spec)` rows, wiring up the
/// `target_sources` provenance the descriptor generator reads. `namespaces` seeds
/// `Runfile.namespaces` (the real include-namespace list).
fn merge_of(rows: Vec<(&str, SourceKind, &str, CommandSpec)>, namespaces: Vec<&str>) -> MergeResult {
	let mut targets = HashMap::new();
	let mut target_sources = HashMap::new();
	let mut source_dirs = HashMap::new();
	for (name, kind, file, s) in rows {
		let path = PathBuf::from(file);
		targets.insert(name.to_string(), s);
		target_sources.insert(name.to_string(), (path.clone(), kind));
		source_dirs.insert(name.to_string(), path.parent().unwrap().to_path_buf());
	}
	MergeResult {
		runfile: Runfile {
			schema: "x".into(),
			includes: None,
			targets,
			globals: None,
			namespaces: namespaces.into_iter().map(String::from).collect(),
		},
		source_dirs,
		target_sources,
		conflicts: HashMap::new(),
	}
}

#[test]
fn groups_by_kind_and_reports_provenance() {
	let merge = merge_of(
		vec![
			(
				"build",
				SourceKind::Local,
				"/proj/Runfile.json",
				spec(Some("Build"), vec!["cargo build"]),
			),
			(
				"test",
				SourceKind::Local,
				"/proj/Runfile.json",
				spec(None, vec!["cargo test"]),
			),
			(
				"api:deploy",
				SourceKind::Included,
				"/proj/api/Runfile.json",
				spec(None, vec!["echo deploy"]),
			),
			(
				"backup",
				SourceKind::Global,
				"/home/me/globals.json",
				spec(Some("Back up"), vec!["echo back"]),
			),
		],
		vec!["api"],
	);

	let doc = generate_task_descriptors(&merge);
	assert_eq!(doc.format_version, 1);

	// Ordered local → included → global.
	let kinds: Vec<DescriptorKind> = doc.sources.iter().map(|s| s.kind).collect();
	assert_eq!(
		kinds,
		vec![DescriptorKind::Local, DescriptorKind::Included, DescriptorKind::Global]
	);

	// Local source: both targets, sorted by name; provenance path preserved.
	let local = &doc.sources[0];
	assert_eq!(local.file_path, "/proj/Runfile.json");
	let names: Vec<&str> = local.targets.iter().map(|t| t.name.as_str()).collect();
	assert_eq!(names, vec!["build", "test"]);

	let build = &local.targets[0];
	assert_eq!(build.description.as_deref(), Some("Build"));
	assert_eq!(build.namespace, None);

	// Included source: the namespaced target carries its namespace.
	let included = &doc.sources[1];
	assert_eq!(included.kind, DescriptorKind::Included);
	assert_eq!(included.targets[0].name, "api:deploy");
	assert_eq!(included.targets[0].namespace.as_deref(), Some("api"));

	// Global source.
	assert_eq!(doc.sources[2].kind, DescriptorKind::Global);
	assert_eq!(doc.sources[2].targets[0].name, "backup");
}

#[test]
fn skips_internal_and_excluded_targets() {
	let mut excluded = spec(None, vec!["echo nope"]);
	excluded.metadata = Some(Metadata {
		exclude_from_generate_command: Some(true),
		extra: Default::default(),
	});
	let merge = merge_of(
		vec![
			(
				"build",
				SourceKind::Local,
				"/proj/Runfile.json",
				spec(None, vec!["echo build"]),
			),
			(
				"_helper",
				SourceKind::Local,
				"/proj/Runfile.json",
				spec(None, vec!["echo helper"]),
			),
			("hidden", SourceKind::Local, "/proj/Runfile.json", excluded),
		],
		vec![],
	);

	let doc = generate_task_descriptors(&merge);
	let names: Vec<&str> = doc
		.sources
		.iter()
		.flat_map(|s| &s.targets)
		.map(|t| t.name.as_str())
		.collect();
	assert_eq!(
		names,
		vec!["build"],
		"internal `_helper` and excluded `hidden` are filtered out"
	);
}

#[test]
fn colon_named_local_target_is_not_namespaced() {
	// `all:package` merely contains a colon; `all` is not a real namespace root.
	let merge = merge_of(
		vec![(
			"all:package",
			SourceKind::Local,
			"/proj/Runfile.json",
			spec(None, vec!["echo all"]),
		)],
		vec!["api"],
	);
	let doc = generate_task_descriptors(&merge);
	assert_eq!(doc.sources[0].targets[0].namespace, None);
}
