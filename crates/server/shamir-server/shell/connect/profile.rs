//! Connection profiles (design `admin-shell.md` §3.2 "Profiles", §7).
//!
//! A profile is a strict flat Ktav `.ktav` file in the operator's private
//! profile directory holding ONLY allowlisted, non-secret fields. Loading
//! fails closed: every path component is opened relative to the previous
//! handle with symlink/reparse following disabled, the profile directory
//! and leaf file must be owner-private and owned by the current OS
//! account, with exactly one directory entry. Nothing is ever written or
//! created here.
//!

use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use zeroize::Zeroizing;

#[cfg(unix)]
use rustix::fs::{fstat, openat, FileType, Mode, OFlags};
#[cfg(unix)]
use rustix::process::geteuid;
use std::io::Read;

use crate::shell::output::ShellError;

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::os::windows::io::FromRawHandle;
#[cfg(windows)]
use std::ptr;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, INVALID_HANDLE_VALUE};
#[cfg(windows)]
use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
#[cfg(windows)]
use windows_sys::Win32::Security::{
    AclSizeInformation, CopySid, EqualSid, GetAce, GetAclInformation, GetLengthSid,
    GetSecurityDescriptorControl, GetTokenInformation, IsValidSid, TokenUser, ACCESS_ALLOWED_ACE,
    ACE_HEADER, ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PSID,
    SE_DACL_PRESENT, SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER,
};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FileAttributeTagInfo, FileStandardInfo, GetFileInformationByHandleEx,
    FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_SHARE_READ, FILE_STANDARD_INFO, OPEN_EXISTING,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// The verified connection profile (design §3.2: never a secret).
///
/// Deserialized strictly with `ktav::from_str`: unknown keys are refused
/// (`deny_unknown_fields`), every field is a scalar string, and raw
/// parser errors are collapsed into the fixed `InvalidProfile` refusal.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::shell) struct Profile {
    /// Required. `host:port` for `tls-tcp`, the pipe/socket path for `ipc`.
    pub(in crate::shell) address: String,
    /// Required. Exactly `tls-tcp` or `ipc` — everything else is a typed
    /// §3.2 refusal at connect time (plaintext/token/WSS are not PR1).
    pub(in crate::shell) transport: String,
    /// Required SCRAM user name, printable, separator-free.
    pub(in crate::shell) username: String,
    /// Optional captured `SHA256(server_pub_key)` hex pin. Absent is
    /// allowed HERE (the design says the profile may be provisioned
    /// without a pin yet); the connection layer refuses with
    /// `ShellError::MissingPin` when it is absent at connect time.
    pub(in crate::shell) pin: Option<String>,
    /// Optional TLS SNI override, hostname form.
    pub(in crate::shell) sni: Option<String>,
    /// Optional CA PEM path (token transports, PR2+; parsed and bounded,
    /// never used by PR1 SCRAM transports).
    pub(in crate::shell) ca_path: Option<PathBuf>,
    /// Optional routing database: plain, separator-free name — never a
    /// URL and never a header secret.
    pub(in crate::shell) database: Option<String>,
}

/// Profile files are strictly `NAME.ktav`.
const PROFILE_SUFFIX: &str = ".ktav";

/// Hard size bounds; a profile is a handful of short lines.
const MAX_NAME_LEN: usize = 64;
const MAX_ADDRESS_LEN: usize = 255;
const MAX_HOST_LEN: usize = 255;
const MAX_PATH_LEN: usize = 1024;
/// Bounded read cap (§3.2 secret-reader convention, generous for a profile).
const MAX_PROFILE_BYTES: usize = 16 * 1024;

// ── Public entry points ──────────────────────────────────────────────

/// Loads and verifies profile `name` (design §3.2, §4.1 `--profile`).
///
/// Fails closed on: an invalid profile name, a missing/unwritable
/// profiles directory (never created or fixed up), any symlink/reparse
/// component, wrong owner or mode/ACL, extra hardlinks, or a strict
/// Ktav parse that is not exactly the allowlisted flat field set.
pub(in crate::shell) fn load(name: &str) -> Result<Profile, ShellError> {
    let root = profiles_root()?;
    load_from(&root, name)
}

/// Loads and verifies profile `name` from an EXPLICIT profiles root
/// (scripted-regression entry point: no reliance on process-global
/// environment). Every precondition of [`load`] is preserved: the name is
/// whitelisted and the root/leaf are verified by the same fail-closed
/// handle-chain walk (`read_verified_profile`), then parsed strictly.
/// The root is only read — never created or fixed up.
pub(in crate::shell) fn load_from(root: &Path, name: &str) -> Result<Profile, ShellError> {
    validate_profile_name(name)?;
    let bytes = read_verified_profile(root, name)?;
    parse_profile(bytes)
}

