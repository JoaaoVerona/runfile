//! `run :update` — self-update by re-running the published install script.
//!
//! Rather than re-implement download / unpack / binary-swap logic in Rust,
//! `:update` runs the install script a release ships, scoped (via
//! `RUNFILE_INSTALL_DIR`) to the directory the running binary already lives in.
//! This means there is a single source of truth for "how runfile gets onto a
//! machine" — the install scripts — and the updater inherits every
//! platform/arch detail they already handle.
//!
//! What the updater adds is knowing what it is doing. It asks the channel for
//! its newest release *before* downloading anything, so it can say there is
//! nothing to do, decline to downgrade by accident, and install exactly the
//! version it named; afterwards it runs the new binary, so the version it
//! reports is the one that landed rather than the one it asked for.
//!
//! Releases come from one of two [`Channel`]s: Gitea, which cuts every release
//! and is the default, and the GitHub mirror, which releases what it is pushed.
//!
//! npm-managed installs are handed to npm: their binary lives inside
//! `node_modules` and is owned by the package manager, so the right update path
//! is `npm install -g @runfile/cli@latest`, not a sideways overwrite.

use crate::help::{Row, Section};
use std::cmp::Ordering;
use std::path::Path;
use std::process::{Command, ExitCode, Output, Stdio};

/// Where a release is downloaded from.
///
/// Both hosts answer `<releases>/latest` with a redirect to the newest
/// release's `/releases/tag/<tag>` page, and both serve a named release's
/// assets at `<releases>/download/<tag>/<asset>`. Those two are all the updater
/// and the installers use, so a channel is one URL. The hosts' own `latest`
/// download aliases would not do: Gitea's is `/releases/download/latest/<asset>`
/// and GitHub's `/releases/latest/download/<asset>`, the segments swapped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Channel {
	/// git.joaoverona.com, which builds every release.
	Gitea,
	/// The GitHub mirror, which releases whatever `run mirror` last pushed it.
	GitHub,
}

impl Channel {
	pub(crate) const ALL: [Channel; 2] = [Channel::Gitea, Channel::GitHub];

	/// The name `--channel` and the installers' `RUNFILE_CHANNEL` take.
	pub(crate) fn name(self) -> &'static str {
		match self {
			Channel::Gitea => "gitea",
			Channel::GitHub => "github",
		}
	}

	pub(crate) fn host(self) -> &'static str {
		match self {
			Channel::Gitea => "git.joaoverona.com",
			Channel::GitHub => "github.com",
		}
	}

	/// The repository's releases, which every other URL here is built on.
	pub(crate) fn releases(self) -> &'static str {
		match self {
			Channel::Gitea => "https://git.joaoverona.com/joaaoverona/runfile/releases",
			Channel::GitHub => "https://github.com/JoaaoVerona/runfile/releases",
		}
	}

	fn named(name: &str) -> Option<Channel> {
		Channel::ALL.into_iter().find(|c| c.name() == name)
	}

	/// A named release's copy of `asset`.
	fn asset(self, tag: &str, asset: &str) -> String {
		format!("{}/download/{tag}/{asset}", self.releases())
	}
}

const INTRO: &str = "run :update [version]   —   replace this binary with a published release";

const SECTIONS: &[Section] = &[
	Section(
		"Arguments",
		&[Row(
			"version",
			"a release tag such as v1.2.0; default is the newest release",
		)],
	),
	Section(
		"Flags",
		&[
			Row(
				"--channel <name>",
				"gitea, where every release is cut (default), or github, the mirror",
			),
			Row(
				"--force",
				"install even when this binary is already that version, or newer",
			),
		],
	),
];

pub(crate) fn usage() -> String {
	crate::help::render(INTRO, SECTIONS)
}

const BINARY: &str = if cfg!(windows) { "run.exe" } else { "run" };
const NULL_DEVICE: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };

/// What `run :update …` asked for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Request {
	/// A release tag, always with its leading `v`.
	pub(crate) version: Option<String>,
	pub(crate) channel: Channel,
	/// Whether `--channel` was written, which an npm install cannot honour.
	pub(crate) channel_given: bool,
	pub(crate) force: bool,
}

