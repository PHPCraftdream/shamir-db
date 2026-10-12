use shamir_engine::meta::MetaEnvelope;
use shamir_types::types::value::QueryValue;

use crate::{DbError, DbResult};

/// QueryValue needs a self-describing format and the envelope codec (bincode)
/// cannot deserialize it, so the envelope carries MessagePack bytes.
pub(super) fn encode_schema_records(records: &[QueryValue]) -> DbResult<Vec<u8>> {
    let payload = rmp_serde::to_vec_named(records).map_err(|e| DbError::Codec(e.to_string()))?;
    MetaEnvelope::new(payload)
        .encode()
        .map_err(|e| DbError::Codec(e.to_string()))
}

pub(super) fn decode_schema_records(bytes: &[u8]) -> DbResult<Vec<QueryValue>> {
    let payload: Vec<u8> = MetaEnvelope::open(bytes).map_err(|e| DbError::Codec(e.to_string()))?;
    rmp_serde::from_slice(&payload).map_err(|e| DbError::Codec(e.to_string()))
}