/// Lists provisionable profile names (design §4.1 `profile list`); each
/// stem is accepted only after the same strict verification as [`load`].
pub(in crate::shell) fn list() -> Result<Vec<String>, ShellError> {
    let root = profiles_root()?;
    let mut names = Vec::new();
    for entry in verified_dir_names(&root)? {
        let Some(stem) = entry.strip_suffix(PROFILE_SUFFIX) else {
            continue;
        };
        if validate_profile_name(stem).is_err() {
            continue;
        }
        // Verify the leaf exactly as strictly as `load` before accepting
        // the stem; the bytes are dropped unread.
        read_verified_profile(&root, stem)?;
        names.push(stem.to_string());
    }
    names.sort();
    names.dedup();
    Ok(names)
}

// ── Profile-name whitelist (strict plain names) ─────────────────────

/// Strict plain-name whitelist (also used for the routing-database
/// field): ASCII letters/digits/`-`/`_`, 1..[`MAX_NAME_LEN`] chars, no
/// leading `-`, no `.`/`..`, no Windows reserved device name.
fn validate_plain_name(name: &str) -> Result<(), ShellError> {
    if name.is_empty() {
        return Err(ShellError::Usage);
    }
    if name.len() > MAX_NAME_LEN {
        return Err(ShellError::Usage);
    }
    let mut chars = name.chars();
    if matches!(chars.next(), Some('-') | Some('.')) {
        return Err(ShellError::Usage);
    }
    if name == "." || name == ".." {
        return Err(ShellError::Usage);
    }
    for ch in name.chars() {
        if !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_') {
            return Err(ShellError::Usage);
        }
    }
    if windows_reserved_device_stem(name) {
        return Err(ShellError::Usage);
    }
    Ok(())
}

fn validate_profile_name(name: &str) -> Result<(), ShellError> {
    validate_plain_name(name)
}

/// Windows reserved device names (CreateFile semantics): the stem before
/// the first dot, case-insensitively. `CON.txt` is still reserved.
fn windows_reserved_device_stem(name: &str) -> bool {
    let stem = match name.split_once('.') {
        Some((stem, _)) => stem,
        None => name,
    };
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    RESERVED.iter().any(|r| stem.eq_ignore_ascii_case(r))
}

/// The routing database is a plain name: separator-free, no URL or
/// header syntax, no whitespace or control bytes (design §3.2: "no URLs
/// and no auth-header secrets").
fn validate_database(name: &str) -> Result<(), ShellError> {
    validate_plain_name(name)
}

// ── Field validation (post-deserialize) ─────────────────────────────

/// A scalar value must be nonempty, bounded, and free of control bytes:
/// a file-supplied string must never smuggle line breaks or terminal
/// escapes into output (design §6).
fn validate_scalar(value: &str, max: usize) -> Result<(), ShellError> {
    if value.is_empty() || value.len() > max {
        return Err(ShellError::InvalidProfile);
    }
    if value.chars().any(|ch| ch.is_control() || ch == '\u{7f}') {
        return Err(ShellError::InvalidProfile);
    }
    Ok(())
}

/// `sni`: hostname form — ASCII alphanumerics, `-`, `.`, no leading or
/// trailing dot or hyphen in any label.
fn validate_hostname(value: &str) -> Result<(), ShellError> {
    validate_scalar(value, MAX_HOST_LEN)?;
    if !value.is_ascii() {
        return Err(ShellError::InvalidProfile);
    }
    for label in value.split('.') {
        if label.is_empty()
            || label.starts_with('-')
            || label.ends_with('-')
            || !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err(ShellError::InvalidProfile);
        }
    }
    Ok(())
}

/// Validates the deserialized [`Profile`] fields: required scalars are
/// nonempty, bounded, control-free; `username` is separator-free; `sni`
/// is strict hostname form; `ca_path`/`database` are bounded plain
/// names. A missing `pin` is allowed here (refused at connect time).
fn validate_fields(profile: &Profile) -> Result<(), ShellError> {
    validate_scalar(&profile.address, MAX_ADDRESS_LEN)?;
    validate_scalar(&profile.transport, MAX_NAME_LEN)?;
    validate_scalar(&profile.username, MAX_NAME_LEN)?;
    if profile
        .username
        .chars()
        .any(|ch| matches!(ch, '/' | '\\' | ':' | '@' | '"' | '\'') || ch.is_whitespace())
    {
        return Err(ShellError::InvalidProfile);
    }
    if let Some(sni) = profile.sni.as_deref() {
        validate_hostname(sni)?;
    }
    if let Some(ca_path) = profile.ca_path.as_deref() {
        let path = ca_path.to_string_lossy();
        validate_scalar(&path, MAX_PATH_LEN)?;
    }
    if let Some(database) = profile.database.as_deref() {
        validate_database(database)?;
    }
    Ok(())
}

// ── Strict Ktav parse ───────────────────────────────────────────────

