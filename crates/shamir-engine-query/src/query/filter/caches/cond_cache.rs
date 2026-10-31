//! `CondCache` — pre-compiled `$cond` condition cache (#643).
//!
//! `resolve_filter_query`'s `FilterValue::Cond` arm used to call
//! `compile_filter(&cond.condition, ctx.interner)` on EVERY evaluation —
//! i.e. once per record in a per-row hot loop — even though `cond.condition`
//! (a `Box<Filter>`) is static per query: identical on every call for the
//! SAME `Cond` node, exactly like the top-level WHERE filter (already
//! compiled once outside the per-row loop).
//!
//! This module provides an opt-in cache keyed by the ADDRESS of the boxed
//! condition node, guarded by a structural-hash pre-filter plus full
//! `PartialEq` content equality (see `CondCache`'s doc). Callers that build
//! a `CondCache`
//! once (e.g. `SelectProjection::new`) and thread it through
//! `FilterContext::cond_cache` get pre-compiled `FilterNode`s for every
//! `$cond` in the tree; callers that never populate `cond_cache` fall back
//! to `FilterContext`'s own lazily-populated `local_cond_cache` (see
//! `resolve.rs`'s `Cond` arm and `compile_cond_cached`), which covers
//! WHERE, `when`, `for_each`'s `over`, and write-value resolution without
//! requiring any of those callers to pre-scan a tree up front.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use shamir_types::core::interner::Interner;
use shamir_types::types::common::{THasher, TMap};

use crate::query::filter::evaluation::filter_node::FilterNode;
use crate::query::filter::{try_compile_filter, Filter, FilterValue};

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
/// Prepared, plan-local `$cond` condition cache (#643 + L3-RV-ENGINE-62C6F2B1).
///
/// Structure:
///
/// * `slots` — compiled `FilterNode`s keyed by the ADDRESS of the boxed
///   `cond.condition` node inside the plan's `FilterValue` tree, each
///   guarded by an allocation-free STRUCTURAL HASH of the condition PLUS a
///   full `PartialEq` content check (L3-RV-ENGINE-62C6F2B1 fix): the hash is
///   only a fast pre-filter; on a slot hit the stored `Filter` clone is
///   compared STRUCTURALLY with the queried condition, so a 64-bit hash
///   collision plus address reuse can no longer serve a stale compiled node.
///   The hit path is one integer-keyed lookup plus an allocation-free
///   equality comparison — NO `Debug` formatting, no string key, no
///   allocation.
///   The slots are written ONLY by [`prescan_cond_cache`], which walks the
///   exact tree the cache will be evaluated against — and every production
///   site stores the `CondCache` inside the same plan struct that owns that
///   tree (`SelectProjection::funcs_cond_cache`) or in a stack frame that
///   borrows the tree and does not outlive it (`group_projection`). The
///   guard pair (hash pre-filter + structural equality) makes the slots
///   safe even for the (non-production) case of a cache outliving its
///   tree: a freed address reallocated to a DIFFERENT condition is
///   rejected — even when the two conditions collide on the 64-bit
///   structural hash — by the content check instead of serving a stale
///   compiled node.
/// * `by_content` — the historical content-keyed map
///   (`format!("{:?}")` of the condition, safe across independent
///   allocations and freed/reallocated memory). It is authoritative on a
///   slot MISS: the content key is computed only there — on miss/insert —
///   never on the per-row hit path.
/// * `content_key_computations` — cost oracle for the regression tests:
///   counts every content-key computation. On a pure hit path it must stay
///   constant (see `cond_cache_tests.rs`).
///
/// Two textually identical conditions in DIFFERENT trees do not collide on
/// the slot layer (distinct nodes → distinct addresses); they may share one
/// `by_content` entry, which is a deliberate, correctness-preserving
/// sharing (equal content ⇒ equal compiled node).
#[derive(Default)]
pub struct CondCache {
    slots: TMap<usize, (u64, Filter, Arc<FilterNode>)>,
    by_content: TMap<String, Arc<FilterNode>>,
    content_key_computations: std::sync::atomic::AtomicUsize,
}

