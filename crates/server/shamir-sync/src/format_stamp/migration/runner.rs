use std::fs;
use std::path::Path;

use crate::format_stamp::codec::{decode, STAMP_FILE_NAME, STAMP_TMP_NAME};
use crate::format_stamp::io::write_stamp;
use crate::format_stamp::model::{
    now_unix_ms, CorruptKind, EpochPolicy, FormatStamp, LastFormatMigration, StampError,
};

use super::error::MigrationError;
use super::journal::{read, resume_plan, JournalRecord, JournalWriter, ResumePlan};
use super::module::{FormatStep, PlanLine, StepContext};
use super::registry::chain_for;

/// Result after the format-migration chain and final stamp have been persisted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatMigrationOutcome {
    pub final_epoch: u64,
    pub last_module_id: Option<String>,
}

struct StartState {
    from_epoch: u64,
    completed: Option<FormatMigrationOutcome>,
}

/// Read the current migration boundary without creating or changing any files.
fn read_start_state(dir: &Path, policy: EpochPolicy) -> Result<StartState, MigrationError> {
    match fs::read(dir.join(STAMP_FILE_NAME)) {
        Ok(bytes) => {
            let stamp = decode(&bytes, dir).map_err(|source| MigrationError::Refused { source })?;
            let epoch = stamp.format_epoch();
            if epoch > policy.current {
                return Err(MigrationError::Refused {
                    source: StampError::NewerEpoch {
                        dir: dir.to_path_buf(),
                        found: epoch,
                        supported: policy.current,
                    },
                });
            }
            if epoch < policy.base {
                return Err(MigrationError::Refused {
                    source: StampError::OlderEpoch {
                        dir: dir.to_path_buf(),
                        found: epoch,
                        current: policy.current,
                    },
                });
            }
            if epoch == policy.current {
                return Ok(StartState {
                    from_epoch: epoch,
                    completed: Some(FormatMigrationOutcome {
                        final_epoch: epoch,
                        last_module_id: stamp
                            .last_format_migration()
                            .map(|entry| entry.id.to_owned()),
                    }),
                });
            }
            Ok(StartState {
                from_epoch: epoch,
                completed: None,
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match fs::symlink_metadata(dir.join(STAMP_TMP_NAME)) {
                Ok(_) => {
                    return Err(MigrationError::Refused {
                        source: StampError::Corrupt {
                            dir: dir.to_path_buf(),
                            kind: CorruptKind::TornTmp,
                        },
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(MigrationError::Io {
                        dir: dir.to_path_buf(),
                        source,
                    });
                }
            }
            Ok(StartState {
                from_epoch: policy.base,
                completed: None,
            })
        }
        Err(source) => Err(MigrationError::Io {
            dir: dir.to_path_buf(),
            source,
        }),
    }
}

/// Build a read-only plan; steps receive a no-write journal handle.
pub fn plan_migration(
    dir: &Path,
    policy: EpochPolicy,
    modules: &[&dyn FormatStep],
) -> Result<Vec<PlanLine>, MigrationError> {
    let start = read_start_state(dir, policy)?;
    if start.completed.is_some() {
        return Ok(Vec::new());
    }
    let chain = chain_for(EpochPolicy::new(policy.current, start.from_epoch), modules)?;
    let journal = JournalWriter::plan_handle(dir);
    let mut lines = Vec::new();
    for step in chain {
        let descriptor = step.descriptor();
        lines.push(PlanLine {
            description: format!(
                "{}: {} (touches: {}; estimate: {})",
                descriptor.id,
                descriptor.summary,
                descriptor.touches.join(", "),
                descriptor.work_estimate
            ),
        });
        let context = StepContext {
            data_dir: dir,
            journal: &journal,
            policy,
        };
        lines.extend(step.plan(&context)?);
    }
    Ok(lines)
}

/// Execute or resume a format-migration chain synchronously.
pub fn run_migration(
    dir: &Path,
    policy: EpochPolicy,
    modules: &[&dyn FormatStep],
    binary_version: &str,
    integrity: &dyn Fn(&Path) -> Result<(), MigrationError>,
) -> Result<FormatMigrationOutcome, MigrationError> {
    let start = read_start_state(dir, policy)?;
    if let Some(outcome) = start.completed {
        return Ok(outcome);
    }
    let chain = chain_for(EpochPolicy::new(policy.current, start.from_epoch), modules)?;
    if chain.is_empty() {
        return Ok(FormatMigrationOutcome {
            final_epoch: policy.current,
            last_module_id: None,
        });
    }
    let expected_modules: Vec<String> = chain
        .iter()
        .map(|step| step.descriptor().id.to_owned())
        .collect();
    let records = read(dir).map_err(|error| journal_read_error(dir, error))?;
    let resume = resume_plan(
        &records,
        &expected_modules,
        start.from_epoch,
        policy.current,
        binary_version,
    );
    let cursor = match resume {
        ResumePlan::Fresh => 0,
        ResumePlan::Resume { next_module_index } => next_module_index,
        ResumePlan::Mismatch => {
            return Err(MigrationError::BrokenChain {
                from: start.from_epoch,
                expected: policy.current,
                found: None,
            });
        }
    };
    let writer = match resume {
        ResumePlan::Fresh => {
            let writer = JournalWriter::create(dir)?;
            writer.append(&JournalRecord::ChainStarted {
                from_epoch: start.from_epoch,
                to_epoch: policy.current,
                binary_version: binary_version.to_owned(),
                modules: expected_modules,
                unix_ms: now_unix_ms(),
            })?;
            writer
        }
        ResumePlan::Resume { .. } => JournalWriter::open_existing(dir)?,
        ResumePlan::Mismatch => unreachable!(),
    };

    for step in chain.iter().skip(cursor) {
        let descriptor = step.descriptor();
        writer.append(&JournalRecord::StepStarted {
            module: descriptor.id.to_owned(),
            from_epoch: descriptor.from_epoch,
            to_epoch: descriptor.to_epoch,
            unix_ms: now_unix_ms(),
        })?;
        let mut context = StepContext {
            data_dir: dir,
            journal: &writer,
            policy,
        };
        step.apply(&mut context)
            .map_err(|source| MigrationError::StepFailed {
                module: descriptor.id,
                source: Box::new(source),
            })?;
        writer.append(&JournalRecord::StepFinished {
            module: descriptor.id.to_owned(),
            unix_ms: now_unix_ms(),
        })?;
    }

    integrity(dir)?;
    let last = chain.last().expect("non-empty chain").descriptor();
    let stamp = FormatStamp::with_epoch(policy.current, binary_version, now_unix_ms())
        .with_last_format_migration(LastFormatMigration {
            id: last.id.to_owned(),
            from_epoch: last.from_epoch,
            to_epoch: last.to_epoch,
            unix_ms: now_unix_ms(),
        });
    write_stamp(dir, &stamp).map_err(|source| MigrationError::Io {
        dir: dir.to_path_buf(),
        source: std::io::Error::other(source),
    })?;
    writer.append(&JournalRecord::ChainFinished {
        to_epoch: policy.current,
        unix_ms: now_unix_ms(),
    })?;
    Ok(FormatMigrationOutcome {
        final_epoch: policy.current,
        last_module_id: Some(last.id.to_owned()),
    })
}

fn journal_read_error(dir: &Path, error: super::journal::JournalParseError) -> MigrationError {
    MigrationError::Journal {
        dir: dir.to_path_buf(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, error.message),
    }
}
