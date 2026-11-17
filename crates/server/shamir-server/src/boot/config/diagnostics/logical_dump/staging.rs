use std::fs::{self, File};
use std::path::Path;

use anyhow::{bail, Context, Result};
use tempfile::{Builder, TempDir};

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

    pub(super) fn path(&self) -> &Path {
        self.directory.path()
    }

    /// The hard-link operation is the publication commit point. A subsequent
    /// Unix directory-sync failure returns an explicit "published" durability
    /// error: the final file is complete and verified, and MUST NOT be removed
    /// or blindly retried/replaced. Windows has file-sync/readback durability
    /// only; filesystems without hard-link support fail before publication.
    pub(super) fn publish(&self, output: &Path) -> Result<()> {
        // A same-volume hard link atomically creates an absent name, never
        // replaces one. Unlike rename(), this also fails closed on Unix races.
        // Unsupported filesystems fail; there is deliberately no copy fallback.
        #[cfg(unix)]
        let parent = File::open(
            output
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?;
        #[cfg(unix)]
        parent.sync_all()?;
        fs::hard_link(self.path().join("dump"), output)
            .context("logical-export atomic no-replace publication")?;
        #[cfg(unix)]
        parent.sync_all().context("logical-export published a verified artifact, but final directory sync failed; final file retained")?;
        Ok(())
    }

    pub(super) fn create(&self) -> Result<File> {
        Ok(File::options()
            .write(true)
            .create_new(true)
            .open(self.path().join("dump"))?)
    }
}
