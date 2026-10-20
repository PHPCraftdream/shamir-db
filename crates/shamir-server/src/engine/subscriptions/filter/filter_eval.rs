use std::sync::Arc;

use shamir_db::engine::table::InternerReadView;
use shamir_db::query::filter::FilterNode;
use shamir_db::types::value::InnerValue;
use shamir_query_types::filter::Filter;

/// A compiled subscription filter cached for reuse across events.
///
/// **Correctness**: the interner can grow after subscription time (new field
/// names are interned as new record shapes appear). Compilation uses the
/// interner's `get_ind()` (read-only lookup) to resolve field paths; if a
/// path isn't found, the node folds to `FilterNode::False`. A filter compiled
/// against an early interner state would therefore miss records whose fields
/// were interned later.
///
/// To stay correct, we track the interner `generation()` (a monotonic
/// `AtomicU64` incremented on every new interning — O(1) lock-free) at
/// compile time. On each event we compare the current generation: if
/// unchanged, the cached `FilterNode` is reused (the hot path — a single
/// `u64` compare). If the interner grew, we recompile against the live
/// interner state.
///
/// Stored in an `Arc` so it can be cheaply swapped by any thread.
pub(crate) struct CompiledFilter {
    /// The compiled filter tree (resolves field paths, pre-computed sets).
    node: FilterNode,
    /// The interner `generation()` at compile time — used to detect growth.
    generation: u64,
}

/// Thread-safe cache slot for a `CompiledFilter`. Uses `ArcSwap` for lock-free
/// RCU reads on the hot path (every event does a single atomic load).
pub(crate) type CompiledFilterSlot = arc_swap::ArcSwapOption<CompiledFilter>;

/// Evaluate a `Filter` against raw msgpack record bytes via the zero-copy
/// `RecordView` lens, falling back to `InnerValue` decode for bare-scalar
/// (non-map) records.
///
/// `interner` must be the table's populated read-only interner view (from
/// `ShamirDb::get_table_interner_read_view`; S5c-X F1 Round 6 — the raw
/// `&Interner` / `Arc<OnceCell<Interner>>` no longer crosses the boundary).
///
/// Delegates to the engine's compiled `FilterNode::matches(&impl RecordRef)`
/// evaluator (via the view's engine-side `compile_filter` / `matches_bytes`,
/// which build the `FilterContext` engine-side), so all filter semantics
/// (Eq/Ne/Gt/Gte/Lt/Lte/In/NotIn/IsNull/IsNotNull/Exists/NotExists/
/// Like/Regex/Contains/ContainsAny/ContainsAll/Between/Fts/And/Or/Not)
/// are handled identically to the engine's query path.
///
/// This replaces the previous hand-rolled `filter_matches_inner` path which
/// walked `InnerValue::Map` manually.
pub fn filter_matches_bytes(filter: &Filter, bytes: &[u8], interner: &InternerReadView) -> bool {
    let compiled = interner.compile_filter(filter);
    interner.matches_bytes(&compiled, bytes)
}

/// Backward-compat shim: evaluate a `Filter` against a pre-decoded
/// `InnerValue` + interner. Delegates to the engine's compiled filter
/// evaluator via `RecordRef for InnerValue`.
///
/// Retained for callers that already hold a decoded `InnerValue` (e.g.
/// the journal-backfill path where `decode_record_value_inner` was already
/// called for value decoding).
pub fn filter_matches_inner(
    filter: &Filter,
    value: &InnerValue,
    interner: &InternerReadView,
) -> bool {
    let compiled = interner.compile_filter(filter);
    interner.matches_inner(&compiled, value)
}

/// Convenience: wrap raw bytes in an `Arc<[u8]>` for cache insertion.
/// Avoids a redundant copy when the caller already has `&[u8]`.
pub(crate) fn bytes_to_arc(bytes: &[u8]) -> Arc<[u8]> {
    Arc::from(bytes)
}

/// Evaluate a cached `CompiledFilter` against raw msgpack record bytes.
///
/// This is the hot-path entry point used by `bridge_task`. The caller passes
/// a `CompiledFilterSlot` (an `ArcSwapOption`) which is lazily populated on
/// first use and recompiled when the interner grows (detected via
/// `generation()` comparison). In the steady state (interner stable), this
/// is a single atomic load + `u64` compare + `FilterNode::matches` — no
/// `compile_filter` call, no `Regex::new`, no `TSet` build, no `String` clone.
///
/// **Correctness**: if `interner.generation()` has advanced since the cached
/// node was compiled, we recompile against the live interner so newly-interned
/// field paths are resolved correctly. The recompiled node is stored back
/// into the slot via `ArcSwap::store` for use by subsequent events.
pub(crate) fn filter_matches_bytes_cached(
    slot: &CompiledFilterSlot,
    filter: &Filter,
    bytes: &[u8],
    interner: &InternerReadView,
) -> bool {
    let cur_gen = interner.generation();

    // Load the cached compiled filter (single atomic load).
    if let Some(cached) = slot.load_full() {
        if cached.generation == cur_gen {
            // HOT PATH: interner unchanged — reuse the cached node directly.
            // Zero compile_filter cost, zero allocations.
            return filter_matches_with_node(&cached.node, bytes, interner);
        }
        // INTERNER GREW: recompile against the live interner state so
        // newly-interned field paths are resolved correctly. This is the
        // rare growth path (new field names written after subscription).
    }

    // COLD PATH: first event OR interner grew — compile once and cache.
    let compiled = Arc::new(CompiledFilter {
        node: interner.compile_filter(filter),
        generation: cur_gen,
    });
    slot.store(Some(Arc::clone(&compiled)));
    filter_matches_with_node(&compiled.node, bytes, interner)
}

/// Evaluate a pre-compiled `FilterNode` against raw bytes + interner.
/// Delegates to the engine-side evaluator (the `FilterContext` is built
/// there from an empty shared refs map — subscriptions never use
/// QueryRef/Param/FnCall), keeping the raw `&Interner` encapsulated.
fn filter_matches_with_node(node: &FilterNode, bytes: &[u8], interner: &InternerReadView) -> bool {
    interner.matches_bytes(node, bytes)
}
