//! Reading a `.zip`: what a release for Windows ships in.
//!
//! Only what release archives use -- entries stored or deflated, Zip64 sizes
//! included -- which is a page of code against a crate that brings a dozen of
//! its own. An entry is refused when it is encrypted, compressed some other
//! way, a link, or named so that it would land outside the folder: absolute,
//! with a drive, or through a `..`.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

fn u16_at(b: &[u8], at: usize) -> Result<u16, String> {
	b.get(at..at + 2)
		.map(|s| u16::from_le_bytes([s[0], s[1]]))
		.ok_or_else(truncated)
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, String> {
	b.get(at..at + 4)
		.map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
		.ok_or_else(truncated)
}

fn u64_at(b: &[u8], at: usize) -> Result<u64, String> {
	b.get(at..at + 8)
		.map(|s| u64::from_le_bytes(s.try_into().expect("eight bytes")))
		.ok_or_else(truncated)
}

fn truncated() -> String {
	"the zip archive is cut short".into()
}

const END: u32 = 0x0605_4b50;
const END64: u32 = 0x0606_4b50;
const LOCATOR64: u32 = 0x0706_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const LOCAL: u32 = 0x0403_4b50;

pub(crate) fn extract(b: &[u8], out: &Path) -> Result<(), String> {
	// The end record is the last thing in the file, followed by a comment of
	// at most 64 KiB.
	let lowest = b.len().saturating_sub(22 + 0xffff);
	let end = (lowest..=b.len().saturating_sub(22))
		.rev()
		.find(|&i| u32_at(b, i) == Ok(END))
		.ok_or("it is not a zip archive: it has no end record")?;
	let mut entries = u64::from(u16_at(b, end + 10)?);
	let mut offset = u64::from(u32_at(b, end + 16)?);
	if (entries == 0xffff || offset == 0xffff_ffff) && end >= 20 && u32_at(b, end - 20) == Ok(LOCATOR64) {
		let rec = usize::try_from(u64_at(b, end - 20 + 8)?).map_err(|_| truncated())?;
		if u32_at(b, rec)? != END64 {
			return Err("the zip archive's Zip64 record is not where it says".into());
		}
		entries = u64_at(b, rec + 32)?;
		offset = u64_at(b, rec + 48)?;
	}
	let mut at = usize::try_from(offset).map_err(|_| truncated())?;
	for _ in 0..entries {
		if u32_at(b, at)? != CENTRAL {
			return Err("the zip archive's directory is damaged".into());
		}
		let made_by = u16_at(b, at + 4)?;
		let flags = u16_at(b, at + 8)?;
		let method = u16_at(b, at + 10)?;
		let crc = u32_at(b, at + 16)?;
		let mut packed = u64::from(u32_at(b, at + 20)?);
		let mut size = u64::from(u32_at(b, at + 24)?);
		let name_len = usize::from(u16_at(b, at + 28)?);
		let extra_len = usize::from(u16_at(b, at + 30)?);
		let comment_len = usize::from(u16_at(b, at + 32)?);
		let external = u32_at(b, at + 38)?;
		let mut local = u64::from(u32_at(b, at + 42)?);
		let name_bytes = b.get(at + 46..at + 46 + name_len).ok_or_else(truncated)?;
		let name = String::from_utf8_lossy(name_bytes).replace('\\', "/");
		let extra = b
			.get(at + 46 + name_len..at + 46 + name_len + extra_len)
			.ok_or_else(truncated)?;
		zip64_sizes(extra, &mut size, &mut packed, &mut local)?;
		at += 46 + name_len + extra_len + comment_len;

		if flags & 1 != 0 {
			return Err(format!("`{name}` in the zip archive is encrypted"));
		}
		// The high half of the external attributes is a Unix mode, when the
		// archive was made somewhere that has one.
		let unix = (made_by >> 8) == 3;
		let mode = if unix { (external >> 16) & 0o177_777 } else { 0 };
		if unix && mode & 0o170_000 == 0o120_000 {
			return Err(format!("`{name}` in the zip archive is a link, which is not unpacked"));
		}
		let target =
			inside(out, &name).ok_or_else(|| format!("`{name}` in the zip archive would be written outside it"))?;
		if name.ends_with('/') {
			std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
			continue;
		}
		let local = usize::try_from(local).map_err(|_| truncated())?;
		if u32_at(b, local)? != LOCAL {
			return Err(format!("`{name}` is not where the zip archive's directory says"));
		}
		let data = local + 30 + usize::from(u16_at(b, local + 26)?) + usize::from(u16_at(b, local + 28)?);
		let packed = usize::try_from(packed).map_err(|_| truncated())?;
		let raw = b.get(data..data + packed).ok_or_else(truncated)?;
		let bytes = match method {
			0 => raw.to_vec(),
			8 => {
				let mut v = Vec::with_capacity(usize::try_from(size).unwrap_or(0).min(1 << 30));
				flate2::read::DeflateDecoder::new(raw)
					.read_to_end(&mut v)
					.map_err(|e| format!("`{name}` in the zip archive is damaged: {e}"))?;
				v
			}
			m => {
				return Err(format!(
					"`{name}` in the zip archive is compressed with method {m}, which is not read"
				));
			}
		};
		let mut sum = flate2::Crc::new();
		sum.update(&bytes);
		if sum.sum() != crc || bytes.len() as u64 != size {
			return Err(format!(
				"`{name}` in the zip archive is damaged: its checksum does not match"
			));
		}
		if let Some(parent) = target.parent() {
			std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
		}
		std::fs::write(&target, &bytes).map_err(|e| e.to_string())?;
		#[cfg(unix)]
		if unix && mode & 0o777 != 0 {
			use std::os::unix::fs::PermissionsExt;
			let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode & 0o777));
		}
	}
	Ok(())
}

