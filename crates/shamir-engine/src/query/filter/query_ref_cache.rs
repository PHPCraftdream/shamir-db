//! `QueryRefCache` — lazy per-scan cache for `$query` path resolution (F2).
//!
//! `resolve_filter_query`'s `FilterValue::QueryRef` arm used to RE-PARSE the
//! `path` string (`rest.find(['.', '['])` / `usize::parse` / prefix strips,
//! one pass per path segment) AND re-walk the referenced `QueryResult`'s
//! `Map`/`List` structure on EVERY record — even though `path` (an
//! `Option<String>`) is static per query and the resolved target is invariant
//! across every row of one scan (`ctx.resolved_refs` is a fixed
//! `&'a TMap<String, QueryResult>` reference for the whole scan).
//!
//! This is the same family of redundancy F1 (`FieldPathCache`) and #643
//! (`CondCache`) already removed for `FieldRef`/`Cond`, BUT the mechanism is
//! structurally DIFFERENT from F1: see [`QueryRefCache`]'s doc comment.
//!
//! This module provides an opt-in cache keyed by the raw pointer address of
//! the `QueryRef` node itself. Callers that build a `QueryRefCache` once
//! (e.g. `SelectProjection::new`) and thread it through
//! `FilterContext::query_ref_cache` skip the per-row path re-parse + target
//! re-walk on every record after the first; callers that never populate
//! `query_ref_cache` (the overwhelming majority — WHERE, `when`, `for_each`'s
//! `over`, write-value resolution) are completely unaffected —
//! `resolve_filter_query` falls back to `resolve_query_ref_value` exactly as
//! before.

use std::sync::OnceLock;

use shamir_types::types::common::TMap;
use shamir_types::types::value::QueryValue;

use crate::query::filter::{Filter, FilterValue};

/// Pointer-keyed cache mapping a `QueryRef` node (by raw address of the
/// enclosing `FilterValue`) to a lazily-populated cell holding the resolved
/// `Option<QueryValue>` for the CURRENT scan.
///
/// # Why this is LAZY, not eager (the structural difference from F1)
///
/// F1's `FieldPathCache` could be populated EAGERLY at prescan time because
/// a `FieldRef`'s resolution (path → interned keys) is 100% static per query
/// — no scan data is involved. A `$query`/`QueryRef`'s resolution is NOT
/// static-at-compile-time in the same way: the actual referenced
/// `QueryResult` data (`ctx.resolved_refs`) only exists once the
/// sub-batch/read execution begins — it is NOT available at
/// `SelectProjection::new()`. So [`prescan_query_ref_cache`] can only
/// RESERVE a slot (`OnceLock::new()`, empty); the slot is FILLED lazily on
/// the first row that hits the node (via `OnceLock::get_or_init` in
/// `resolve_filter_query`'s `QueryRef` arm).
///
/// This is the exact same shape `In`'s `ref_column_sets`
/// (`filter_node.rs`) already uses in-tree — NOT `FieldPathCache`/
/// `CondCache`'s eager-prescan pattern. The resolved value depends on
/// per-scan runtime data (`ctx.resolved_refs`) that doesn't exist yet at
/// compile time, exactly the condition the brief flags for choosing the
/// OnceLock-per-node lazy template.
///
/// # What this caches (and what it does NOT)
///
/// On every row after the first, per `QueryRef` node, this cache removes:
/// the `alias.strip_prefix` + `resolved_refs.get` map lookup (cheap already),
/// the STRING PATH PARSING (`find`/`usize::parse`/prefix strips, one pass per
/// segment), and the multi-step `Map`/`List` navigation walk to locate the
/// target value.
///
/// It does NOT remove the final `QueryValue::clone` of the resolved target —
/// `resolve_filter_query`'s public contract returns `Option<QueryValue>`
/// (owned) everywhere, and changing that to `Option<Cow<QueryValue>>` would
/// ripple through `FnCall`/`Expr`/`Cond`/`Array` resolution — a much larger,
/// riskier refactor explicitly out of scope for F2. Only the unavoidable
/// final `QueryValue::clone` remains; the per-row parsing + navigation is
/// gone.
///
/// # Safety / validity invariant
///
/// The pointer key (`fv as *const FilterValue as usize`) is SAFE and stable
/// ONLY because the cache is built once from an owned, never-cloned-per-row
/// `FilterValue` tree (see [`prescan_query_ref_cache`], called once at
/// query-compile time, e.g. `SelectProjection::new`): the `FilterValue` tree
/// this cache was built from must outlive the cache and must never be
/// cloned/moved after construction — pointer identity is the cache key. If
/// the tree were cloned, the clone's `QueryRef` nodes would live at different
/// addresses and the cache would silently miss (falling back to per-row
/// resolution, which is correct but uncached — a soft failure, not a
/// memory-safety one, since the key is only ever used to look up an entry,
/// never dereferenced).
///
/// # Per-scan freshness
///
/// The cache MUST be rebuilt (`new_map()` + [`prescan_query_ref_cache`]) per
/// `SelectProjection`/scan, never reused across unrelated queries: the
/// cached `QueryValue` is a snapshot of ONE scan's `ctx.resolved_refs`, and
/// `resolved_refs` differs per scan. The `OnceLock` cells start empty for
/// each fresh scan, so the first row repopulates them against that scan's
/// own `resolved_refs`. `SelectProjection` owns its cache as a plain field
/// built in `new()`, guaranteeing one fresh cache per projection lifetime.
pub type QueryRefCache = TMap<usize, OnceLock<Option<QueryValue>>>;

