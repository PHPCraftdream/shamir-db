use super::wasm_function::{read_guest_mem, HostState};
use shamir_types::types::value::QueryValue;

/// Host implementation of `batch_put(key_ptr, key_len, val_ptr, val_len)`.
pub(super) fn host_batch_put(
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
pub(super) fn host_batch_get(
    mut caller: wasmtime::Caller<'_, HostState>,
    key_ptr: i32,
    key_len: i32,
) -> Result<i64, wasmtime::Error> {
    let memory = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| wasmtime::Error::msg("missing export `memory`"))?;

    let key_bytes = read_guest_mem(memory.data(&caller), key_ptr, key_len)?;
    let key = String::from_utf8(key_bytes)
        .map_err(|_| wasmtime::Error::msg("batch_get: key is not valid UTF-8"))?;

    let value = match caller.data().batch.get(&key) {
        Some(v) => v,
        None => return Ok(0),
    };

    let encoded = value
        .to_bytes()
        .map_err(|e| wasmtime::Error::msg(format!("batch_get: encode error: {e}")))?;

    // Clone the Arcs we need before &mut Caller is consumed by calling alloc.
    let batch = caller.data().batch.clone();
    let globals = caller.data().globals.clone();

    let alloc_fn = caller
        .get_export("shamir_alloc")
        .and_then(|e| e.into_func())
        .ok_or_else(|| wasmtime::Error::msg("missing export `shamir_alloc`"))?;
    let alloc_typed = alloc_fn.typed::<i32, i32>(&caller)?;

    // Release the read-only borrow on Caller before the mutable call.
    let _ = memory;
    let out_len =
        i32::try_from(encoded.len()).map_err(|_| wasmtime::Error::msg("value too large"))?;
    let out_ptr = alloc_typed.call(&mut caller, out_len)?;

    if out_ptr < 0 {
        return Err(wasmtime::Error::msg(
            "shamir_alloc returned negative pointer",
        ));
    }

    // Re-acquire the memory export and write the encoded value.
    let memory = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| wasmtime::Error::msg("missing export `memory`"))?;
    let start = out_ptr as usize;
    let end = start.saturating_add(encoded.len());
    if end > memory.data_size(&caller) {
        return Err(wasmtime::Error::msg(
            "shamir_alloc returned pointer outside memory bounds",
        ));
    }
    memory.data_mut(&mut caller)[start..end].copy_from_slice(&encoded);

    // Suppress unused-variable warnings (the clones are for borrow-scope hygiene).
    let _ = (batch, globals);

    let packed = ((out_ptr as i64) << 32) | (encoded.len() as i64 & 0xFFFF_FFFF);
    Ok(packed)
}