/// Deserializes verified bytes via strict `ktav::from_str` ([`Profile`],
/// `deny_unknown_fields`); bytes stay in a [`Zeroizing`] buffer. Every
/// parse failure collapses into the fixed `InvalidProfile` refusal.
fn parse_profile(bytes: Zeroizing<Vec<u8>>) -> Result<Profile, ShellError> {
    let text = std::str::from_utf8(&bytes).map_err(|_| ShellError::InvalidProfile)?;
    let profile: Profile = ktav::from_str(text).map_err(|_| ShellError::InvalidProfile)?;
    validate_fields(&profile)?;
    Ok(profile)
}

// ── Default directories (design §3.2; read-only, never created) ─────

/// Resolves the profiles directory (fixed default location, fail-closed).
#[cfg(unix)]
fn profiles_root() -> Result<PathBuf, ShellError> {
    const RELATIVE: &str = "shamir-shell/profiles";
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(xdg) => {
            let xdg = PathBuf::from(xdg);
            if !xdg.is_absolute() {
                return Err(ShellError::Usage);
            }
            xdg
        }
        None => {
            let home = std::env::var_os("HOME").ok_or(ShellError::Usage)?;
            let home = PathBuf::from(home);
            if !home.is_absolute() {
                return Err(ShellError::Usage);
            }
            home.join(".config")
        }
    };
    Ok(base.join(RELATIVE))
}

#[cfg(windows)]
fn profiles_root() -> Result<PathBuf, ShellError> {
    const RELATIVE: &str = r"ShamirDB\shell\profiles";
    let base = std::env::var_os("LOCALAPPDATA").ok_or(ShellError::Usage)?;
    let base = PathBuf::from(base);
    if !base.is_absolute() {
        return Err(ShellError::Usage);
    }
    Ok(base.join(RELATIVE))
}

fn profile_file_name(name: &str) -> String {
    let mut file = String::with_capacity(name.len() + PROFILE_SUFFIX.len());
    file.push_str(name);
    file.push_str(PROFILE_SUFFIX);
    file
}

// ── Bounded read (shared by both platforms) ─────────────────────────

/// Bounded whole-file read through std `File` with a hard
/// [`MAX_PROFILE_BYTES`] cap.
fn read_bounded(mut file: std::fs::File) -> Result<Zeroizing<Vec<u8>>, ShellError> {
    // Preallocated to the hard cap; both buffers are [`Zeroizing`] so
    // plaintext never lingers in untracked bytes.
    let mut bytes = Zeroizing::new(Vec::with_capacity(MAX_PROFILE_BYTES));
    let mut scratch = Zeroizing::new([0u8; 4096]);
    loop {
        let read = file
            .read(&mut scratch[..])
            .map_err(|_| ShellError::InvalidProfile)?;
        if read == 0 {
            break;
        }
        if bytes.len() + read > MAX_PROFILE_BYTES {
            return Err(ShellError::InvalidProfile);
        }
        bytes.extend_from_slice(&scratch[..read]);
    }
    Ok(bytes)
}

// ── Platform: verified open + bounded read (Unix) ───────────────────

/// Strict fd-relative `Normal`-only component list of an absolute path.
#[cfg(unix)]
fn unix_components(root: &Path) -> Result<Vec<String>, ShellError> {
    let mut out = Vec::new();
    for component in root.components() {
        match component {
            Component::Normal(part) => {
                let name = part.to_str().ok_or(ShellError::InsecureProfile)?;
                out.push(name.to_string());
            }
            Component::RootDir => {}
            _ => return Err(ShellError::InsecureProfile),
        }
    }
    if out.is_empty() {
        return Err(ShellError::InsecureProfile);
    }
    Ok(out)
}

/// Opens the profiles directory through a fail-closed `openat` handle
/// chain; only the FINAL directory must be euid-owned `0700` (shared
/// parents are not owner-checked), and `verify_private_dir` re-checks
/// it by path afterwards.
#[cfg(unix)]
fn open_verified_dir(root: &Path) -> Result<std::os::fd::OwnedFd, ShellError> {
    // `load_from` accepts an explicit root (scripted entry point): a
    // relative root would resolve against the process CWD, so refuse it.
    if !root.is_absolute() {
        return Err(ShellError::InsecureProfile);
    }
    // NOTE: `Mode` bitflag `BitOr` is not `const`, so the mask is built
    // in an ordinary expression instead of a `const` item.
    let dir_mode = Mode::RUSR | Mode::WUSR | Mode::XUSR;

    let dir_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let euid = geteuid().as_raw();
    let components = unix_components(root)?;
    let last = components.len() - 1;

    let mut fd = openat(rustix::fs::CWD, "/", dir_flags, Mode::empty())
        .map_err(|_| ShellError::InsecureProfile)?;
    for (index, component) in components.iter().enumerate() {
        fd = openat(&fd, component.as_str(), dir_flags, Mode::empty())
            .map_err(|_| ShellError::InsecureProfile)?;
        let stat = fstat(&fd).map_err(|_| ShellError::InsecureProfile)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
            return Err(ShellError::InsecureProfile);
        }
        if index == last && (stat.st_uid != euid || Mode::from_raw_mode(stat.st_mode) != dir_mode) {
            return Err(ShellError::InsecureProfile);
        }
    }
    shamir_private_dir::verify_private_dir(root).map_err(|_| ShellError::InsecureProfile)?;
    Ok(fd)
}

