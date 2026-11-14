//! A `run` word that names a package rather than a target.
//!
//! `run github.com/JoaaoVerona/j18n@v0.14.1 sync` fetches that repository at
//! that version and runs its `runfiles/_main.run`; `run ../tools/j18n sync`
//! runs the one in that folder. Both are written where a target name goes, in a
//! file and on the command line alike, and **a `/` is what tells them apart**:
//! a target is its file name, with `:` where its directories were, so no
//! target's name can hold one.
//!
//! This module only reads the word. Fetching, caching and re-checking a
//! version are `runfile-remote`'s; running the package is the runtime's. It
//! lives here because the checker refuses a reference that can never be
//! fetched -- a missing `@`, an `http://` address -- before a target runs, as it
//! refuses everything else that fails every time.

/// The target a package runs: `runfiles/_main.run`, hidden from the package's
/// own `:list` by its `_`.
pub const ENTRY: &str = "_main";

/// What a reference names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reference {
	/// A repository on a host, at a version.
	Remote(Remote),
	/// A folder on this machine, as written: relative to the working directory
	/// of the line that names it, unless it is absolute.
	Path(String),
}

/// `host/owner/repo[/folder…]@version`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
	/// Lower-cased, with a `:port` when one was written.
	pub host: String,
	/// The repository's path on the host: `owner/repo`, or more segments where a
	/// `.git` marked where it ends (a GitLab subgroup).
	pub repo: String,
	/// The package's folder inside the repository, `/`-separated; empty for its
	/// root.
	pub dir: String,
	pub rev: Rev,
}

/// The version a remote reference pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rev {
	/// A full commit id, lower-cased. It names one tree forever, so it is
	/// fetched once and never asked about again.
	Commit(String),
	/// A tag or a branch, which the host is asked about now and then, since
	/// either can move.
	Name(String),
}

impl Remote {
	/// Where git fetches it from. Always HTTPS: it is the one address every
	/// host serves anonymously, and it goes through credential helpers rather
	/// than an SSH agent that may be waiting for a person to approve it.
	pub fn clone_url(&self) -> String {
		format!("https://{}/{}.git", self.host, self.repo)
	}

	/// The repository without the version or the folder: what the cache keeps
	/// together, and what a first fetch asks consent for.
	pub fn repository(&self) -> String {
		format!("{}/{}", self.host, self.repo)
	}

	/// The reference written the one way it can be.
	pub fn shown(&self) -> String {
		let dir = if self.dir.is_empty() {
			String::new()
		} else {
			format!("/{}", self.dir)
		};
		let rev = match &self.rev {
			Rev::Commit(c) | Rev::Name(c) => c,
		};
		format!("{}{dir}@{rev}", self.repository())
	}
}

/// Why a reference cannot be fetched or found, as the message the checker and
/// the runner both print.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct RefError(pub String);

/// Whether a `run` word names a package rather than a target: it holds a `/`,
/// or is `.` or `..`.
pub fn is_reference(word: &str) -> bool {
	word == "." || word == ".." || word.contains('/')
}

/// Read a word [`is_reference`] says is one.
pub fn parse(word: &str) -> Result<Reference, RefError> {
	if is_path(word) {
		return Ok(Reference::Path(word.to_string()));
	}
	let bad = |m: String| Err(RefError(m));
	let mut rest = word;
	if let Some(scheme_end) = rest.find("://") {
		let scheme = &rest[..scheme_end];
		if !scheme.eq_ignore_ascii_case("https") {
			return bad(format!(
				"`{word}`: a package is fetched over https, and `{scheme}://` is not; write it as `{}`",
				suggest_https(&rest[scheme_end + 3..])
			));
		}
		rest = &rest[scheme_end + 3..];
	}
	// `git@github.com:owner/repo`: an ssh address, whose user comes before a
	// `:` with no `/` ahead of it.
	if let (Some(at), Some(colon)) = (rest.find('@'), rest.find(':'))
		&& at < colon
		&& !rest[..colon].contains('/')
	{
		return bad(format!(
			"`{word}`: a package is named by its host and path, as `{}`, not by an ssh address",
			suggest_https(rest)
		));
	}
	let (location, rev) = match rest.split_once('@') {
		Some((l, r)) => (l, Some(r)),
		None => (rest, None),
	};
	let location = location.trim_end_matches('/');
	let segments: Vec<&str> = location.split('/').collect();
	let host = segments[0];
	if host.contains(':') && !host_with_port(host) || host.contains('@') {
		return bad(format!(
			"`{word}`: a package is named by its host and path, as `github.com/owner/repo@v1.0.0`, not by an ssh address"
		));
	}
	if !host.contains('.') || !valid_host(host) {
		return bad(format!(
			"`{word}` is neither a package on disk, which starts with `./`, `../` or `/`, nor one on a host, which starts with its name, as `github.com/owner/repo@v1.0.0`"
		));
	}
	let path = &segments[1..];
	if let Some(s) = path.iter().find(|s| !valid_segment(s)) {
		return bad(format!(
			"`{word}`: `{s}` is not a name a repository or a folder in one can have"
		));
	}
	// A `.git` marks where the repository ends, which is how a GitLab subgroup
	// is written; otherwise it is the first two segments, which is every
	// GitHub, Bitbucket, Gitea and Forgejo repository there is.
	let end = path
		.iter()
		.enumerate()
		.skip(1)
		.find(|(_, s)| s.ends_with(".git") && s.len() > 4)
		.map_or(2, |(i, _)| i + 1);
	if path.len() < end || path.len() < 2 {
		return bad(format!(
			"`{word}` names a host and no repository; write the owner and the repository after it, as `{host}/owner/repo@v1.0.0`"
		));
	}
	let mut repo: Vec<&str> = path[..end].to_vec();
	if let Some(last) = repo.last_mut() {
		*last = last.strip_suffix(".git").unwrap_or(last);
	}
	let dir = path[end..].join("/");
	let Some(rev) = rev.filter(|r| !r.is_empty()) else {
		return bad(format!(
			"`{word}` names no version: pin it with `@` and a tag, a branch or a commit, as `{location}@v1.0.0`"
		));
	};
	let rev = if is_full_commit(rev) {
		Rev::Commit(rev.to_ascii_lowercase())
	} else if valid_ref_name(rev) {
		Rev::Name(rev.to_string())
	} else {
		return bad(format!(
			"`{word}`: `{rev}` is not a tag, a branch or a commit id git accepts"
		));
	};
	Ok(Reference::Remote(Remote {
		host: host.to_ascii_lowercase(),
		repo: repo.join("/"),
		dir,
		rev,
	}))
}

