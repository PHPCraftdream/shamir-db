//! Windows implementation: an atomically created directory with an
//! explicit protected one-ACE DACL, and a full owner/DACL verification at
//! every open.
//!
//! All Win32 FFI of this feature lives here. The module (and the whole
//! dependency on `windows-sys`) is `#[cfg(windows)]`; every `unsafe` block
//! carries a `// SAFETY:` comment.

use std::ffi::c_void;
use std::fs;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, ERROR_INSUFFICIENT_BUFFER, HANDLE};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    GetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    AclSizeInformation, EqualSid, GetAce, GetAclInformation, GetSecurityDescriptorControl,
    GetSidSubAuthorityCount, GetTokenInformation, TokenUser, ACCESS_ALLOWED_ACE, ACL,
    ACL_SIZE_INFORMATION, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, OBJECT_INHERIT_ACE,
    OWNER_SECURITY_INFORMATION, PSID, SECURITY_ATTRIBUTES, SE_DACL_PRESENT, SE_DACL_PROTECTED,
    TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// `FILE_ALL_ACCESS` = `STANDARD_RIGHTS_REQUIRED | SYNCHRONIZE`
/// (`0x1F01FF`). Defined here (rather than imported) so the expected ACE
/// mask is explicit and independent of where windows-sys re-exports it.
const FILE_ALL_ACCESS: u32 = 0x001f_01ff;

/// Win32 `ACCESS_ALLOWED_ACE_TYPE` (`0`). Defined here so the expected
/// ACE type is explicit without pulling in the
/// `Win32_System_SystemServices` feature for a single constant.
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;

/// Win32 `FILE_ATTRIBUTE_REPARSE_POINT` — a junction/symlink/mount point
/// bit; set on anything that is not a plain on-disk directory entry.
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;

/// SDDL for a directory: `D:` protected (`P`) DACL, one inheritable ACE
/// allowing `FA` (FILE_ALL_ACCESS) to the current user SID.
const DIRECTORY_SDDL: &str = "D:P(A;OICI;FA;;;<sid>)";
/// SDDL for a named pipe: `D:` protected (`P`) DACL, one ACE allowing
/// `GA` (Generic All) to the current user SID.
const PIPE_SDDL: &str = "D:P(A;;GA;;;<sid>)";

// ── Public safe API ───────────────────────────────────────────────────

/// Creates `path` (one level; the parent must exist) that only the current
/// OS account can access. The directory is created ATOMICALLY with an
/// explicit security descriptor `D:P(A;OICI;FA;;;<current-user-SID>)`
/// (protected DACL, one inheritable full-access ACE for the current
/// process token's user SID) — never "create then fix up". An already
/// existing path is accepted only if [`verify_private_dir`] passes.
pub fn create_private_dir(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path).is_ok() {
        return verify_private_dir(path);
    }
    let sec_desc = OwnerOnlySecurityDescriptor::from_sddl(DIRECTORY_SDDL)?;
    let wide = path_to_wide(path)?;
    // SAFETY: `wide` is a valid null-terminated UTF-16 string for the
    // duration of this call; `sec_desc.as_security_attributes()` points at
    // a valid `SECURITY_ATTRIBUTES` owned by `sec_desc`, which outlives
    // the call. `CreateDirectoryW` copies the descriptor, it does not
    // retain the pointer.
    let ok = unsafe { CreateDirectoryW(wide.as_ptr(), sec_desc.as_security_attributes()) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    verify_private_dir(path)
}

