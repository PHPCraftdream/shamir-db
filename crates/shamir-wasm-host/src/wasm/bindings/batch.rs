use super::super::wasm_function::{read_guest_mem, write_value_to_guest, HostState};
use shamir_types::types::value::QueryValue;
use std::future::Future;

/// Host implementation of `batch_put(key_ptr, key_len, val_ptr, val_len)`.
pub(in crate::wasm) fn host_batch_put(
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
        .map_err(|_| wasmtime::Error::msg("batch_put: key is not valid UTF-8"))?;
    let value = QueryValue::from_bytes(&val_bytes)
        .map_err(|e| wasmtime::Error::msg(format!("batch_put: value decode error: {e}")))?;

    caller.data().batch.put(key, value);
    Ok(())
}

/// Host implementation of `batch_get(key_ptr, key_len) -> i64`.
pub(in crate::wasm) fn host_batch_get(
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
            .map_err(|_| wasmtime::Error::msg("batch_get: key is not valid UTF-8"))?;

        let value = caller.data().batch.get(&key);
        write_value_to_guest(&mut caller, value).await
    })
}