/// Recursively walk a `FilterValue` tree, RESERVING an empty cache slot for
/// every nested `QueryRef` node (at ANY nesting depth — inside `FnCall`
/// args, `Expr` operands, `Cond` `then`/`or_else` branches, `Array`
/// elements, and inside the condition `Filter` trees themselves, which may
/// embed further `FilterValue`s — e.g. an `Eq`'s `value` — that in turn
/// contain nested `QueryRef`s).
///
/// Mirrors `resolve_filter_query`'s own dispatch structure (and
/// `prescan_field_path_cache`/`prescan_cond_cache`'s recursion shape) so
/// every `FilterValue` shape capable of containing a `QueryRef` is visited.
///
/// NOTE the signature difference from `prescan_field_path_cache` /
/// `prescan_cond_cache`: this prescan takes NO `interner` and NO
/// `resolved_refs` parameter. It CANNOT resolve the value at prescan time —
/// `ctx.resolved_refs` does not exist yet at `SelectProjection::new()` time
/// (it only materialises once the scan/sub-batch begins). So this prescan
/// only RESERVES the slot; the value is filled lazily during
/// `resolve_filter_query`'s `QueryRef` arm via `OnceLock::get_or_init` on
/// the first row that hits the node.
pub fn prescan_query_ref_cache(fv: &FilterValue, cache: &mut QueryRefCache) {
    match fv {
        FilterValue::Null
        | FilterValue::Bool(_)
        | FilterValue::Int(_)
        | FilterValue::Float(_)
        | FilterValue::String(_)
        | FilterValue::Binary(_)
        | FilterValue::FieldRef { .. }
        | FilterValue::Param { .. } => {}
        FilterValue::QueryRef { .. } => {
            // Reserve a slot — an EMPTY `OnceLock`, no value yet. The value
            // is filled lazily on the first row via `get_or_init` (see
            // `resolve.rs`'s `QueryRef` arm). The key is the enclosing node's
            // pointer identity (NOT the inline `alias`/`path` fields) — same
            // invariant `FieldPathCache`/`CondCache` document: the tree this
            // cache was built from must outlive the cache and never be
            // cloned/moved after construction. `or_default()` constructs an
            // empty `OnceLock` (its `Default` impl) — zero-cost.
            cache.entry(fv as *const FilterValue as usize).or_default();
        }
        FilterValue::Array(items) => {
            for item in items {
                prescan_query_ref_cache(item, cache);
            }
        }
        FilterValue::FnCall { call } => {
            for arg in call.args() {
                prescan_query_ref_cache(arg, cache);
            }
        }
        FilterValue::Expr { expr } => {
            for arg in &expr.args {
                prescan_query_ref_cache(arg, cache);
            }
        }
        FilterValue::Cond { cond } => {
            // The condition's `Filter` tree may itself embed `FilterValue`s
            // (e.g. `Filter::Eq { value, .. }`) that contain further nested
            // `QueryRef`s — walk it too so those get reserved.
            prescan_filter(&cond.condition, cache);
            prescan_query_ref_cache(&cond.then, cache);
            prescan_query_ref_cache(&cond.or_else, cache);
        }
    }
}

/// Walk a `Filter` AST's embedded `FilterValue`s (comparison operands,
/// membership lists, etc.) looking for further nested `QueryRef`s, and
/// recurse into logical combinators (`And`/`Or`/`Not`). Mirrors
/// `field_path_cache.rs`'s / `cond_cache.rs`'s `prescan_filter` dispatch
/// shape.
fn prescan_filter(filter: &Filter, cache: &mut QueryRefCache) {
    match filter {
        Filter::Eq { value, .. }
        | Filter::Ne { value, .. }
        | Filter::Gt { value, .. }
        | Filter::Gte { value, .. }
        | Filter::Lt { value, .. }
        | Filter::Lte { value, .. }
        | Filter::FieldEq { value, .. }
        | Filter::Contains { value, .. } => {
            prescan_query_ref_cache(value, cache);
        }
        Filter::In { values, .. }
        | Filter::NotIn { values, .. }
        | Filter::ContainsAny { values, .. }
        | Filter::ContainsAll { values, .. } => {
            for v in values {
                prescan_query_ref_cache(v, cache);
            }
        }
        Filter::Between { from, to, .. } => {
            prescan_query_ref_cache(from, cache);
            prescan_query_ref_cache(to, cache);
        }
        Filter::ValueCompare { left, right, .. } => {
            prescan_query_ref_cache(left, cache);
            prescan_query_ref_cache(right, cache);
        }
        Filter::Computed {
            expr_args, value, ..
        } => {
            if let Some(args) = expr_args {
                for a in args {
                    prescan_query_ref_cache(a, cache);
                }
            }
            prescan_query_ref_cache(value, cache);
        }
        Filter::And { filters } | Filter::Or { filters } => {
            for f in filters {
                prescan_filter(f, cache);
            }
        }
        Filter::Not { filter } => prescan_filter(filter, cache),
        Filter::Like { .. }
        | Filter::ILike { .. }
        | Filter::Regex { .. }
        | Filter::IsNull { .. }
        | Filter::IsNotNull { .. }
        | Filter::Exists { .. }
        | Filter::NotExists { .. }
        | Filter::Fts { .. }
        | Filter::VectorSimilarity { .. } => {}
    }
}
