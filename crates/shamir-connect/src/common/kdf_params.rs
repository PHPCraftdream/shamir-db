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
