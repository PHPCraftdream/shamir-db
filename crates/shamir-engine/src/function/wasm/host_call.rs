use super::super::context::{FnBatch, FnCtx};
use super::super::params::Params;
use super::wasm_function::{read_guest_mem, HostState};
use shamir_types::types::value::QueryValue;

// ── Async host import: call ───────────────────────────────────────────
//
// Borrow-dance across await:
// 1. Read name + params bytes from guest memory into owned Vecs (sync).
// 2. Clone Arc<BatchContext>, Arc<GlobalVars>, Option<Arc<FunctionRegistry>>,
//    depth, depth_limit from caller.data() into owned locals.
// 3. Drop all borrows on Caller.
// 4. .await the registry.invoke call (may take arbitrary time).
// 5. Re-acquire Caller (mutable), get memory + shamir_alloc, write result.

/// Host implementation of `call(name_ptr, name_len, params_ptr, params_len) -> i64`.
///
/// Invokes another registered function by name. The callee shares the same
/// batch context and globals. On error (depth exceeded, function not found,
/// callee trapped), the host import traps — propagating as the caller's
/// `FunctionError::Compute`.
/// Host implementation of `call(name_ptr, name_len, params_ptr, params_len) -> i64`.
///
/// Invokes another registered function by name. The callee shares the same
/// batch context and globals. On error (depth exceeded, function not found,
/// callee trapped), the host import traps — propagating as the caller's
/// `FunctionError::Compute`.
///
/// # Borrow dance across await
///
/// 1. Read `name` + `params` bytes from guest memory via `Caller` (sync).
/// 2. Clone `Arc<BatchContext>`, `Arc<GlobalVars>`, `Option<Arc<FunctionRegistry>>`,
///    `depth`, `depth_limit` from `caller.data()` into owned locals.
/// 3. Drop the `&Caller` borrow on `memory` / `data`.
/// 4. `.await` the `registry.invoke` call.
/// 5. Re-acquire `Caller` (mutable), get `memory` + `shamir_alloc`, write result.
///
/// The `Caller<'_, HostState>` is kept alive for the entire future lifetime
/// because the returned `Box<dyn Future + Send + '_>` captures it. Wasmtime
/// guarantees `Caller` is `Send` when `T: Send` and async support is active.
pub(super) fn host_call(
    mut caller: wasmtime::Caller<'_, HostState>,
    (name_ptr, name_len, params_ptr, params_len): (i32, i32, i32, i32),
) -> Box<dyn std::future::Future<Output = Result<i64, wasmtime::Error>> + Send + '_> {
    Box::new(async move {
        // ── Phase 1: read inputs + clone Arcs (all sync, before any await) ──
        let name_bytes;
        let params_bytes;
        let registry;
        let batch_ctx;
        let globals;
        let next_depth;
        let depth_limit;
        {
            let memory = caller
                .get_export("memory")
                .and_then(|e| e.into_memory())
                .ok_or_else(|| wasmtime::Error::msg("call: missing export `memory`"))?;

            name_bytes = read_guest_mem(memory.data(&caller), name_ptr, name_len)?;
            params_bytes = read_guest_mem(memory.data(&caller), params_ptr, params_len)?;

            let state = caller.data();
            registry = state.registry.clone();
            batch_ctx = state.batch.clone();
            globals = state.globals.clone();
            next_depth = state.depth.saturating_add(1);
            depth_limit = state.depth_limit;
        }
        // Borrows on caller (memory, data) are dropped. Caller itself is still alive.

        let name = String::from_utf8(name_bytes)
            .map_err(|_| wasmtime::Error::msg("call: name is not valid UTF-8"))?;

        let params_value = QueryValue::from_bytes(&params_bytes)
            .map_err(|e| wasmtime::Error::msg(format!("call: params decode error: {e}")))?;
        let params = Params::from_value(params_value)
            .map_err(|e| wasmtime::Error::msg(format!("call: params not a map: {e}")))?;

        // ── Depth check ──
        if next_depth > depth_limit {
            return Err(wasmtime::Error::msg(format!(
                "call depth limit exceeded: {next_depth} > {depth_limit}"
            )));
        }

        let reg = registry
            .ok_or_else(|| wasmtime::Error::msg(format!("call: function not found: {name}")))?;

        // ── Phase 2: await the callee ──
        // TODO(Shomer R2): thread actor from parent FnCtx into child
        let child_ctx = FnCtx::with_globals(globals)
            .with_registry(reg)
            .with_depth(next_depth)
            .with_depth_limit(depth_limit);
        let child_batch = FnBatch::with_context(batch_ctx);

        let result = child_ctx
            .registry()
            .expect("registry just set")
            .invoke(&name, &child_ctx, &child_batch, &params)
            .await
            .map_err(|e| wasmtime::Error::msg(format!("call: {e}")))?;

        // ── Phase 3: write result back into guest memory ──
        let encoded = result
            .to_bytes()
            .map_err(|e| wasmtime::Error::msg(format!("call: result encode error: {e}")))?;

        let alloc_fn = caller
            .get_export("shamir_alloc")
            .and_then(|e| e.into_func())
            .ok_or_else(|| wasmtime::Error::msg("call: missing export `shamir_alloc`"))?;
        let alloc_typed = alloc_fn.typed::<i32, i32>(&caller)?;

        let out_len =
            i32::try_from(encoded.len()).map_err(|_| wasmtime::Error::msg("result too large"))?;
        let out_ptr = alloc_typed.call_async(&mut caller, out_len).await?;

        if out_ptr < 0 {
            return Err(wasmtime::Error::msg(
                "shamir_alloc returned negative pointer",
            ));
        }

        let memory = caller
            .get_export("memory")
            .and_then(|e| e.into_memory())
            .ok_or_else(|| wasmtime::Error::msg("call: missing export `memory`"))?;
        let start = out_ptr as usize;
        let end = start.saturating_add(encoded.len());
        if end > memory.data_size(&caller) {
            return Err(wasmtime::Error::msg(
                "shamir_alloc returned pointer outside memory bounds",
            ));
        }
        memory.data_mut(&mut caller)[start..end].copy_from_slice(&encoded);

        let packed = ((out_ptr as i64) << 32) | (encoded.len() as i64 & 0xFFFF_FFFF);
        Ok(packed)
    })
}
