use super::super::wasm_function::{read_guest_mem, write_value_to_guest, HostState};
use shamir_types::types::value::QueryValue;
use std::future::Future;

/// Host implementation of `global_set(key_ptr, key_len, val_ptr, val_len)`.
///
/// The `env.*` namespace is write-protected on THIS import: no WASM guest
/// may write into it via `global_set`, regardless of `secret_grants` (grants
/// only gate reads — see [`host_global_get`]). `env.*` globals are OS-seeded
/// once via `GlobalVars::seed_env` and shared across every function
/// invocation for the lifetime of the process, so an unguarded write here
/// would let one guest permanently corrupt a secret for every other function
/// sharing the same `GlobalVars` instance. A `key` starting with `"env."`
/// traps — matching this file's existing convention (`wasmtime::Error::msg(..)`)
/// of surfacing host-import misuse as a guest-visible trap rather than a
/// silently-succeeding no-op.
///
/// Scope note: this closes the WASM-guest path specifically (the only path
/// reachable from compiled, untrusted bytecode — the wasmtime linker wires
/// the guest `global_set` import exclusively to this function). It does NOT
/// gate `FnCtx::global_set` (`context.rs`), the native-Rust-side setter used
/// by trusted, in-process `ShamirFunction` implementations — those are
/// compiled-in, not guest-supplied, so they sit outside this fix's threat
/// model, but a future native `env.*` writer would need its own guard if one
/// is ever added.
pub(in crate::wasm) fn host_global_set(
    mut caller: wasmtime::Caller<'_, HostState>,
    key_ptr: i32,
    key_len: i32,
    val_ptr: i32,
    val_len: i32,
) -> Result<(), wasmtime::Error> {
    let memory = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| wasmtime::Error::msg("missing export `memory`"))?;

    let key_bytes = read_guest_mem(memory.data(&caller), key_ptr, key_len)?;
    let val_bytes = read_guest_mem(memory.data(&caller), val_ptr, val_len)?;

    let key = String::from_utf8(key_bytes)
        .map_err(|_| wasmtime::Error::msg("global_set: key is not valid UTF-8"))?;

    // Write-protect the `env.*` namespace unconditionally — no secret_grants
    // check here (that's a read-only concept). A guest may never overwrite
    // an OS-seeded secret, no matter what it's been granted to read.
    if key.starts_with("env.") {
        return Err(wasmtime::Error::msg(format!(
            "global_set: writes to the `env.*` namespace are not permitted (key: {key})"
        )));
    }

    let value = QueryValue::from_bytes(&val_bytes)
        .map_err(|e| wasmtime::Error::msg(format!("global_set: value decode error: {e}")))?;

    caller.data().globals.set(key, value);
    Ok(())
}

/// Host implementation of `global_get(key_ptr, key_len) -> i64`.
///
/// If the requested key starts with `"env."`, the suffix (the env-var name)
/// must be present in the invocation's `secret_grants` — otherwise the
/// global looks absent (returns 0). Non-`env.` keys are returned normally.
pub(in crate::wasm) fn host_global_get(
    mut caller: wasmtime::Caller<'_, HostState>,
    (key_ptr, key_len): (i32, i32),
) -> Box<dyn Future<Output = Result<i64, wasmtime::Error>> + Send + '_> {
    Box::new(async move {
        let memory = caller
            .get_export("memory")
            .and_then(|e| e.into_memory())
            .ok_or_else(|| wasmtime::Error::msg("missing export `memory`"))?;

        let key_bytes = read_guest_mem(memory.data(&caller), key_ptr, key_len)?;
        let key = String::from_utf8(key_bytes)
            .map_err(|_| wasmtime::Error::msg("global_get: key is not valid UTF-8"))?;

        // Secret-grant enforcement (slice 9): env.* globals require a grant.
        if let Some(env_name) = key.strip_prefix("env.") {
            if !caller.data().secret_grants.contains(env_name) {
                return Ok(0);
            }
        }

        let value = caller.data().globals.get(&key);
        write_value_to_guest(&mut caller, value).await
    })
}
