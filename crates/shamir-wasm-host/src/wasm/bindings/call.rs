use super::super::fuel::{pool_refund, FuelLease};
use super::super::wasm_function::{read_guest_mem, HostState};
use crate::runtime::context::{FnBatch, FnCtx};
use crate::runtime::params::Params;
use shamir_types::types::value::QueryValue;
use std::sync::atomic::Ordering;

// ── Async host import: call ───────────────────────────────────────────
//
// Borrow-dance across await:
// 1. Read name + params bytes from guest memory into owned Vecs (sync).
// 2. Clone Arc<BatchContext>, Arc<GlobalVars>, Option<Arc<FunctionRegistry>>,
//    depth, depth_limit from caller.data() into owned locals.
// 3. Drop all borrows on Caller.
// 4. .await the registry.invoke call (may take arbitrary time).
// 5. Re-acquire Caller (mutable), get memory + shamir_alloc, write result.

/// Decodes guest call parameters: msgpack `QueryValue` map → `Params`.
///
/// Map keys are preserved verbatim (RV-WASM-HOST-B1A78B5B); the legacy
/// typed-prefix interpretation is confined to `LegacyPrefixed` in
/// `shamir-types` and never runs on guest input.
pub(crate) fn decode_guest_params(bytes: &[u8]) -> Result<Params, wasmtime::Error> {
    let params_value = QueryValue::from_bytes(bytes)
        .map_err(|e| wasmtime::Error::msg(format!("params decode error: {e}")))?;
    Params::from_value(params_value)
        .map_err(|e| wasmtime::Error::msg(format!("params not a map: {e}")))
}

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
pub(in crate::wasm) fn host_call(
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
        let fuel_pool;
        let baseline;
        let own_cap_left;
        let actor;
        let db_gateway;
        let repo;
        let net_gateway;
        let secret_grants;
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
            fuel_pool = state.fuel_pool.clone();
            baseline = state.fuel_baseline.load(Ordering::Relaxed);
            own_cap_left = state.own_cap_left.load(Ordering::Relaxed);
            // RI-7: clone the per-invocation identity + capabilities so the
            // child FnCtx (Phase 2) inherits the SAME actor instead of
            // Actor::System, and keeps db/repo/net/secret-grant access
            // instead of silently losing them (same unfinished-plumbing site).
            actor = state.actor.clone();
            db_gateway = state.db.clone();
            repo = state.repo.clone();
            net_gateway = state.net.clone();
            secret_grants = state.secret_grants.clone();
        }
        // Borrows on caller (memory, data) are dropped. Caller itself is still alive.
        #[cfg(test)]
        let observer = caller.data().observer.clone();

        // RV-WASM-HOST-6F709AC2 (exclusive-lease design): suspend the parent —
        // REVOKE its executable fuel first, then release the reservation, so the
        // child can draw the parent's unused capacity while the parent holds
        // nothing executable. Segment consumption is charged against the parent's
        // own cap.
        let parent_unused = caller.get_fuel().unwrap_or(0);
        let parent_spent = baseline.saturating_sub(parent_unused);
        caller
            .set_fuel(0)
            .map_err(|e| wasmtime::Error::msg(format!("call: suspend parent fuel: {e}")))?;
        {
            let state = caller.data();
            state.fuel_baseline.store(0, Ordering::Relaxed);
            state
                .own_cap_left
                .store(own_cap_left.saturating_sub(parent_spent), Ordering::Relaxed);
        }
        pool_refund(&fuel_pool, parent_unused);
        #[cfg(test)]
        if let Some(obs) = observer.as_ref() {
            // Observe the REAL fuel after the revoke (must be 0) and record
            // the parent's segment consumption baseline − before.
            let after = caller.get_fuel().unwrap_or(0);
            let store_id = caller.data().store_id;
            obs.record_suspend(store_id, baseline, parent_unused, after);
        }

        let name = String::from_utf8(name_bytes)
            .map_err(|_| wasmtime::Error::msg("call: name is not valid UTF-8"))?;

        // RV-WASM-HOST-B1A78B5B: literal guest map keys are preserved here —
        // the legacy typed-prefix interpretation lives only in
        // `shamir_types::types::value::LegacyPrefixed`.
        let params = decode_guest_params(&params_bytes)
            .map_err(|e| wasmtime::Error::msg(format!("call: params decode error: {e}")))?;

        // ── Depth check ──
        if next_depth > depth_limit {
            return Err(wasmtime::Error::msg(format!(
                "call depth limit exceeded: {next_depth} > {depth_limit}"
            )));
        }

        let reg = registry
            .ok_or_else(|| wasmtime::Error::msg(format!("call: function not found: {name}")))?;

        // ── Phase 2: await the callee ──
        // RI-7: thread the parent invocation's actor into the child FnCtx so
        // a nested ctx.call(...) inherits the SAME actor instead of silently
        // defaulting to Actor::System (a confused-deputy privilege-escalation
        // primitive). db/repo/net/secret_grants are threaded the same way so
        // the nested call inherits the parent's capabilities instead of
        // silently losing them (the same unfinished-plumbing site — fail-
        // closed, lower urgency than the actor fail-open, fixed together).
        let mut child_ctx = FnCtx::with_globals(globals)
            .with_registry(reg)
            .with_depth(next_depth)
            .with_depth_limit(depth_limit)
            .with_actor(actor)
            .with_shared_secret_grants(secret_grants)
            .with_fuel_budget(fuel_pool.clone());
        #[cfg(test)]
        if let Some(obs) = observer.clone() {
            child_ctx = child_ctx.with_fuel_observer(obs);
        }
        if let Some(db) = db_gateway {
            child_ctx = child_ctx.with_db(db, repo);
        }
        if let Some(net) = net_gateway {
            child_ctx = child_ctx.with_net(net);
        }
        let child_batch = FnBatch::with_context(batch_ctx);

        let invoked = child_ctx
            .registry()
            .expect("registry just set")
            .invoke(&name, &child_ctx, &child_batch, &params)
            .await;
        // Reacquire for the parent BEFORE any parent guest code runs again (the
        // shamir_alloc below) and BEFORE propagating the child's result or error.
        // The ceiling is min(own_cap_left, the amount just relinquished) — the
        // suspended parent gave up `parent_unused`, so it may reclaim at most
        // that much, AND at most its own remaining cap. A failed `set_fuel`
        // install refunds through the [`FuelLease`] drop on the `?` path.
        {
            let cap_left = caller.data().own_cap_left.load(Ordering::Relaxed);
            let lease = FuelLease::take(&fuel_pool, cap_left.min(parent_unused));
            let reacquired = lease.amount();
            caller
                .set_fuel(reacquired)
                .map_err(|e| wasmtime::Error::msg(format!("call: resume parent fuel: {e}")))?;
            lease.consume();
            caller
                .data()
                .fuel_baseline
                .store(reacquired, Ordering::Relaxed);
            #[cfg(test)]
            if let Some(obs) = observer.as_ref() {
                // Observe the REAL fuel after the reacquire install.
                let actual = caller.get_fuel().unwrap_or(0);
                let store_id = caller.data().store_id;
                obs.record_install(store_id, reacquired, actual, cap_left);
            }
        }
        // Preserve the child's typed cause (fuel / interrupt / aggregate) by
        // wrapping the `FunctionError` as the SOURCE of the wasmtime error —
        // never stringified here; `map_wasm_error` recovers it by downcast.
        let result = invoked.map_err(wasmtime::Error::new)?;

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
