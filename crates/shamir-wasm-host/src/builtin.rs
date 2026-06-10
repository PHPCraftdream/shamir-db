//! Built-in functions shipped with the engine.

use super::context::{FnBatch, FnCtx};
use super::contract::ShamirFunction;
use super::error::{FnResult, FunctionError};
use super::params::Params;
use super::scalar::builtin_scalars;
use async_trait::async_trait;
use shamir_types::types::value::{InnerValue, QueryValue};

/// `argon2id(password, salt, [memory_kb, time, parallelism, length]) -> Bin`
///
/// The async, runtime-safe shell around the **single** Argon2id implementation,
/// which lives in `shamir-funclib` (`crypto/argon2id`). The KDF is CPU- and
/// memory-bound, so this wrapper offloads it to `tokio::task::spawn_blocking`
/// — the async runtime's worker threads are never occupied by the hash. The
/// hashing logic, OWASP defaults, and parameter bounds are owned by funclib;
/// this type only decodes the `Params` map, fills defaults, and delegates.
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
/// These mirror `shamir_funclib::crypto`'s constants so the named-param shell
/// and the positional funclib call agree.
const DEFAULT_MEMORY_KB: u32 = 19_456;
const DEFAULT_TIME: u32 = 2;
const DEFAULT_PARALLELISM: u32 = 1;
const DEFAULT_LENGTH: u32 = 32;

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
        let length = params.opt_u32("length")?.unwrap_or(DEFAULT_LENGTH);

        // Positional argument list for the funclib scalar:
        // (password, salt, memory_kb, time, parallelism, length).
        let args = vec![
            InnerValue::Bin(password),
            InnerValue::Bin(salt),
            InnerValue::Int(memory_kb as i64),
            InnerValue::Int(time as i64),
            InnerValue::Int(parallelism as i64),
            InnerValue::Int(length as i64),
        ];

        // CPU/memory-bound — never run it on an async worker thread. The
        // single implementation lives in funclib; we invoke it through the
        // shared scalar registry here.
        let digest = tokio::task::spawn_blocking(move || -> FnResult<Vec<u8>> {
            let out = builtin_scalars()
                .call("crypto/argon2id", &args)
                .map_err(|e| match e.code.as_str() {
                    // Out-of-range / malformed parameters are caller faults.
                    "out_of_range" | "bad_params" => FunctionError::BadParam {
                        name: "params".into(),
                        reason: e.code,
                    },
                    // Everything else (e.g. salt too short) is a compute error.
                    other => FunctionError::Compute(other.to_string()),
                })?;
            match out {
                InnerValue::Bin(b) => Ok(b),
                other => Err(FunctionError::Compute(format!(
                    "argon2id returned non-Bin value: {other:?}"
                ))),
            }
        })
        .await
        .map_err(|_| FunctionError::Cancelled)??;

        Ok(QueryValue::Bin(digest))
    }
}