/// Reads `root/NAME.ktav` through a fail-closed handle chain; the leaf
/// must be a regular, euid-owned `0600` file with hardlink count 1.
#[cfg(unix)]
fn read_verified_profile(root: &Path, name: &str) -> Result<Zeroizing<Vec<u8>>, ShellError> {
    let file_mode = Mode::RUSR | Mode::WUSR;

    let dir_fd = open_verified_dir(root)?;
    let leaf_flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
    let fd = openat(
        &dir_fd,
        profile_file_name(name).as_str(),
        leaf_flags,
        Mode::empty(),
    )
    .map_err(|_| ShellError::InsecureProfile)?;
    drop(dir_fd);
    let stat = fstat(&fd).map_err(|_| ShellError::InsecureProfile)?;
    let euid = geteuid().as_raw();
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != euid
        || Mode::from_raw_mode(stat.st_mode) != file_mode
        || stat.st_nlink != 1
    {
        return Err(ShellError::InsecureProfile);
    }
    // The verified leaf handle is the ONLY channel to the file contents:
    // it was opened through the nofollow chain above and never leaves
    // this function until the read completes.
    let file = std::fs::File::from(fd);
    read_bounded(file)
}

/// Verified directory listing (Unix) via rustix `Dir::read` (rustix 1.1.6
/// also implements `Iterator for Dir` as `next` calling `Self::read` on
/// both the linux_raw and libc backends; iterating directly is equivalent).
#[cfg(unix)]
fn verified_dir_names(root: &Path) -> Result<Vec<String>, ShellError> {
    let fd = open_verified_dir(root)?;
    let dir = rustix::fs::Dir::new(fd).map_err(|_| ShellError::InsecureProfile)?;
    let mut names = Vec::new();
    for entry in dir {
        let entry = entry.map_err(|_| ShellError::InsecureProfile)?;
        let name = entry
            .file_name()
            .to_str()
            .ok_or(ShellError::InsecureProfile)?;
        if name != "." && name != ".." {
            names.push(name.to_string());
        }
    }
    Ok(names)
}

// ── Platform: verified open + bounded read (Windows) ────────────────

/// RAII owner of a Win32 `HANDLE`; closed exactly once on every path.
#[cfg(windows)]
struct OwnedHandle(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: `self.0` is a Win32 handle opened by `CreateFileW` and
        // owned exclusively by this struct; `CloseHandle` is its
        // documented destructor and is called exactly once here.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// `READ_CONTROL` (`0x00020000`): right to read the security descriptor.
/// Defined locally (like the helper crate defines `FILE_ALL_ACCESS`) to
/// avoid pulling a whole feature for one constant.
#[cfg(windows)]
const READ_CONTROL: u32 = 0x0002_0000;
/// `FILE_GENERIC_READ` (`0x00120089`).
#[cfg(windows)]
const FILE_GENERIC_READ: u32 = 0x0012_0089;
/// `FILE_ALL_ACCESS` = `STANDARD_RIGHTS_REQUIRED | SYNCHRONIZE`.
#[cfg(windows)]
const FILE_ALL_ACCESS: u32 = 0x001f_01ff;
/// `ACCESS_ALLOWED_ACE_TYPE` (`0`).
#[cfg(windows)]
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
/// `OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE` (`0x03`): the flag set a
/// protected profile-directory ACE is provisioned with.
#[cfg(windows)]
const DIRECTORY_ACE_FLAGS: u8 = (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE) as u8;
/// `OBJECT_INHERIT_ACE` (`0x1`), defined locally like the other masks.
#[cfg(windows)]
const OBJECT_INHERIT_ACE: u32 = 0x1;
/// `CONTAINER_INHERIT_ACE` (`0x2`), defined locally like the other masks.
#[cfg(windows)]
const CONTAINER_INHERIT_ACE: u32 = 0x2;
/// `FILE_ATTRIBUTE_REPARSE_POINT` (`0x0400`).
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
/// `GENERIC_ALL` (`0x10000000`): the generic-map form a fixture may use.
#[cfg(windows)]
const GENERIC_ALL: u32 = 0x1000_0000;

#[cfg(windows)]
fn wide_path(path: &Path) -> Result<Vec<u16>, ShellError> {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return Err(ShellError::InsecureProfile);
    }
    wide.push(0);
    Ok(wide)
}

