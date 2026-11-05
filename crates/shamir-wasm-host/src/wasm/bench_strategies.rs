//! Bench-only engine strategy selection (cargo feature `bench-strategies`).
//!
//! `SHAMIR_WASM_STRATEGY` picks the code generator for an A/B run of the same suite:
//! `cranelift` (default), `winch` (single-pass baseline compiler) or `pulley`
//! (portable interpreter). Anything else is rejected. The feature is off in every
//! shipped build, so the server cannot be steered through the environment.

use crate::runtime::error::{FnResult, FunctionError};

/// Env var naming the strategy of the benchmarked engine.
pub const STRATEGY_ENV: &str = "SHAMIR_WASM_STRATEGY";

pub(super) fn apply(config: &mut wasmtime::Config) -> FnResult<()> {
    match std::env::var(STRATEGY_ENV).ok().as_deref() {
        None | Some("" | "cranelift") => {}
        Some("winch") => {
            config.strategy(wasmtime::Strategy::Winch);
        }
        Some("pulley") => {
            config
                .target("pulley64")
                .map_err(|error| FunctionError::Compute(error.to_string()))?;
        }
        Some(other) => {
            return Err(FunctionError::Compute(format!(
                "{STRATEGY_ENV}=`{other}`: expected cranelift, winch or pulley"
            )));
        }
    }
    Ok(())
}