pub(crate) fn parse_args(args: &[String]) -> Result<Request, String> {
	let mut req = Request {
		version: None,
		channel: Channel::Gitea,
		channel_given: false,
		force: false,
	};
	let names = || Channel::ALL.map(Channel::name).join(" or ");
	let mut words = args.iter();
	while let Some(word) = words.next() {
		let channel = match word.strip_prefix("--channel") {
			Some("") => Some(
				words
					.next()
					.ok_or_else(|| format!("`--channel` needs a name: {}", names()))?
					.as_str(),
			),
			Some(rest) => rest.strip_prefix('='),
			None => None,
		};
		if let Some(name) = channel {
			req.channel = Channel::named(name).ok_or_else(|| format!("unknown channel `{name}`; it is {}", names()))?;
			req.channel_given = true;
		} else if word == "--force" {
			req.force = true;
		} else if word.starts_with('-') {
			return Err(format!("unknown flag `{word}`; see `run :update --help`"));
		} else if let Some(first) = &req.version {
			return Err(format!("one version at a time: got `{first}` and `{word}`"));
		} else {
			req.version = Some(release_tag(word)?);
		}
	}
	Ok(req)
}

/// `v1.2.0` as it is, and `1.2.0` as `v1.2.0`: every release is tagged with
/// the `v`, and the tag goes into a URL, so the bare form would ask for a
/// release that does not exist.
fn release_tag(version: &str) -> Result<String, String> {
	if !is_valid_version_tag(version) {
		return Err(format!(
			"invalid version {version:?}: a release tag starts with a letter or digit and contains only letters, \
			 digits, '.', '-' and '_' (e.g. v1.2.0)"
		));
	}
	Ok(if version.starts_with(|c: char| c.is_ascii_digit()) {
		format!("v{version}")
	} else {
		version.to_string()
	})
}

/// How the running binary was installed — determines whether `:update` can
/// manage it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum InstallKind {
	/// Binary lives inside a `node_modules` tree → owned by a JS package
	/// manager. We refuse and defer to it.
	Npm,
	/// Installed via the install scripts (or any other standalone copy) →
	/// safe to overwrite in place.
	Standalone,
}

/// Classify an install purely from the executable path. Pure (no I/O) so it's
/// unit-testable. The `node_modules` component is the signal that a JS package
/// manager owns the binary (the npm package extracts to
/// `.../node_modules/@runfile/cli/bin/<platform>/run`).
pub(crate) fn classify_install(exe: &Path) -> InstallKind {
	let in_node_modules = exe.components().any(|c| c.as_os_str() == "node_modules");
	if in_node_modules {
		InstallKind::Npm
	} else {
		InstallKind::Standalone
	}
}

/// Whether `v` is a safe release-tag string. The install scripts put the
/// version into a URL path and the Windows path hands it to PowerShell, so any
/// value outside this allow-list — shell metacharacters, whitespace, quotes,
/// `/`, `;`, `|`, `$`, backticks — is rejected. Release tags look like
/// `v0.35.0` / `0.35.0`, all covered by `[A-Za-z0-9._-]`.
///
/// It has to start with a letter or a digit, as the refusal says. A leading
/// `-` is how `run :update --bogus` was read as a request for a release called
/// `--bogus` and sent to the server, rather than being reported here; and a
/// leading `.` let `..` through, a dot-segment curl removes from the URL.
pub(crate) fn is_valid_version_tag(v: &str) -> bool {
	!v.is_empty()
		&& v.len() <= 64
		&& v.starts_with(|c: char| c.is_ascii_alphanumeric())
		&& v.chars()
			.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// A version's three numbers, then its pre-release suffix, if any.
type VersionKey<'a> = ([u64; 3], Option<&'a str>);

/// `1.2.3` or `v1.2.3-rc.1` as something to order by.
fn version_key(v: &str) -> Option<VersionKey<'_>> {
	let v = v.strip_prefix('v').unwrap_or(v);
	let (core, pre) = match v.split_once('-') {
		Some((core, pre)) => (core, Some(pre)),
		None => (v, None),
	};
	let mut n = core.split('.').map(|part| part.parse::<u64>().ok());
	let key = [n.next()??, n.next()??, n.next()??];
	n.next().is_none().then_some((key, pre))
}

