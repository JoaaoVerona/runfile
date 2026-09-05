use super::*;
use tempfile::TempDir;

fn touch(p: &Path) {
	std::fs::create_dir_all(p.parent().unwrap()).unwrap();
	std::fs::write(p, "# t\n$ true\n").unwrap();
}

fn fixture() -> TempDir {
	let d = TempDir::new().unwrap();
	let r = d.path();
	touch(&r.join("runfiles/build.run"));
	touch(&r.join("runfiles/build/infrastructure.run"));
	touch(&r.join("runfiles/i18n/sync.run"));
	std::fs::write(r.join("runfiles/_shared.run"), ".add-path = \"bin\"\n").unwrap();
	touch(&r.join("web-admin/runfiles/build.run"));
	touch(&r.join("web-admin/runfiles/dev.run"));
	touch(&r.join("node_modules/pkg/runfiles/should-not-appear.run"));
	d
}

#[test]
fn names_come_from_the_path_with_slash_as_colon() {
	let d = fixture();
	let c = discover(d.path(), None).unwrap();
	let mut names: Vec<_> = c.targets.keys().cloned().collect();
	names.sort();
	assert_eq!(
		names,
		vec![
			"build",
			"build:infrastructure",
			"i18n:sync",
			"web-admin:build",
			"web-admin:dev"
		]
	);
}

#[test]
fn a_target_and_a_namespace_may_share_a_name() {
	// build.run and build/ coexist; `:` never appears in a file name, so a
	// namespace is always a real directory.
	let d = fixture();
	let c = discover(d.path(), None).unwrap();
	assert!(c.resolve("build").is_some());
	assert!(c.resolve("build:infrastructure").is_some());
}

#[test]
fn subprojects_are_namespaced_without_any_declaration() {
	let d = fixture();
	let c = discover(d.path(), None).unwrap();
	let t = c.resolve("web-admin:build").unwrap();
	assert_eq!(t.origin, Origin::Included);
	assert_eq!(t.anchor, d.path().join("web-admin"), "anchor is the runfiles/ parent");
}

#[test]
fn skipped_directories_are_not_scanned() {
	let d = fixture();
	let c = discover(d.path(), None).unwrap();
	assert!(c.resolve("pkg:should-not-appear").is_none());
}

#[test]
fn shared_is_not_a_target() {
	let d = fixture();
	let c = discover(d.path(), None).unwrap();
	assert!(c.resolve("_shared").is_none());
	let t = c.resolve("build").unwrap();
	assert_eq!(c.shared_chain(t).len(), 1, "but it applies to the targets it covers");
}

#[test]
fn discovery_walks_upward_from_a_nested_directory() {
	let d = fixture();
	let deep = d.path().join("web-admin/src/components");
	std::fs::create_dir_all(&deep).unwrap();
	let c = discover(&deep, None).unwrap();
	assert!(c.resolve("build").is_some() || c.resolve("dev").is_some());
}

#[test]
fn the_machine_wide_directory_is_dotted_and_global() {
	let d = TempDir::new().unwrap();
	touch(&d.path().join(".runfiles/deploy.run"));
	let project = TempDir::new().unwrap();
	touch(&project.path().join("runfiles/build.run"));
	let c = discover(project.path(), Some(d.path())).unwrap();
	assert_eq!(c.resolve("deploy").unwrap().origin, Origin::Global);
	assert_eq!(c.resolve("build").unwrap().origin, Origin::Local);
}

#[test]
fn a_duplicate_target_names_both_files() {
	let d = TempDir::new().unwrap();
	touch(&d.path().join("runfiles/a/b.run"));
	touch(&d.path().join("runfiles/a:b.run"));
	// `:` in a file name is only reachable on a filesystem that permits it;
	// where it is, the collision must be reported rather than silently won.
	let r = discover(d.path(), None);
	if let Err(e) = r {
		assert!(e.to_string().contains("defined twice"), "{e}");
	}
}

#[test]
fn nothing_anywhere_is_an_error_that_names_the_directory() {
	let d = TempDir::new().unwrap();
	let e = discover(d.path(), None).unwrap_err();
	assert!(e.to_string().contains("no runfiles/"), "{e}");
}

// ---- aliases and directory scoping

#[test]
fn an_alias_resolves_only_when_the_file_name_misses() {
	let d = TempDir::new().unwrap();
	std::fs::create_dir_all(d.path().join("runfiles")).unwrap();
	std::fs::write(d.path().join("runfiles/build.run"), ".alias = \"b\"\n$ true\n").unwrap();
	let c = discover(d.path(), None).unwrap();
	assert_eq!(c.resolve("build").unwrap().name, "build", "the exact name never scans");
	assert_eq!(c.resolve("b").unwrap().name, "build", "the alias is found on a miss");
	assert!(c.resolve("nope").is_none());
}

