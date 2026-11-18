use std::fs::{self, File};
use std::path::Path;

use anyhow::{bail, Context, Result};
use tempfile::{Builder, TempDir};

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

/// Live publication: the same atomic no-clobber commit point, but a Unix
/// directory-sync failure AFTER the successful link is only logged with the
/// fixed warning and the artifact is reported as published (`Ok`): the final
/// file is complete and verified and must never be removed or retried. Only
/// a failure of the link itself is returned as an error.
pub(crate) fn publish_live_stage(stage_dir: &Path, output: &Path) -> Result<()> {
    if commit_stage_link(stage_dir, output)?.is_err() {
        eprintln!(
            "logical-export published a verified artifact, but final directory sync failed; final file retained"
        );
    }
    Ok(())
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