/// Semver's order, near enough: numbers first, and a pre-release before the
/// release it leads up to. `None` when either side is not a version at all.
pub(crate) fn compare(a: &str, b: &str) -> Option<Ordering> {
	let (a_num, a_pre) = version_key(a)?;
	let (b_num, b_pre) = version_key(b)?;
	Some(a_num.cmp(&b_num).then(match (a_pre, b_pre) {
		(None, None) => Ordering::Equal,
		(None, Some(_)) => Ordering::Greater,
		(Some(_), None) => Ordering::Less,
		(Some(a), Some(b)) => a.cmp(b),
	}))
}

/// What an update will do, decided before anything is downloaded.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Plan {
	/// This binary is already the release asked for.
	UpToDate,
	/// This binary is newer than the channel's newest release — a build from
	/// source, or a mirror that has not been pushed yet. Installing would be a
	/// downgrade nobody asked for.
	Ahead,
	Upgrade,
	Reinstall,
	Downgrade,
}

/// `named` is whether the version was written rather than looked up: naming
/// one is asking for it, older or not.
pub(crate) fn plan(current: &str, target: &str, named: bool, force: bool) -> Plan {
	match compare(current, target) {
		// A tag that is not a version: install it, and let the check afterwards
		// say what landed.
		None | Some(Ordering::Less) => Plan::Upgrade,
		Some(Ordering::Equal) if force => Plan::Reinstall,
		Some(Ordering::Equal) => Plan::UpToDate,
		Some(Ordering::Greater) if named || force => Plan::Downgrade,
		Some(Ordering::Greater) => Plan::Ahead,
	}
}

pub fn cmd_update(args: &[String]) -> Result<ExitCode, String> {
	let req = parse_args(args)?;
	let current = env!("CARGO_PKG_VERSION");
	let exe = std::env::current_exe().map_err(|e| format!("could not locate the running executable: {e}"))?;

	if classify_install(&exe) == InstallKind::Npm {
		if req.channel_given {
			return Err(format!(
				"this run was installed with npm, which has one source; update it with: npm install -g {}",
				npm_package_spec(req.version.as_deref())
			));
		}
		return update_via_npm(&exe, req.version.as_deref());
	}

	let dir = exe
		.parent()
		.ok_or_else(|| format!("could not determine the install directory from {}", exe.display()))?;
	let channel = req.channel;
	let named = req.version.is_some();
	let tag = match req.version {
		Some(tag) => tag,
		None => {
			let tag = newest_release(channel)?;
			eprintln!("Newest release on {}: {tag}", channel.host());
			tag
		}
	};

	let (doing, done) = match plan(current, &tag, named, req.force) {
		Plan::UpToDate => {
			eprintln!("run v{current} is up to date.");
			return Ok(ExitCode::SUCCESS);
		}
		Plan::Ahead => {
			let flag = match channel {
				Channel::Gitea => String::new(),
				other => format!(" --channel={}", other.name()),
			};
			eprintln!("run v{current} is newer than that, so there is nothing to update.");
			eprintln!("To install {tag} anyway: run :update {tag}{flag}");
			return Ok(ExitCode::SUCCESS);
		}
		Plan::Upgrade => ("Updating", "Updated"),
		Plan::Reinstall => ("Reinstalling", "Reinstalled"),
		Plan::Downgrade => ("Downgrading", "Downgraded"),
	};

	let change = |to: &str| match compare(current, to) {
		Some(Ordering::Equal) => format!("run v{current}"),
		_ => format!("run v{current} → v{}", to.trim_start_matches('v')),
	};
	eprintln!(
		"{doing} {} in {}, from {}...",
		change(&tag),
		dir.display(),
		channel.host()
	);

	run_installer(channel, &tag, dir)?;

	// The installer always writes `run`, whatever this binary is called.
	let bin = dir.join(BINARY);
	let installed = binary_version(&bin)?;
	if compare(&installed, &tag) != Some(Ordering::Equal) {
		return Err(format!(
			"the installer finished, but {} reports v{installed} rather than {tag}",
			bin.display()
		));
	}

	#[cfg(windows)]
	schedule_old_deletion_at_reboot(&exe);

	eprintln!("{done} {} ({}).", change(&installed), channel.host());
	Ok(ExitCode::SUCCESS)
}