/// Opens `path` with reparse following disabled and no write/delete sharing.
///
/// `FILE_FLAG_OPEN_REPARSE_POINT` plus `FILE_FLAG_BACKUP_SEMANTICS` pin the
/// opened object for the checks and the read that follow.
#[cfg(windows)]
fn open_pinned(path: &Path, desired_access: u32) -> Result<OwnedHandle, ShellError> {
    let wide = wide_path(path)?;
    // SAFETY: `wide` is a valid null-terminated UTF-16 path for the call;
    // no security attributes, no template. The returned handle (or
    // `INVALID_HANDLE_VALUE`) is owned by the caller from this point.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            desired_access,
            FILE_SHARE_READ, // write/delete sharing refused: object pinned
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return Err(ShellError::InvalidProfile);
    }
    Ok(OwnedHandle(handle))
}

/// Handle-bound attributes: no reparse point, hardlink count 1, and the
/// expected directory/file kind.
#[cfg(windows)]
fn check_handle_attributes(
    handle: windows_sys::Win32::Foundation::HANDLE,
    expect_directory: bool,
) -> Result<(), ShellError> {
    let mut standard = FILE_STANDARD_INFO {
        AllocationSize: 0,
        EndOfFile: 0,
        NumberOfLinks: 0,
        DeletePending: false,
        Directory: false,
    };
    // SAFETY: `standard` is a valid, correctly sized out-buffer for
    // `FileStandardInfo`; the call only writes it.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileStandardInfo,
            &mut standard as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        )
    };
    if ok == 0 {
        return Err(ShellError::InsecureProfile);
    }
    // The hardlink-count-1 pin applies to files only; a directory on
    // NTFS legitimately reports more than one link.
    if !expect_directory && standard.NumberOfLinks != 1 {
        return Err(ShellError::InsecureProfile);
    }
    if standard.Directory != expect_directory {
        return Err(ShellError::InsecureProfile);
    }
    let mut tag = FILE_ATTRIBUTE_TAG_INFO {
        FileAttributes: 0,
        ReparseTag: 0,
    };
    // SAFETY: `tag` is a valid, correctly sized out-buffer for
    // `FileAttributeTagInfo`; the call only writes it.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileAttributeTagInfo,
            &mut tag as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    };
    if ok == 0 {
        return Err(ShellError::InsecureProfile);
    }
    if tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(ShellError::InsecureProfile);
    }
    Ok(())
}

/// Reads the current process token's user SID (two-call
/// `GetTokenInformation` pattern, mirroring `shamir-private-dir`).
#[cfg(windows)]
fn current_user_sid() -> Result<Vec<usize>, ShellError> {
    let mut token: windows_sys::Win32::Foundation::HANDLE = ptr::null_mut();
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle needing no
    // close; `OpenProcessToken` writes an owned handle into `token`,
    // closed below on every path.
    let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
    if ok == 0 {
        return Err(ShellError::InsecureProfile);
    }
    let result = (|| {
        let mut needed: u32 = 0;
        // SAFETY: null buffer with length 0 is the documented size probe;
        // `needed` receives the required buffer size.
        unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut needed) };
        if needed == 0 {
            return Err(ShellError::InsecureProfile);
        }
        let buffer_bytes = needed as usize;
        // Word-aligned backing storage (`Vec<usize>` keeps 4/8-byte
        // alignment on all supported targets), as required by
        // `GetTokenInformation`'s `TokenUser` layout.
        let mut buffer = vec![0usize; buffer_bytes.div_ceil(std::mem::size_of::<usize>())];
        let mut actual: u32 = 0;
        // SAFETY: `buffer` is word-aligned and carries at least `needed`
        // bytes as reported by the probe (the extra tail bytes, if any,
        // are never read: only `buffer_bytes` are meaningful).
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr() as *mut core::ffi::c_void,
                needed,
                &mut actual,
            )
        };
        if ok == 0 || (actual as usize) > buffer_bytes {
            return Err(ShellError::InsecureProfile);
        }
        // The buffer begins with a `TOKEN_USER` whose `User.Sid` points
        // INTO the same retained buffer, not at the buffer's start. Read
        // the header (the buffer is word-aligned, so `read_unaligned` is
        // belt-and-braces), then copy the SID out with
        // `GetLengthSid`/`CopySid` into our own word-aligned allocation
        // (PSIDs require 4-byte alignment, and this owned storage
        // outlives the token buffer).
        // SAFETY: `actual` (the bytes the OS reports as written) covers
        // at least `size_of::<TOKEN_USER>()`; the read makes no further
        // alignment assumption.
        if (actual as usize) < std::mem::size_of::<TOKEN_USER>() {
            return Err(ShellError::InsecureProfile);
        }
        let token_user = unsafe { ptr::read_unaligned(buffer.as_ptr() as *const TOKEN_USER) };
        let sid = token_user.User.Sid;
        if sid.is_null() {
            return Err(ShellError::InsecureProfile);
        }
        // SAFETY: `sid` is the OS-returned PSID inside the live buffer.
        let len = unsafe { GetLengthSid(sid) };
        if len == 0 {
            return Err(ShellError::InsecureProfile);
        }
        // Owned, word-aligned SID storage (`Vec<usize>` of the same
        // capacity in bytes as `len`), suitable for `EqualSid` later.
        let sid_len = len as usize;
        let mut sid_bytes = vec![0usize; sid_len.div_ceil(std::mem::size_of::<usize>())];
        // SAFETY: `sid_bytes` is word-aligned and carries at least `len`
        // bytes, as reported by `GetLengthSid` for the source `sid`.
        let copied = unsafe { CopySid(len, sid_bytes.as_mut_ptr() as PSID, sid) };
        if copied == 0 {
            return Err(ShellError::InsecureProfile);
        }
        Ok(sid_bytes)
    })();
    // SAFETY: `token` is the owned handle opened above; closed exactly
    // once on every exit path.
    unsafe {
        CloseHandle(token);
    }
    result
}

