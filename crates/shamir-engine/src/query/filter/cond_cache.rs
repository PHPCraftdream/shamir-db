//! `CondCache` — pre-compiled `$cond` condition cache (#643).
//!
//! `resolve_filter_query`'s `FilterValue::Cond` arm used to call
//! `compile_filter(&cond.condition, ctx.interner)` on EVERY evaluation —
//! i.e. once per record in a per-row hot loop — even though `cond.condition`
//! (a `Box<Filter>`) is static per query: identical on every call for the
//! SAME `Cond` node, exactly like the top-level WHERE filter (already
//! compiled once outside the per-row loop).
//!
//! This module provides an opt-in cache keyed by the *content* of the
//! boxed `Filter` AST (see [`cond_key`]). Callers that build a `CondCache`
//! once (e.g. `SelectProjection::new`) and thread it through
//! `FilterContext::cond_cache` get pre-compiled `FilterNode`s for every
//! `$cond` in the tree; callers that never populate `cond_cache` fall back
//! to `FilterContext`'s own lazily-populated `local_cond_cache` (see
//! `resolve.rs`'s `Cond` arm and [`compile_cond_cached`]), which covers
//! WHERE, `when`, `for_each`'s `over`, and write-value resolution without
//! requiring any of those callers to pre-scan a tree up front.

use std::sync::Arc;

use shamir_types::core::interner::Interner;
use shamir_types::types::common::{THasher, TMap};

use super::compile::compile_filter;
use super::filter_node::FilterNode;
use crate::query::filter::{Filter, FilterValue};

/// Content-keyed cache mapping a `$cond`'s `condition: Box<Filter>` to its
/// pre-compiled `FilterNode`.
///
/// # Why content, not raw address (group 13 Defect 2 fix)
///
/// This cache used to be keyed on the raw pointer address of the boxed
/// `Filter` (`&*cond.condition as *const Filter as usize`). That is sound
/// ONLY as long as the cache never outlives the exact allocation it was
/// built from — but nothing in the type system enforced that. A `CondCache`
/// built from a `Filter` tree that is later dropped, followed by an
/// UNRELATED, semantically different `Filter` tree happening to be
/// allocated at the same (now-reused) address, would silently serve the
/// FIRST query's stale compiled predicate for the SECOND query — a wrong
/// answer, not merely a missed optimization. The key is now
/// [`format!("{:?}", condition)`] (the `Filter`'s full recursive `Debug`
/// representation): two conditions collide in this map if and only if they
/// are equal in content, regardless of where either lives in memory or
/// whether one was freed and reallocated over. This trades a per-lookup
/// `Debug`-format allocation for the address-reuse hazard being eliminated
/// outright — a good trade given the thing being cached (e.g. a compiled
/// `Regex`) is far more expensive to rebuild than a string format. As a
/// side effect, two independently-allocated but textually identical
/// conditions now correctly share one cache entry (they used to silently
/// miss each other under raw-address keying).
pub type CondCache = TMap<String, Arc<FilterNode>>;

/// Compute the content-derived cache key for a `Cond`'s condition.
#[inline]
fn cond_key(condition: &Filter) -> String {
    format!("{condition:?}")
}

/// `Sync`-safe fallback cache backing `FilterContext::local_cond_cache`
/// (group 13 Defect 1). Deliberately a DIFFERENT type from [`CondCache`]
/// (which stays a plain `TMap`, fine for its own single-threaded
/// build-once-then-read-only usage in `SelectProjection`): this one is
/// mutated through a SHARED `&FilterContext` reference, and `FilterContext`
/// must stay `Sync` because several batch-executor futures capture
/// `&FilterContext` across an `.await` and are boxed as
/// `Pin<Box<dyn Future<..> + Send>>` (`&T: Send` requires `T: Sync`).
/// `std::cell::RefCell` — the first thing tried here — is unconditionally
/// `!Sync` and broke that bound; `scc::HashMap` is this crate's standard
/// lock-free concurrent map (see CLAUDE.md's concurrency-primitive table)
/// and provides the same interior mutability without it.
pub(crate) type LocalCondCache = scc::HashMap<String, Arc<FilterNode>, THasher>;

/// Build an empty [`LocalCondCache`].
pub(crate) fn new_local_cond_cache() -> LocalCondCache {
    scc::HashMap::with_hasher(THasher::default())
}

/// Compile `condition` and cache it in `local_cache`, reusing an existing
/// entry when one is already present. Fallback path (group 13 Defect 1)
/// for callers that have NOT pre-scanned a static tree into an
/// explicit [`CondCache`] via [`prescan_cond_cache`] (WHERE, `when`,
/// `for_each`, write-value resolution): the FIRST evaluation of a given
/// `$cond` against one `FilterContext` compiles and caches it via this
/// function; every later evaluation of a content-identical condition
/// sharing that SAME context (e.g. every row of one WHERE-clause scan)
/// reuses the compiled `FilterNode` — see `resolve.rs`'s `Cond` arm, which
/// calls this only when `ctx.cond_cache` is `None`.
///
/// Uses the same content-derived key as [`cond_cache_get`], so — unlike the
/// pointer-keyed design this replaced — it is sound regardless of where
/// `condition` lives in memory or how long `local_cache` outlives it.
pub(crate) fn compile_cond_cached(
    local_cache: &LocalCondCache,
    condition: &Filter,
    interner: &Interner,
) -> Arc<FilterNode> {
    let key = cond_key(condition);
    if let Some(node) = local_cache.read_sync(&key, |_, v| Arc::clone(v)) {
        return node;
    }
    let node = Arc::new(compile_filter(condition, interner));
    match local_cache.insert_sync(key, Arc::clone(&node)) {
        Ok(()) => node,
        // Lost a race with a concurrent insert for the SAME key (defensive
        // only — a `FilterContext` is built fresh per query/op and never
        // shared across concurrently-executing threads in practice, see
        // its own doc comment). Use the winner's entry so every caller
        // observes ONE canonical compiled node instead of two equal-but-
        // distinct `Arc` allocations.
        Err((key, _attempted)) => local_cache
            .read_sync(&key, |_, v| Arc::clone(v))
            .unwrap_or(node),
    }
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

/// Look up a compiled `FilterNode` for a `Cond`'s condition by content.
/// Returns `None` on a cache miss (caller falls back to `compile_filter`).
#[inline]
pub fn cond_cache_get<'a>(cache: &'a CondCache, condition: &Filter) -> Option<&'a Arc<FilterNode>> {
    cache.get(&cond_key(condition))
}
