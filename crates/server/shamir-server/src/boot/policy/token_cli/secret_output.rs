use std::io::{self, IsTerminal, Write};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FileIdInfo, GetFileInformationByHandleEx, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

pub enum SecretOutput {
    Stdout,
    File(std::fs::File, bool),
}

/// A rejected secret output destination. Nothing is ever written when this
/// error is returned.
#[derive(Debug, thiserror::Error)]
pub(super) enum OutputPathError {
    /// The path resolves into the reserved `audit.log*` namespace of the
    /// data directory: the active log, any rotation archive, or any entry
    /// below a reserved-named component, or into that same directory by
    /// on-disk identity. The reserved FILE NAME is matched
    /// ASCII-case-insensitively on EVERY platform — over-refusal is harmless
    /// and under-refusal is not; on a case-insensitive mount `AUDIT.LOG` IS
    /// the audit log, and on a case-sensitive filesystem refusing it only
    /// blocks a name nobody should use. Only the DIRECTORY-component
    /// comparison follows the platform.
    #[error(
        "the secret output path resolves to the audit log, one of its \
             archives, or something below that namespace; nothing was written"
    )]
    ReservedAuditName,
    /// An existing ancestor of the destination could not be resolved; the
    /// OS error detail is preserved.
    #[error("cannot resolve the secret output path: {0}")]
    Unresolvable(#[source] std::io::Error),
    #[error(
        "the secret output path ends with a path separator, which names a directory; \
             nothing was written"
    )]
    TrailingSeparator,
}

/// Directory-COMPONENT matching follows the PLATFORM, as a compile-time
/// predicate `cfg!(any(windows, target_os = "macos"))`: the default Windows
/// mounts and the default macOS APFS volume are case-insensitive, so on those
/// an `AUDIT.LOG` directory entry IS the audit log directory; elsewhere
/// (Linux, and a case-sensitive APFS volume) the comparison is byte-exact
/// because it names a distinct directory. This is ONLY about the DIRECTORY
/// prefix: the reserved FILE NAME is deliberately matched
/// ASCII-case-insensitively on EVERY platform (`reserved_name_matches`
/// below), because over-refusal of a name nobody should use is harmless.
fn dir_component_eq(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// True when `name` is the active `audit.log` or one of its rotation
/// archives. The FILE NAME is matched ASCII-case-insensitively on EVERY
/// platform because over-refusal is harmless and under-refusal is not: on a
/// case-insensitive mount (Windows, default macOS APFS, a case-insensitive
/// Linux mount) `AUDIT.LOG` IS the audit log, and on a case-sensitive
/// filesystem refusing it only blocks a name nobody should use. Only the
/// DIRECTORY prefix above (`dir_component_eq`) follows the platform.
fn reserved_name_matches(name: &std::ffi::OsStr) -> bool {
    const PREFIX: &[u8] = b"audit.log";
    let bytes = name.as_encoded_bytes();
    bytes.len() >= PREFIX.len() && bytes[..PREFIX.len()].eq_ignore_ascii_case(PREFIX)
}

/// True when `path` lies under `dir` and the component of `path` directly
/// below `dir` begins with the reserved `audit.log` prefix.
fn reserved_under(path: &Path, dir: &Path) -> bool {
    let mut path_components = path.components();
    for dir_component in dir.components() {
        let same = match path_components.next() {
            Some(c) => dir_component_eq(c.as_os_str(), dir_component.as_os_str()),
            None => return false,
        };
        if !same {
            return false;
        }
    }
    path_components
        .next()
        .is_some_and(|next| reserved_name_matches(next.as_os_str()))
}

/// The identity of the directory behind `path`, independent of how it was
/// spelled: two paths with different components (a bind mount, say) compare
/// equal when they reach the same directory.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DirIdentity {
    dev: u64,
    ino: u64,
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DirIdentity {
    volume: u64,
    index: [u8; 16],
}

#[cfg(unix)]
fn dir_identity(path: &Path) -> Option<DirIdentity> {
    let meta = std::fs::metadata(path).ok()?;
    Some(DirIdentity {
        dev: meta.dev(),
        ino: meta.ino(),
    })
}

#[cfg(windows)]
fn dir_identity(path: &Path) -> Option<DirIdentity> {
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is a null-terminated UTF-16 path valid for the call;
    // no security attributes and no template file. When the call succeeds
    // the returned handle is owned by this function and closed exactly once
    // below.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return None;
    }
    let mut info = FILE_ID_INFO::default();
    // SAFETY: `info` is a correctly sized and aligned out-buffer for the
    // `FileIdInfo` class and the call only writes into it; `handle` is live
    // for the duration of the call.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            &mut info as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    // SAFETY: `handle` came from the successful open above, is owned by this
    // function, and is closed exactly once here; the query result lives in
    // `ok`, so no early return can skip the close.
    unsafe { CloseHandle(handle) };
    if ok == 0 {
        return None;
    }
    Some(DirIdentity {
        volume: info.VolumeSerialNumber,
        index: info.FileId.Identifier,
    })
}

/// The first entry of `path.ancestors()` that exists and is a directory:
/// the directory a new final name below `path` would be created in.
fn nearest_existing_dir(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|ancestor| std::fs::metadata(ancestor).is_ok_and(|meta| meta.is_dir()))
        .map(Path::to_path_buf)
}

