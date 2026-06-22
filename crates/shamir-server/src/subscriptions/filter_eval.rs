use std::sync::Arc;

use shamir_collections::TMap;
use shamir_db::core::interner::Interner;
use shamir_db::query::filter::{compile_filter, FilterContext, FilterNode};
use shamir_db::query::read::QueryResult;
use shamir_db::record_view::RecordView;
use shamir_db::types::value::InnerValue;
use shamir_query_types::filter::Filter;
use tokio::sync::OnceCell;

/// A compiled subscription filter cached for reuse across events.
///
/// **Correctness**: the interner can grow after subscription time (new field
/// names are interned as new record shapes appear). `compile_filter` uses
/// `interner.get_ind()` (read-only lookup) to resolve field paths; if a path
/// isn't found, the node folds to `FilterNode::False`. A filter compiled
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
/// `interner_cell` must already be populated (guaranteed when the cell came
/// from `ShamirDb::get_table_interner_cell`); panics in debug builds if
/// the cell is empty (indicates a programming error).
///
/// Delegates to the engine's compiled `FilterNode::matches(&impl RecordRef)`
/// evaluator via `compile_filter` + `FilterContext`, so all filter semantics
/// (Eq/Ne/Gt/Gte/Lt/Lte/In/NotIn/IsNull/IsNotNull/Exists/NotExists/
/// Like/Regex/Contains/ContainsAny/ContainsAll/Between/Fts/And/Or/Not)
/// are handled identically to the engine's query path.
///
/// This replaces the previous hand-rolled `filter_matches_inner` path which
/// walked `InnerValue::Map` manually.
pub fn filter_matches_bytes(
    filter: &Filter,
    bytes: &[u8],
    interner_cell: &OnceCell<Interner>,
) -> bool {
    // SAFETY: the cell is always populated before being stored in the
    // decode cache (see ShamirDb::get_table_interner_cell).
    let interner = match interner_cell.get() {
        Some(i) => i,
        None => {
            debug_assert!(false, "interner_cell must be populated before filter eval");
            return false;
        }
    };

    let compiled = compile_filter(filter, interner);
    let ctx = make_filter_context(interner);

    // Try zero-copy RecordView lens first (handles map-shaped records).
    match RecordView::new(bytes) {
        Ok(view) => compiled.matches(&view, &ctx),
        // Non-map bytes (bare scalar / legacy record): fall back to full
        // InnerValue decode. Mirrors the engine's doctor/delete/update
        // fallback path — we never silently drop a record from changefeed
        // matching.
        Err(_) => match InnerValue::from_bytes(bytes) {
            Ok(inner) => compiled.matches(&inner, &ctx),
            Err(_) => {
                tracing::warn!(
                    "subscription filter: failed to decode record bytes \
                     (both RecordView and InnerValue), skipping (fail-closed)"
                );
                false
            }
        },
    }
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
    interner_cell: &OnceCell<Interner>,
) -> bool {
    let interner = match interner_cell.get() {
        Some(i) => i,
        None => {
            debug_assert!(false, "interner_cell must be populated before filter eval");
            return false;
        }
    };

    let compiled = compile_filter(filter, interner);
    let ctx = make_filter_context(interner);
    compiled.matches(value, &ctx)
}

/// Build a minimal `FilterContext` for subscription filter evaluation.
///
/// Subscriptions don't use QueryRef, Param, FnCall, or Actor-based filters,
/// so all auxiliary fields are empty / System.
fn make_filter_context(interner: &Interner) -> FilterContext<'_> {
    static EMPTY_REFS: std::sync::OnceLock<TMap<String, QueryResult>> = std::sync::OnceLock::new();
    let refs = EMPTY_REFS.get_or_init(shamir_collections::new_map);
    FilterContext::new(interner, refs)
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
    interner_cell: &OnceCell<Interner>,
) -> bool {
    let interner = match interner_cell.get() {
        Some(i) => i,
        None => {
            debug_assert!(false, "interner_cell must be populated before filter eval");
            return false;
        }
    };

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
        node: compile_filter(filter, interner),
        generation: cur_gen,
    });
    slot.store(Some(Arc::clone(&compiled)));
    filter_matches_with_node(&compiled.node, bytes, interner)
}

/// Evaluate a pre-compiled `FilterNode` against raw bytes + interner.
fn filter_matches_with_node(node: &FilterNode, bytes: &[u8], interner: &Interner) -> bool {
    let ctx = make_filter_context(interner);
    match RecordView::new(bytes) {
        Ok(view) => node.matches(&view, &ctx),
        Err(_) => match InnerValue::from_bytes(bytes) {
            Ok(inner) => node.matches(&inner, &ctx),
            Err(_) => {
                tracing::warn!(
                    "subscription filter: failed to decode record bytes \
                     (both RecordView and InnerValue), skipping (fail-closed)"
                );
                false
            }
        },
    }
}
