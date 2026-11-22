use std::ffi::OsStr;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use fs4::fs_std::FileExt;

use super::SNAPSHOT_LEASE_FILE_NAME;

/// The single canonical-source decision of [`resolve_source`]: `Missing` is
/// a refusal (the caller must never fall back to the original pathname),
/// `Unmanaged` is a lock-free read of the resolved canonical path (an
/// export), `Managed` carries the canonical path plus the held shared lease.
#[derive(Debug)]
pub(super) enum Resolution {
    Missing,
    Unmanaged(PathBuf),
    Managed(PathBuf),
}

/// The reader outcome of [`read_snapshot`]: the classification of
/// [`Resolution`] with the lease already taken (the marker handle is held
/// open for the whole read; dropping it releases the lease).
#[derive(Debug)]
pub(super) enum SourceSnapshot {
    Missing,
    Unmanaged { canonical: PathBuf },
    Managed { canonical: PathBuf, lease: File },
}

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
/// Readers NEVER create the marker: a path is managed only when the marker
/// already exists in the snapshot's canonical parent (an export without the
/// marker reads lock-free even when it carries an owner tag), and the
/// marker is opened READ-ONLY (a shared `flock(LOCK_SH)`/`LockFileEx`
/// shared lock needs only read access, so a read-only-media export parent
/// still works).
pub(super) fn read_snapshot(snapshot_dir: &Path) -> io::Result<SourceSnapshot> {
    read_snapshot_with_lookup(snapshot_dir, |path| fs::canonicalize(path))
}

/// Private test seam for [`read_snapshot`]: the initial canonicalizing
/// lookup goes through `lookup`, so a test can make the snapshot
/// deterministically ABSENT at the decision point while the directory in
/// fact exists. Production passes `fs::canonicalize`.
pub(super) fn read_snapshot_with_lookup(
    snapshot_dir: &Path,
    lookup: impl FnOnce(&Path) -> io::Result<PathBuf>,
) -> io::Result<SourceSnapshot> {
    match resolve_source_with_lookup(snapshot_dir, lookup)? {
        Resolution::Missing => Ok(SourceSnapshot::Missing),
        Resolution::Unmanaged(canonical) => Ok(SourceSnapshot::Unmanaged { canonical }),
        Resolution::Managed(canonical) => {
            let marker = canonical.parent().expect("canonical snapshot has a parent");
            let file = open_existing_marker_read_only(&marker.join(SNAPSHOT_LEASE_FILE_NAME))?;
            FileExt::lock_shared(&file)?;
            Ok(SourceSnapshot::Managed {
                canonical,
                lease: file,
            })
        }
    }
}

/// The single canonical-source decision behind the reader entries: classify
/// `snapshot_dir` without taking or creating any lease. Fails closed on an
/// invalid marker (non-plain or non-file), per the timestamp/plain-directory
/// rules. Marker ABSENCE is `Resolution::Unmanaged` (an export); a marker
/// stat error other than NotFound is an `Err` — the two are never confused.
pub(super) fn resolve_source(snapshot_dir: &Path) -> io::Result<Resolution> {
    resolve_source_with_lookup(snapshot_dir, |path| fs::canonicalize(path))
}

fn resolve_source_with_lookup(
    snapshot_dir: &Path,
    lookup: impl FnOnce(&Path) -> io::Result<PathBuf>,
) -> io::Result<Resolution> {
    let snapshot = match lookup(snapshot_dir) {
        Ok(path) => path,
        // Missing or dangling at the decision point: the caller refuses;
        // it must never fall back to opening the original pathname.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Resolution::Missing),
        Err(error) => return Err(error),
    };
    let Some(root) = snapshot.parent() else {
        return Ok(Resolution::Unmanaged(snapshot));
    };
    if !snapshot.file_name().is_some_and(timestamp_name)
        || metadata_if_present(&snapshot)?.is_some_and(|meta| !meta.is_dir() || !plain(&meta))
    {
        return Ok(Resolution::Unmanaged(snapshot));
    }

    // Managed ONLY when the marker already exists in the canonical parent.
    // Readers never create it and never consult the snapshot sidecar: a
    // timestamped child tagged with an owner id but exported (copied) into
    // a parent without the marker is read lock-free.
    //
    // DOCUMENTED WINDOW (by design): only an ABSENT marker classifies the
    // snapshot as an unleased export, and that absence alone is no
    // protection — a reader that started before boot/repair provisioning
    // stays unleased for its WHOLE read, and the copy can become
    // deletion-eligible the moment the marker appears. Finish or postpone
    // such reads (guide 07-operations); readers never create or repair
    // the marker.
    let marker = root.join(SNAPSHOT_LEASE_FILE_NAME);
    if !marker_exists(&marker)? {
        return Ok(Resolution::Unmanaged(snapshot));
    }
    Ok(Resolution::Managed(snapshot))
}

/// Open an ALREADY-EXISTING, validated plain marker read-only (the caller
/// ran `marker_exists`, which fails closed on a non-plain/non-file leaf).
fn open_existing_marker_read_only(marker: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).open(marker)
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
