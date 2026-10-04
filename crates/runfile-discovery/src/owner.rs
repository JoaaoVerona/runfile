//! Who owns a directory, as far as trusting it to say what `run` runs goes.
//!
//! The upward walk takes the nearest `runfiles/` above the working directory,
//! and a directory another account can create -- `/tmp/runfiles`, or
//! `C:\runfiles`, which any signed-in Windows user may make -- would otherwise
//! answer for every directory beneath it that has none of its own. That is the
//! shape of git's CVE-2022-24765, and the check is git's: compare owners.
//!
//! Three owners are trusted. The account running `run`; the machine's own
//! administrators (root, or `Administrators` and `SYSTEM` on Windows), who can
//! change anything of ours anyway; and whoever owns the directory the walk
//! started in. The last is what keeps working inside somebody else's tree a
//! deliberate act rather than a refusal -- `cd` into a colleague's checkout and
//! run a target there, exactly as `make` would, or a container running as root
//! over a bind mount its user owns. What it refuses is the walk *crossing* to
//! an owner the start did not have: standing in your own directory under `/tmp`
//! and being handed another user's targets.

use std::path::Path;

/// One account, as the platform identifies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Owner(Id);

#[cfg(unix)]
type Id = u32;

/// A SID's bytes. Two SIDs are the same account exactly when their bytes are
/// equal, which is all `EqualSid` compares.
#[cfg(windows)]
type Id = Vec<u8>;

#[cfg(not(any(unix, windows)))]
type Id = ();

impl Owner {
	/// The account `run` is running as.
	pub(crate) fn me() -> std::io::Result<Owner> {
		imp::me().map(Owner)
	}

	/// Who owns `path`. A symlink is judged by the link *and* by what it points
	/// at, since either one's owner chose where the walk goes.
	pub(crate) fn of(path: &Path) -> std::io::Result<Vec<Owner>> {
		imp::of(path).map(|ids| ids.into_iter().map(Owner).collect())
	}

	/// Root, or Windows' `Administrators` and `SYSTEM`: an account that can
	/// rewrite this user's files whatever we decide, so refusing it protects
	/// nothing.
	pub(crate) fn is_admin(&self) -> bool {
		imp::is_admin(&self.0)
	}

	/// How an error names the account.
	pub(crate) fn describe(&self) -> String {
		imp::describe(&self.0)
	}

	/// A stand-in for an account, for the test that needs a stranger. Unix
	/// only: a Windows test cannot rely on what its temporary directories are
	/// owned by, an elevated one making them the Administrators group's.
	#[cfg(all(test, unix))]
	pub(crate) fn from_id(id: Id) -> Owner {
		Owner(id)
	}
}

#[cfg(unix)]
mod imp {
	use std::os::unix::fs::MetadataExt;
	use std::path::Path;

	pub(super) fn me() -> std::io::Result<u32> {
		// SAFETY: `geteuid` takes nothing and cannot fail.
		Ok(unsafe { libc::geteuid() })
	}

	pub(super) fn of(path: &Path) -> std::io::Result<Vec<u32>> {
		let link = std::fs::symlink_metadata(path)?;
		let mut ids = vec![link.uid()];
		if link.file_type().is_symlink() {
			ids.push(std::fs::metadata(path)?.uid());
		}
		Ok(ids)
	}

	pub(super) fn is_admin(id: &u32) -> bool {
		*id == 0
	}

	pub(super) fn describe(id: &u32) -> String {
		format!("uid {id}")
	}
}

