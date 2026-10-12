use std::io::{Cursor, Read};

use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};
use shamir_collections::TFxMap;
use shamir_tx::collect_referenced_ids;
use shamir_types::types::value::InnerValue;

use super::error::invalid;
use super::SeedError;

pub(crate) const FRAME_LIMIT: usize = 16 * 1024 * 1024;
pub(crate) const METADATA_LIMIT: u64 = 64 * 1024 * 1024;
pub(crate) const ARTIFACT_LIMIT: u64 = 1024 * 1024 * 1024 * 1024;
pub(crate) const CHUNK_LIMIT: u64 = 64 * 1024 * 1024;

pub(crate) fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, SeedError> {
    rmp_serde::to_vec_named(value).map_err(|e| invalid(e.to_string()))
}

pub(crate) fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, SeedError> {
    if bytes.len() as u64 > METADATA_LIMIT {
        return Err(invalid("decoder byte budget exceeded"));
    }
    super::validate_lengths::validate_lengths(bytes)?;
    let mut decoder = rmp_serde::Deserializer::new(Cursor::new(bytes));
    decoder.set_max_depth(64);
    let value = T::deserialize(&mut decoder).map_err(|e| invalid(e.to_string()))?;
    if decoder.position() != u64::try_from(bytes.len()).map_err(|e| invalid(e.to_string()))? {
        return Err(invalid("trailing MessagePack bytes"));
    }
    Ok(value)
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    // SHA-256 integrity only; neither this digest nor Complete authenticates a seed.
    hex::encode(Sha256::digest(bytes))
}

pub(crate) fn referenced(bytes: &[u8]) -> Result<TFxMap<u64, ()>, SeedError> {
    if bytes.len() > FRAME_LIMIT {
        return Err(invalid("row exceeds 16 MiB"));
    }
    decode::<super::record_shape::RecordShape>(bytes)?;
    let value: InnerValue = decode(bytes)?;
    if !matches!(value, InnerValue::Map(_)) {
        return Err(invalid("record payload is not a map"));
    }
    let mut ids = TFxMap::default();
    collect_referenced_ids(&value, &mut ids);
    Ok(ids)
}

pub(crate) fn hash_reader(reader: &mut impl Read) -> Result<(u64, String), SeedError> {
    let mut buffer = vec![0; 32 * 1024];
    let mut hash = Sha256::new();
    let mut size = 0u64;
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        size = size
            .checked_add(n as u64)
            .ok_or_else(|| invalid("byte overflow"))?;
        if size > ARTIFACT_LIMIT {
            return Err(invalid("artifact byte budget exceeded"));
        }
        hash.update(&buffer[..n]);
    }
    Ok((size, hex::encode(hash.finalize())))
}