impl CondCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Compiled-node lookup for one `Cond` condition. Hit path: address-keyed
    /// slot lookup validated by the structural hash (fast pre-filter) and
    /// then FULL `Filter` content equality (`Filter` derives `PartialEq`;
    /// allocation-free) — zero formatting. Miss: content key computed once,
    /// then the content map; returns `None` if the condition was never
    /// prescanned.
    pub fn get(&self, condition: &Filter) -> Option<&Arc<FilterNode>> {
        let addr = condition as *const Filter as usize;
        if let Some((guard, cached, node)) = self.slots.get(&addr) {
            if *guard == structural_hash(condition) && *cached == *condition {
                return Some(node);
            }
            // Freed address reallocated to a different condition (hash
            // collision or not): fall through to the content map (which may
            // legitimately hold an equal-content entry from another
            // prescanned tree).
        }
        self.by_content.get(&self.content_key(condition))
    }

    /// Insert a compiled node for `condition`. Called by the prescan only —
    /// it holds the plan's tree, so the slot address is valid for the
    /// cache's lifetime by construction. Computes the content key exactly
    /// once.
    fn insert(&mut self, condition: &Filter, node: Arc<FilterNode>) {
        let addr = condition as *const Filter as usize;
        let guard = structural_hash(condition);
        self.slots
            .entry(addr)
            .or_insert_with(|| (guard, condition.clone(), Arc::clone(&node)));
        self.by_content
            .entry(self.content_key(condition))
            .or_insert(node);
    }

    /// Number of content-key computations so far (test oracle: a hit must
    /// not change this).
    pub fn content_key_computations(&self) -> usize {
        self.content_key_computations
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Number of prepared plan-local slots (one per prescanned condition).
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// True when no condition has been prescanned into this cache yet.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    fn content_key(&self, condition: &Filter) -> String {
        self.content_key_computations
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("{condition:?}")
    }
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
///
/// L3-RV-ENGINE-62C6F2B1: keyed by the condition's ADDRESS (a `usize`) —
/// the per-lookup `format!("{condition:?}")` content key (an allocation +
/// full AST traversal on EVERY call, hit included) is gone. The value
/// carries the structural hash AND a `Filter` clone; every hit re-checks
/// content equality (`Filter` derives `PartialEq`) before serving the
/// compiled node, so a dropped condition whose address is reused by a
/// different one is detected and the stale entry replaced — the same
/// guard-pair design as [`CondCache`]'s slots. (An address key means two
/// textually-identical conditions at different addresses no longer share
/// one entry — each compiles once per `FilterContext`; that is a missed
/// dedup on a per-query-bounded map, not a correctness issue, and equal
/// content still yields equal compiled nodes.)
pub(crate) type LocalCondCache = scc::HashMap<usize, (u64, Filter, Arc<FilterNode>), THasher>;

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
/// function; every later evaluation of the same condition node sharing
/// that SAME context (e.g. every row of one WHERE-clause scan) reuses the
/// compiled `FilterNode` — see `resolve.rs`'s `Cond` arm, which
/// calls this only when `ctx.cond_cache` is `None`.
///
/// L3-RV-ENGINE-62C6F2B1: the lookup is address-keyed with a hash
/// pre-filter + full `PartialEq` content re-check on hits — no
/// `format!("{:?}")` on any path. A stale entry left behind by a dropped
/// condition whose address got reused is detected via the content check,
/// evicted, and replaced by the freshly compiled node, so the cache stays
/// sound regardless of allocation reuse or cache lifetime.
pub(crate) fn compile_cond_cached(
    local_cache: &LocalCondCache,
    condition: &Filter,
    interner: &Interner,
) -> Option<Arc<FilterNode>> {
    let addr = condition as *const Filter as usize;
    // Content equality runs against the stored filter in place: a hit clones
    // only the `Arc`, never the `Filter` tree.
    match local_cache.read_sync(&addr, |_, v| (v.1 == *condition).then(|| Arc::clone(&v.2))) {
        Some(Some(node)) => return Some(node),
        // Address reused by a DIFFERENT condition: the stale entry must not
        // serve (and must not shadow the new condition) — evict it, then
        // fall through to compile-and-insert below.
        Some(None) => {
            local_cache.remove_sync(&addr);
        }
        None => {}
    }
    let node = Arc::new(try_compile_filter(condition, interner).ok()?);
    let entry = (
        structural_hash(condition),
        condition.clone(),
        Arc::clone(&node),
    );
    match local_cache.insert_sync(addr, entry) {
        Ok(()) => Some(node),
        // Lost a race with a concurrent insert for the SAME address
        // (defensive only — a `FilterContext` is built fresh per query/op
        // and never shared across concurrently-executing threads in
        // practice, see its own doc comment). Prefer the winner's entry
        // when it holds the SAME condition (one canonical compiled node);
        // when it does not, keep our own freshly compiled node — the
        // winner's stale content must never be served for ours.
        Err((_, _attempted)) => {
            let winner = local_cache.read_sync(&addr, |_, v| {
                if v.1 == *condition {
                    Some(Arc::clone(&v.2))
                } else {
                    None
                }
            });
            winner.flatten().or(Some(node))
        }
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
            if let Ok(node) = try_compile_filter(&cond.condition, interner) {
                cache.insert(&cond.condition, Arc::new(node));
            }
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

/// Look up a compiled `FilterNode` for a `Cond`'s condition via the
/// prepared plan-local slots (no formatting on a hit). Returns `None` on a
/// cache miss (caller falls back to `compile_filter`).
#[inline]
pub fn cond_cache_get<'a>(cache: &'a CondCache, condition: &Filter) -> Option<&'a Arc<FilterNode>> {
    cache.get(condition)
}

