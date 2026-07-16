//! `CondCache` — pre-compiled `$cond` condition cache (#643).
//!
//! `resolve_filter_query`'s `FilterValue::Cond` arm used to call
//! `compile_filter(&cond.condition, ctx.interner)` on EVERY evaluation —
//! i.e. once per record in a per-row hot loop — even though `cond.condition`
//! (a `Box<Filter>`) is static per query: identical on every call for the
//! SAME `Cond` node, exactly like the top-level WHERE filter (already
//! compiled once outside the per-row loop).
//!
//! This module provides an opt-in cache keyed by the raw pointer address of
//! the boxed `Filter` AST. Callers that build a `CondCache` once (e.g.
//! `SelectProjection::new`) and thread it through `FilterContext::cond_cache`
//! get pre-compiled `FilterNode`s for every `$cond` in the tree; callers that
//! never populate `cond_cache` (the overwhelming majority — WHERE, `when`,
//! `for_each`'s `over`, write-value resolution) are completely unaffected —
//! `resolve_filter_query` falls back to `compile_filter` exactly as before.

use std::sync::Arc;

use shamir_types::core::interner::Interner;
use shamir_types::types::common::TMap;

use super::compile::compile_filter;
use super::filter_node::FilterNode;
use crate::query::filter::{Filter, FilterValue};

/// Pointer-keyed cache mapping a `$cond`'s `condition: Box<Filter>` (by raw
/// address) to its pre-compiled `FilterNode`.
///
/// # Safety / validity invariant
///
/// The pointer key (`&*cond.condition as *const Filter as usize`) is SAFE
/// and stable ONLY because the cache is built once from an owned,
/// never-cloned-per-row `FilterValue` tree (see [`prescan_cond_cache`],
/// called once at query-compile time, e.g. `SelectProjection::new`): the
/// `FilterValue` tree this cache was built from must outlive the cache and
/// must never be cloned/moved after construction — pointer identity is the
/// cache key. If the tree were cloned, the clone's `Cond` nodes would live
/// at different addresses and the cache would silently miss (falling back
/// to `compile_filter`, which is correct but uncached — a soft failure, not
/// a memory-safety one, since the key is only ever used to look up an
/// entry, never dereferenced).
pub type CondCache = TMap<usize, Arc<FilterNode>>;

/// Compute the pointer-identity cache key for a `Cond`'s condition.
#[inline]
fn cond_key(condition: &Filter) -> usize {
    condition as *const Filter as usize
}

/// Recursively walk a `FilterValue` tree, compiling and caching every
/// nested `$cond`'s condition (at ANY nesting depth — inside `FnCall` args,
/// `Expr` operands, `Cond` `then`/`or_else` branches, `Array` elements, and
/// inside the condition `Filter` trees themselves, which may embed further
/// `FilterValue`s — e.g. an `Eq`'s `value` — that in turn contain nested
/// `$cond`s).
///
/// Mirrors `resolve_filter_query`'s own dispatch structure so every
/// `FilterValue` shape capable of containing a `Cond` is visited.
pub fn prescan_cond_cache(fv: &FilterValue, interner: &Interner, cache: &mut CondCache) {
    match fv {
        FilterValue::Null
        | FilterValue::Bool(_)
        | FilterValue::Int(_)
        | FilterValue::Float(_)
        | FilterValue::String(_)
        | FilterValue::Binary(_)
        | FilterValue::FieldRef { .. }
        | FilterValue::QueryRef { .. }
        | FilterValue::Param { .. } => {}
        FilterValue::Array(items) => {
            for item in items {
                prescan_cond_cache(item, interner, cache);
            }
        }
        FilterValue::FnCall { call } => {
            for arg in call.args() {
                prescan_cond_cache(arg, interner, cache);
            }
        }
        FilterValue::Expr { expr } => {
            for arg in &expr.args {
                prescan_cond_cache(arg, interner, cache);
            }
        }
        FilterValue::Cond { cond } => {
            let key = cond_key(&cond.condition);
            cache
                .entry(key)
                .or_insert_with(|| Arc::new(compile_filter(&cond.condition, interner)));
            // The condition's `Filter` tree may itself embed `FilterValue`s
            // (e.g. `Filter::Eq { value, .. }`) that contain further nested
            // `$cond`s — walk it too so those get cached.
            prescan_filter(&cond.condition, interner, cache);
            prescan_cond_cache(&cond.then, interner, cache);
            prescan_cond_cache(&cond.or_else, interner, cache);
        }
    }
}

/// Walk a `Filter` AST's embedded `FilterValue`s (comparison operands,
/// membership lists, etc.) looking for further nested `$cond`s, and recurse
/// into logical combinators (`And`/`Or`/`Not`).
fn prescan_filter(filter: &Filter, interner: &Interner, cache: &mut CondCache) {
    match filter {
        Filter::Eq { value, .. }
        | Filter::Ne { value, .. }
        | Filter::Gt { value, .. }
        | Filter::Gte { value, .. }
        | Filter::Lt { value, .. }
        | Filter::Lte { value, .. }
        | Filter::FieldEq { value, .. }
        | Filter::Contains { value, .. } => {
            prescan_cond_cache(value, interner, cache);
        }
        Filter::In { values, .. }
        | Filter::NotIn { values, .. }
        | Filter::ContainsAny { values, .. }
        | Filter::ContainsAll { values, .. } => {
            for v in values {
                prescan_cond_cache(v, interner, cache);
            }
        }
        Filter::Between { from, to, .. } => {
            prescan_cond_cache(from, interner, cache);
            prescan_cond_cache(to, interner, cache);
        }
        Filter::ValueCompare { left, right, .. } => {
            prescan_cond_cache(left, interner, cache);
            prescan_cond_cache(right, interner, cache);
        }
        Filter::Computed {
            expr_args, value, ..
        } => {
            if let Some(args) = expr_args {
                for a in args {
                    prescan_cond_cache(a, interner, cache);
                }
            }
            prescan_cond_cache(value, interner, cache);
        }
        Filter::And { filters } | Filter::Or { filters } => {
            for f in filters {
                prescan_filter(f, interner, cache);
            }
        }
        Filter::Not { filter } => prescan_filter(filter, interner, cache),
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

/// Look up a compiled `FilterNode` for a `Cond`'s condition by pointer
/// identity. Returns `None` on a cache miss (caller falls back to
/// `compile_filter`).
#[inline]
pub fn cond_cache_get<'a>(cache: &'a CondCache, condition: &Filter) -> Option<&'a Arc<FilterNode>> {
    cache.get(&cond_key(condition))
}