/// Verifies `path` is a real directory and NOT a reparse point (checked
/// via `symlink_metadata`, i.e. without following links), that its owner
/// is the current user SID, and that its DACL is exactly the one private
/// ACE. Any violation is [`io::ErrorKind::PermissionDenied`] with a
/// message naming what failed.
///
/// The exact checks, and what each enforces:
/// - reparse-point bit (via `symlink_metadata`): the path is a real
///   directory, never a symlink/junction;
/// - owner via `EqualSid` against the current process token's user;
/// - security-descriptor control word via `GetSecurityDescriptorControl`:
///   `SE_DACL_PRESENT` set AND the DACL pointer non-null (a NULL DACL
///   would grant everyone) AND `SE_DACL_PROTECTED` set (no inherited ACE
///   from the parent can contribute access);
/// - DACL walk: `AceCount == 1`, and the single ACE has type allow
///   (`ACCESS_ALLOWED_ACE_TYPE`), flags exactly `OBJECT_INHERIT_ACE |
///   CONTAINER_INHERIT_ACE` (SDDL `OICI`), mask exactly `FILE_ALL_ACCESS`,
///   and a SID equal (via `EqualSid`) to the current user's.
///
/// The DACL is checked by WALKING it (owner via `EqualSid`, ACE count,
/// ACE type/flags/mask/SID), not by comparing SDDL strings: walking is
/// exact (a second ACE, a wrong mask, a wrong owner or an unprotected
/// DACL each fail with their own message) and does not depend on SDDL
/// canonicalisation round-trips. This matches what
/// [`create_private_dir`] produces (`D:P(A;OICI;FA;;;<sid>)`).
pub fn verify_private_dir(path: &Path) -> io::Result<()> {
    // `symlink_metadata` does not follow the final component: a junction
    // or symlink reports the reparse-point bit here.
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Err(denied("path is not a directory"));
    }
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(denied("path is a reparse point (symlink or junction)"));
    }

    let current_sid = current_user_sid()?;
    let wide = path_to_wide(path)?;
    let mut owner: PSID = ptr::null_mut();
    let mut dacl: *mut ACL = ptr::null_mut();
    let mut descriptor: *mut c_void = ptr::null_mut();
    // SAFETY: `wide` is a valid null-terminated UTF-16 path; the out
    // params are valid pointers of the documented types. On success
    // `descriptor` is a heap buffer allocated by the OS, owned here and
    // freed via `LocalFree` below on every path.
    let error = unsafe {
        GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    let result = (|| {
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        if owner.is_null() {
            return Err(denied("directory has no owner"));
        }
        // SAFETY: `owner` and `current_sid.as_ptr()` are both valid PSIDs
        // (returned by the OS above / freshly built from the process
        // token); `EqualSid` only reads them.
        if unsafe { EqualSid(owner, current_sid.as_ptr() as PSID) } == 0 {
            return Err(denied("directory is owned by another account"));
        }
        verify_control_and_dacl(descriptor, dacl, &current_sid)
    })();
    // SAFETY: `descriptor` was allocated by `GetNamedSecurityInfoW`
    // (which uses `LocalAlloc` per its documented contract) whenever the
    // call succeeded, and is freed exactly once, here, on every exit
    // path. `LocalFree` accepts null and fails harmlessly on failure.
    unsafe {
        LocalFree(descriptor);
    }
    result
}

/// Checks the security descriptor's control word (DACL present, non-null
/// and protected) and that the DACL holds exactly the one expected ACE.
fn verify_control_and_dacl(
    descriptor: *mut c_void,
    dacl: *mut ACL,
    current_sid: &[u8],
) -> io::Result<()> {
    let mut control: u16 = 0;
    let mut revision: u32 = 0;
    // SAFETY: `descriptor` is the valid `PSECURITY_DESCRIPTOR` returned by
    // `GetNamedSecurityInfoW` (freed by the caller after this returns);
    // the out-params are valid pointers of the documented types.
    let ok = unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    if control & SE_DACL_PRESENT == 0 {
        return Err(denied("directory has no DACL (SE_DACL_PRESENT unset)"));
    }
    // A NULL DACL grants EVERYONE full access; an absent entry is caught
    // above, a present-but-null pointer here.
    if dacl.is_null() {
        return Err(denied("directory has a NULL DACL"));
    }
    if control & SE_DACL_PROTECTED == 0 {
        return Err(denied(
            "directory DACL is not protected (SE_DACL_PROTECTED unset)",
        ));
    }
    verify_dacl(dacl, current_sid)
}