/// Allocation-free structural hash of a `Filter` tree (L3-RV-ENGINE-62C6F2B1).
///
/// Slot-hit guard: a freed slot address reallocated to a DIFFERENT
/// condition must not serve the previous tree's compiled node. The full
/// `Debug` content key would defeat the purpose of the prepared slots
/// (it is exactly the per-hit formatting the card removes), so the guard is
/// a traversal-only hash: variant discriminants + primitive payloads +
/// recursive child hashes, folded through the workspace `FxHasher`
/// (`THasher`). No allocation, no string key. Hash collisions are not a
/// correctness hazard on their own — they only matter combined with an
/// address reuse AND a colliding tree, and the content map remains the
/// authoritative source on every miss/insert.
fn structural_hash(filter: &Filter) -> u64 {
    let mut hasher = std::hash::BuildHasher::build_hasher(&THasher::default());
    hash_filter(&mut hasher, filter);
    hasher.finish()
}

/// Test-only re-export of [`structural_hash`] so regression tests can
/// inject stale, hash-matching entries into a [`LocalCondCache`] to prove
/// the content check rejects them (L3-RV-ENGINE-62C6F2B1).
#[cfg(test)]
pub(crate) fn structural_hash_for_tests(filter: &Filter) -> u64 {
    structural_hash(filter)
}

fn hash_filter<H: Hasher>(hasher: &mut H, filter: &Filter) {
    match filter {
        Filter::Eq { field, value } => {
            0u8.hash(hasher);
            hash_field(hasher, field);
            hash_fv(hasher, value);
        }
        Filter::Ne { field, value } => {
            1u8.hash(hasher);
            hash_field(hasher, field);
            hash_fv(hasher, value);
        }
        Filter::Gt { field, value } => {
            2u8.hash(hasher);
            hash_field(hasher, field);
            hash_fv(hasher, value);
        }
        Filter::Gte { field, value } => {
            3u8.hash(hasher);
            hash_field(hasher, field);
            hash_fv(hasher, value);
        }
        Filter::Lt { field, value } => {
            4u8.hash(hasher);
            hash_field(hasher, field);
            hash_fv(hasher, value);
        }
        Filter::Lte { field, value } => {
            5u8.hash(hasher);
            hash_field(hasher, field);
            hash_fv(hasher, value);
        }
        Filter::Like { field, pattern } => {
            6u8.hash(hasher);
            hash_field(hasher, field);
            pattern.hash(hasher);
        }
        Filter::ILike { field, pattern } => {
            7u8.hash(hasher);
            hash_field(hasher, field);
            pattern.hash(hasher);
        }
        Filter::Regex { field, pattern } => {
            8u8.hash(hasher);
            hash_field(hasher, field);
            pattern.hash(hasher);
        }
        Filter::IsNull { field } => {
            9u8.hash(hasher);
            hash_field(hasher, field);
        }
        Filter::IsNotNull { field } => {
            10u8.hash(hasher);
            hash_field(hasher, field);
        }
        Filter::In { field, values } => {
            11u8.hash(hasher);
            hash_field(hasher, field);
            values.len().hash(hasher);
            for v in values {
                hash_fv(hasher, v);
            }
        }
        Filter::NotIn { field, values } => {
            12u8.hash(hasher);
            hash_field(hasher, field);
            values.len().hash(hasher);
            for v in values {
                hash_fv(hasher, v);
            }
        }
        Filter::Contains { field, value } => {
            13u8.hash(hasher);
            hash_field(hasher, field);
            hash_fv(hasher, value);
        }
        Filter::ContainsAny { field, values } => {
            14u8.hash(hasher);
            hash_field(hasher, field);
            values.len().hash(hasher);
            for v in values {
                hash_fv(hasher, v);
            }
        }
        Filter::ContainsAll { field, values } => {
            15u8.hash(hasher);
            hash_field(hasher, field);
            values.len().hash(hasher);
            for v in values {
                hash_fv(hasher, v);
            }
        }
        Filter::Between { field, from, to } => {
            16u8.hash(hasher);
            hash_field(hasher, field);
            hash_fv(hasher, from);
            hash_fv(hasher, to);
        }
        Filter::Exists { field } => {
            17u8.hash(hasher);
            hash_field(hasher, field);
        }
        Filter::NotExists { field } => {
            18u8.hash(hasher);
            hash_field(hasher, field);
        }
        Filter::And { filters } => {
            19u8.hash(hasher);
            filters.len().hash(hasher);
            for f in filters {
                hash_filter(hasher, f);
            }
        }
        Filter::Or { filters } => {
            20u8.hash(hasher);
            filters.len().hash(hasher);
            for f in filters {
                hash_filter(hasher, f);
            }
        }
        Filter::Not { filter } => {
            21u8.hash(hasher);
            hash_filter(hasher, filter);
        }
        Filter::FieldEq { field, value } => {
            22u8.hash(hasher);
            hash_field(hasher, field);
            hash_fv(hasher, value);
        }
        Filter::ValueCompare { left, cmp, right } => {
            23u8.hash(hasher);
            std::mem::discriminant(cmp).hash(hasher);
            hash_fv(hasher, left);
            hash_fv(hasher, right);
        }
        Filter::Fts {
            field,
            query,
            mode,
            limit,
        } => {
            24u8.hash(hasher);
            hash_field(hasher, field);
            query.hash(hasher);
            mode.hash(hasher);
            limit.hash(hasher);
        }
        Filter::VectorSimilarity {
            field,
            query,
            k,
            ef_search,
            oversample,
        } => {
            25u8.hash(hasher);
            hash_field(hasher, field);
            query.len().hash(hasher);
            for v in query {
                v.to_bits().hash(hasher);
            }
            k.hash(hasher);
            ef_search.hash(hasher);
            oversample.map(f32::to_bits).hash(hasher);
        }
        Filter::Computed {
            expr_op,
            field,
            expr_args,
            cmp,
            value,
        } => {
            26u8.hash(hasher);
            expr_op.hash(hasher);
            hash_field(hasher, field);
            match expr_args {
                Some(args) => {
                    args.len().hash(hasher);
                    for a in args {
                        hash_fv(hasher, a);
                    }
                }
                None => 0usize.hash(hasher),
            }
            cmp.hash(hasher);
            hash_fv(hasher, value);
        }
    }
}

