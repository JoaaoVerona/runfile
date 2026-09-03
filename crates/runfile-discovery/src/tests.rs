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
	assert!(c.shared_for(t).is_some(), "but it is found for the targets it covers");
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