/// Checks the DACL holds exactly the one expected ACE.
fn verify_dacl(dacl: *mut ACL, current_sid: &[u8]) -> io::Result<()> {
    let mut size = ACL_SIZE_INFORMATION {
        AceCount: 0,
        AclBytesInUse: 0,
        AclBytesFree: 0,
    };
    // SAFETY: `dacl` is the valid PACL returned by
    // `GetNamedSecurityInfoW`; `size` is the matching
    // `ACL_SIZE_INFORMATION` out-buffer with its documented length.
    let ok = unsafe {
        GetAclInformation(
            dacl,
            &mut size as *mut ACL_SIZE_INFORMATION as *mut c_void,
            std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    if size.AceCount != 1 {
        return Err(denied(&format!(
            "directory DACL has {} ACEs, expected exactly 1",
            size.AceCount
        )));
    }
    let mut ace: *mut ACCESS_ALLOWED_ACE = ptr::null_mut();
    // SAFETY: `dacl` is valid and ACE index 0 exists (`AceCount == 1`);
    // `GetAce` writes a pointer INTO the ACL buffer, valid while the ACL
    // is alive (it is, for the rest of this function).
    if unsafe { GetAce(dacl, 0, &mut ace as *mut _ as *mut *mut c_void) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `ace` points at a valid `ACCESS_ALLOWED_ACE` inside the ACL.
    let ace = unsafe { &*ace };
    if ace.Header.AceType != ACCESS_ALLOWED_ACE_TYPE {
        return Err(denied("directory DACL ACE is not an allow ACE"));
    }
    // Exactly OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE (SDDL `OICI`):
    // child files and subdirectories inherit it, nothing else.
    if ace.Header.AceFlags != (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE) as u8 {
        return Err(denied(
            "directory DACL ACE has unexpected inheritance flags",
        ));
    }
    if ace.Mask != FILE_ALL_ACCESS {
        return Err(denied("directory DACL ACE is not full access"));
    }
    // SAFETY: `ace.SidStart` marks the first byte of the ACE's trailing
    // SID (valid per the ACE layout inside the ACL buffer) and
    // `current_sid` is a valid SID buffer; `EqualSid` only reads.
    if unsafe {
        EqualSid(
            &ace.SidStart as *const u32 as PSID,
            current_sid.as_ptr() as PSID,
        )
    } == 0
    {
        return Err(denied("directory DACL ACE grants another account"));
    }
    Ok(())
}

/// RAII owner of a Win32 `SECURITY_DESCRIPTOR` restricting access to the
/// current process token's user SID, plus the `SECURITY_ATTRIBUTES`
/// wrapper the Win32 create calls (`CreateNamedPipe`, `CreateDirectoryW`)
/// expect. `ConvertStringSecurityDescriptorToSecurityDescriptorW`
/// allocates the descriptor via `LocalAlloc` — freed on `Drop` via
/// `LocalFree`.
pub struct OwnerOnlySecurityDescriptor {
    attrs: SECURITY_ATTRIBUTES,
}

// SAFETY: the owned `SECURITY_DESCRIPTOR` buffer is only ever read from
// (passed by pointer into a Win32 create call, in a call this crate or a
// consumer makes under a `&self`/`&_` borrow) — no interior mutability, no
// thread-affine Win32 handle involved (unlike a `HANDLE`, a
// `LocalAlloc`-backed buffer has no thread-affinity requirement).
unsafe impl Send for OwnerOnlySecurityDescriptor {}
unsafe impl Sync for OwnerOnlySecurityDescriptor {}

impl OwnerOnlySecurityDescriptor {
    /// Builds the named-pipe descriptor: protected DACL with a single
    /// Generic-All ACE for the current process token's user SID
    /// (`D:P(A;;GA;;;<sid>)`).
    pub fn for_current_user() -> io::Result<Self> {
        Self::from_sddl(PIPE_SDDL)
    }

    fn from_sddl(template: &str) -> io::Result<Self> {
        let sid = current_user_sid_string()?;
        let sddl = template.replace("<sid>", &sid);
        let wide = to_wide_null(&sddl);

        let mut psd: *mut c_void = ptr::null_mut();
        // SAFETY: `wide` is a valid null-terminated UTF-16 string for the
        // duration of this call. `psd` is a valid out-param. On success
        // `psd` is heap-allocated by the OS (`LocalAlloc`) and owned by
        // this struct from here on — freed in `Drop`.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut psd,
                ptr::null_mut(),
            )
        };
        if ok == 0 || psd.is_null() {
            return Err(io::Error::last_os_error());
        }

        Ok(Self {
            attrs: SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: psd,
                bInheritHandle: 0,
            },
        })
    }

    /// The raw `SECURITY_ATTRIBUTES` pointer for Win32 create calls.
    /// Returning a raw pointer from a safe fn is deliberate: dereferencing
    /// (and keeping the borrow alive across the call) stays the
    /// consumer's `unsafe` obligation.
    pub fn as_security_attributes(&self) -> *const SECURITY_ATTRIBUTES {
        &self.attrs
    }
}

