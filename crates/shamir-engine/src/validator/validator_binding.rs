use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use shamir_types::types::record_id::RecordId;

use super::WriteOp;

/// A single validator-to-table binding stored in the info-twin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorBinding {
    /// Catalogue record `_id` of the validator (resolved from name at
    /// bind time).
    pub validator_id: RecordId,
    /// Which write operations this validator fires on.
    pub ops: SmallVec<[WriteOp; 4]>,
    /// Execution priority: lower = earlier. Range `[1000, 9999]`.
    pub priority: u16,
}
