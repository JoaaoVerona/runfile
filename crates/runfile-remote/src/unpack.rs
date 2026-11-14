//! `download()`: a URL fetched once into the cache and unpacked there.
//!
//! The URL is the key. One that names its version -- a release asset, which is
//! what this is for -- names one file forever, so it is fetched the first time
//! it is asked for and kept; a SHA-256, when given, is checked on that download
//! and fetches again whenever what is kept is not what it says.
//!
//! An archive is recognised by what it holds rather than by its name, and
//! unpacked here rather than by a `tar` on PATH: GNU tar reads no `.zip` and
//! needs an `xz` beside it for `.tar.xz`, and Windows' reads both but not on
//! every version. Nothing it holds is written outside the folder it is unpacked
//! into.

use crate::{Cache, RemoteError, fail, remove_tree, settle};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub(crate) fn download(
	cache: &Cache,
	url: &str,
	pin: Option<&str>,
	say: &dyn Fn(&str),
) -> Result<PathBuf, RemoteError> {
	let lower = url.to_ascii_lowercase();
	if !(lower.starts_with("https://") || lower.starts_with("file://")) {
		return fail(format!(
			"`download()` fetches `https://` and `file://` addresses, and `{url}` is neither"
		));
	}
	if let Some(p) = pin
		&& !(p.len() == 64 && p.bytes().all(|b| b.is_ascii_hexdigit()))
	{
		return fail(format!("`{p}` is not a SHA-256: it is written as 64 hex digits"));
	}
	let key = hex::encode(Sha256::digest(url.as_bytes()));
	let dir = cache.root().join("downloads").join(&key[..32]);
	if let Some(found) = kept(&dir, url, pin) {
		return Ok(found);
	}
	say(&format!("downloading {url}"));
	let tmp = cache.scratch()?;
	let out = (|| {
		let blob = tmp.join("download");
		curl(url, &blob)?;
		let sum = hex::encode(Sha256::digest(
			std::fs::read(&blob).map_err(|e| RemoteError(format!("could not read what {url} sent: {e}")))?,
		));
		if let Some(p) = pin
			&& !p.eq_ignore_ascii_case(&sum)
		{
			return fail(format!(
				"{url} is not what was pinned: its SHA-256 is {sum}, and {p} was expected"
			));
		}
		let entry = tmp.join("entry");
		let content = entry.join("content");
		std::fs::create_dir_all(&content).map_err(|e| RemoteError(format!("could not unpack {url}: {e}")))?;
		let file = unpack(&blob, &file_name(url), &content)
			.map_err(|e| RemoteError(format!("could not unpack {url}: {e}")))?;
		let path = match &file {
			Some(name) => format!("content/{name}"),
			None => "content".to_string(),
		};
		let meta = serde_json::json!({ "url": url, "sha256": sum, "path": path, "fetched": crate::now() });
		std::fs::write(entry.join("meta.json"), meta.to_string())
			.map_err(|e| RemoteError(format!("could not write to the cache: {e}")))?;
		// What was kept before was not what the pin says; it goes, and this
		// takes its place.
		if dir.exists() {
			let _ = std::fs::rename(&dir, tmp.join("replaced"));
		}
		settle(&entry, &dir)?;
		kept(&dir, url, pin).ok_or_else(|| RemoteError(format!("could not keep what {url} sent in the cache")))
	})();
	remove_tree(&tmp);
	out
}

/// What the cache already holds for `url`, if it holds it and it is what
/// `pin` says.
fn kept(dir: &Path, url: &str, pin: Option<&str>) -> Option<PathBuf> {
	let meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("meta.json")).ok()?).ok()?;
	if meta.get("url")?.as_str()? != url {
		return None;
	}
	let sum = meta.get("sha256")?.as_str()?;
	if pin.is_some_and(|p| !p.eq_ignore_ascii_case(sum)) {
		return None;
	}
	let mut path = dir.to_path_buf();
	for seg in meta.get("path")?.as_str()?.split('/') {
		path.push(seg);
	}
	path.exists().then_some(path)
}