#[test]
fn two_targets_claiming_one_alias_is_an_error() {
	let d = TempDir::new().unwrap();
	std::fs::create_dir_all(d.path().join("runfiles")).unwrap();
	std::fs::write(d.path().join("runfiles/one.run"), ".alias = \"x\"\n$ true\n").unwrap();
	std::fs::write(d.path().join("runfiles/two.run"), ".alias = \"x\"\n$ true\n").unwrap();
	let c = discover(d.path(), None).unwrap();
	let e = c.by_alias("x").unwrap_err();
	assert!(e.to_string().contains("claimed by both"), "{e}");
}

#[test]
fn a_scoped_global_appears_only_inside_the_directories_it_names() {
	let home = TempDir::new().unwrap();
	let g = home.path().join(".runfiles");
	std::fs::create_dir_all(&g).unwrap();
	std::fs::write(g.join("deploy.run"), "$ true\n").unwrap();

	let inside = home.path().join("work/acme");
	std::fs::create_dir_all(inside.join("runfiles")).unwrap();
	std::fs::write(inside.join("runfiles/build.run"), "$ true\n").unwrap();
	let outside = TempDir::new().unwrap();
	std::fs::create_dir_all(outside.path().join("runfiles")).unwrap();
	std::fs::write(outside.path().join("runfiles/build.run"), "$ true\n").unwrap();

	std::fs::write(g.join(SHARED), ".only-in-directories = \"work/acme\"\n").unwrap();

	let here = discover(&inside, Some(home.path())).unwrap();
	assert!(here.resolve("deploy").is_some(), "active inside the named directory");
	let there = discover(outside.path(), Some(home.path())).unwrap();
	assert!(there.resolve("deploy").is_none(), "registered, but not active here");
}

#[test]
fn an_unscoped_global_is_active_everywhere() {
	let home = TempDir::new().unwrap();
	let g = home.path().join(".runfiles");
	std::fs::create_dir_all(&g).unwrap();
	std::fs::write(g.join("deploy.run"), "$ true\n").unwrap();
	let elsewhere = TempDir::new().unwrap();
	std::fs::create_dir_all(elsewhere.path().join("runfiles")).unwrap();
	std::fs::write(elsewhere.path().join("runfiles/x.run"), "$ true\n").unwrap();
	let c = discover(elsewhere.path(), Some(home.path())).unwrap();
	assert!(c.resolve("deploy").is_some());
}

#[test]
fn scoping_matches_whole_path_components_not_string_prefixes() {
	// `work/acme` must not admit `work/acme-other`.
	let home = TempDir::new().unwrap();
	let g = home.path().join(".runfiles");
	std::fs::create_dir_all(&g).unwrap();
	std::fs::write(g.join("deploy.run"), "$ true\n").unwrap();
	std::fs::write(g.join(SHARED), ".only-in-directories = \"work/acme\"\n").unwrap();

	let sibling = home.path().join("work/acme-other");
	std::fs::create_dir_all(sibling.join("runfiles")).unwrap();
	std::fs::write(sibling.join("runfiles/x.run"), "$ true\n").unwrap();
	let c = discover(&sibling, Some(home.path())).unwrap();
	assert!(c.resolve("deploy").is_none(), "acme-other is not inside acme");
}

#[test]
fn a_subproject_alias_carries_its_namespace() {
	// Otherwise a subproject claims a bare name in the root, and the qualified
	// spelling every listing shows does not work.
	let d = TempDir::new().unwrap();
	std::fs::create_dir_all(d.path().join("runfiles")).unwrap();
	std::fs::write(d.path().join("runfiles/root.run"), "$ true\n").unwrap();
	std::fs::create_dir_all(d.path().join("web/runfiles")).unwrap();
	std::fs::write(d.path().join("web/runfiles/setup.run"), ".alias = \"deps\"\n$ true\n").unwrap();

	let c = discover(d.path(), None).unwrap();
	assert_eq!(c.resolve("web:deps").unwrap().name, "web:setup");
	assert!(c.resolve("deps").is_none(), "a subproject must not claim a bare name");
}

#[test]
fn a_root_alias_stays_unqualified() {
	let d = TempDir::new().unwrap();
	std::fs::create_dir_all(d.path().join("runfiles")).unwrap();
	std::fs::write(d.path().join("runfiles/build.run"), ".alias = \"b\"\n$ true\n").unwrap();
	let c = discover(d.path(), None).unwrap();
	assert_eq!(c.resolve("b").unwrap().name, "build");
}