/// `./x`, `../x`, `/x`, `.`, `..`, and on Windows a drive-rooted path.
fn is_path(word: &str) -> bool {
	if matches!(word, "." | "..") || word.starts_with("./") || word.starts_with("../") || word.starts_with('/') {
		return true;
	}
	let b = word.as_bytes();
	b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'/' | b'\\')
}

/// A full commit id: forty hex digits, or sixty-four in a SHA-256 repository.
/// An abbreviated one is not: no host fetches by a prefix.
pub fn is_full_commit(rev: &str) -> bool {
	matches!(rev.len(), 40 | 64) && rev.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Whether a version that names no tag or branch looks like a commit written
/// short, which is worth saying when the host knows nothing by that name.
pub fn looks_abbreviated(rev: &str) -> bool {
	(7..40).contains(&rev.len())
		&& rev.bytes().all(|b| b.is_ascii_hexdigit())
		&& rev.bytes().any(|b| b.is_ascii_alphabetic())
}

fn valid_host(host: &str) -> bool {
	let name = host.split(':').next().unwrap_or(host);
	!name.is_empty()
		&& name
			.bytes()
			.all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
		&& name
			.split('.')
			.all(|l| !l.is_empty() && !l.starts_with('-') && !l.ends_with('-'))
}

fn host_with_port(host: &str) -> bool {
	match host.split_once(':') {
		Some((name, port)) => !name.is_empty() && !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()),
		None => false,
	}
}

/// A path segment safe to fetch and to cache under: what repository and folder
/// names are made of on every host.
fn valid_segment(s: &str) -> bool {
	!s.is_empty()
		&& s != "."
		&& s != ".."
		&& s.bytes()
			.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'~' | b'+'))
}

/// `git check-ref-format`'s rules, the ones that matter for a name typed after
/// an `@`.
fn valid_ref_name(r: &str) -> bool {
	!r.starts_with('-')
		&& !r.starts_with('/')
		&& !r.ends_with('/')
		&& !r.ends_with('.')
		&& !r.ends_with(".lock")
		&& !r.contains("..")
		&& !r.contains("//")
		&& !r.contains("@{")
		&& !r.split('/').any(|c| c.starts_with('.'))
		&& r.chars()
			.all(|c| !c.is_control() && !c.is_whitespace() && !matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\'))
}

/// `git@github.com:owner/repo`, `ssh://…` and `http://…` written the way a
/// reference is.
fn suggest_https(after_scheme: &str) -> String {
	// A user is everything up to an `@` that comes before the first `/`.
	let s = match after_scheme.find('@') {
		Some(at) if !after_scheme[..at].contains('/') => &after_scheme[at + 1..],
		_ => after_scheme,
	};
	// `host:owner/repo` is scp's spelling, unless what follows the `:` is a port.
	let mut s = match s.split_once(':') {
		Some((host, rest)) if !host.contains('/') && !rest.starts_with(|c: char| c.is_ascii_digit()) => {
			format!("{host}/{rest}")
		}
		_ => s.to_string(),
	};
	if let Some(at) = s.find('@') {
		let (loc, rev) = s.split_at(at);
		s = format!("{}{rev}", loc.strip_suffix(".git").unwrap_or(loc));
	} else {
		s = format!("{}@<version>", s.strip_suffix(".git").unwrap_or(&s));
	}
	s
}
