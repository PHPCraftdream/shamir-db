use crate::types::record_id::RecordId;
use crate::types::value::InnerValue;

/// (id, unixTime created, unixTime updated, Data)
pub type RepoRecord = (RecordId, u64, u64, InnerValue);