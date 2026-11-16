//! Frame decoder: layout gate, checksum, body and epoch agreement
//! (design 2.3, 2.5).

use sha2::{Digest, Sha256};
use std::path::Path;

use crate::format_stamp::model::{CorruptKind, FormatStamp, StampError, BASE_FORMAT_EPOCH};

use super::header::{read_header, HEADER_LEN, STAMP_LAYOUT_VERSION, TRAILER_LEN};

/// Decode a complete stamp frame. A newer layout version is reported as
/// [`StampError::NewerLayout`] with the header epoch (design 2.5).
pub fn decode(bytes: &[u8], dir: &Path) -> Result<FormatStamp, StampError> {
    let corrupt = |kind| StampError::Corrupt {
        dir: dir.to_path_buf(),
        kind,
    };
    let (layout_version, header_epoch) = read_header(bytes).map_err(corrupt)?;
    if layout_version > STAMP_LAYOUT_VERSION {
        return Err(StampError::NewerLayout {
            dir: dir.to_path_buf(),
            layout: layout_version,
            epoch: header_epoch,
        });
    }
    let body_end = bytes.len() - TRAILER_LEN;
    let expected = Sha256::digest(&bytes[..body_end]);
    if expected.as_slice() != &bytes[body_end..] {
        return Err(corrupt(CorruptKind::BadChecksum));
    }
    let body = std::str::from_utf8(&bytes[HEADER_LEN..body_end])
        .map_err(|_| corrupt(CorruptKind::BadBody))?;
    let stamp: FormatStamp =
        serde_json::from_str(body).map_err(|_| corrupt(CorruptKind::BadBody))?;
    if stamp.format_epoch() != header_epoch {
        return Err(corrupt(CorruptKind::EpochMismatch));
    }
    // An epoch below the base cannot be produced by any shipped binary.
    if stamp.format_epoch() < BASE_FORMAT_EPOCH {
        return Err(corrupt(CorruptKind::BadBody));
    }
    Ok(stamp)
}
