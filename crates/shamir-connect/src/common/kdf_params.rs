//! Argon2id parameter set.
//!
//! Per spec §3.7 + §4.1: server defaults are `memory_kb=131072 (128 MB),
//! time=4, parallelism=1, argon2_version=0x13`.

use crate::common::error::{Error, Result};
use crate::common::types::limits;

/// Argon2id parameter tuple, embedded in `auth_message` as raw bytes (spec §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    /// Memory cost in KB (spec §3.7).
    pub memory_kb: u32,
    /// Time cost (passes).
    pub time: u32,
    /// Parallelism lanes.
    pub parallelism: u32,
    /// Argon2 algorithm version byte (RFC 9106 v1.3 = 0x13).
    pub argon2_version: u8,
}

impl KdfParams {
    /// Server defaults per spec §3.7.
    pub const DEFAULT: KdfParams = KdfParams {
        memory_kb: 131_072, // 128 MB
        time: 4,
        parallelism: 1,
        argon2_version: limits::ARGON2_VERSION_V13,
    };

    /// Validate against client-side hard limits before launching Argon2id (§5.1.1).
    ///
    /// Returns [`Error::KdfParamsRejected`] if any limit is exceeded.
    pub fn validate_client_limits(&self) -> Result<()> {
        if self.memory_kb > limits::KDF_MAX_MEMORY_KB
            || self.time > limits::KDF_MAX_TIME
            || self.parallelism > limits::KDF_MAX_PARALLEL
            || self.argon2_version != limits::ARGON2_VERSION_V13
        {
            return Err(Error::KdfParamsRejected);
        }
        Ok(())
    }

    /// Validate against server-side floor (§3.7.2). Server config rejects below.
    pub fn validate_server_floor(&self) -> Result<()> {
        if self.memory_kb < limits::KDF_MIN_MEMORY_KB
            || self.time < limits::KDF_MIN_TIME
            || self.parallelism < limits::KDF_MIN_PARALLELISM
        {
            return Err(Error::InvalidInput("kdf_params below server floor"));
        }
        Ok(())
    }
}

/// Hard client-safety cap on memory (KiB). Defense-in-depth on top of
/// [`limits::KDF_MAX_MEMORY_KB`]: any server publishing parameters above
/// this ceiling is treated as malicious / misconfigured and rejected
/// before Argon2id starts allocating.
pub const HARD_MAX_MEMORY_KB: u32 = 512 * 1024;

/// Hard client-safety cap on Argon2id time iterations. Mirrors
/// [`HARD_MAX_MEMORY_KB`] semantics.
pub const HARD_MAX_TIME: u32 = 16;

/// Reject KDF parameters that exceed safe-client-side bounds.
///
/// Defense against server-side downgrade attacks: a malicious operator
/// (or compromised server) could publish parameters that OOM low-end
/// clients (e.g. `m=1 GiB, t=64`). The wire-level cap
/// [`KdfParams::validate_client_limits`] already enforces stricter
/// limits per spec §5.1.1; this function provides a separate outer
/// envelope (`512 MiB` / `16` iterations) used as a sanity guard at the
/// client-handshake call site, so a future spec bump or per-listener
/// override cannot accidentally remove the hard ceiling.
pub fn validate_client_kdf_safe(params: &KdfParams) -> std::result::Result<(), String> {
    if params.memory_kb > HARD_MAX_MEMORY_KB {
        return Err(format!(
            "server-requested KDF memory {} KiB exceeds client safety cap {} KiB",
            params.memory_kb, HARD_MAX_MEMORY_KB
        ));
    }
    if params.time > HARD_MAX_TIME {
        return Err(format!(
            "server-requested KDF time {} exceeds client safety cap {}",
            params.time, HARD_MAX_TIME
        ));
    }
    Ok(())
}
