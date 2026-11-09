//! The compile gate: bounds concurrent compiles and caps admitted compile time.

use crate::runtime::error::{FnResult, FunctionError};
use crate::wasm::admission::AdmissionPolicy;
use crate::wasm::engine::WasmEngine;
use std::sync::Arc;

/// Runs a blocking compile job under the engine's compile semaphore.
///
/// The acquired permit is handed to `job`, so a caller that spawns its own
/// worker can move the permit onward and pin its Drop point to the worker's
/// real exit (see `gate::lang_compile`); otherwise the permit drops when the
/// job closure ends on the compile thread, which still covers a timeout
/// (the thread cannot be cancelled and finishes in the background). Under
/// [`AdmissionPolicy::Admission`] the join is capped by
/// `AdmissionLimits::max_compile_time`; on expiry the call returns an error
/// but the job cannot be cancelled and finishes in the background (the
/// structural caps are the real bound on its work). Under
/// [`AdmissionPolicy::Load`] there is no time cap, the semaphore still applies.
pub(in crate::wasm) async fn run_gated_compile<F, T>(
    engine: &Arc<WasmEngine>,
    policy: &AdmissionPolicy,
    job: F,
) -> FnResult<T>
where
    F: FnOnce(tokio::sync::OwnedSemaphorePermit) -> FnResult<T> + Send + 'static,
    T: Send + 'static,
{
    let permit = Arc::clone(engine.compile_permits())
        .acquire_owned()
        .await
        .map_err(|e| FunctionError::Compute(format!("compile gate closed: {e}")))?;
    #[cfg(test)]
    let faults = Arc::clone(engine.faults());
    let handle = tokio::task::spawn_blocking(move || {
        #[cfg(test)]
        let _probe = faults.enter_compile();
        // The permit is owned by the job frame from here; a job that moves
        // it into a worker thread keeps it alive until that worker exits.
        job(permit)
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
