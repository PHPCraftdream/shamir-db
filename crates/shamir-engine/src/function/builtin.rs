//! Built-in functions shipped with the engine.

use super::context::{FnBatch, FnCtx};
use super::contract::ShamirFunction;
use super::error::{FnResult, FunctionError};
use super::params::Params;
use argon2::{Algorithm, Argon2, Params as Argon2Params, Version};
use async_trait::async_trait;
use shamir_types::types::value::QueryValue;

/// `argon2id(password, salt, [memory_kb, time, parallelism, length]) -> Bin`
///
/// The first built-in and the proof of the execution model: Argon2id is
/// CPU- and memory-bound, so the hash runs on `tokio::task::spawn_blocking`
/// — the async runtime's worker threads are never occupied by the KDF.
///
/// Parameters:
/// * `password` — `Bin | Str`, required.
/// * `salt` — `Bin | Str`, required (Argon2 requires ≥ 8 bytes).
/// * `memory_kb` — `Int`, optional, default `19456` (19 MiB, OWASP).
/// * `time` — `Int`, optional, default `2`.
/// * `parallelism` — `Int`, optional, default `1`.
/// * `length` — `Int`, optional, default `32` (output bytes).
pub struct Argon2idFunction;

/// OWASP-recommended defaults for an interactive-login Argon2id profile.
const DEFAULT_MEMORY_KB: u32 = 19_456;
const DEFAULT_TIME: u32 = 2;
const DEFAULT_PARALLELISM: u32 = 1;
const DEFAULT_LENGTH: u32 = 32;

/// Upper bounds for caller-supplied Argon2id parameters to prevent
/// resource exhaustion on untrusted input.
const MAX_MEMORY_KB: u32 = 1_048_576; // 1 GiB
const MAX_TIME: u32 = 16;
const MAX_PARALLELISM: u32 = 16;
const MAX_LENGTH: u32 = 256;

#[async_trait]
impl ShamirFunction for Argon2idFunction {
    async fn call(&self, _ctx: &FnCtx, _batch: &FnBatch, params: &Params) -> FnResult<QueryValue> {
        let password = params.bytes("password")?;
        let salt = params.bytes("salt")?;
        let memory_kb = params.opt_u32("memory_kb")?.unwrap_or(DEFAULT_MEMORY_KB);
        let time = params.opt_u32("time")?.unwrap_or(DEFAULT_TIME);
        let parallelism = params
            .opt_u32("parallelism")?
            .unwrap_or(DEFAULT_PARALLELISM);
        let length = params.opt_u32("length")?.unwrap_or(DEFAULT_LENGTH) as usize;

        if memory_kb > MAX_MEMORY_KB {
            return Err(FunctionError::BadParam {
                name: "memory_kb".into(),
                reason: format!("memory_kb exceeds maximum ({MAX_MEMORY_KB} KiB)"),
            });
        }
        if time > MAX_TIME {
            return Err(FunctionError::BadParam {
                name: "time".into(),
                reason: format!("time exceeds maximum ({MAX_TIME})"),
            });
        }
        if parallelism > MAX_PARALLELISM {
            return Err(FunctionError::BadParam {
                name: "parallelism".into(),
                reason: format!("parallelism exceeds maximum ({MAX_PARALLELISM})"),
            });
        }
        if length > MAX_LENGTH as usize {
            return Err(FunctionError::BadParam {
                name: "length".into(),
                reason: format!("length exceeds maximum ({MAX_LENGTH})"),
            });
        }

        // CPU/memory-bound — never run it on an async worker thread.
        let digest = tokio::task::spawn_blocking(move || -> FnResult<Vec<u8>> {
            let cfg =
                Argon2Params::new(memory_kb, time, parallelism, Some(length)).map_err(|e| {
                    FunctionError::BadParam {
                        name: "params".into(),
                        reason: e.to_string(),
                    }
                })?;
            let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, cfg);
            let mut out = vec![0u8; length];
            argon
                .hash_password_into(&password, &salt, &mut out)
                .map_err(|e| FunctionError::Compute(e.to_string()))?;
            Ok(out)
        })
        .await
        .map_err(|_| FunctionError::Cancelled)??;

        Ok(QueryValue::Bin(digest))
    }
}
