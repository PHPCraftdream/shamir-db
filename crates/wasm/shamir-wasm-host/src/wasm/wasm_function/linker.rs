use super::state::HostState;
use crate::runtime::error::{FnResult, FunctionError};
use crate::wasm::bindings::batch::{host_batch_get, host_batch_put};
use crate::wasm::bindings::call::{host_call, host_call_checked};
use crate::wasm::bindings::db::{host_db_execute, host_db_get, host_db_insert, host_db_query};
use crate::wasm::bindings::globals::{host_global_get, host_global_set};
use crate::wasm::bindings::http::host_http_fetch;
use crate::wasm::bindings::raise::host_raise_error;
use wasmtime::{Engine, InstancePre, Linker, Module};

/// Build a [`Linker`] with all host imports registered, resolving nothing
/// yet against any particular module.
///
/// `pub(crate)` (rather than private) so
/// `tests::wasm::admission::sanitizer_tests::sanctioned_list_matches_linker_registrations`
/// can build the real registered [`Linker`] surface and cross-check its
/// `(module, name)` pairs (via [`Linker::iter`]) against
/// [`super::admission::SANCTIONED_HOST_IMPORTS`] — this is the
/// mechanism keeping the sanitizer's allowlist and the linker's actual
/// registrations from silently drifting apart.
pub(crate) fn build_linker(engine: &Engine) -> FnResult<Linker<HostState>> {
    let mut linker: Linker<HostState> = Linker::new(engine);

    // Sync host imports under "shamir_host".
    linker
        .func_wrap("shamir_host", "batch_put", host_batch_put)
        .map_err(|e| FunctionError::Compute(format!("linker batch_put: {e}")))?;
    linker
        .func_wrap_async("shamir_host", "batch_get", host_batch_get)
        .map_err(|e| FunctionError::Compute(format!("linker batch_get: {e}")))?;
    linker
        .func_wrap("shamir_host", "global_set", host_global_set)
        .map_err(|e| FunctionError::Compute(format!("linker global_set: {e}")))?;
    linker
        .func_wrap_async("shamir_host", "global_get", host_global_get)
        .map_err(|e| FunctionError::Compute(format!("linker global_get: {e}")))?;

    linker
        .func_wrap("shamir_host", "raise_error", host_raise_error)
        .map_err(|e| FunctionError::Compute(format!("linker raise_error: {e}")))?;

    // Async host imports.
    linker
        .func_wrap_async("shamir_host", "call", host_call)
        .map_err(|e| FunctionError::Compute(format!("linker call: {e}")))?;
    linker
        .func_wrap_async("shamir_host", "call_checked", host_call_checked)
        .map_err(|e| FunctionError::Compute(format!("linker call_checked: {e}")))?;
    linker
        .func_wrap_async("shamir_host", "db_get", host_db_get)
        .map_err(|e| FunctionError::Compute(format!("linker db_get: {e}")))?;
    linker
        .func_wrap_async("shamir_host", "db_insert", host_db_insert)
        .map_err(|e| FunctionError::Compute(format!("linker db_insert: {e}")))?;
    linker
        .func_wrap_async("shamir_host", "db_query", host_db_query)
        .map_err(|e| FunctionError::Compute(format!("linker db_query: {e}")))?;
    linker
        .func_wrap_async("shamir_host", "db_execute", host_db_execute)
        .map_err(|e| FunctionError::Compute(format!("linker db_execute: {e}")))?;
    linker
        .func_wrap_async("shamir_host", "http_fetch", host_http_fetch)
        .map_err(|e| FunctionError::Compute(format!("linker http_fetch: {e}")))?;

    Ok(linker)
}

/// Pre-instantiate a [`Linker`] against the given module: the resulting
/// [`InstancePre`] resolves imports once; each `instantiate_async` only
/// needs a fresh [`Store`].
pub(super) fn build_instance_pre(
    engine: &Engine,
    module: &Module,
) -> FnResult<InstancePre<HostState>> {
    let linker = build_linker(engine)?;
    linker
        .instantiate_pre(module)
        .map_err(|e| FunctionError::Compute(format!("instantiate_pre failed: {e}")))
}