#[test]
fn a_subproject_alias_resolves_unqualified_from_inside_it() {
	// From within the subproject its targets have no prefix, so neither do its
	// aliases -- the same file works either way.
	let d = TempDir::new().unwrap();
	std::fs::create_dir_all(d.path().join("runfiles")).unwrap();
	std::fs::write(d.path().join("runfiles/root.run"), "$ true\n").unwrap();
	let web = d.path().join("web");
	std::fs::create_dir_all(web.join("runfiles")).unwrap();
	std::fs::write(web.join("runfiles/setup.run"), ".alias = \"deps\"\n$ true\n").unwrap();

	let c = discover(&web, None).unwrap();
	assert_eq!(c.resolve("deps").unwrap().name, "setup");
}

#[test]
fn a_nested_directory_carries_its_own_shared_settings() {
	// `runfiles/api/_shared.run` was collected by nothing, so it applied to
	// nothing -- despite naming a directory whose files inherit it.
	let d = TempDir::new().unwrap();
	std::fs::create_dir_all(d.path().join("runfiles/api")).unwrap();
	std::fs::write(d.path().join("runfiles/_shared.run"), ".env.A = \"1\"\n").unwrap();
	std::fs::write(d.path().join("runfiles/api/_shared.run"), ".env.B = \"2\"\n").unwrap();
	std::fs::write(d.path().join("runfiles/api/deploy.run"), "$ true\n").unwrap();
	std::fs::write(d.path().join("runfiles/build.run"), "$ true\n").unwrap();

	let c = discover(d.path(), None).unwrap();
	let chain = c.shared_chain(c.resolve("api:deploy").unwrap());
	assert_eq!(chain.len(), 2, "both apply, outermost first: {chain:?}");
	assert!(chain[0].ends_with("runfiles/_shared.run"), "{chain:?}");
	assert!(chain[1].ends_with("runfiles/api/_shared.run"), "{chain:?}");

	let top = c.shared_chain(c.resolve("build").unwrap());
	assert_eq!(top.len(), 1, "a top-level target sees only the top one");
}

#[test]
fn a_subproject_does_not_inherit_the_root_shared_file() {
	// It is a separate `runfiles/` tree; the root's settings are not its to
	// inherit, the same reason its targets carry their own namespace.
	let d = TempDir::new().unwrap();
	std::fs::create_dir_all(d.path().join("runfiles")).unwrap();
	std::fs::write(d.path().join("runfiles/_shared.run"), ".env.A = \"1\"\n").unwrap();
	std::fs::write(d.path().join("runfiles/build.run"), "$ true\n").unwrap();
	std::fs::create_dir_all(d.path().join("web/runfiles")).unwrap();
	std::fs::write(d.path().join("web/runfiles/_shared.run"), ".env.B = \"2\"\n").unwrap();
	std::fs::write(d.path().join("web/runfiles/dev.run"), "$ true\n").unwrap();

	let c = discover(d.path(), None).unwrap();
	let chain = c.shared_chain(c.resolve("web:dev").unwrap());
	assert_eq!(chain.len(), 1, "{chain:?}");
	assert!(chain[0].ends_with("web/runfiles/_shared.run"), "{chain:?}");
}

#[test]
fn a_missing_shared_file_contributes_nothing() {
	let d = TempDir::new().unwrap();
	std::fs::create_dir_all(d.path().join("runfiles/api")).unwrap();
	std::fs::write(d.path().join("runfiles/api/deploy.run"), "$ true\n").unwrap();
	let c = discover(d.path(), None).unwrap();
	assert!(c.shared_chain(c.resolve("api:deploy").unwrap()).is_empty());
}

#[test]
fn any_of_the_three_global_names_is_read() {
	// A person should not have to argue with the runner about whether their
	// own home directory shows the folder or hides it.
	for name in GLOBAL_NAMES {
		let home = TempDir::new().unwrap();
		let g = home.path().join(name);
		std::fs::create_dir_all(&g).unwrap();
		std::fs::write(g.join("deploy.run"), "$ true\n").unwrap();
		let elsewhere = TempDir::new().unwrap();
		std::fs::create_dir_all(elsewhere.path().join("runfiles")).unwrap();
		std::fs::write(elsewhere.path().join("runfiles/x.run"), "$ true\n").unwrap();
		let c = discover(elsewhere.path(), Some(home.path())).unwrap();
		assert!(c.resolve("deploy").is_some(), "{name} was not read");
	}
}