/// Verifies the handle's DACL: the owner must be the current user's SID
/// and the DACL must be protected, present, non-null, with exactly one
/// allow ACE for the current user. `dir_form` (FINAL directory)
/// requires `FILE_ALL_ACCESS` with `OBJECT_INHERIT_ACE |
/// CONTAINER_INHERIT_ACE`; `!dir_form` (leaf FILE) requires an explicitly
/// provisioned ACE with EXACTLY zero flags and mask `FILE_ALL_ACCESS` or
/// `GENERIC_ALL`, with the DACL still `SE_DACL_PROTECTED` (checked
/// above). Ancestors are attribute-checked only (shared Windows
/// prefixes are not owner-checked here).
#[cfg(windows)]
fn check_owner_only_dacl(
    handle: windows_sys::Win32::Foundation::HANDLE,
    dir_form: bool,
) -> Result<(), ShellError> {
    let current_sid = current_user_sid()?;
    let mut owner: PSID = ptr::null_mut();
    let mut dacl: *mut windows_sys::Win32::Security::ACL = ptr::null_mut();
    let mut descriptor: *mut core::ffi::c_void = ptr::null_mut();
    // SAFETY: `handle` is a valid open Win32 handle; the out-params are
    // valid pointers of the documented types. On success `descriptor` is
    // an OS-allocated buffer owned here, freed via `LocalFree` below on
    // every path.
    let error = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if error != 0 {
        return Err(ShellError::InsecureProfile);
    }
    let result = (|| {
        if owner.is_null() {
            return Err(ShellError::InsecureProfile);
        }
        // SAFETY: `owner` and `current_sid.as_ptr()` are valid PSIDs
        // (OS-returned above / freshly built from the process token);
        // `EqualSid` only reads.
        if unsafe { EqualSid(owner, current_sid.as_ptr() as PSID) } == 0 {
            return Err(ShellError::InsecureProfile);
        }
        let mut control: u16 = 0;
        let mut revision: u32 = 0;
        // SAFETY: `descriptor` is the valid PSECURITY_DESCRIPTOR returned
        // above; the out-params are valid pointers.
        let ok = unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) };
        if ok == 0 {
            return Err(ShellError::InsecureProfile);
        }
        if control & SE_DACL_PRESENT == 0 || dacl.is_null() {
            return Err(ShellError::InsecureProfile);
        }
        if control & SE_DACL_PROTECTED == 0 {
            return Err(ShellError::InsecureProfile);
        }
        let mut size = ACL_SIZE_INFORMATION {
            AceCount: 0,
            AclBytesInUse: 0,
            AclBytesFree: 0,
        };
        // SAFETY: `dacl` is the valid PACL returned above; `size` is the
        // matching out-buffer for `AclSizeInformation`.
        let ok = unsafe {
            GetAclInformation(
                dacl,
                &mut size as *mut _ as *mut core::ffi::c_void,
                std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
        };
        if ok == 0 {
            return Err(ShellError::InsecureProfile);
        }
        if size.AceCount != 1 {
            return Err(ShellError::InsecureProfile);
        }
        let mut ace: *mut core::ffi::c_void = ptr::null_mut();
        // SAFETY: `dacl` is valid and ACE index 0 exists (`AceCount == 1`);
        // `GetAce` writes a pointer INTO the ACL buffer, alive for the
        // rest of this closure.
        if unsafe { GetAce(dacl, 0, &mut ace) } == 0 {
            return Err(ShellError::InsecureProfile);
        }
        if ace.is_null() {
            return Err(ShellError::InsecureProfile);
        }
        // Stage 1, BEFORE any ACE byte is read: `GetAce` places `ace`
        // inside the ACL buffer, but the bounds oracle is the
        // OS-reported `AclBytesInUse`. Prove the ACE's actual offset
        // from the ACL base with a checked address difference, then
        // prove the complete `ACE_HEADER` fits in the remaining bytes
        // before reading it.
        let acl_use = size.AclBytesInUse as usize;
        // Strict-provenance checked address difference (`.addr()`).
        let ace_offset = match (ace as *const u8)
            .addr()
            .checked_sub((dacl as *const u8).addr())
        {
            Some(off) if off <= acl_use => off,
            _ => return Err(ShellError::InsecureProfile),
        };
        let ace_room = match acl_use.checked_sub(ace_offset) {
            Some(room) => room,
            None => return Err(ShellError::InsecureProfile),
        };
        if ace_room < std::mem::size_of::<ACE_HEADER>() {
            return Err(ShellError::InsecureProfile);
        }
        // SAFETY: `ace` is the `GetAce` pointer for ACE 0 of the valid
        // ACL buffer; this read touches only the header bytes proven to
        // lie within `AclBytesInUse` bytes of the ACL base.
        let header = unsafe { ptr::read_unaligned(ace as *const ACE_HEADER) };
        // Layout constants: `ACE_HEADER` (4 bytes) + `Mask` (4) precede
        // the SID at offset 8; a minimal well-formed SID occupies 8
        // bytes (revision, count, authority). `MIN_ACE_BYTES` must also
        // cover the whole fixed `ACCESS_ALLOWED_ACE` layout.
        const ACE_SID_OFFSET: usize = 8;
        const SID_MIN_LEN: usize = 8;
        const MIN_ACE_BYTES: usize = ACE_SID_OFFSET + SID_MIN_LEN;
        const _: () = assert!(MIN_ACE_BYTES >= std::mem::size_of::<ACCESS_ALLOWED_ACE>());
        let ace_size = header.AceSize as usize;
        // Stage 2: the DECLARED `AceSize` must cover header + mask + a
        // minimal SID and fit in the bytes remaining from the ACE's
        // actual offset (not from the ACL base).
        if ace_size < MIN_ACE_BYTES || ace_size > ace_room {
            return Err(ShellError::InsecureProfile);
        }
        if header.AceType != ACCESS_ALLOWED_ACE_TYPE {
            return Err(ShellError::InsecureProfile);
        }
        // Stage 3: every byte touched below lies in the proven window
        // `ace_offset .. ace_offset + ace_size` (within `AclBytesInUse`).
        // SAFETY: `ace + ACE_SID_OFFSET` and `+ 1` are within that
        // window; unaligned reads only.
        let sid_ptr = unsafe { (ace as *const u8).add(ACE_SID_OFFSET) };
        let sid_rev = unsafe { ptr::read_unaligned(sid_ptr) };
        let sid_subauth = unsafe { ptr::read_unaligned(sid_ptr.add(1)) };
        if sid_rev != 1 {
            // SDDL_REVISION_1 is the only SID revision Windows defines.
            return Err(ShellError::InsecureProfile);
        }
        // Checked arithmetic: `checked_mul` first (the `4*n` tail), then
        // `checked_add` for the fixed 8-byte SID header, chained through
        // `and_then` so neither can wrap.
        let sid_len = match 4usize
            .checked_mul(sid_subauth as usize)
            .and_then(|tail| 8usize.checked_add(tail))
        {
            Some(l) => l,
            None => return Err(ShellError::InsecureProfile),
        };
        let ace_tail = match ace_size.checked_sub(ACE_SID_OFFSET) {
            Some(t) => t,
            None => return Err(ShellError::InsecureProfile),
        };
        if sid_len != ace_tail {
            // SID length must exactly fill the ACE's declared tail.
            return Err(ShellError::InsecureProfile);
        }
        // SAFETY: `ace_size >= MIN_ACE_BYTES` covers the fixed layout
        // (const assert above) and the whole ACE lies inside the in-use
        // ACL bytes; this value copy touches exactly those bytes.
        let allowed = unsafe { ptr::read_unaligned(ace as *const ACCESS_ALLOWED_ACE) };
        let flags_ok = if dir_form {
            allowed.Header.AceFlags == DIRECTORY_ACE_FLAGS
        } else {
            // Leaf FILE: EXACTLY zero flags — no inherit bits (a file
            // ACE cannot propagate) and no inherited-copy marker; the
            // `SE_DACL_PROTECTED` control enforced above already rules
            // out anything inherited.
            allowed.Header.AceFlags == 0
        };
        if !flags_ok {
            return Err(ShellError::InsecureProfile);
        }
        let mask_ok = if dir_form {
            // Directories are re-checked by `verify_private_dir` (exact
            // `FILE_ALL_ACCESS` + `OICI` flags); anything else fails here.
            allowed.Mask == FILE_ALL_ACCESS
        } else {
            allowed.Mask == FILE_ALL_ACCESS || allowed.Mask == GENERIC_ALL
        };
        if !mask_ok {
            return Err(ShellError::InsecureProfile);
        }
        // SAFETY: `sid_ptr` is the validated SID start (offset
        // `ACE_SID_OFFSET` inside the proven in-use window) with exactly
        // `sid_len` bytes addressable (proven equal to the declared
        // tail, which fits `AclBytesInUse`); `IsValidSid` and `EqualSid`
        // only read. This raw pointer — not a field reference narrowed
        // from the struct copy — carries the provenance passed to Win32.
        let ace_sid = sid_ptr as PSID;
        if unsafe { IsValidSid(ace_sid) } == 0 {
            return Err(ShellError::InsecureProfile);
        }
        if unsafe { EqualSid(ace_sid, current_sid.as_ptr() as PSID) } == 0 {
            return Err(ShellError::InsecureProfile);
        }
        Ok(())
    })();
    // SAFETY: `descriptor` was allocated by `GetSecurityInfo` via
    // `LocalAlloc` on success and is freed exactly once, on every path.
    unsafe {
        LocalFree(descriptor);
    }
    result
}

