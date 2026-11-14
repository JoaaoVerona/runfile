//! `run :fetch <package>...` -- fetch packages into the cache now, and ask
//! the host about each tag or branch whatever its age.
//!
//! A run fetches what it needs on its own and asks about a tag or branch once a
//! week, so this is for the times waiting is the wrong answer: a branch that
//! moved today, a CI job filling its cache in a step of its own, or a machine
//! about to go offline. It also prints what each reference stands for, which is
//! the one place that question has an answer short of running the package.

use crate::help::{Row, Section};
use runfile_lang::reference::{self, Reference};
use runfile_remote::{Cache, Network};
use std::process::ExitCode;

const SECTIONS: &[Section] = &[
	Section(
		"Usage",
		&[Row(
			"run :fetch <package>...",
			"fetch each into the cache, asking the host what its tag or branch points at now",
		)],
	),
	Section(
		"Packages",
		&[
			Row("github.com/owner/repo@v1.0.0", "a tag"),
			Row(
				"gitlab.com/group/repo/tools@main",
				"a folder of a repository, at a branch",
			),
			Row("git.example.com/owner/repo@<commit>", "a commit, all 40 characters"),
		],
	),
];

pub(crate) fn usage() -> String {
	crate::help::render("run :fetch -- fetch packages into the cache", SECTIONS)
}

pub(crate) fn cmd_fetch(args: &[String]) -> Result<ExitCode, String> {
	if let Some(flag) = args.iter().find(|a| a.starts_with('-')) {
		return Err(format!("unknown flag `{flag}` for `:fetch`\n{}", usage()));
	}
	let cache = Cache::open_default().ok_or_else(|| {
		format!(
			"there is no cache directory to fetch into; set {} to one",
			runfile_remote::CACHE_DIR_ENV_VAR
		)
	})?;
	let say = |m: &str| eprintln!("{} {m}", runfile_runtime::exec::tag());
	for word in args {
		let r = match reference::parse(word) {
			Ok(Reference::Remote(r)) => r,
			Ok(Reference::Path(_)) => {
				return Err(format!("`{word}` is a folder on this machine, which needs no fetching"));
			}
			Err(e) if reference::is_reference(word) => return Err(e.0),
			Err(_) => {
				return Err(format!(
					"`{word}` is not a package: one is named by its host and path, as `github.com/owner/repo@v1.0.0`"
				));
			}
		};
		let pkg = cache.package(&r, Network::Refresh, &say).map_err(|e| e.0)?;
		let what = match pkg.version.as_str() {
			"" => runfile_remote::short(&pkg.commit).to_string(),
			tag => format!("{} (tag {tag})", runfile_remote::short(&pkg.commit)),
		};
		println!("{}: {what}", r.shown());
	}
	Ok(ExitCode::SUCCESS)
}
