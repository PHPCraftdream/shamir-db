//! The total decision table of design 3.4.

use std::fs;
use std::path::Path;

use crate::format_stamp::codec::{STAMP_FILE_NAME, STAMP_TMP_NAME};
use crate::format_stamp::io::{data_present, inspect, write_stamp, IgnoreSet, StampState};
use crate::format_stamp::model::{now_unix_ms, CorruptKind, EpochPolicy, FormatStamp, StampError};
use tracing::{info, warn};

use super::{StampMode, StampOutcome};

/// Version recorded in `created_by` / `last_opened_by` (design 1.6).
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Run the startup rules once, after the data-dir lock and before any
/// store is opened (design 3.2). Writer mode may write; Inspect mode never
/// writes or deletes anything.
pub fn check_data_dir(
    dir: &Path,
    mode: StampMode,
    ignore: &IgnoreSet,
    policy: &EpochPolicy,
) -> Result<StampOutcome, StampError> {
    match inspect(dir) {
        StampState::Absent { leftover_tmp } => absent(dir, mode, ignore, policy, leftover_tmp),
        StampState::Corrupt { kind } => corrupt(dir, mode, ignore, policy, kind),
        StampState::NewerLayout { layout, epoch } => newer_layout(dir, policy, layout, epoch),
        StampState::Valid(stamp) => present(dir, mode, policy, stamp),
    }
}

/// Rows 3-6: no stamp.
fn absent(
    dir: &Path,
    mode: StampMode,
    ignore: &IgnoreSet,
    policy: &EpochPolicy,
    leftover_tmp: bool,
) -> Result<StampOutcome, StampError> {
    let has_data = data_present(dir, ignore);
    if has_data && leftover_tmp {
        // Row 6: an interrupted first write; the tmp is never trusted or
        // deleted here.
        return Err(StampError::Corrupt {
            dir: dir.to_path_buf(),
            kind: CorruptKind::TornTmp,
        });
    }
    if !has_data {
        // Row 3: (Absent, Empty) — write the stamp as the first file.
        return match mode {
            StampMode::Writer => {
                remove_leftover_tmp(dir)?;
                write_stamp(
                    dir,
                    &FormatStamp::with_epoch(policy.current, VERSION, now_unix_ms()),
                )?;
                info!("created format stamp, epoch {}", policy.current);
                Ok(StampOutcome::Created)
            }
            StampMode::Inspect => Ok(StampOutcome::InspectedStampless),
        };
    }
    // Rows 5a/5b: stampless data. Production constants guarantee
    // BASE <= CURRENT (design 3.6); a policy with base > current is treated
    // like base == current (adopt).
    if policy.base < policy.current {
        return Err(StampError::AbsentOlderBase {
            dir: dir.to_path_buf(),
            current: policy.current,
        });
    }
    match mode {
        StampMode::Writer => {
            write_stamp(
                dir,
                &FormatStamp::adopted(policy.base, VERSION, now_unix_ms()),
            )?;
            warn!(
                "data directory {} had no format stamp; adopted base epoch {}",
                dir.display(),
                policy.base
            );
            Ok(StampOutcome::Adopted)
        }
        StampMode::Inspect => Ok(StampOutcome::InspectedStampless),
    }
}

/// Rows 15/18: the stamp exists but does not decode.
fn corrupt(
    dir: &Path,
    mode: StampMode,
    ignore: &IgnoreSet,
    policy: &EpochPolicy,
    kind: CorruptKind,
) -> Result<StampOutcome, StampError> {
    if data_present(dir, ignore) {
        // Row 18: refuse, keep the evidence.
        return Err(StampError::Corrupt {
            dir: dir.to_path_buf(),
            kind,
        });
    }
    match mode {
        // Row 15, inspect: a damaged stamp is not an empty-dir case.
        StampMode::Inspect => Err(StampError::Corrupt {
            dir: dir.to_path_buf(),
            kind,
        }),
        StampMode::Writer => {
            // Row 15, writer: keep the evidence, then act as row 3.
            let evidence = dir.join(format!("format.stamp.corrupt-{}", now_unix_ms()));
            fs::rename(dir.join(STAMP_FILE_NAME), &evidence).map_err(|e| StampError::Io {
                dir: dir.to_path_buf(),
                source: e,
            })?;
            remove_leftover_tmp(dir)?;
            write_stamp(
                dir,
                &FormatStamp::with_epoch(policy.current, VERSION, now_unix_ms()),
            )?;
            warn!(
                "corrupt format stamp ({}) in a directory with no data; replaced",
                kind
            );
            Ok(StampOutcome::Created)
        }
    }
}

/// Rows 19/20: a frame this binary cannot fully read.
fn newer_layout(
    dir: &Path,
    policy: &EpochPolicy,
    layout: u16,
    epoch: u64,
) -> Result<StampOutcome, StampError> {
    if epoch > policy.current {
        return Err(StampError::NewerEpoch {
            dir: dir.to_path_buf(),
            found: epoch,
            supported: policy.current,
        });
    }
    Err(StampError::NewerLayout {
        dir: dir.to_path_buf(),
        layout,
        epoch,
    })
}

/// Rows 7-12 and 21: a valid stamp.
fn present(
    dir: &Path,
    mode: StampMode,
    policy: &EpochPolicy,
    stamp: FormatStamp,
) -> Result<StampOutcome, StampError> {
    let found = stamp.format_epoch();
    match found.cmp(&policy.current) {
        std::cmp::Ordering::Greater => Err(StampError::NewerEpoch {
            dir: dir.to_path_buf(),
            found,
            supported: policy.current,
        }),
        std::cmp::Ordering::Less => match mode {
            StampMode::Inspect => Ok(StampOutcome::InspectedOlder),
            StampMode::Writer => Err(StampError::OlderEpoch {
                dir: dir.to_path_buf(),
                found,
                current: policy.current,
            }),
        },
        std::cmp::Ordering::Equal => {
            if mode == StampMode::Inspect {
                return Ok(StampOutcome::Unchanged);
            }
            // Row 21: the stale tmp of an interrupted replacement is deleted
            // once the epoch rule lets the start proceed.
            remove_leftover_tmp(dir)?;
            let version_changed =
                stamp.last_opened_by().map(|actor| actor.version.as_str()) != Some(VERSION);
            if version_changed {
                write_stamp(dir, &stamp.with_last_opened(VERSION, now_unix_ms()))?;
                return Ok(StampOutcome::VersionUpdated);
            }
            Ok(StampOutcome::Unchanged)
        }
    }
}

fn remove_leftover_tmp(dir: &Path) -> Result<(), StampError> {
    let tmp = dir.join(STAMP_TMP_NAME);
    match fs::remove_file(&tmp) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(StampError::Io {
            dir: dir.to_path_buf(),
            source: e,
        }),
    }
}
