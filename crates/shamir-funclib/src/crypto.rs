//! `/crypto` scalar category — pure, deterministic crypto primitives.
//!
//! Functions registered (plain names, no folder prefix):
//! `sha256 sha512 sha3_256 blake3 hmac_sha256 ct_eq argon2id`.
//!
//! Conventions (mirroring `math.rs`):
//! - Hashes take a single `Bin` argument via [`arg_bytes`] and return the raw
//!   digest as `Bin` via [`v_bytes`].
//! - `hmac_sha256(key, msg)` takes two `Bin` arguments (key, message) and
//!   returns the 32-byte MAC as `Bin`.
//! - `ct_eq(a, b)` compares two `Bin` arguments in constant time (via `subtle`)
//!   and returns a `Bool`.
//! - `argon2id(password, salt, [memory_kb, time, parallelism, length])` is the
//!   Argon2id KDF. It is **deterministic** given its inputs (same password +
//!   salt + params → same digest), so it fits the pure-scalar contract — but
//!   it is **CPU- and memory-bound** (tens of ms at OWASP defaults). A caller
//!   dispatching it on an async runtime MUST offload to `spawn_blocking`; do
//!   not invoke it inline on a runtime worker.
//! - Every function here is `pure + deterministic` (no randomness, no clock),
//!   so all use [`FnEntry::pure`]. Non-deterministic procedural crypto
//!   (random / uuid) and asymmetric / PQC primitives remain out of scope.

use crate::registry::{arg_bytes, arg_i64, v_bool, v_bytes, FnEntry, ScalarError, ScalarRegistry};
use argon2::{Algorithm, Argon2, Params as Argon2Params, Version};
use hmac::{Mac, SimpleHmac};
use sha2::{Digest, Sha256, Sha512};
use sha3::Sha3_256;
use shamir_types::types::value::InnerValue;
use subtle::ConstantTimeEq;

// Argon2id parameter defaults (OWASP interactive-login profile) and upper
// bounds (resource-exhaustion guard on untrusted input). Mirrors the engine's
// async `Argon2idFunction` so both paths agree bit-for-bit.
const A2_DEFAULT_MEMORY_KB: u32 = 19_456;
const A2_DEFAULT_TIME: u32 = 2;
const A2_DEFAULT_PARALLELISM: u32 = 1;
const A2_DEFAULT_LENGTH: u32 = 32;
const A2_MAX_MEMORY_KB: u32 = 1_048_576;
const A2_MAX_TIME: u32 = 16;
const A2_MAX_PARALLELISM: u32 = 16;
const A2_MAX_LENGTH: u32 = 256;

/// Read an optional `u32` argument at index `i`, falling back to `default`
/// when absent. Out-of-`u32`-range integers are `"out_of_range"`.
fn opt_u32(a: &[InnerValue], i: usize, default: u32) -> Result<u32, ScalarError> {
    if i >= a.len() {
        return Ok(default);
    }
    let n = arg_i64(a, i)?;
    u32::try_from(n).map_err(|_| ScalarError::new("out_of_range"))
}

/// `argon2id(password, salt, [memory_kb, time, parallelism, length]) -> Bin`.
fn argon2id_fn(a: &[InnerValue]) -> Result<InnerValue, ScalarError> {
    let password = arg_bytes(a, 0)?;
    let salt = arg_bytes(a, 1)?;
    let memory_kb = opt_u32(a, 2, A2_DEFAULT_MEMORY_KB)?;
    let time = opt_u32(a, 3, A2_DEFAULT_TIME)?;
    let parallelism = opt_u32(a, 4, A2_DEFAULT_PARALLELISM)?;
    let length = opt_u32(a, 5, A2_DEFAULT_LENGTH)? as usize;

    if memory_kb > A2_MAX_MEMORY_KB
        || time > A2_MAX_TIME
        || parallelism > A2_MAX_PARALLELISM
        || length > A2_MAX_LENGTH as usize
    {
        return Err(ScalarError::new("out_of_range"));
    }

    let cfg = Argon2Params::new(memory_kb, time, parallelism, Some(length))
        .map_err(|_| ScalarError::new("bad_params"))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, cfg);
    let mut out = vec![0u8; length];
    argon
        .hash_password_into(password, salt, &mut out)
        .map_err(|_| ScalarError::new("compute"))?;
    Ok(v_bytes(out))
}

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
    // argon2id(password, salt, [memory_kb, time, parallelism, length]).
    reg.register("argon2id", FnEntry::pure(argon2id_fn, 2, Some(6)));
}

#[cfg(test)]
mod tests;
