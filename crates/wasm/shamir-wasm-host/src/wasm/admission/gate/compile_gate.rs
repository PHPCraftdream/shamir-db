//! The compile gate: bounds concurrent compiles and caps admitted compile time.

use crate::runtime::error::{FnResult, FunctionError};
use crate::wasm::admission::AdmissionPolicy;
use crate::wasm::engine::WasmEngine;
use std::sync::Arc;

/// Runs a blocking compile job under the engine's compile semaphore.
///
/// An owned permit is moved into the blocking closure, so it is released only
/// when the compile thread really ends, even after a timeout. Under
/// [`AdmissionPolicy::Admission`] the join is capped by
/// `AdmissionLimits::max_compile_time`; on expiry the call returns an error
/// but the thread cannot be cancelled and finishes in the background (the
/// structural caps are the real bound on its work). Under
/// [`AdmissionPolicy::Load`] there is no time cap, the semaphore still applies.
pub(in crate::wasm) async fn run_gated_compile<F, T>(
    engine: &Arc<WasmEngine>,
    policy: &AdmissionPolicy,
    job: F,
) -> FnResult<T>
where
    F: FnOnce() -> FnResult<T> + Send + 'static,
    T: Send + 'static,
{
    let permit = Arc::clone(engine.compile_permits())
        .acquire_owned()
        .await
        .map_err(|e| FunctionError::Compute(format!("compile gate closed: {e}")))?;
    #[cfg(test)]
    let faults = Arc::clone(engine.faults());
    let handle = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        #[cfg(test)]
        let _probe = faults.enter_compile();
        job()
    });
    let joined = match policy {
        AdmissionPolicy::Admission(limits) => {
            match tokio::time::timeout(limits.max_compile_time, handle).await {
                Ok(joined) => joined,
                Err(_) => {
                    return Err(FunctionError::Compute(format!(
                        "wasm admission: the compile exceeded the compile time cap of {:?}; the module is rejected",
                        limits.max_compile_time
                    )));
                }
            }
        }
        AdmissionPolicy::Load => handle.await,
    };
    joined.map_err(|e| FunctionError::Compute(format!("blocking compile join failure: {e}")))?
}