/// Walks the verified ancestor chain (Windows, shared by the file and
/// listing paths): every prefix is opened handle-bound, attribute-checked
/// and retained; the design's `verify_private_dir` re-checks the profile
/// directory (last prefix) by path.
#[cfg(windows)]
fn walk_verified_ancestors(root: &Path) -> Result<Vec<OwnedHandle>, ShellError> {
    if !root.is_absolute() {
        return Err(ShellError::InsecureProfile);
    }
    let mut prefixes: Vec<PathBuf> = Vec::new();
    let mut current = PathBuf::new();
    for component in root.components() {
        match component {
            Component::Prefix(prefix) => current.push(prefix.as_os_str()),
            Component::RootDir => current.push(std::path::MAIN_SEPARATOR_STR),
            Component::Normal(part) => {
                current.push(part);
                prefixes.push(current.clone());
            }
            _ => {
                return Err(ShellError::InsecureProfile);
            }
        }
    }
    if prefixes.is_empty() {
        return Err(ShellError::InsecureProfile);
    }
    let mut handles: Vec<OwnedHandle> = Vec::new();
    for (index, prefix) in prefixes.iter().enumerate() {
        let handle = open_pinned(prefix, READ_CONTROL)?;
        check_handle_attributes(handle.0, true)?;
        let last = index == prefixes.len() - 1;
        // ONLY the FINAL directory is owner-DACL-checked (shared
        // ancestors legitimately carry inherited DACLs); it uses the
        // designed protected-directory form on the SAME pinned handle
        // (READ_CONTROL was already requested), then is re-checked by
        // `verify_private_dir` by path.
        if last {
            check_owner_only_dacl(handle.0, true)?;
        }
        handles.push(handle);
        if last {
            shamir_private_dir::verify_private_dir(prefix)
                .map_err(|_| ShellError::InsecureProfile)?;
        }
    }
    Ok(handles)
}

