//! Fixed stamp frame constants and the header-only reader (design 2.3, D3).

use crate::format_stamp::model::CorruptKind;

/// On-disk file names; also the ignore-set members of design 3.1.
pub const STAMP_FILE_NAME: &str = "format.stamp";
pub const STAMP_TMP_NAME: &str = "format.stamp.tmp";

/// Magic prefix of every layout.
pub const MAGIC: [u8; 4] = *b"SHFS";
/// Frame layout this binary reads and writes.
pub const STAMP_LAYOUT_VERSION: u16 = 1;
/// Magic(4) + layout(2) + epoch(8) + body_len(4).
pub const HEADER_LEN: usize = 18;
/// SHA-256 trailer length.
pub const TRAILER_LEN: usize = 32;
/// Maximum JSON body length (design 2.3).
pub const MAX_BODY_LEN: usize = 65536;

/// Header-only reader: validates magic and frame length only. The checksum
/// is NOT verified (a future layout may differ); returns
/// `(layout_version, format_epoch)`.
pub fn read_header(bytes: &[u8]) -> Result<(u16, u64), CorruptKind> {
    if bytes.len() < HEADER_LEN {
        return Err(CorruptKind::ShortFile);
    }
    if bytes[0..4] != MAGIC {
        return Err(CorruptKind::BadMagic);
    }
    let layout_version = u16::from_le_bytes(bytes[4..6].try_into().expect("2 bytes"));
    let format_epoch = u64::from_le_bytes(bytes[6..14].try_into().expect("8 bytes"));
    let body_len = u32::from_le_bytes(bytes[14..18].try_into().expect("4 bytes")) as usize;
    if bytes.len() < HEADER_LEN + body_len + TRAILER_LEN {
        return Err(CorruptKind::Truncated);
    }
    Ok((layout_version, format_epoch))
}