/// Fetch with curl, which every platform this runs on has -- Windows since 10
/// -- and which `:update` already uses, so the runner carries no TLS of its own.
fn curl(url: &str, to: &Path) -> Result<(), RemoteError> {
	let out = Command::new("curl")
		.args([
			"-fsSL",
			"--proto",
			"=https,file",
			"--proto-redir",
			"=https",
			"--retry",
			"2",
			"--connect-timeout",
			"30",
			// Give up on a transfer that has stalled for a minute.
			"--speed-limit",
			"1",
			"--speed-time",
			"60",
			"-o",
		])
		.arg(to)
		.arg(url)
		.stdin(Stdio::null())
		.stdout(Stdio::null())
		.stderr(Stdio::piped())
		.output()
		.map_err(|e| match e.kind() {
			std::io::ErrorKind::NotFound => {
				RemoteError("curl is not installed, or not on PATH; `download()` fetches with it".into())
			}
			_ => RemoteError(format!("could not start curl: {e}")),
		})?;
	if out.status.success() {
		return Ok(());
	}
	let said = String::from_utf8_lossy(&out.stderr);
	let said = said
		.lines()
		.map(str::trim)
		.rfind(|l| !l.is_empty())
		.unwrap_or("")
		.trim_start_matches("curl: ");
	fail(format!("could not download {url}: {said}"))
}

/// The last segment of the URL's path: what a file that is not an archive is
/// called once it is kept.
fn file_name(url: &str) -> String {
	let path = url.split(['?', '#']).next().unwrap_or(url);
	let last = path.rsplit('/').next().unwrap_or("");
	let clean: String = last
		.chars()
		.filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
		.collect();
	match clean.trim_start_matches('.') {
		"" => "download".to_string(),
		c => c.to_string(),
	}
}

/// Unpack `blob` into `out`. Answers with the file's name when it was not an
/// archive and was kept whole, decompressed if it was a lone `.gz` or `.xz`.
pub(crate) fn unpack(blob: &Path, name: &str, out: &Path) -> Result<Option<String>, String> {
	let bytes = std::fs::read(blob).map_err(|e| e.to_string())?;
	let (inner, name) = if bytes.starts_with(&[0x1f, 0x8b]) {
		let mut v = Vec::new();
		flate2::read::GzDecoder::new(&bytes[..])
			.read_to_end(&mut v)
			.map_err(|e| format!("it is not a gzip file: {e}"))?;
		(v, strip_ext(name, &[".tgz", ".gz"]))
	} else if bytes.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0x00]) {
		let mut v = Vec::new();
		lzma_rs::xz_decompress(&mut std::io::BufReader::new(&bytes[..]), &mut v)
			.map_err(|e| format!("it is not an xz file: {e:?}"))?;
		(v, strip_ext(name, &[".txz", ".xz"]))
	} else {
		(bytes, name.to_string())
	};
	if inner.starts_with(b"PK\x03\x04") || inner.starts_with(b"PK\x05\x06") {
		crate::zip::extract(&inner, out)?;
		return Ok(None);
	}
	if is_tar(&inner) {
		let mut a = tar::Archive::new(&inner[..]);
		a.set_overwrite(true);
		// `unpack` writes nothing outside `out`: an entry naming `..`, an
		// absolute path, or a way out through a link it made is refused.
		a.unpack(out)
			.map_err(|e| format!("the archive could not be unpacked: {e}"))?;
		return Ok(None);
	}
	let name = strip_ext(&name, &[".tar"]);
	let target = out.join(&name);
	std::fs::write(&target, &inner).map_err(|e| e.to_string())?;
	executable(&target);
	Ok(Some(name))
}

fn strip_ext(name: &str, exts: &[&str]) -> String {
	for e in exts {
		if let Some(s) = name.strip_suffix(e)
			&& !s.is_empty()
		{
			// A `.tgz` was a `.tar.gz`.
			return if *e == ".tgz" || *e == ".txz" {
				format!("{s}.tar")
			} else {
				s.to_string()
			};
		}
	}
	name.to_string()
}

fn is_tar(b: &[u8]) -> bool {
	b.len() >= 263 && &b[257..262] == b"ustar"
}

/// A lone file fetched to be run is almost always a program.
#[cfg(unix)]
pub(crate) fn executable(p: &Path) {
	use std::os::unix::fs::PermissionsExt;
	let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755));
}

#[cfg(not(unix))]
pub(crate) fn executable(_: &Path) {}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_file_is_named_after_the_last_segment_of_its_url() {
		assert_eq!(file_name("https://h/x/tool-linux?raw=1#a"), "tool-linux");
		assert_eq!(file_name("https://h/"), "download");
		assert_eq!(file_name("https://h/../..%2f"), "2f");
		assert_eq!(strip_ext("tool.gz", &[".tgz", ".gz"]), "tool");
		assert_eq!(strip_ext("x.tgz", &[".tgz", ".gz"]), "x.tar");
	}
}
