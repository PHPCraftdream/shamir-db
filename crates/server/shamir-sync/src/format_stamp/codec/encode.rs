//! Frame encoder: header + JSON body + SHA-256 trailer (design 2.3, D3).

use sha2::{Digest, Sha256};
use std::path::PathBuf;

use crate::format_stamp::model::{CorruptKind, FormatStamp, StampError};

use super::header::{HEADER_LEN, MAGIC, MAX_BODY_LEN, STAMP_LAYOUT_VERSION, TRAILER_LEN};

/// Encode a complete stamp frame. Fails only when the JSON body exceeds
/// [`MAX_BODY_LEN`] (classified `BadBody`: the body is not representable).
pub fn encode(stamp: &FormatStamp) -> Result<Vec<u8>, StampError> {
    let body = serde_json::to_string(stamp).map_err(|_| StampError::Corrupt {
        dir: PathBuf::new(),
        kind: CorruptKind::BadBody,
    })?;
    let body_bytes = body.as_bytes();
    if body_bytes.len() > MAX_BODY_LEN {
        return Err(StampError::Corrupt {
            dir: PathBuf::new(),
            kind: CorruptKind::BadBody,
        });
    }
    let mut out = Vec::with_capacity(HEADER_LEN + body_bytes.len() + TRAILER_LEN);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&STAMP_LAYOUT_VERSION.to_le_bytes());
    out.extend_from_slice(&stamp.format_epoch().to_le_bytes());
    out.extend_from_slice(&(body_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(body_bytes);
    let checksum = Sha256::digest(&out);
    out.extend_from_slice(&checksum);
    Ok(out)
}
