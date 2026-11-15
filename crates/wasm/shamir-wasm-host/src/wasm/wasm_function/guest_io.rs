use crate::runtime::error::{FnResult, FunctionError};
use crate::runtime::params::Params;
use shamir_types::codecs::{Codec, MessagePackCodec};
use shamir_types::types::value::QueryValue;

use super::state::HostState;

// ── Host-import helpers ──────────────────────────────────────────────

/// Read `len` bytes from the memory data slice starting at `ptr`.
///
/// Returns a wasm trap on out-of-bounds.
pub(in crate::wasm) fn read_guest_mem(
    mem_data: &[u8],
    ptr: i32,
    len: i32,
) -> Result<Vec<u8>, wasmtime::Error> {
    if ptr < 0 || len < 0 {
        return Err(wasmtime::Error::msg("negative pointer or length"));
    }
    let start = ptr as usize;
    let end = start.saturating_add(len as usize);
    if end > mem_data.len() {
        return Err(wasmtime::Error::msg(
            "host import read past end of guest memory",
        ));
    }
    Ok(mem_data[start..end].to_vec())
}

/// Helper: write a msgpack-encoded `QueryValue` back into guest memory via
/// `shamir_alloc`, returning the packed `(ptr, len)` i64. Returns `Ok(0)`
/// if `value` is `None`.
///
/// Uses explicit reborrows (`&mut *caller`) because `Caller` is `&mut` and
/// method calls consume it.
pub(in crate::wasm) async fn write_value_to_guest(
    caller: &mut wasmtime::Caller<'_, HostState>,
    value: Option<QueryValue>,
) -> Result<i64, wasmtime::Error> {
    let value = match value {
        Some(v) => v,
        None => return Ok(0),
    };
    let encoded = value
        .to_bytes()
        .map_err(|e| wasmtime::Error::msg(format!("encode error: {e}")))?;

    let alloc_fn = caller
        .get_export("shamir_alloc")
        .and_then(|e| e.into_func())
        .ok_or_else(|| wasmtime::Error::msg("missing export `shamir_alloc`"))?;
    let alloc_typed = alloc_fn.typed::<i32, i32>(&mut *caller)?;

    let out_len =
        i32::try_from(encoded.len()).map_err(|_| wasmtime::Error::msg("value too large"))?;
    let out_ptr = alloc_typed.call_async(&mut *caller, out_len).await?;

    if out_ptr < 0 {
        return Err(wasmtime::Error::msg(
            "shamir_alloc returned negative pointer",
        ));
    }

    let memory = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| wasmtime::Error::msg("missing export `memory`"))?;
    let start = out_ptr as usize;
    let end = start.saturating_add(encoded.len());
    if end > memory.data_size(&mut *caller) {
        return Err(wasmtime::Error::msg(
            "shamir_alloc returned pointer outside memory bounds",
        ));
    }
    memory.data_mut(&mut *caller)[start..end].copy_from_slice(&encoded);

    let packed = ((out_ptr as i64) << 32) | (encoded.len() as i64 & 0xFFFF_FFFF);
    Ok(packed)
}

/// Write raw bytes into guest memory via `shamir_alloc`, returning the
/// packed `(ptr, len)` i64. The byte-slice form of `write_value_to_guest`
/// (the payload is already-encoded msgpack, e.g. a `BatchResponse`).
pub(in crate::wasm) async fn write_bytes_to_guest(
    caller: &mut wasmtime::Caller<'_, HostState>,
    bytes: &[u8],
) -> Result<i64, wasmtime::Error> {
    let alloc_fn = caller
        .get_export("shamir_alloc")
        .and_then(|e| e.into_func())
        .ok_or_else(|| wasmtime::Error::msg("missing export `shamir_alloc`"))?;
    let alloc_typed = alloc_fn.typed::<i32, i32>(&mut *caller)?;
    let out_len =
        i32::try_from(bytes.len()).map_err(|_| wasmtime::Error::msg("value too large"))?;
    let out_ptr = alloc_typed.call_async(&mut *caller, out_len).await?;
    if out_ptr < 0 {
        return Err(wasmtime::Error::msg(
            "shamir_alloc returned negative pointer",
        ));
    }
    let memory = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| wasmtime::Error::msg("missing export `memory`"))?;
    let start = out_ptr as usize;
    let end = start.saturating_add(bytes.len());
    if end > memory.data_size(&mut *caller) {
        return Err(wasmtime::Error::msg(
            "shamir_alloc returned pointer outside memory bounds",
        ));
    }
    memory.data_mut(&mut *caller)[start..end].copy_from_slice(bytes);
    let packed = ((out_ptr as i64) << 32) | (bytes.len() as i64 & 0xFFFF_FFFF);
    Ok(packed)
}

pub(crate) fn encode_params(params: &Params) -> FnResult<Vec<u8>> {
    MessagePackCodec::encode(&MessagePackCodec, params.raw())
        .map_err(|e| FunctionError::Compute(format!("params encode error: {e}")))
}
