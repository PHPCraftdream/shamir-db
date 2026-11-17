use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use crate::format_stamp::migration::{
    FormatStep, MigrationError, ModuleDescriptor, PlanLine, StepContext,
};

pub const MARKER_FILE_NAME: &str = "failing_step.marker";
pub const MARKER_CONTENTS: &[u8] = b"e0001_0002";

/// Test-only migration step that can fail after recording its marker.
pub struct FailingStep {
    fail_after_marker: bool,
}

static DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    id: "e0001_0002",
    from_epoch: 1,
    to_epoch: 2,
    summary: "empty test step",
    touches: &[],
    work_estimate: "none",
};

impl FailingStep {
    pub const fn failing() -> Self {
        Self {
            fail_after_marker: true,
        }
    }

    pub const fn successful() -> Self {
        Self {
            fail_after_marker: false,
        }
    }
}

impl FormatStep for FailingStep {
    fn descriptor(&self) -> &ModuleDescriptor {
        &DESCRIPTOR
    }

    fn apply(&self, ctx: &mut StepContext<'_>) -> Result<(), MigrationError> {
        write_marker_once(ctx.data_dir)?;
        if self.fail_after_marker {
            return Err(MigrationError::Io {
                dir: ctx.data_dir.to_path_buf(),
                source: std::io::Error::other("intentional failing test step"),
            });
        }
        Ok(())
    }

    fn plan(&self, _ctx: &StepContext<'_>) -> Result<Vec<PlanLine>, MigrationError> {
        Ok(vec![PlanLine {
            description: "e0001_0002: test step marker".to_owned(),
        }])
    }
}

fn write_marker_once(data_dir: &Path) -> Result<(), MigrationError> {
    let marker = data_dir.join(MARKER_FILE_NAME);
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
    {
        Ok(mut file) => {
            file.write_all(MARKER_CONTENTS)
                .and_then(|()| file.sync_all())
                .map_err(|source| MigrationError::Io {
                    dir: data_dir.to_path_buf(),
                    source,
                })?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let contents = fs::read(&marker).map_err(|source| MigrationError::Io {
                dir: data_dir.to_path_buf(),
                source,
            })?;
            if contents != MARKER_CONTENTS {
                return Err(MigrationError::Io {
                    dir: data_dir.to_path_buf(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "failing-step marker has unexpected contents",
                    ),
                });
            }
        }
        Err(source) => {
            return Err(MigrationError::Io {
                dir: data_dir.to_path_buf(),
                source,
            });
        }
    }
    Ok(())
}