/// The channel's newest release, by following `<releases>/latest` to the tag
/// page it redirects to. Both hosts leave drafts and pre-releases out of it.
fn newest_release(channel: Channel) -> Result<String, String> {
	let latest = format!("{}/latest", channel.releases());
	let out = Command::new("curl")
		.args(["-fsSL", "-o", NULL_DEVICE, "-w", "%{url_effective}", &latest])
		.output()
		.map_err(|e| format!("could not run curl: {e}"))?;
	if !out.status.success() {
		return Err(format!("found no release on {}: {}", channel.host(), curl_error(&out)));
	}
	let page = String::from_utf8_lossy(&out.stdout);
	tag_of_page(&page).ok_or_else(|| format!("{latest} led to {page}, which is not a release"))
}

/// `…/releases/tag/v1.2.3` → `v1.2.3`.
fn tag_of_page(url: &str) -> Option<String> {
	let (_, tag) = url.trim().rsplit_once("/releases/tag/")?;
	is_valid_version_tag(tag).then(|| tag.to_string())
}

/// What a binary says it is, from its own `--version`: the one account that
/// cannot disagree with the file that was actually installed.
fn binary_version(bin: &Path) -> Result<String, String> {
	let out = Command::new(bin)
		.arg("--version")
		.output()
		.map_err(|e| format!("could not run {}: {e}", bin.display()))?;
	let said = String::from_utf8_lossy(&out.stdout);
	match said.trim().strip_prefix("run ") {
		Some(version) if out.status.success() => Ok(version.to_string()),
		_ => Err(format!("{} --version answered {:?}", bin.display(), said.trim())),
	}
}

/// curl's own account of a failure, which names the status or the host.
fn curl_error(out: &Output) -> String {
	let why = String::from_utf8_lossy(&out.stderr).trim().to_string();
	if why.is_empty() {
		format!("curl exited with {}", out.status)
	} else {
		why
	}
}

/// Schedule every `<exe>.old-<guid>` aside-file (a previous binary renamed
/// aside by `install.ps1` because it was the running process) for deletion at
/// the next reboot via `MoveFileExW(.., NULL, MOVEFILE_DELAY_UNTIL_REBOOT)`.
///
/// `install.ps1` renames the running binary to a GUID-suffixed name so the
/// rename can never collide with a still-locked leftover, which means there
/// can be more than one aside-file present — we scan the install dir and
/// schedule each.
///
/// The flag records the path in the registry for the Session Manager to delete
/// at boot, so it doesn't matter that the file is currently locked. Best-effort
/// only: the underlying registry write requires administrator rights, so on the
/// common per-user install this silently no-ops — `install.ps1` sweeps any
/// stale aside-files on the next update regardless, so nothing is orphaned
/// permanently in practice.
#[cfg(windows)]
fn schedule_old_deletion_at_reboot(exe: &Path) {
	use std::os::windows::ffi::OsStrExt;
	use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW};

	let (Some(dir), Some(name)) = (exe.parent(), exe.file_name().and_then(|n| n.to_str())) else {
		return;
	};
	let prefix = format!("{name}.old");

	let Ok(entries) = std::fs::read_dir(dir) else {
		return;
	};
	for entry in entries.flatten() {
		let fname = entry.file_name();
		let Some(fname) = fname.to_str() else { continue };
		if !fname.starts_with(&prefix) {
			continue;
		}

		let wide: Vec<u16> = entry
			.path()
			.as_os_str()
			.encode_wide()
			.chain(std::iter::once(0))
			.collect();

		// SAFETY: `wide` is a NUL-terminated UTF-16 string living for the
		// duration of the call; a null destination requests deletion rather
		// than a move. The call only registers a pending operation and returns
		// immediately. A zero return means failure (typically
		// ERROR_ACCESS_DENIED without admin), which we intentionally ignore —
		// the next-update sweep is the fallback.
		unsafe {
			MoveFileExW(wide.as_ptr(), std::ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT);
		}
	}
}

/// Build the `npm install -g` package spec. npm uses bare semver, but our
/// release tags carry a leading `v` — strip it so `--version v0.19.0` and
/// `--version 0.19.0` both map to `@runfile/cli@0.19.0`. `None` → `@latest`.
pub(crate) fn npm_package_spec(version: Option<&str>) -> String {
	match version {
		Some(v) => format!("@runfile/cli@{}", v.trim_start_matches('v')),
		None => "@runfile/cli@latest".to_string(),
	}
}

