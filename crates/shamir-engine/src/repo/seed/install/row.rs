use std::io::Cursor;

use bytes::Bytes;
use serde::Deserialize;
use shamir_collections::TFxMap;
use shamir_storage::error::{DbError, DbResult};
use shamir_tx::{collect_referenced_ids, remap_inner_value_bytes};
use shamir_types::types::value::InnerValue;

/// One verified row at a time: no table-sized buffers or synthetic events.
/// The artifact verifier owns container-length validation before this boundary.
pub(super) fn translate_row(bytes: Bytes, remap: &TFxMap<u64, u64>) -> DbResult<Bytes> {
    if bytes.is_empty() || bytes.len() > 16 * 1024 * 1024 {
        return Err(DbError::Codec("seed row exceeds frame budget".into()));
    }
    let mut decoder = rmp_serde::Deserializer::new(Cursor::new(bytes.as_ref()));
    decoder.set_max_depth(64);
    let value = InnerValue::deserialize(&mut decoder)
        .map_err(|e| DbError::Codec(format!("seed row: {e}")))?;
    if decoder.position()
        != u64::try_from(bytes.len()).map_err(|e| DbError::Codec(e.to_string()))?
        || !matches!(&value, InnerValue::Map(_))
    {
        return Err(DbError::Codec(
            "seed row is not exactly one record map".into(),
        ));
    }
    let mut ids = TFxMap::default();
    collect_referenced_ids(&value, &mut ids);
    if ids.keys().any(|id| !remap.contains_key(id)) {
        return Err(DbError::Codec(
            "seed row has an unbound source field".into(),
        ));
    }
    // Use the SAME recursive Map/List/Set translation as follower apply.
    remap_inner_value_bytes(bytes, remap)
        .map_err(|e| DbError::Codec(format!("seed translation: {e}")))
}