impl Drop for OwnerOnlySecurityDescriptor {
    fn drop(&mut self) {
        if !self.attrs.lpSecurityDescriptor.is_null() {
            // SAFETY: `lpSecurityDescriptor` was allocated by
            // `ConvertStringSecurityDescriptorToSecurityDescriptorW`
            // (which uses `LocalAlloc` internally per its own documented
            // contract) and is freed exactly once, here, when this — its
            // sole owner — is dropped.
            unsafe {
                LocalFree(self.attrs.lpSecurityDescriptor);
            }
        }
    }
}

/// Test support (compiled ONLY for this crate's tests): rewrites `path`'s
/// DACL to the SAME single owner ACE but UNPROTECTED — the exact
/// `D:(A;OICI;FA;;;<current-user-sid>)` of [`create_private_dir`] minus
/// the `P` flag — so a rejection from [`verify_private_dir`] can only come
/// from the `SE_DACL_PROTECTED` control-word check. Owner and group are
/// left untouched (`DACL_SECURITY_INFORMATION` only).
#[cfg(all(windows, test))]
pub(crate) fn set_unprotected_owner_only_dacl_for_test(path: &Path) -> io::Result<()> {
    use windows_sys::Win32::Security::Authorization::SetNamedSecurityInfoW;
    use windows_sys::Win32::Security::{
        GetSecurityDescriptorDacl, UNPROTECTED_DACL_SECURITY_INFORMATION,
    };

    let sid = current_user_sid_string()?;
    let sddl = "D:(A;OICI;FA;;;<sid>)".replace("<sid>", &sid);
    let sddl_wide = to_wide_null(&sddl);
    let mut psd: *mut c_void = ptr::null_mut();
    // SAFETY: `sddl_wide` is a valid null-terminated UTF-16 string for the
    // duration of this call; `psd` is a valid out-param. On success `psd`
    // is heap-allocated by the OS (`LocalAlloc`) and freed below on every
    // exit path.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl_wide.as_ptr(),
            SDDL_REVISION_1,
            &mut psd,
            ptr::null_mut(),
        )
    };
    if ok == 0 || psd.is_null() {
        return Err(io::Error::last_os_error());
    }
    let mut dacl_present: i32 = 0;
    let mut dacl_defaulted: i32 = 0;
    let mut dacl: *mut ACL = ptr::null_mut();
    // SAFETY: `psd` is the valid descriptor allocated above; the three
    // out-params are valid locals. On success `dacl` points INTO `psd` (no
    // separate allocation), so it stays valid until `psd` is freed below.
    let got = unsafe {
        GetSecurityDescriptorDacl(psd, &mut dacl_present, &mut dacl, &mut dacl_defaulted)
    };
    if got == 0 || dacl_present == 0 || dacl.is_null() {
        let error = io::Error::last_os_error();
        // SAFETY: freeing the same `LocalAlloc`-backed descriptor exactly once.
        unsafe {
            LocalFree(psd);
        }
        return Err(error);
    }
    let path_wide = path_to_wide(path)?;
    // SAFETY: `path_wide` is a valid null-terminated UTF-16 path and `dacl`
    // a valid ACL living inside `psd`; `SetNamedSecurityInfoW` copies what
    // it needs and does not retain the pointer. Only the DACL is written
    // (owner/group params are null), with
    // `UNPROTECTED_DACL_SECURITY_INFORMATION` so inheritance stays enabled,
    // i.e. `SE_DACL_PROTECTED` ends up CLEAR.
    let error = unsafe {
        SetNamedSecurityInfoW(
            path_wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | UNPROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            dacl as *const ACL,
            ptr::null_mut(),
        )
    };
    // SAFETY: freeing the same `LocalAlloc`-backed descriptor exactly once.
    unsafe {
        LocalFree(psd);
    }
    if error != 0 {
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────

/// Raw SID bytes of the current process token's user. Used for the
/// `EqualSid` comparisons in [`verify_private_dir`].
fn current_user_sid() -> io::Result<Vec<u8>> {
    with_process_token_user(|_buffer, token_user| {
        let sid = token_user.User.Sid;
        // SAFETY: `sid` is a valid PSID inside `buffer`;
        // `GetSidSubAuthorityCount` returns a pointer to its sub-authority
        // count byte, from which the documented SID length follows.
        let length = unsafe { 8 + 4 * usize::from(*GetSidSubAuthorityCount(sid)) };
        // SAFETY: the first `length` bytes of `buffer` hold the valid SID.
        Ok(unsafe { std::slice::from_raw_parts(sid as *const u8, length) }.to_vec())
    })
}

/// String-SID (`S-1-5-21-...`) of the current process token's user — the
/// Windows account this process runs as. Used as the sole principal in
/// every SDDL built here.
fn current_user_sid_string() -> io::Result<String> {
    with_process_token_user(|_buffer, token_user| {
        let sid = token_user.User.Sid;
        let mut sid_str_ptr: *mut u16 = ptr::null_mut();
        // SAFETY: `sid` is the valid PSID obtained from the token. On
        // success the OS allocates the string via `LocalAlloc`, freed just
        // below.
        let ok = unsafe { ConvertSidToStringSidW(sid, &mut sid_str_ptr) };
        if ok == 0 || sid_str_ptr.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `sid_str_ptr` is a valid null-terminated UTF-16 string
        // freshly returned by `ConvertSidToStringSidW`; the length scan
        // and the slice both stay inside it.
        let len = unsafe {
            let mut n = 0isize;
            while *sid_str_ptr.offset(n) != 0 {
                n += 1;
            }
            n as usize
        };
        let slice = unsafe { std::slice::from_raw_parts(sid_str_ptr, len) };
        let sid_string = String::from_utf16_lossy(slice);
        // SAFETY: freeing the same `LocalAlloc`-backed buffer exactly
        // once.
        unsafe {
            LocalFree(sid_str_ptr as *mut c_void);
        }
        Ok(sid_string)
    })
}

/// Opens the current process token, reads its `TOKEN_USER` and runs
/// `use_it` with the buffer + a typed view into it. The token handle is
/// closed on every exit path; the buffer outlives the closure's result.
fn with_process_token_user<T>(
    use_it: impl FnOnce(&[u8], &TOKEN_USER) -> io::Result<T>,
) -> io::Result<T> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: `GetCurrentProcess()` returns a pseudo-handle that needs no
    // closing. `OpenProcessToken` writes a real, owned handle into
    // `token` on success, closed below before returning.
    let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let result = (|| {
        // First call to learn the required buffer size.
        let mut needed: u32 = 0;
        // SAFETY: null buffer + 0 length is the documented way to query
        // the required size; `GetTokenInformation` writes it to `needed`
        // and returns an expected `ERROR_INSUFFICIENT_BUFFER` failure.
        unsafe {
            GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut needed);
        }
        if needed == 0 {
            return Err(io::Error::last_os_error());
        }
        let last_err = io::Error::last_os_error();
        if last_err.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
            return Err(last_err);
        }

        let mut buffer = vec![0u8; needed as usize];
        let mut actual: u32 = 0;
        // SAFETY: `buffer` is `needed` bytes, matching what the prior
        // call reported as required for a `TOKEN_USER` at this token.
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr() as *mut c_void,
                needed,
                &mut actual,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `buffer` was just filled by `GetTokenInformation` with
        // a `TOKEN_USER` — `TOKEN_USER.User.Sid` is a valid `PSID` for
        // the lifetime of `buffer` (it points INTO `buffer`, not to a
        // separate allocation).
        let token_user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
        use_it(&buffer, token_user)
    })();
    // SAFETY: `token` is a real, owned handle opened above — closed
    // exactly once, on every exit path (success or error) of this
    // function.
    unsafe {
        CloseHandle(token);
    }
    result
}

fn denied(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16 (with terminating NUL) of EXACTLY the path Rust inspected via
/// `symlink_metadata` — encoded losslessly from the `OsStr`, never through
/// `to_string_lossy`. Fails with [`io::ErrorKind::InvalidInput`] on
/// interior NULs, which Win32 would silently truncate.
fn path_to_wide(path: &Path) -> io::Result<Vec<u16>> {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path contains an interior NUL",
        ));
    }
    wide.push(0);
    Ok(wide)
}