/// Update an npm-managed install.
///
/// On Unix we run `npm install -g <spec>` directly — replacing a running
/// binary's file is fine there. On Windows we can't: npm would have to
/// overwrite the currently-running `run.exe` (locked by this process), and
/// since npm owns the extraction we can't apply the rename-aside trick the
/// standalone path uses — a failed overwrite mid-reify can corrupt the global
/// package. So on Windows we print the command for the user to run in a fresh
/// shell, where no `run.exe` is executing.
fn update_via_npm(exe: &Path, version: Option<&str>) -> Result<ExitCode, String> {
	let spec = npm_package_spec(version);

	#[cfg(windows)]
	{
		let _ = exe;
		eprintln!("run is managed by npm. Update it from a fresh shell with:");
		eprintln!();
		eprintln!("  npm install -g {spec}");
		eprintln!();
		eprintln!("(Auto-update isn't safe here: npm must overwrite the running run.exe, which Windows locks.)");
		Ok(ExitCode::FAILURE)
	}

	#[cfg(not(windows))]
	{
		let current = env!("CARGO_PKG_VERSION");
		eprintln!("run v{current} is managed by npm; updating it with: npm install -g {spec}");
		let status = Command::new("npm")
			.args(["install", "-g", &spec])
			.status()
			.map_err(|e| format!("could not run npm ({e}); update with: npm install -g {spec}"))?;
		if !status.success() {
			return Err(format!("`npm install -g {spec}` exited with {status}"));
		}
		// npm replaced the package in place, so the path this binary ran from
		// now holds whatever it installed.
		let installed = binary_version(exe)?;
		match compare(current, &installed) {
			Some(Ordering::Equal) => eprintln!("run v{current} is up to date (npm)."),
			_ => eprintln!("Updated run v{current} → v{installed} (npm)."),
		}
		Ok(ExitCode::SUCCESS)
	}
}

/// Run the install script `tag` shipped with, for `tag`, into `dir`.
///
/// Its standard output is kept back unless it fails. It reports downloading
/// and installing, which `cmd_update` says itself with both versions in hand,
/// and suggests putting the directory on PATH, which is moot for a binary that
/// was just run from there. Its errors reach the terminal as they happen.
fn run_installer(channel: Channel, tag: &str, dir: &Path) -> Result<(), String> {
	#[cfg(windows)]
	let out = {
		// Fetch the install script and run it in-process via `iex` (bypasses
		// execution policy, like the documented installer). Two PS 5.1 gotchas
		// are handled explicitly:
		//   1. `-UseBasicParsing` — without it, Windows PowerShell pipes the
		//      response through the legacy IE DOM engine, which can HANG.
		//   2. `.Content` is a `byte[]` for any response not typed as text —
		//      GitHub serves release assets as application/octet-stream, and
		//      Gitea's text/plain is its own choice to keep — so we decode UTF-8
		//      before `iex`, otherwise the bytes stringify to "36 69 114 ..."
		//      and won't parse.
		// The version is pinned via the RUNFILE_VERSION env var (install.ps1
		// reads it) since this form can't pass positional args; the dir is
		// scoped via RUNFILE_INSTALL_DIR.
		let url = channel.asset(tag, "install.ps1");
		let ps_cmd = format!(
			"$c=(iwr '{url}' -UseBasicParsing).Content; \
			 if($c -is [byte[]]){{$c=[Text.Encoding]::UTF8.GetString($c)}}; iex $c"
		);
		Command::new("powershell")
			.args(["-NoProfile", "-Command", &ps_cmd])
			.env("RUNFILE_INSTALL_DIR", dir)
			.env("RUNFILE_VERSION", tag)
			.env("RUNFILE_CHANNEL", channel.name())
			.stderr(Stdio::inherit())
			.output()
			.map_err(|e| format!("could not run PowerShell: {e}"))?
	};

	#[cfg(not(windows))]
	let out = {
		use std::io::Write;

		let script = fetch(&channel.asset(tag, "install.sh"))?;

		// `sh -s -- <tag>` reads the script from stdin and hands it its
		// positional args. The tag is an argument rather than text in a
		// command line, so nothing in it is ever read by a shell.
		let mut sh = Command::new("sh")
			.args(["-s", "--", tag])
			.env("RUNFILE_INSTALL_DIR", dir)
			.env("RUNFILE_CHANNEL", channel.name())
			.stdin(Stdio::piped())
			.stdout(Stdio::piped())
			.spawn()
			.map_err(|e| format!("could not run sh: {e}"))?;
		// A write that fails means `sh` has already exited, and its status
		// says why; a script cut short that still exits 0 is caught by the
		// version check after it.
		let _ = sh.stdin.take().expect("stdin is piped").write_all(&script);
		sh.wait_with_output().map_err(|e| format!("could not run sh: {e}"))?
	};

	if out.status.success() {
		return Ok(());
	}
	let said = String::from_utf8_lossy(&out.stdout);
	Err(match said.trim() {
		"" => format!("the install script exited with {}", out.status),
		said => format!("the install script exited with {}:\n{said}", out.status),
	})
}

