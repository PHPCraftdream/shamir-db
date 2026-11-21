use std::ffi::OsStr;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use fs4::fs_std::FileExt;
use serde_json::{Map, Value};

use super::online::manifest::BACKUP_META_FILE_NAME;
use super::SNAPSHOT_LEASE_FILE_NAME;

const MAX_SIDECAR: u64 = 64 << 10;
const MAX_OWNER: usize = 256;

/// Provision the stable marker in a managed root before retention can
/// delete snapshots. Precondition: the caller validated `root` as a private
/// managed backup root.
pub(super) fn provision(root: &Path) -> io::Result<()> {
    open_or_create(root).map(drop)
}

/// Return `None` only on OS lock contention. Dropping the handle releases the lease.
pub(super) fn try_exclusive(root: &Path) -> io::Result<Option<File>> {
    let file = open_or_create(root)?;
    match FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(Some(file)),
        Err(error) if error.raw_os_error() == fs4::lock_contended_error().raw_os_error() => {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// Synchronous standalone readers only: may block behind retention. Hold the
/// returned handle through the last source read; do not remove the marker.
pub(super) fn read_snapshot(snapshot_dir: &Path) -> io::Result<Option<File>> {
    let snapshot = match fs::canonicalize(snapshot_dir) {
        Ok(path) => path,
        // Leave missing and dangling paths to the verifier's ManifestMissing check.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let Some(root) = snapshot.parent() else {
        return Ok(None);
    };
    if !snapshot.file_name().is_some_and(timestamp_name)
        || metadata_if_present(&snapshot)?.is_some_and(|meta| !meta.is_dir() || !plain(&meta))
    {
        return Ok(None);
    }

    let marker = root.join(SNAPSHOT_LEASE_FILE_NAME);
    // A pruner must publish the marker before deletion, so recheck after a
    // negative bootstrap probe in case deletion removed the legacy sidecar.
    if !marker_exists(&marker)? && !has_legacy_tag(&snapshot)? && !marker_exists(&marker)? {
        return Ok(None);
    }
    let file = open_or_create(root)?;
    FileExt::lock_shared(&file)?;
    Ok(Some(file))
}

fn open_or_create(root: &Path) -> io::Result<File> {
    // Check the leaf without following links; callers trust the private root
    // and cooperating processes never replace or unlink the marker.
    let root = root.components().collect::<PathBuf>();
    let meta = fs::symlink_metadata(&root)?;
    if !meta.is_dir() || !plain(&meta) {
        return Err(invalid_leaf(&root));
    }
    let path = root.join(SNAPSHOT_LEASE_FILE_NAME);
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if marker_exists(&path)? {
        return options.open(path);
    }
    let mut create = options.clone();
    create.create_new(true);
    #[cfg(unix)]
    create.mode(0o600);
    // Windows creation inherits the managed root's owner-only DACL.
    match create.open(&path) {
        Ok(file) => {
            // The create mode never exposes broader permissions. Restore owner
            // access through the handle if the umask stripped it.
            #[cfg(unix)]
            if file.metadata()?.permissions().mode() & 0o777 != 0o600 {
                file.set_permissions(fs::Permissions::from_mode(0o600))?;
            }
            Ok(file)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let meta = fs::symlink_metadata(&path)?;
            if !meta.is_file() || !plain(&meta) {
                return Err(invalid_leaf(&path));
            }
            options.open(path)
        }
        Err(error) => Err(error),
    }
}

fn marker_exists(path: &Path) -> io::Result<bool> {
    match metadata_if_present(path)? {
        Some(meta) if meta.is_file() && plain(&meta) => Ok(true),
        Some(_) => Err(invalid_leaf(path)),
        None => Ok(false),
    }
}

fn invalid_leaf(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("invalid snapshot lease path: {}", path.display()),
    )
}

fn metadata_if_present(path: &Path) -> io::Result<Option<Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(meta) => Ok(Some(meta)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn plain(meta: &Metadata) -> bool {
    #[cfg(windows)]
    {
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return false;
        }
    }
    !meta.file_type().is_symlink()
}

fn has_legacy_tag(snapshot: &Path) -> io::Result<bool> {
    let path = snapshot.join(BACKUP_META_FILE_NAME);
    let Some(meta) = metadata_if_present(&path)? else {
        return Ok(false);
    };
    if !meta.is_file() || !plain(&meta) || meta.len() > MAX_SIDECAR {
        return Ok(false);
    }
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    match file.take(meta.len() + 1).read_to_end(&mut bytes) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    }
    if bytes.len() as u64 != meta.len() {
        return Ok(false);
    }
    let Ok(sidecar) = serde_json::from_slice::<Map<String, Value>>(&bytes) else {
        return Ok(false);
    };
    Ok(sidecar.get("scheduled_retention").is_some_and(|tag| {
        tag.get("version").and_then(Value::as_u64) == Some(1)
            && tag
                .get("owner")
                .and_then(Value::as_str)
                .is_some_and(|owner| !owner.is_empty() && owner.len() <= MAX_OWNER)
    }))
}

fn timestamp_name(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let bytes = name.as_bytes();
    if bytes.len() != 15
        || bytes[8] != b'_'
        || !bytes[..8].iter().chain(&bytes[9..]).all(u8::is_ascii_digit)
    {
        return false;
    }
    let number = |start: usize, end: usize| {
        bytes[start..end]
            .iter()
            .fold(0u32, |value, &digit| value * 10 + u32::from(digit - b'0'))
    };
    let year = number(0, 4);
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match number(4, 6) {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&number(6, 8))
        && number(9, 11) < 24
        && number(11, 13) < 60
        && number(13, 15) < 60
}
