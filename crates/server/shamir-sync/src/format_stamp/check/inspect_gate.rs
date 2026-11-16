//! Inspect-mode refusal decision for engine-opening offline tools
//! (design 3.3: `doctor`, `access-tree`, seed export refuse newer /
//! older / corrupt and stampless-below-base; they never write).

use std::path::Path;

use crate::format_stamp::check::{check_data_dir, StampMode, StampOutcome};
use crate::format_stamp::io::{inspect, IgnoreSet, StampState};
use crate::format_stamp::model::{EpochPolicy, StampError};

/// Run the Inspect check and map every observation an engine-opening tool
/// must not act on into a `StampError`. Never writes or deletes anything.
pub fn inspect_gate(dir: &Path, policy: &EpochPolicy) -> Result<StampOutcome, StampError> {
    match check_data_dir(dir, StampMode::Inspect, &IgnoreSet::none(), policy)? {
        StampOutcome::InspectedOlder => {
            // check_data_dir just classified the stamp as valid and older.
            let found = match inspect(dir) {
                StampState::Valid(stamp) => stamp.format_epoch(),
                _ => policy.current,
            };
            Err(StampError::OlderEpoch {
                dir: dir.to_path_buf(),
                found,
                current: policy.current,
            })
        }
        StampOutcome::InspectedStampless if policy.base < policy.current => {
            Err(StampError::AbsentOlderBase {
                dir: dir.to_path_buf(),
                current: policy.current,
            })
        }
        outcome => Ok(outcome),
    }
}
