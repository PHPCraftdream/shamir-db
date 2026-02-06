use crate::types::common::TSet;
use crate::types::record_id::RecordId;

pub struct IndexRecord (TSet<RecordId>);

pub struct IndexRecordKey {
    pub is_unique: u8,
    pub path: Vec<u64>,
    pub hash1: u64,
    pub hash2: u64,
}