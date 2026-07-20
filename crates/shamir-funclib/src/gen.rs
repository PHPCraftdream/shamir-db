//! `/gen` scalar category — value-generation functions (impure / non-deterministic).
//!
//! Functions registered (plain names, folder-qualified to `gen/…` by
//! [`crate::register_builtins`]): `uuid_v4 random random_bytes`.
//!
//! Conventions (mirroring [`crate::datetime`]'s impure-function convention):
//! - Every function here reads a source of randomness and is therefore
//!   **non-deterministic and impure** (`pure:false, deterministic:false`).
//!   They can NEVER back a functional index — they are the textbook impure
//!   case. They are registered via raw `FnEntry { .. }` (not `FnEntry::pure`)
//!   to set `pure/deterministic/trusted_pure` to `false`, exactly as
//!   `datetime::now`/`age` do.
//! - `uuid_v4()` takes 0 args and returns a canonical lowercase hyphenated
//!   RFC-4122 v4 UUID `Str` (16 random bytes with version/variant fixups).
//! - `random()` takes 0 args and returns an `F64` in `[0.0, 1.0)`.
//! - `random_bytes(n)` takes 1 arg (`n` bytes) and returns a `Bin` of `n`
//!   random bytes; `n == 0` returns an empty `Bin`.

use crate::registry::{arg_i64, v_bytes, v_f64, v_str, FnEntry, ScalarError, ScalarRegistry};
use rand::RngCore;

/// Generate a v4 UUID: 16 random bytes with the RFC-4122 §4.4 version
/// (bits 48-51 → `0100`) and variant (top 2 bits of byte 8 → `10`) fixups,
/// formatted as the canonical lowercase hyphenated hex string.
fn uuid_v4_string() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    // Version: high nibble of byte 6 = 0b0100 (v4).
    bytes[6] = (bytes[6] & 0x0F) | 0x40;
    // Variant: top 2 bits of byte 8 = 0b10.
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    // Format as xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx (lowercase).
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    )
}

/// Register the `/gen` functions.
pub fn register(reg: &mut ScalarRegistry) {
    reg.register(
        "uuid_v4",
        FnEntry {
            f: std::sync::Arc::new(|_a: &[_]| Ok(v_str(uuid_v4_string()))),
            min_args: 0,
            max_args: Some(0),
            pure: false,
            deterministic: false,
            trusted_pure: false,
        },
    );
    reg.register(
        "random",
        FnEntry {
            f: std::sync::Arc::new(|_a: &[_]| v_f64(rand::random::<f64>())),
            min_args: 0,
            max_args: Some(0),
            pure: false,
            deterministic: false,
            trusted_pure: false,
        },
    );
    reg.register(
        "random_bytes",
        FnEntry {
            f: std::sync::Arc::new(|a: &[_]| {
                let n = arg_i64(a, 0)?;
                if n < 0 {
                    return Err(ScalarError::new("out_of_range"));
                }
                let mut buf = vec![0u8; n as usize];
                rand::rng().fill_bytes(&mut buf);
                Ok(v_bytes(buf))
            }),
            min_args: 1,
            max_args: Some(1),
            pure: false,
            deterministic: false,
            trusted_pure: false,
        },
    );
}

#[cfg(test)]
mod tests;