/// Reads `root\NAME.ktav` through a fail-closed verified walk: pinned
/// ancestors under protected owner-only DACLs, a leaf regular file owned
/// by the current user under the same DACL, hardlink count 1, bounded
/// read.
#[cfg(windows)]
fn read_verified_profile(root: &Path, name: &str) -> Result<Zeroizing<Vec<u8>>, ShellError> {
    let file_name = profile_file_name(name);
    let mut current = root.to_path_buf();
    current.push(&file_name);
    let leaf_path = current;

    let handles = walk_verified_ancestors(root)?;
    // Leaf: READ_CONTROL for the ACL check plus generic read for the data.
    let leaf = open_pinned(&leaf_path, READ_CONTROL | FILE_GENERIC_READ)?;
    check_handle_attributes(leaf.0, false)?;
    // The ACL is verified on the SAME handle the read uses, so the
    // descriptor that was checked is the object that is read. The leaf is
    // a FILE: `dir_form = false` (explicit ACE, exactly zero flags, mask
    // `FILE_ALL_ACCESS`/`GENERIC_ALL`, protected DACL), never the
    // directory OICI form.
    check_owner_only_dacl(leaf.0, false)?;

    // Hand the raw handle to a std `File` for the common bounded read;
    // our RAII wrapper's Drop is disengaged so the handle closes exactly
    // once (when the `File` drops).
    let raw = leaf.0;
    std::mem::forget(leaf);
    drop(handles);
    // SAFETY: `raw` is the exclusive, still-open Win32 handle pinned by
    // `leaf` (its Drop was disengaged above), opened `READ_CONTROL |
    // FILE_GENERIC_READ` on a verified regular file; transferring it to
    // `std::fs::File` via `from_raw_handle` gives the `File` sole
    // ownership and the duty to `CloseHandle` it on drop.
    let file = unsafe { std::fs::File::from_raw_handle(raw) };
    read_bounded(file)
}

/// Verified directory listing (Windows); names are re-validated in
/// [`list`].
#[cfg(windows)]
fn verified_dir_names(root: &Path) -> Result<Vec<String>, ShellError> {
    let handles = walk_verified_ancestors(root)?;
    let entries = std::fs::read_dir(root).map_err(|_| ShellError::InsecureProfile)?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| ShellError::InsecureProfile)?;
        let name = entry.file_name();
        if let Some(name) = name.to_str() {
            if name != "." && name != ".." {
                names.push(name.to_string());
            }
        }
    }
    drop(handles);
    Ok(names)
}