/// The destination where `prepare`'s create_new will land: the canonicalized
/// nearest existing ancestor of `path` plus the lexical remainder, so the
/// FINAL entry is the ORIGINAL name (existing entries and final links are
/// never followed for creation) while `..` segments and symlinked or missing
/// directories above it are resolved. This is operator input checked under
/// the maintenance lock; canonicalization is best-effort resolution, not a
/// defense against a concurrent adversary outside the audited directories.
fn lexical_candidate(path: &Path) -> Result<PathBuf, OutputPathError> {
    let mut last_missing: Option<std::io::Error> = None;
    for ancestor in path.ancestors().skip(1) {
        match std::fs::canonicalize(ancestor) {
            Ok(canonical_ancestor) => {
                // The remainder is what follows the resolved ancestor.
                let skip = ancestor.components().count();
                let mut candidate = canonical_ancestor;
                for component in path.components().skip(skip) {
                    // `PathBuf::push` re-parses a Windows component such as
                    // `z:notes` as a drive prefix and replaces the base; a
                    // ':' is also an NTFS stream separator. Refuse it there.
                    if cfg!(windows) && component.as_os_str().to_string_lossy().contains(':') {
                        return Err(OutputPathError::Unresolvable(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "a secret output path component must not contain ':' on Windows",
                        )));
                    }
                    candidate.push(component);
                }
                return Ok(candidate);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => last_missing = Some(e),
            Err(e) => return Err(OutputPathError::Unresolvable(e)),
        }
    }
    Err(OutputPathError::Unresolvable(last_missing.unwrap_or_else(
        || {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no resolvable ancestor of the output path",
            )
        },
    )))
}

fn is_dangling_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
}

/// Rejects a secret output path that resolves into the reserved audit
/// namespace of `data_dir` (the DIRECTORY prefix compares
/// case-insensitively on Windows and macOS; the reserved FILE NAME is
/// matched case-insensitively everywhere) and returns the validated
/// destination for `prepare` and cleanup, so
/// no later link resolution can differ from the validated one. Checked, in
/// order: the lexical destination under its canonical parent (a reserved
/// final name is refused even when an existing link points out), the whole
/// resolved entry (a final link must not reach a reserved target under a
/// harmless name), and — for a DANGLING final link only — its target through
/// the same bounded single-hop logic. A missing `data_dir` has no namespace
/// to collide with; a bare or fully relative path is resolved against the
/// current directory; a path with no final file name falls through to the
/// ordinary exclusive-create error of `prepare`.
pub(super) fn validate_out_file(path: &Path, data_dir: &Path) -> Result<PathBuf, OutputPathError> {
    // A trailing separator names a directory, never a file. Backslash is a
    // separator only on Windows: on Unix it is an ordinary filename byte, so
    // `secret\` is a legal destination there and must not be refused.
    let raw = path.as_os_str().as_encoded_bytes();
    if raw.ends_with(b"/") || (cfg!(windows) && raw.ends_with(b"\\")) {
        return Err(OutputPathError::TrailingSeparator);
    }
    let canonical_dir = match std::fs::canonicalize(data_dir) {
        Ok(dir) => dir,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(path.to_path_buf()),
        Err(e) => return Err(OutputPathError::Unresolvable(e)),
    };
    let path = if path.is_relative() {
        std::env::current_dir()
            .map_err(OutputPathError::Unresolvable)?
            .join(path)
    } else {
        path.to_path_buf()
    };
    let destination = lexical_candidate(&path)?;
    if reserved_under(&destination, &canonical_dir) {
        return Err(OutputPathError::ReservedAuditName);
    }
    // Components do not identify a directory: a bind mount reaches the same
    // directory under another spelling. Components are the FALLBACK;
    // identity is authoritative when it is available.
    if let (Some(dest_dir), Some(dir_identity_canonical)) = (
        nearest_existing_dir(&destination),
        dir_identity(&canonical_dir),
    ) {
        if dir_identity(&dest_dir) == Some(dir_identity_canonical)
            && destination.file_name().is_some_and(reserved_name_matches)
        {
            return Err(OutputPathError::ReservedAuditName);
        }
    }
    if let Ok(full) = std::fs::canonicalize(&path) {
        if reserved_under(&full, &canonical_dir) {
            return Err(OutputPathError::ReservedAuditName);
        }
        return Ok(destination);
    }
    if is_dangling_symlink(&path) {
        if let Ok(target) = std::fs::read_link(&path) {
            let anchored = if target.is_relative() {
                destination
                    .parent()
                    .map(|parent| parent.join(target))
                    .unwrap_or_else(|| destination.clone())
            } else {
                target
            };
            let target_candidate = lexical_candidate(&anchored)?;
            if reserved_under(&target_candidate, &canonical_dir) {
                return Err(OutputPathError::ReservedAuditName);
            }
            if let Ok(full_target) = std::fs::canonicalize(&anchored) {
                if reserved_under(&full_target, &canonical_dir) {
                    return Err(OutputPathError::ReservedAuditName);
                }
            }
        }
    }
    Ok(destination)
}

pub fn prepare(path: Option<&Path>, terminal: bool) -> anyhow::Result<SecretOutput> {
    if let Some(path) = path {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options
            .open(path)
            .map_err(|_| anyhow::anyhow!("cannot exclusively create secret output file"))?;
        Ok(SecretOutput::File(file, cfg!(windows)))
    } else if terminal {
        Ok(SecretOutput::Stdout)
    } else {
        anyhow::bail!("secret output unavailable; specify --out-file")
    }
}

pub fn deliver(
    output: SecretOutput,
    secret: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> io::Result<()> {
    match output {
        SecretOutput::Stdout => {
            stdout.write_all(secret.as_bytes())?;
            stdout.write_all(b"\n")
        }
        SecretOutput::File(mut file, warn_windows) => {
            if warn_windows {
                stderr.write_all(b"warning: Windows file ACLs were not tightened\n")?;
            }
            file.write_all(secret.as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()
        }
    }
}

pub fn stdout_is_terminal() -> bool {
    io::stdout().is_terminal()
}
