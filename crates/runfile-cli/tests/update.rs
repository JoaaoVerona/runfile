//! `run :update` end to end, against a fake release server.
//!
//! A `curl` on PATH stands in for both hosts. It answers `<releases>/latest`
//! with the tag page a real host redirects to, serves this repository's own
//! install.sh as every release's installer, and serves an archive whose `run`
//! reports a version the test picks -- so the updater, the installer and the
//! check between them all run for real, and nothing leaves the machine. Every
//! update runs a copy of the binary in a scratch directory, since an update
//! replaces the file it runs from.
//!
//! Unix only: the fake is a shell script, and the Windows updater runs
//! install.ps1 instead.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Mutex, MutexGuard, PoisonError};
use tempfile::TempDir;

const CURRENT: &str = env!("CARGO_PKG_VERSION");

const FAKE_CURL: &str = r#"#!/bin/sh
echo "$*" >> "$FAKE_LOG"
out= write= url=
while [ $# -gt 0 ]; do
	case "$1" in
		-o) out=$2; shift 2 ;;
		-w) write=$2; shift 2 ;;
		-*) shift ;;
		*) url=$1; shift ;;
	esac
done
case "$url" in
	*/releases/latest) [ -n "$write" ] && printf '%s' "${url%/latest}/tag/$FAKE_TAG" ;;
	*/download/"$FAKE_TAG"/install.sh) cat "$FAKE_INSTALLER" ;;
	*/download/"$FAKE_TAG"/*.tar.xz) cp "$FAKE_ARCHIVE" "$out" ;;
	*) echo "curl: (22) The requested URL returned error: 404" >&2; exit 22 ;;
esac
"#;

/// The fake server: a directory holding the fake `curl` and the archive it
/// serves, and a log of every request made of it.
///
/// It holds [`SERIAL`] for as long as it lives, so the tests here take turns.
/// Each writes an executable and then runs it, and a fork from another test
/// thread in between inherits the file still open for writing -- the child
/// holds it until it execs, and running the file meanwhile fails with "Text
/// file busy". Not forking while one is being written is the only fix that is
/// not a retry.
struct Server {
	_serial: MutexGuard<'static, ()>,
	root: TempDir,
	copies: std::cell::Cell<usize>,
}

static SERIAL: Mutex<()> = Mutex::new(());

impl Server {
	/// `None` where this machine cannot build the archive -- no xz support in
	/// its tar -- which is a machine the installer could not run on either.
	fn new() -> Option<Server> {
		// A test that failed while holding it has nothing to leave behind.
		let serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
		let os = match std::env::consts::OS {
			"linux" => "unknown-linux-musl",
			"macos" => "apple-darwin",
			_ => return None,
		};
		let name = format!("runfile-cli-{}-{os}", std::env::consts::ARCH);
		let root = TempDir::new().unwrap();
		let stage = root.path().join("stage").join(&name);
		std::fs::create_dir_all(&stage).unwrap();
		executable(&stage.join("run"), "#!/bin/sh\necho \"run $FAKE_VERSION\"\n");
		let packed = Command::new("tar")
			.args(["-cJf", "archive.tar.xz", "-C", "stage", &name])
			.current_dir(root.path())
			.status()
			.unwrap();
		if !packed.success() {
			eprintln!("skipping: tar here cannot write .tar.xz");
			return None;
		}
		std::fs::create_dir(root.path().join("bin")).unwrap();
		executable(&root.path().join("bin/curl"), FAKE_CURL);
		Some(Server {
			_serial: serial,
			root,
			copies: std::cell::Cell::new(0),
		})
	}

	/// Run `:update` from a fresh copy of the binary, with the server
	/// answering `tag` as the newest release and its archive's `run`
	/// reporting `version`. Answers the output and where the copy lives.
	fn update(&self, tag: &str, version: &str, args: &[&str]) -> (Output, PathBuf) {
		self.copies.set(self.copies.get() + 1);
		let dir = self.root.path().join(format!("install-{}", self.copies.get()));
		std::fs::create_dir_all(&dir).unwrap();
		let bin = dir.join("run");
		std::fs::copy(env!("CARGO_BIN_EXE_run"), &bin).unwrap();
		let installer = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.cicd/release-assets/install.sh");
		let path = format!(
			"{}:{}",
			self.root.path().join("bin").display(),
			std::env::var("PATH").unwrap()
		);
		let out = Command::new(&bin)
			.arg(":update")
			.args(args)
			.env("PATH", path)
			.env("FAKE_LOG", self.root.path().join("log"))
			.env("FAKE_TAG", tag)
			.env("FAKE_VERSION", version)
			.env("FAKE_INSTALLER", installer)
			.env("FAKE_ARCHIVE", self.root.path().join("archive.tar.xz"))
			.env_remove("RUNFILE_CHANNEL")
			.output()
			.unwrap();
		(out, bin)
	}

	/// The `run` inside the archive the server hands out.
	fn archived(&self) -> Vec<u8> {
		let stage = self.root.path().join("stage");
		let dir = std::fs::read_dir(&stage).unwrap().next().unwrap().unwrap();
		std::fs::read(dir.path().join("run")).unwrap()
	}

	/// Every request made of the server so far, one per line.
	fn requests(&self) -> Vec<String> {
		std::fs::read_to_string(self.root.path().join("log"))
			.unwrap_or_default()
			.lines()
			.map(String::from)
			.collect()
	}
}

fn executable(path: &Path, body: &str) {
	std::fs::write(path, body).unwrap();
	std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn err(o: &Output) -> String {
	String::from_utf8_lossy(&o.stderr).into_owned()
}

const GITEA: &str = "https://git.joaoverona.com/joaaoverona/runfile/releases";
const GITHUB: &str = "https://github.com/JoaaoVerona/runfile/releases";

#[test]
fn an_update_installs_the_newest_release_by_its_tag_and_reports_what_landed() {
	let Some(s) = Server::new() else { return };
	let (o, bin) = s.update("v99.0.0", "99.0.0", &[]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(
		err(&o).contains("Newest release on git.joaoverona.com: v99.0.0"),
		"{}",
		err(&o)
	);
	assert!(
		err(&o).contains(&format!("Updated run v{CURRENT} → v99.0.0 (git.joaoverona.com).")),
		"{}",
		err(&o)
	);
	assert!(
		std::fs::read(&bin).unwrap() == s.archived(),
		"the file it ran from was replaced"
	);

	// Looked up once, then everything by the tag it found -- the installer
	// included, which is what the channel and the tag handed to it are for.
	let asked = s.requests().join("\n");
	assert!(asked.contains(&format!("{GITEA}/latest")), "{asked}");
	assert!(
		asked.contains(&format!("{GITEA}/download/v99.0.0/install.sh")),
		"{asked}"
	);
	assert!(
		asked.contains(&format!("{GITEA}/download/v99.0.0/runfile-cli-")),
		"{asked}"
	);
}

#[test]
fn nothing_newer_means_nothing_is_downloaded() {
	let Some(s) = Server::new() else { return };

	let (o, bin) = s.update(&format!("v{CURRENT}"), CURRENT, &[]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(
		err(&o).contains(&format!("run v{CURRENT} is up to date.")),
		"{}",
		err(&o)
	);
	assert_eq!(s.requests().len(), 1, "only the lookup: {:?}", s.requests());
	assert_eq!(
		std::fs::read(&bin).unwrap(),
		std::fs::read(env!("CARGO_BIN_EXE_run")).unwrap()
	);

	// Older than this binary: a source build, or a mirror not pushed yet.
	let (o, _) = s.update("v0.0.1", "0.0.1", &[]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(err(&o).contains("is newer than that"), "{}", err(&o));
	assert!(
		err(&o).contains("run :update v0.0.1"),
		"says how to go back anyway: {}",
		err(&o)
	);
	assert_eq!(s.requests().len(), 2, "only the lookups: {:?}", s.requests());
}

#[test]
fn the_github_channel_takes_everything_from_the_mirror() {
	let Some(s) = Server::new() else { return };
	let (o, _) = s.update("v99.0.0", "99.0.0", &["--channel=github"]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(err(&o).contains("Newest release on github.com: v99.0.0"), "{}", err(&o));
	assert!(err(&o).contains("(github.com)."), "{}", err(&o));
	let asked = s.requests().join("\n");
	assert!(!asked.contains(GITEA), "nothing from Gitea: {asked}");
	assert!(
		asked.contains(&format!("{GITHUB}/download/v99.0.0/runfile-cli-")),
		"{asked}"
	);
}

#[test]
fn a_named_version_is_installed_without_a_lookup_even_when_older() {
	let Some(s) = Server::new() else { return };
	let (o, bin) = s.update("v0.0.1", "0.0.1", &["0.0.1"]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(
		err(&o).contains(&format!("Downgraded run v{CURRENT} → v0.0.1")),
		"{}",
		err(&o)
	);
	assert!(
		std::fs::read(&bin).unwrap() == s.archived(),
		"the file it ran from was replaced"
	);
	assert!(!s.requests().join("\n").contains("/latest"), "{:?}", s.requests());
}

#[test]
fn a_release_that_is_not_there_is_an_error_and_changes_nothing() {
	let Some(s) = Server::new() else { return };
	let (o, bin) = s.update("v99.0.0", "99.0.0", &["v42.0.0"]);
	assert!(!o.status.success());
	assert!(
		err(&o).contains(&format!("could not download {GITEA}/download/v42.0.0/install.sh")),
		"{}",
		err(&o)
	);
	assert_eq!(
		std::fs::read(&bin).unwrap(),
		std::fs::read(env!("CARGO_BIN_EXE_run")).unwrap()
	);
}

#[test]
fn an_install_that_lands_another_version_is_an_error() {
	// The version reported is read off the new binary, not assumed from the
	// request, so an installer that put the wrong file in place says so.
	let Some(s) = Server::new() else { return };
	let (o, _) = s.update("v99.0.0", "98.0.0", &[]);
	assert!(!o.status.success());
	assert!(err(&o).contains("reports v98.0.0 rather than v99.0.0"), "{}", err(&o));
}
