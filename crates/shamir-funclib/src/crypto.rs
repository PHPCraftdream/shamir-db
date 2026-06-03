//! `/crypto` scalar category — pure, deterministic symmetric primitives only.
//!
//! Functions registered (plain names, no folder prefix):
//! `sha256 sha512 sha3_256 blake3 hmac_sha256 ct_eq`.
//!
//! Conventions (mirroring `math.rs`):
//! - Hashes take a single `Bin` argument via [`arg_bytes`] and return the raw
//!   digest as `Bin` via [`v_bytes`].
//! - `hmac_sha256(key, msg)` takes two `Bin` arguments (key, message) and
//!   returns the 32-byte MAC as `Bin`.
//! - `ct_eq(a, b)` compares two `Bin` arguments in constant time (via `subtle`)
//!   and returns a `Bool`.
//! - Every function here is `pure + deterministic` (no randomness, no clock),
//!   so all use [`FnEntry::pure`]. Procedural crypto (random/uuid/argon2/
//!   asymmetric/PQC) is deliberately out of scope for this crate.

use crate::registry::{arg_bytes, v_bool, v_bytes, FnEntry, ScalarError, ScalarRegistry};
use hmac::{Mac, SimpleHmac};
use sha2::{Digest, Sha256, Sha512};
use sha3::Sha3_256;
use subtle::ConstantTimeEq;

/// Register the `/crypto` functions.
pub fn register(reg: &mut ScalarRegistry) {
    reg.register(
        "sha256",
        FnEntry::pure(
            |a| {
                let bytes = arg_bytes(a, 0)?;
                Ok(v_bytes(Sha256::digest(bytes).to_vec()))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "sha512",
        FnEntry::pure(
            |a| {
                let bytes = arg_bytes(a, 0)?;
                Ok(v_bytes(Sha512::digest(bytes).to_vec()))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "sha3_256",
        FnEntry::pure(
            |a| {
                let bytes = arg_bytes(a, 0)?;
                Ok(v_bytes(Sha3_256::digest(bytes).to_vec()))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "blake3",
        FnEntry::pure(
            |a| {
                let bytes = arg_bytes(a, 0)?;
                Ok(v_bytes(blake3::hash(bytes).as_bytes().to_vec()))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "hmac_sha256",
        FnEntry::pure(
            |a| {
                let key = arg_bytes(a, 0)?;
                let msg = arg_bytes(a, 1)?;
                // SimpleHmac accepts keys of any length; new_from_slice is infallible here.
                let mut mac = SimpleHmac::<Sha256>::new_from_slice(key)
                    .map_err(|_| ScalarError::new("bad_key"))?;
                mac.update(msg);
                Ok(v_bytes(mac.finalize().into_bytes().to_vec()))
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "ct_eq",
        FnEntry::pure(
            |a| {
                let lhs = arg_bytes(a, 0)?;
                let rhs = arg_bytes(a, 1)?;
                // subtle's ct_eq is constant-time only for equal-length inputs;
                // a length mismatch is a definite inequality.
                let eq = lhs.len() == rhs.len() && bool::from(lhs.ct_eq(rhs));
                Ok(v_bool(eq))
            },
            2,
            Some(2),
        ),
    );
}

#[cfg(test)]
mod tests;