fn hash_field<H: Hasher>(hasher: &mut H, field: &[String]) {
    field.len().hash(hasher);
    for seg in field {
        seg.hash(hasher);
    }
}

fn hash_fv<H: Hasher>(hasher: &mut H, value: &FilterValue) {
    match value {
        FilterValue::Null => 0u8.hash(hasher),
        FilterValue::Bool(b) => {
            1u8.hash(hasher);
            b.hash(hasher);
        }
        FilterValue::Int(i) => {
            2u8.hash(hasher);
            i.hash(hasher);
        }
        FilterValue::Float(f) => {
            3u8.hash(hasher);
            f.to_bits().hash(hasher);
        }
        FilterValue::Binary(b) => {
            4u8.hash(hasher);
            b.hash(hasher);
        }
        FilterValue::String(s) => {
            5u8.hash(hasher);
            s.hash(hasher);
        }
        FilterValue::Array(items) => {
            6u8.hash(hasher);
            items.len().hash(hasher);
            for item in items {
                hash_fv(hasher, item);
            }
        }
        FilterValue::FieldRef { path } => {
            7u8.hash(hasher);
            hash_field(hasher, path);
        }
        FilterValue::QueryRef { alias, path } => {
            8u8.hash(hasher);
            alias.hash(hasher);
            path.hash(hasher);
        }
        FilterValue::FnCall { call } => {
            9u8.hash(hasher);
            call.name().hash(hasher);
            let args = call.args();
            args.len().hash(hasher);
            for a in args {
                hash_fv(hasher, a);
            }
        }
        FilterValue::Expr { expr } => {
            10u8.hash(hasher);
            std::mem::discriminant(&expr.op).hash(hasher);
            expr.args.len().hash(hasher);
            for a in &expr.args {
                hash_fv(hasher, a);
            }
        }
        FilterValue::Cond { cond } => {
            11u8.hash(hasher);
            hash_filter(hasher, &cond.condition);
            hash_fv(hasher, &cond.then);
            hash_fv(hasher, &cond.or_else);
        }
        FilterValue::Param { name } => {
            12u8.hash(hasher);
            name.hash(hasher);
        }
    }
}
