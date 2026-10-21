//! Frame layout (112-byte header + payload):
//! `magic(8) ns(32) key(32) payload_len u64 LE(8) sha256(bytes[0..80] || payload)(32) payload`.
//! The magic is the explicit 8-byte constant `SHMWIC\0\x01`.

use sha2::{Digest, Sha256};

pub(crate) const HEADER_LEN: usize = 112;
pub(crate) const MAX_PAYLOAD: usize = 4 * 1024 * 1024;
const MAGIC: &[u8; 8] = b"SHMWIC\x00\x01";

pub(crate) fn encode(namespace: &[u8; 32], key: &[u8; 32], payload: &[u8]) -> Option<Vec<u8>> {
    if payload.len() > MAX_PAYLOAD {
        return None;
    }
    let mut frame = Vec::with_capacity(HEADER_LEN + payload.len());
    frame.extend_from_slice(MAGIC);
    frame.extend_from_slice(namespace);
    frame.extend_from_slice(key);
    frame.extend_from_slice(&u64::try_from(payload.len()).ok()?.to_le_bytes());
    let mut digest = Sha256::new();
    digest.update(&frame);
    digest.update(payload);
    frame.extend_from_slice(&digest.finalize());
    frame.extend_from_slice(payload);
    Some(frame)
}

pub(crate) fn decode<'a>(
    frame: &'a [u8],
    namespace: &[u8; 32],
    key: &[u8; 32],
) -> Option<&'a [u8]> {
    if frame.len() < HEADER_LEN
        || &frame[..8] != MAGIC
        || &frame[8..40] != namespace
        || &frame[40..72] != key
    {
        return None;
    }
    let payload_len = usize::try_from(u64::from_le_bytes(frame[72..80].try_into().ok()?)).ok()?;
    if payload_len > MAX_PAYLOAD || frame.len() != HEADER_LEN.checked_add(payload_len)? {
        return None;
    }
    let mut digest = Sha256::new();
    digest.update(&frame[..80]);
    digest.update(&frame[HEADER_LEN..]);
    let digest_bytes = digest.finalize();
    (digest_bytes.as_slice() == &frame[80..112]).then_some(&frame[HEADER_LEN..])
}
