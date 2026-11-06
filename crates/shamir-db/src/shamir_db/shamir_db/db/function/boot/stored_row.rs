use base64::Engine;
use shamir_engine::function::{ModuleHash, TierMode};
use shamir_types::types::value::QueryValue;

use super::super::types::parse_optimize;
use super::legacy_hash::legacy_wasm_hash;

/// Outcome of checking stored bytes against the row's hash field.
pub(in crate::shamir_db::shamir_db) enum HashVerdict {
    /// Bytes match; carries the composite hash (computed in memory).
    Match(ModuleHash),
    /// Row has no hash field.
    Unknown,
    /// Bytes differ from the recorded hash.
    Mismatch { expected: String, actual: String },
}

/// Verify `bytes` against `module_hash`, else the legacy `wasm_hash`.
pub(in crate::shamir_db::shamir_db) fn verify_stored_hash(
    rec: &QueryValue,
    bytes: &[u8],
) -> HashVerdict {
    let computed = ModuleHash::of(bytes);
    if let Some(stored) = rec["module_hash"].as_str() {
        return match ModuleHash::from_hex(stored) {
            Ok(h) if h == computed => HashVerdict::Match(computed),
            _ => HashVerdict::Mismatch {
                expected: stored.to_string(),
                actual: computed.to_hex(),
            },
        };
    }
    if let Some(stored) = rec["wasm_hash"].as_str() {
        let legacy = legacy_wasm_hash(bytes);
        return if stored == legacy {
            HashVerdict::Match(computed)
        } else {
            HashVerdict::Mismatch {
                expected: stored.to_string(),
                actual: legacy,
            }
        };
    }
    HashVerdict::Unknown
}

/// Decode `wasm_b64`; `None` when the field is absent.
pub(in crate::shamir_db::shamir_db) fn decode_stored_bytes(
    rec: &QueryValue,
) -> Option<Result<Vec<u8>, String>> {
    let b64 = rec["wasm_b64"].as_str()?;
    Some(
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| e.to_string()),
    )
}

/// Stored `optimize` mode; absent or unknown means `Auto`.
pub(in crate::shamir_db::shamir_db) fn stored_optimize(rec: &QueryValue) -> TierMode {
    match rec["optimize"].as_str() {
        None => TierMode::Auto,
        Some(s) => parse_optimize(s).unwrap_or_else(|| {
            log::warn!("unknown stored optimize mode '{s}', using auto");
            TierMode::Auto
        }),
    }
}