/// The Zip64 extra field holds, in this order, whichever of the three sizes
/// the directory entry left at their maximum.
fn zip64_sizes(extra: &[u8], size: &mut u64, packed: &mut u64, local: &mut u64) -> Result<(), String> {
	let mut i = 0;
	while i + 4 <= extra.len() {
		let id = u16_at(extra, i)?;
		let len = usize::from(u16_at(extra, i + 2)?);
		if id == 1 {
			let mut k = i + 4;
			for field in [size, packed, local] {
				if *field == 0xffff_ffff {
					*field = u64_at(extra, k)?;
					k += 8;
				}
			}
			return Ok(());
		}
		i += 4 + len;
	}
	Ok(())
}

/// `name` under `out`, or `None` when it would be anywhere else.
fn inside(out: &Path, name: &str) -> Option<PathBuf> {
	let rel = Path::new(name.trim_end_matches('/'));
	let mut p = out.to_path_buf();
	for c in rel.components() {
		match c {
			Component::Normal(s) => p.push(s),
			Component::CurDir => {}
			_ => return None,
		}
	}
	(p != out).then_some(p)
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A zip of one stored entry, built by hand.
	fn one(name: &str, data: &[u8]) -> Vec<u8> {
		let mut crc = flate2::Crc::new();
		crc.update(data);
		let mut z = Vec::new();
		z.extend(LOCAL.to_le_bytes());
		z.extend([20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
		z.extend(crc.sum().to_le_bytes());
		z.extend((data.len() as u32).to_le_bytes());
		z.extend((data.len() as u32).to_le_bytes());
		z.extend((name.len() as u16).to_le_bytes());
		z.extend(0u16.to_le_bytes());
		z.extend(name.as_bytes());
		z.extend(data);
		let cd = z.len() as u32;
		z.extend(CENTRAL.to_le_bytes());
		z.extend([20, 3, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
		z.extend(crc.sum().to_le_bytes());
		z.extend((data.len() as u32).to_le_bytes());
		z.extend((data.len() as u32).to_le_bytes());
		z.extend((name.len() as u16).to_le_bytes());
		z.extend([0, 0, 0, 0, 0, 0, 0, 0]);
		z.extend((0o100_755u32 << 16).to_le_bytes());
		z.extend(0u32.to_le_bytes());
		z.extend(name.as_bytes());
		let cd_len = z.len() as u32 - cd;
		z.extend(END.to_le_bytes());
		z.extend([0, 0, 0, 0, 1, 0, 1, 0]);
		z.extend(cd_len.to_le_bytes());
		z.extend(cd.to_le_bytes());
		z.extend([0, 0]);
		z
	}

	#[test]
	fn a_stored_entry_is_written_with_its_mode() {
		let d = tempfile::TempDir::new().unwrap();
		extract(&one("bin/tool", b"#!/bin/sh\necho hi\n"), d.path()).unwrap();
		assert_eq!(
			std::fs::read(d.path().join("bin/tool")).unwrap(),
			b"#!/bin/sh\necho hi\n"
		);
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;
			let mode = std::fs::metadata(d.path().join("bin/tool"))
				.unwrap()
				.permissions()
				.mode();
			assert_eq!(mode & 0o777, 0o755);
		}
	}

	#[test]
	fn an_entry_that_would_land_outside_is_refused() {
		for name in ["../evil", "/etc/evil", "a/../../evil"] {
			let d = tempfile::TempDir::new().unwrap();
			let e = extract(&one(name, b"x"), d.path()).unwrap_err();
			assert!(e.contains("outside"), "{name}: {e}");
		}
	}

	#[test]
	fn a_damaged_entry_is_refused() {
		let d = tempfile::TempDir::new().unwrap();
		let mut z = one("f", b"hello");
		let at = z.windows(5).position(|w| w == b"hello").unwrap();
		z[at] = b'j';
		assert!(extract(&z, d.path()).unwrap_err().contains("checksum"));
	}
}