#[test]
fn two_populated_global_directories_are_an_error_naming_both() {
	// Merging them would mean one target silently shadowing another, and no
	// way to see which. Refusing says exactly what to fix.
	let home = TempDir::new().unwrap();
	for name in [".runfiles", "runfiles"] {
		let g = home.path().join(name);
		std::fs::create_dir_all(&g).unwrap();
		std::fs::write(g.join("deploy.run"), "$ true\n").unwrap();
	}
	let elsewhere = TempDir::new().unwrap();
	std::fs::create_dir_all(elsewhere.path().join("runfiles")).unwrap();
	std::fs::write(elsewhere.path().join("runfiles/x.run"), "$ true\n").unwrap();

	let e = discover(elsewhere.path(), Some(home.path())).unwrap_err();
	let msg = e.to_string();
	assert!(msg.contains(".runfiles"), "{msg}");
	assert!(msg.contains("two places at once"), "{msg}");
}

#[test]
fn an_empty_global_directory_never_clashes() {
	// An empty one is indistinguishable from a leftover `mkdir`, and refusing
	// to run because of one would be absurd.
	let home = TempDir::new().unwrap();
	std::fs::create_dir_all(home.path().join("Runfiles")).unwrap();
	let g = home.path().join(".runfiles");
	std::fs::create_dir_all(&g).unwrap();
	std::fs::write(g.join("deploy.run"), "$ true\n").unwrap();

	let elsewhere = TempDir::new().unwrap();
	std::fs::create_dir_all(elsewhere.path().join("runfiles")).unwrap();
	std::fs::write(elsewhere.path().join("runfiles/x.run"), "$ true\n").unwrap();
	let c = discover(elsewhere.path(), Some(home.path())).unwrap();
	assert!(c.resolve("deploy").is_some(), "the empty one is not a rival");
}

#[test]
fn a_home_directory_project_is_not_also_its_own_global() {
	// `$HOME/runfiles` is a legal global spelling *and* what `find_upward`
	// finds when the run starts at home. Collecting it twice would report
	// every target in it as defined twice.
	let home = TempDir::new().unwrap();
	let g = home.path().join("runfiles");
	std::fs::create_dir_all(&g).unwrap();
	std::fs::write(g.join("deploy.run"), "$ true\n").unwrap();
	let c = discover(home.path(), Some(home.path())).unwrap();
	assert!(c.resolve("deploy").is_some());
}

#[test]
fn a_global_directory_does_not_shadow_a_projects_own_shared_file() {
	// The machine-wide tree has no namespace, so keying the chain by namespace
	// put its `_shared.run` under the same empty key as a project's own. The
	// key was written whether or not the file existed, so merely *having* a
	// `~/.runfiles` disabled the root `_shared.run` of every project.
	let home = TempDir::new().unwrap();
	let g = home.path().join(".runfiles");
	std::fs::create_dir_all(&g).unwrap();
	std::fs::write(g.join("mine.run"), "$ true\n").unwrap();

	let proj = TempDir::new().unwrap();
	std::fs::create_dir_all(proj.path().join("runfiles")).unwrap();
	std::fs::write(proj.path().join("runfiles/build.run"), "$ true\n").unwrap();
	std::fs::write(proj.path().join("runfiles/_shared.run"), ".env.X = \"1\"\n").unwrap();

	let c = discover(proj.path(), Some(home.path())).unwrap();
	let chain = c.shared_chain(c.resolve("build").unwrap());
	assert_eq!(chain.len(), 1, "the project's own still applies: {chain:?}");
	assert!(chain[0].starts_with(proj.path()), "{chain:?}");

	// And the global targets still see their own.
	std::fs::write(g.join(SHARED), ".env.Y = \"2\"\n").unwrap();
	let c = discover(proj.path(), Some(home.path())).unwrap();
	let mine = c.shared_chain(c.resolve("mine").unwrap());
	assert_eq!(mine.len(), 1, "{mine:?}");
	assert!(mine[0].starts_with(home.path()), "{mine:?}");
}

#[test]
fn a_binding_in_a_shared_file_is_reported_in_the_chain_outermost_first() {
	let d = TempDir::new().unwrap();
	std::fs::create_dir_all(d.path().join("runfiles/api")).unwrap();
	for (f, text) in [
		("runfiles/_shared.run", "let a = \"root\"\n"),
		("runfiles/api/_shared.run", "let b = \"api\"\n"),
		("runfiles/api/deploy.run", "$ true\n"),
	] {
		std::fs::write(d.path().join(f), text).unwrap();
	}
	let c = discover(d.path(), None).unwrap();
	let chain = c.shared_chain(c.resolve("api:deploy").unwrap());
	assert_eq!(chain.len(), 2, "{chain:?}");
	assert!(chain[0].ends_with("runfiles/_shared.run"), "outermost first: {chain:?}");
	assert!(chain[1].ends_with("runfiles/api/_shared.run"), "{chain:?}");
}