/// Download `url` whole, failing if the server did not answer with it.
///
/// Downloaded first and handed to `sh` afterwards, rather than piped as the
/// README's `curl … | sh` does. A pipeline's status is its last command's, and
/// `sh` given an empty script succeeds — so a download that failed (a 404, a
/// private repository, no network) was reported as a successful update that had
/// changed nothing. A person at a terminal sees curl's error; `:update` is the
/// one caller that has to act on it.
#[cfg(not(windows))]
fn fetch(url: &str) -> Result<Vec<u8>, String> {
	let out = Command::new("curl")
		.args(["-fsSL", url])
		.output()
		.map_err(|e| format!("could not run curl: {e}"))?;
	if out.status.success() {
		Ok(out.stdout)
	} else {
		Err(format!("could not download {url}: {}", curl_error(&out)))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn repo_file(path: &str) -> String {
		let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
		std::fs::read_to_string(root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
	}

	fn args(words: &[&str]) -> Result<Request, String> {
		parse_args(&words.iter().map(|w| w.to_string()).collect::<Vec<_>>())
	}

	#[test]
	fn a_release_is_downloaded_by_its_tag_in_the_shape_both_hosts_share() {
		assert_eq!(
			Channel::Gitea.asset("v1.2.3", "install.sh"),
			"https://git.joaoverona.com/joaaoverona/runfile/releases/download/v1.2.3/install.sh"
		);
		assert_eq!(
			Channel::GitHub.asset("v1.2.3", "install.sh"),
			"https://github.com/JoaaoVerona/runfile/releases/download/v1.2.3/install.sh"
		);
	}

	#[test]
	fn the_newest_release_is_read_off_where_latest_redirects() {
		// The pages both hosts redirect `/releases/latest` to, as they answered.
		assert_eq!(
			tag_of_page("https://git.joaoverona.com/joaaoverona/j18n/releases/tag/v0.14.1\n").as_deref(),
			Some("v0.14.1")
		);
		assert_eq!(
			tag_of_page("https://github.com/JoaaoVerona/runfile/releases/tag/v1.2.0").as_deref(),
			Some("v1.2.0")
		);
		// No redirect happened, so there is no tag to read.
		assert_eq!(
			tag_of_page("https://github.com/JoaaoVerona/runfile/releases/latest"),
			None
		);
	}

	#[test]
	fn the_readme_installs_from_the_channel_the_updater_defaults_to() {
		// Updating is installing again. A README pointing somewhere else would
		// install from one place and update from another. It uses Gitea's own
		// `latest` alias, which is fine for a person and a URL nothing here builds.
		let line = format!(
			"curl -fsSL {}/download/latest/install.sh | sh",
			Channel::Gitea.releases()
		);
		assert!(
			repo_file("README.md").contains(&line),
			"README.md does not document `{line}`"
		);
	}

	#[test]
	fn the_installers_know_the_same_channels_as_the_updater() {
		// The updater fetches a release's script and tells it the channel; the
		// script fetches the archive. Were the two to disagree about where a
		// channel lives, `--channel` would run one host's installer against the
		// other's archives.
		for script in ["install.sh", "install.ps1"] {
			let text = repo_file(&format!(".cicd/release-assets/{script}"));
			assert!(text.contains("RUNFILE_CHANNEL"), "{script} ignores the channel");
			for c in Channel::ALL {
				assert!(text.contains(c.releases()), "{script} does not know {}", c.releases());
				// A `case` label in sh, a `switch` label in PowerShell.
				let label = [format!("{})", c.name()), format!("'{}'", c.name())];
				assert!(label.iter().any(|l| text.contains(l)), "{script} has no `{}`", c.name());
			}
		}
	}

	#[test]
	fn arguments_name_a_version_a_channel_and_force() {
		assert_eq!(
			args(&[]).unwrap(),
			Request {
				version: None,
				channel: Channel::Gitea,
				channel_given: false,
				force: false,
			}
		);
		let r = args(&["1.2.0", "--channel=github", "--force"]).unwrap();
		assert_eq!(r.version.as_deref(), Some("v1.2.0"), "the tag carries its `v`");
		assert_eq!((r.channel, r.channel_given, r.force), (Channel::GitHub, true, true));
		assert_eq!(args(&["--channel", "github"]).unwrap().channel, Channel::GitHub);
		assert!(args(&["--channel=gitea"]).unwrap().channel_given);
	}

	#[test]
	fn a_mistaken_argument_is_refused_before_anything_is_downloaded() {
		let err = |words: &[&str]| args(words).unwrap_err();
		assert!(
			err(&["--channel=gitlab"]).contains("gitea or github"),
			"names the channels"
		);
		assert!(err(&["--channel"]).contains("needs a name"));
		assert!(err(&["--bogus"]).contains("unknown flag `--bogus`"));
		assert!(err(&["v1", "v2"]).contains("one version at a time"));
		assert!(err(&["v1;id"]).contains("invalid version"));
	}

	#[test]
	fn versions_order_by_number_and_a_pre_release_comes_first() {
		assert_eq!(compare("1.2.0", "v1.2.0"), Some(Ordering::Equal));
		assert_eq!(compare("1.9.0", "v1.10.0"), Some(Ordering::Less), "numbers, not text");
		assert_eq!(compare("v1.3.0-rc.1", "v1.3.0"), Some(Ordering::Less));
		assert_eq!(compare("v1.3.0-rc.2", "v1.3.0-rc.1"), Some(Ordering::Greater));
		assert_eq!(compare("1.2.0", "nightly"), None);
	}

	#[test]
	fn an_update_is_decided_before_anything_is_downloaded() {
		// current, target, named, force
		assert_eq!(plan("1.2.0", "v1.2.3", false, false), Plan::Upgrade);
		assert_eq!(plan("1.2.3", "v1.2.3", false, false), Plan::UpToDate);
		assert_eq!(plan("1.2.3", "v1.2.3", false, true), Plan::Reinstall);
		// Newer than the channel's newest: a source build, or a mirror that has
		// not been pushed. Only asking for the version, or forcing, goes back.
		assert_eq!(plan("1.3.0", "v1.2.3", false, false), Plan::Ahead);
		assert_eq!(plan("1.3.0", "v1.2.3", true, false), Plan::Downgrade);
		assert_eq!(plan("1.3.0", "v1.2.3", false, true), Plan::Downgrade);
		assert_eq!(plan("1.2.0", "nightly", true, false), Plan::Upgrade);
	}

	#[test]
	fn a_version_that_reads_as_a_flag_is_refused() {
		assert!(!is_valid_version_tag("--help"));
		assert!(!is_valid_version_tag("-v"));
		assert!(is_valid_version_tag("v1.2.0"));
		assert!(is_valid_version_tag("1.2.0-rc.1"));
	}

	#[test]
	fn a_version_carrying_shell_or_path_syntax_is_refused() {
		for v in [
			"",
			"v1; rm -rf ~",
			"$(id)",
			"v1 v2",
			"../v1",
			"v1/x",
			"`id`",
			"..",
			".",
			"_v1",
		] {
			assert!(!is_valid_version_tag(v), "{v:?}");
		}
	}

	#[cfg(not(windows))]
	#[test]
	fn a_download_that_fails_is_an_error_rather_than_an_empty_script() {
		// A `file:` URL fails the same way a 404 does, with no network needed.
		if Command::new("curl").arg("--version").output().is_err() {
			return;
		}
		let url = "file:///nonexistent/runfile/install.sh";
		let err = fetch(url).expect_err("a missing file is not a script");
		assert!(err.contains(url), "{err}");
	}
}
