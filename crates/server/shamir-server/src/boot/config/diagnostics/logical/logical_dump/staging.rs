use std::fs::{self, File};
use std::io;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt as _;
use std::path::Path;

use anyhow::{bail, Context, Result};
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use rustix::fs::{renameat_with, RenameFlags, CWD};
use tempfile::{Builder, TempDir};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

/// Live export stage prefix. The export root must be a private directory the
/// server owns; the runner's startup cleanup removes only directories whose
/// name matches exactly `.shamir-export-` plus 6 ASCII alphanumeric random
/// characters (tempfile's default random suffix length), so a crashed job's
/// stage never survives a restart. Offline staging keeps `.logical-export-`.
pub(crate) const LIVE_STAGE_PREFIX: &str = ".shamir-export-";

/// Synchronous, non-async publication of a verified staged artifact. The
/// hard link is the commit point: an absent final name is created atomically,
/// an existing one is NEVER replaced — semantically stronger than a renaming
/// overwrite, and it fails closed on Unix races. Unsupported filesystems fail
/// before publication; there is deliberately no copy fallback. A Unix
/// directory-sync failure AFTER the link returns the explicit "published,
/// retained" durability error: the artifact is complete and verified and
/// must be reported as published, never removed or retried.
pub(crate) fn publish_stage(stage_dir: &Path, output: &Path) -> Result<()> {
    commit_stage_link(stage_dir, output)?.context(
        "logical-export published a verified artifact, but final directory sync failed; final file retained",
    )
}

/// Only pre-publication failures return `Err`. After the pre-rename directory
/// sync (where supported), `before_rename` can still veto publication. Once the
/// atomic no-replace rename starts, it cannot be preempted. A post-rename
/// directory-sync failure is logged and the artifact stays published.
pub(crate) fn publish_live_stage(
    stage_dir: &Path,
    output: &Path,
    before_rename: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let source = stage_dir.join("dump");
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    let parent = {
        let parent = File::open(
            output
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?;
        parent.sync_all()?;
        parent
    };
    before_rename()?;
    rename_no_replace(&source, output).context("logical-export atomic no-replace rename")?;
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    if parent.sync_all().is_err() {
        tracing::warn!(
            "logical-export published a verified artifact, but final directory sync failed; final file retained"
        );
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn rename_no_replace(source: &Path, output: &Path) -> io::Result<()> {
    renameat_with(CWD, source, CWD, output, RenameFlags::NOREPLACE)?;
    Ok(())
}

#[cfg(windows)]
fn rename_no_replace(source: &Path, output: &Path) -> io::Result<()> {
    fn wide_path(path: &Path) -> io::Result<Vec<u16>> {
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "logical-export path contains an interior NUL",
            ));
        }
        wide.push(0);
        Ok(wide)
    }

    let source = wide_path(source)?;
    let output = wide_path(output)?;
    // SAFETY: Both pointers refer to owned, NUL-terminated UTF-16 buffers
    // without interior NULs, valid for the duration of this call. Zero is a
    // valid flag mask and allows neither replacement nor cross-volume copying.
    if unsafe { MoveFileExW(source.as_ptr(), output.as_ptr(), 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple", windows)))]
fn rename_no_replace(_source: &Path, _output: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "logical-export atomic no-replace rename is unsupported on this target",
    ))
}

/// The commit point is the single atomic no-clobber hard link: an absent
/// final name is created, an existing one is NEVER replaced — there is
/// deliberately no `output.exists()` inference and no copy fallback. The
/// returned inner result is the post-link Unix directory sync: `Err` there
/// means the artifact IS already published.
fn commit_stage_link(stage_dir: &Path, output: &Path) -> Result<Result<()>> {
    #[cfg(unix)]
    let parent = File::open(
        output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?;
    #[cfg(unix)]
    parent.sync_all()?;
    fs::hard_link(stage_dir.join("dump"), output)
        .context("logical-export atomic no-replace publication")?;
    #[cfg(unix)]
    return Ok(parent.sync_all());
    #[cfg(not(unix))]
    Ok(Ok(()))
}

/// Attempt-owned sibling stage. No final pathname exists until verification.
/// TempDir removes failed/cancelled attempts, after all file handles are dropped.
pub(super) struct Stage {
    directory: TempDir,
}

impl Stage {
    pub(super) fn new(output: &Path) -> Result<Self> {
        if output.try_exists()? {
            bail!("logical-export output already exists");
        }
        let parent = output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        Ok(Self {
            directory: Builder::new()
                .prefix(".logical-export-")
                .tempdir_in(parent)?,
        })
    }

    /// Live job stage: same RAII and no-clobber guarantees, but the distinct
    /// [`LIVE_STAGE_PREFIX`] so export-root cleanup recognizes it (exact
    /// prefix + 6 ASCII alphanumeric characters; tempfile's default random
    /// suffix).
    pub(super) fn for_live(output: &Path) -> Result<Self> {
        if output.try_exists()? {
            bail!("logical-export output already exists");
        }
        let parent = output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        Ok(Self {
            directory: Builder::new()
                .prefix(LIVE_STAGE_PREFIX)
                .rand_bytes(6)
                .tempdir_in(parent)?,
        })
    }

    pub(super) fn path(&self) -> &Path {
        self.directory.path()
    }

    /// The hard-link operation is the publication commit point. A subsequent
    /// Unix directory-sync failure returns an explicit "published" durability
    /// error: the final file is complete and verified, and MUST NOT be removed
    /// or blindly retried/replaced. Windows has file-sync/readback durability
    /// only; filesystems without hard-link support fail before publication.
    pub(super) fn publish(&self, output: &Path) -> Result<()> {
        publish_stage(self.path(), output)
    }

    pub(super) fn create(&self) -> Result<File> {
        Ok(File::options()
            .write(true)
            .create_new(true)
            .open(self.path().join("dump"))?)
    }
}