#[cfg(windows)]
mod imp {
	use std::os::windows::ffi::OsStrExt;
	use std::path::Path;
	use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, LocalFree};
	use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT};
	use windows_sys::Win32::Security::{
		GetLengthSid, GetTokenInformation, IsValidSid, IsWellKnownSid, OWNER_SECURITY_INFORMATION,
		PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser, WinBuiltinAdministratorsSid, WinLocalSystemSid,
	};
	use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

	/// A SID's bytes, copied out of memory the system owns.
	///
	/// # Safety
	/// `sid` must point at a valid SID that outlives the call.
	unsafe fn bytes(sid: PSID) -> Vec<u8> {
		// SAFETY: the caller hands a valid SID, whose length `GetLengthSid`
		// reports, and the copy is made before that memory is released.
		unsafe {
			let len = GetLengthSid(sid) as usize;
			std::slice::from_raw_parts(sid as *const u8, len).to_vec()
		}
	}

	pub(super) fn me() -> std::io::Result<Vec<u8>> {
		// SAFETY: every pointer handed over is to a local the call fills, the
		// token handle is closed on every path, and the user SID is copied out of
		// the buffer before the buffer is dropped.
		unsafe {
			let mut token: HANDLE = std::ptr::null_mut();
			if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
				return Err(std::io::Error::last_os_error());
			}
			let mut len = 0u32;
			GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
			// `u64`s, so the buffer is aligned for the `TOKEN_USER` it holds.
			let mut buf = vec![0u64; (len as usize).div_ceil(8).max(1)];
			let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len);
			let err = std::io::Error::last_os_error();
			CloseHandle(token);
			if ok == 0 {
				return Err(err);
			}
			let user = &*(buf.as_ptr() as *const TOKEN_USER);
			Ok(bytes(user.User.Sid))
		}
	}

	pub(super) fn of(path: &Path) -> std::io::Result<Vec<Vec<u8>>> {
		let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
		// SAFETY: `wide` is NUL-terminated and outlives the call; the owner SID
		// points into the descriptor, which is copied from and then freed.
		unsafe {
			let mut owner: PSID = std::ptr::null_mut();
			let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
			let rc = GetNamedSecurityInfoW(
				wide.as_ptr(),
				SE_FILE_OBJECT,
				OWNER_SECURITY_INFORMATION,
				&mut owner,
				std::ptr::null_mut(),
				std::ptr::null_mut(),
				std::ptr::null_mut(),
				&mut sd,
			);
			if rc != ERROR_SUCCESS {
				return Err(std::io::Error::from_raw_os_error(rc as i32));
			}
			// A file system with no security at all (FAT, exFAT) reports no
			// owner. Anyone can write there, so it is nobody's.
			let id = if owner.is_null() || IsValidSid(owner) == 0 {
				Vec::new()
			} else {
				bytes(owner)
			};
			LocalFree(sd);
			Ok(vec![id])
		}
	}

	pub(super) fn is_admin(id: &[u8]) -> bool {
		if id.is_empty() {
			return false;
		}
		let sid = id.as_ptr() as PSID;
		// SAFETY: `id` holds a SID copied whole by `bytes`, and the calls only
		// read it.
		unsafe { IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0 || IsWellKnownSid(sid, WinLocalSystemSid) != 0 }
	}

	pub(super) fn describe(id: &[u8]) -> String {
		if id.is_empty() {
			return "no owner at all".to_string();
		}
		// SAFETY: `id` is a whole SID; the string the call allocates is read up
		// to its NUL and then freed.
		unsafe {
			let mut text: *mut u16 = std::ptr::null_mut();
			if ConvertSidToStringSidW(id.as_ptr() as PSID, &mut text) == 0 || text.is_null() {
				return "another account".to_string();
			}
			let len = (0..).take_while(|&i| *text.add(i) != 0).count();
			let s = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
			LocalFree(text.cast());
			s
		}
	}
}

#[cfg(not(any(unix, windows)))]
mod imp {
	use std::path::Path;

	pub(super) fn me() -> std::io::Result<()> {
		Ok(())
	}

	pub(super) fn of(_: &Path) -> std::io::Result<Vec<()>> {
		Ok(vec![()])
	}

	pub(super) fn is_admin(_: &()) -> bool {
		true
	}

	pub(super) fn describe(_: &()) -> String {
		String::new()
	}
}
