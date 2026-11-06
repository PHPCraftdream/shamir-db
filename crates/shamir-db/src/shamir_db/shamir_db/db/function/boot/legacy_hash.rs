use std::hash::{Hash, Hasher};

/// Legacy rows only: the old 16-hex `wasm_hash` fingerprint.
///
/// Non-cryptographic; kept solely to verify old rows. Hashes the slice with
/// its length prefix, identical to hashing the `Vec<u8>` the old create path used.
pub(in crate::shamir_db::shamir_db) fn legacy_wasm_hash(bytes: &[u8]) -> String {
    let mut hasher = rustc_hash::FxHasher::default();
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}
