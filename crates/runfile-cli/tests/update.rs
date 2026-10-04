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
//! The same server answers the two installers that reach it without `run`:
//! install.sh as `curl … | sh` runs it, and the setup action's install step,
//! taken out of action.yml and handed to bash the way a runner hands it.
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
	*/releases/latest/download/*.tar.xz) cp "$FAKE_ARCHIVE" "$out" ;;
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
	/// The OS half of the target triple the archive is named for.
	os: &'static str,
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
			os,
		})
	}

	/// Run `:update` from a fresh copy of the binary, with the server
	/// answering `tag` as the newest release and its archive's `run`
	/// reporting `version`. Answers the output and where the copy lives.
	fn update(&self, tag: &str, version: &str, args: &[&str]) -> (Output, PathBuf) {
		let bin = self.fresh("install").join("run");
		std::fs::copy(env!("CARGO_BIN_EXE_run"), &bin).unwrap();
		let out = Command::new(&bin)
			.arg(":update")
			.args(args)
			.env("PATH", self.path())
			.env("FAKE_LOG", self.root.path().join("log"))
			.env("FAKE_TAG", tag)
			.env("FAKE_VERSION", version)
			.env("FAKE_INSTALLER", repo("../../.cicd/release-assets/install.sh"))
			.env("FAKE_ARCHIVE", self.root.path().join("archive.tar.xz"))
			.env_remove("RUNFILE_CHANNEL")
			.output()
			.unwrap();
		(out, bin)
	}

	/// Run install.sh itself, with `args` after it and `env` around it, while
	/// the server answers `tag` as the newest release. Answers the output and
	/// the directory it installs into.
	fn install(&self, tag: &str, args: &[&str], env: &[(&str, &str)]) -> (Output, PathBuf) {
		let dir = self.fresh("install");
		let out = Command::new("sh")
			.arg(repo("../../.cicd/release-assets/install.sh"))
			.args(args)
			.env("PATH", self.path())
			.env("FAKE_LOG", self.root.path().join("log"))
			.env("FAKE_TAG", tag)
			.env("FAKE_ARCHIVE", self.root.path().join("archive.tar.xz"))
			.env("RUNFILE_INSTALL_DIR", &dir)
			.env_remove("RUNFILE_CHANNEL")
			.env_remove("RUNFILE_VERSION")
			.envs(env.iter().copied())
			.output()
			.unwrap();
		(out, dir)
	}

	/// Run the setup action's install step with `version` as its input, while
	/// the server has `tag` to give. Answers the output and the runner's temp
	/// directory, where the binary lands under `runfile-bin`.
	fn action(&self, tag: &str, version: &str) -> (Output, PathBuf) {
		let temp = self.fresh("runner");
		let target = format!("{}-{}", std::env::consts::ARCH, self.os);
		let out = Command::new("bash")
			.args(["--noprofile", "--norc", "-eo", "pipefail", "-c"])
			.arg(action_step("Install runfile"))
			.env("PATH", self.path())
			.env("FAKE_LOG", self.root.path().join("log"))
			.env("FAKE_TAG", tag)
			.env("FAKE_VERSION", "0.0.1")
			.env("FAKE_ARCHIVE", self.root.path().join("archive.tar.xz"))
			.env("VERSION", version)
			.env("TARGET", target)
			.env("EXT", "tar.xz")
			.env("RUNNER_TEMP", &temp)
			.env("GITHUB_PATH", temp.join("github-path"))
			// Where a runner unpacks the action: the repository at the ref, so
			// its Cargo.toml is this one.
			.env("GITHUB_ACTION_PATH", repo("../../.github/actions/setup"))
			.output()
			.unwrap();
		(out, temp)
	}

	/// A directory of its own for one run.
	fn fresh(&self, what: &str) -> PathBuf {
		self.copies.set(self.copies.get() + 1);
		let dir = self.root.path().join(format!("{what}-{}", self.copies.get()));
		std::fs::create_dir_all(&dir).unwrap();
		dir
	}

	/// PATH with the fake `curl` in front.
	fn path(&self) -> String {
		format!(
			"{}:{}",
			self.root.path().join("bin").display(),
			std::env::var("PATH").unwrap()
		)
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

/// A path in this repository, from the crate's manifest directory.
fn repo(rel: &str) -> PathBuf {
	Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// The script of the setup action's step called `name`, as a runner hands it
/// to bash: its `run: |` block, without the block's indentation. A block
/// scalar ends at the first line indented less than its first, blank lines
/// aside, which is all the YAML this needs.
fn action_step(name: &str) -> String {
	let text = std::fs::read_to_string(repo("../../.github/actions/setup/action.yml")).unwrap();
	let lines: Vec<&str> = text.lines().collect();
	let step = lines
		.iter()
		.position(|l| l.trim() == format!("- name: {name}"))
		.unwrap_or_else(|| panic!("action.yml has no step named {name:?}"));
	let run = step
		+ lines[step..]
			.iter()
			.position(|l| l.trim() == "run: |")
			.expect("the step has a `run: |` block");
	let indent = lines[run + 1].len() - lines[run + 1].trim_start().len();
	let script: Vec<&str> = lines[run + 1..]
		.iter()
		.take_while(|l| l.trim().is_empty() || l.len() - l.trim_start().len() >= indent)
		.map(|l| l.get(indent..).unwrap_or(""))
		.collect();
	let script = script.join("\n") + "\n";
	// The runner substitutes an expression before bash sees the script, so a
	// step holding one cannot be run as written here.
	assert!(!script.contains("${{"), "{script}");
	script
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

/// Versions that are not release tags, each refused before anything is asked
/// of a server: dot-segments that leave the repository, a tag with a path
/// after it, a newline that would end a log line and open a workflow command,
/// a word, two numbers, blanks, an upper-case `V`, an empty pre-release, and a
/// query and percent-escapes, which a URL reads as something else again.
const HOSTILE: &[&str] = &[
	"../../../../attacker/repo/releases/download/v1",
	"v1.2.3/../../../../attacker/repo/releases/download/v1",
	"v1.2.3\n::error::injected",
	"..",
	"nightly",
	"v1.2",
	" v1.2.3",
	"v1.2.3 ",
	"V1.2.3",
	"v1.2.3-",
	"v1.2.3?x=1",
	"v1.2.3%2F..%2F..",
];

// ------------------------------------------------------------------ install.sh

#[test]
fn install_sh_installs_the_version_runfile_version_names() {
	// The README's way to pin installed the newest release instead: the
	// script read its argument and nothing else (audit SA-036).
	let Some(s) = Server::new() else { return };
	let (o, dir) = s.install("v0.0.1", &[], &[("RUNFILE_VERSION", "0.0.1")]);
	assert!(o.status.success(), "{}", err(&o));
	let asked = s.requests().join("\n");
	assert!(!asked.contains("/latest"), "nothing looked up: {asked}");
	assert!(
		asked.contains(&format!("{GITEA}/download/v0.0.1/runfile-cli-")),
		"a bare version is its v-tag: {asked}"
	);
	assert!(
		asked.contains("--proto =https") && asked.contains("--path-as-is"),
		"{asked}"
	);
	assert!(std::fs::read(dir.join("run")).unwrap() == s.archived());

	// The argument still comes first.
	let (o, _) = s.install("v0.0.1", &["v0.0.1"], &[("RUNFILE_VERSION", "v9.9.9")]);
	assert!(o.status.success(), "{}", err(&o));
	assert!(!s.requests().join("\n").contains("v9.9.9"), "{:?}", s.requests());

	// A pre-release is a release tag too.
	let (o, _) = s.install("v1.3.0-rc.1", &["v1.3.0-rc.1"], &[]);
	assert!(o.status.success(), "{}", err(&o));
}

#[test]
fn install_sh_refuses_a_version_that_is_not_a_release_tag_before_asking_for_anything() {
	// The version goes into the URL's path, and curl removes dot-segments
	// before it sends, so the first of these fetched another repository's
	// archive (audit SA-032).
	let Some(s) = Server::new() else { return };
	for bad in HOSTILE {
		for (args, env) in [(vec![*bad], vec![]), (vec![], vec![("RUNFILE_VERSION", *bad)])] {
			let (o, dir) = s.install("v0.0.1", &args, &env);
			assert!(!o.status.success(), "{bad:?} was taken");
			assert!(err(&o).contains("invalid version"), "{bad:?}: {}", err(&o));
			assert!(
				err(&o).lines().all(|l| !l.starts_with("::")),
				"{bad:?} broke the line: {}",
				err(&o)
			);
			assert!(!dir.join("run").exists());
		}
	}
	assert!(s.requests().is_empty(), "nothing asked: {:?}", s.requests());
}

#[test]
fn install_sh_refuses_a_newest_release_that_is_not_a_tag() {
	// What the lookup names goes into the next URL too.
	let Some(s) = Server::new() else { return };
	let (o, dir) = s.install("nightly", &[], &[]);
	assert!(!o.status.success());
	assert!(err(&o).contains("which is not a release"), "{}", err(&o));
	assert_eq!(s.requests().len(), 1, "only the lookup: {:?}", s.requests());
	assert!(!dir.join("run").exists());
}

// ------------------------------------------------------------ the setup action

#[test]
fn the_action_installs_the_release_its_own_ref_belongs_to_unless_told_otherwise() {
	// Pinning the action to a tag or a SHA froze action.yml and nothing it
	// downloaded, since an empty `version` was `latest` (audit SA-036). Now it
	// is the version the action's own Cargo.toml names -- this checkout's.
	let Some(s) = Server::new() else { return };
	let tag = format!("v{CURRENT}");
	let (o, temp) = s.action(&tag, "");
	assert!(o.status.success(), "{}", err(&o));
	let asked = s.requests().join("\n");
	assert!(
		asked.contains(&format!("{GITHUB}/download/{tag}/runfile-cli-")),
		"{asked}"
	);
	assert!(!asked.contains("/latest"), "{asked}");
	assert!(
		asked.contains("--proto =https") && asked.contains("--path-as-is"),
		"{asked}"
	);
	assert!(
		std::fs::read(temp.join("runfile-bin/run")).unwrap() == s.archived(),
		"the archive's binary is what landed"
	);
	assert!(
		std::fs::read_to_string(temp.join("github-path"))
			.unwrap()
			.contains("runfile-bin"),
		"and it is on PATH for the steps after"
	);
	assert!(String::from_utf8_lossy(&o.stdout).contains("run 0.0.1"), "and it ran");

	// `latest` is the opt-in for the newest, through GitHub's own alias.
	let (o, _) = s.action(&tag, "latest");
	assert!(o.status.success(), "{}", err(&o));
	assert!(
		s.requests()
			.join("\n")
			.contains(&format!("{GITHUB}/latest/download/runfile-cli-")),
		"{:?}",
		s.requests()
	);

	// A bare version is its v-tag, and a pre-release is a release tag too.
	for (given, tag) in [("0.0.1", "v0.0.1"), ("v1.3.0-rc.1", "v1.3.0-rc.1")] {
		let (o, _) = s.action(tag, given);
		assert!(o.status.success(), "{given}: {}", err(&o));
		assert!(
			s.requests()
				.join("\n")
				.contains(&format!("{GITHUB}/download/{tag}/runfile-cli-")),
			"{given}: {:?}",
			s.requests()
		);
	}
}

#[test]
fn the_action_refuses_a_version_that_is_not_a_release_tag_before_downloading_anything() {
	// `version: ../../../../attacker/repo/releases/download/v1` fetched that
	// repository's archive, and the step then ran the `run` in it (audit
	// SA-032). A newline in a refused value must not reach the log as one: a
	// line opening `::` is a workflow command.
	let Some(s) = Server::new() else { return };
	for bad in HOSTILE {
		let (o, temp) = s.action("v0.0.1", bad);
		assert!(!o.status.success(), "{bad:?} was taken");
		assert!(err(&o).contains("invalid version"), "{bad:?}: {}", err(&o));
		assert!(
			err(&o).lines().all(|l| !l.starts_with("::")),
			"{bad:?} opened a workflow command: {}",
			err(&o)
		);
		assert!(!temp.join("runfile-bin").exists());
	}
	assert!(s.requests().is_empty(), "nothing asked: {:?}", s.requests());
}
