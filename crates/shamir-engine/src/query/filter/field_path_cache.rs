//! `FieldPathCache` — pre-interned `FieldRef` path cache (F1).
//!
//! `resolve_filter_query`'s `FilterValue::FieldRef` arm used to call
//! `intern_field_path(path, ctx.interner)` on EVERY evaluation — i.e. once
//! per record in a per-row hot loop — even though `path` (a `Vec<String>`)
//! is static per query: identical on every call for the SAME `FieldRef`
//! node. This is exactly the shape task #643 already fixed for `$cond`'s
//! condition compilation (one layer up) via `CondCache`, and this module
//! mirrors that mechanism.
//!
//! This module provides an opt-in cache keyed by the *content* of the
//! `FieldRef` node itself (see [`field_ref_key`]). Callers that build a
//! `FieldPathCache` once (e.g. `SelectProjection::new`) and thread it
//! through `FilterContext::field_path_cache` get pre-interned
//! `SmallVec<InternerKey>` paths for every `FieldRef` in the tree; callers
//! that never populate `field_path_cache` (the overwhelming majority —
//! WHERE, `when`, `for_each`'s `over`, write-value resolution) are
//! completely unaffected — `resolve_filter_query` falls back to
//! `intern_field_path` exactly as before.

use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::types::common::TMap;
use smallvec::SmallVec;

use super::resolve::intern_field_path;
use crate::query::filter::{Filter, FilterValue};

/// Content-keyed cache mapping a `FieldRef` node (by its full `Debug`
/// representation, i.e. its `path`) to its pre-interned
/// `SmallVec<[InternerKey; 4]>` path.
///
/// # Why content, not raw address (Defect 2 / group 13 fix)
///
/// This used to be keyed on the raw pointer address of the enclosing
/// `FilterValue` (`fv as *const FilterValue as usize`), sound only as long
/// as the cache never outlives the exact allocation it was built from — an
/// invariant the type system never enforced. See `cond_cache.rs`'s doc
/// comment for the full address-reuse hazard this class of bug shares. The
/// key is now [`format!("{:?}", fv)`]: two `FieldRef` nodes collide in this
/// map if and only if they reference the same `path`, regardless of memory
/// address. As a side effect, two independently-allocated `FieldRef { path
/// }` nodes with the same path now correctly share one cache entry.
pub type FieldPathCache = TMap<String, SmallVec<[InternerKey; 4]>>;

/// Compute the content-derived cache key for a `FieldRef` node.
#[inline]
pub(super) fn field_ref_key(fv: &FilterValue) -> String {
    format!("{fv:?}")
}

/// Recursively walk a `FilterValue` tree, interning and caching every
/// nested `FieldRef`'s `path` (at ANY nesting depth — inside `FnCall` args,
/// `Expr` operands, `Cond` `then`/`or_else` branches, `Array` elements, and
/// inside the condition `Filter` trees themselves, which may embed further
/// `FilterValue`s — e.g. an `Eq`'s `value` — that in turn contain nested
/// `FieldRef`s).
///
/// Mirrors `resolve_filter_query`'s own dispatch structure (and
/// `prescan_cond_cache`'s recursion shape) so every `FilterValue` shape
/// capable of containing a `FieldRef` is visited.
///
/// If `intern_field_path` returns `None` for a given `FieldRef` (an unknown
/// field name — can happen if the interner hasn't seen this string yet at
/// prescan time, e.g. a brand-new field only ever referenced dynamically),
/// the insert is skipped silently: `resolve_filter_query`'s `FieldRef` arm
/// already handles a cache miss correctly by falling back to
/// `intern_field_path` per row, so this is not an error, just a soft miss
/// exactly like `CondCache`'s own documented soft-miss behaviour.
pub fn prescan_field_path_cache(fv: &FilterValue, interner: &Interner, cache: &mut FieldPathCache) {
    match fv {
        FilterValue::Null
        | FilterValue::Bool(_)
        | FilterValue::Int(_)
        | FilterValue::Float(_)
        | FilterValue::String(_)
        | FilterValue::Binary(_)
        | FilterValue::QueryRef { .. }
        | FilterValue::Param { .. } => {}
        FilterValue::FieldRef { path } => {
            // Intern ONCE at prescan time. The key is the enclosing node's
            // CONTENT (its `path`, via `field_ref_key`'s `Debug` format),
            // not its address — see this module's doc comment.
            // `entry().or_insert()` (not `or_insert_with`) is used because
            // `intern_field_path` may return `None`, in which case the
            // insert must be skipped entirely — `or_insert_with` can't
            // express that skip.
            if let Some(keys) = intern_field_path(path, interner) {
                let ipath: SmallVec<[InternerKey; 4]> =
                    keys.iter().map(|&id| InternerKey::new(id)).collect();
                cache.entry(field_ref_key(fv)).or_insert(ipath);
            }
        }
        FilterValue::Array(items) => {
            for item in items {
                prescan_field_path_cache(item, interner, cache);
            }
        }
        FilterValue::FnCall { call } => {
            for arg in call.args() {
                prescan_field_path_cache(arg, interner, cache);
            }
        }
        FilterValue::Expr { expr } => {
            for arg in &expr.args {
                prescan_field_path_cache(arg, interner, cache);
            }
        }
        FilterValue::Cond { cond } => {
            // The condition's `Filter` tree may itself embed `FilterValue`s
            // (e.g. `Filter::Eq { value, .. }`) that contain further nested
            // `FieldRef`s — walk it too so those get cached.
            prescan_filter(&cond.condition, interner, cache);
            prescan_field_path_cache(&cond.then, interner, cache);
            prescan_field_path_cache(&cond.or_else, interner, cache);
        }
    }
}

/// Walk a `Filter` AST's embedded `FilterValue`s (comparison operands,
/// membership lists, etc.) looking for further nested `FieldRef`s, and
/// recurse into logical combinators (`And`/`Or`/`Not`). Mirrors
/// `cond_cache.rs`'s `prescan_filter` dispatch shape.
fn prescan_filter(filter: &Filter, interner: &Interner, cache: &mut FieldPathCache) {
    match filter {
        Filter::Eq { value, .. }
        | Filter::Ne { value, .. }
        | Filter::Gt { value, .. }
        | Filter::Gte { value, .. }
        | Filter::Lt { value, .. }
        | Filter::Lte { value, .. }
        | Filter::FieldEq { value, .. }
        | Filter::Contains { value, .. } => {
            prescan_field_path_cache(value, interner, cache);
        }
        Filter::In { values, .. }
        | Filter::NotIn { values, .. }
        | Filter::ContainsAny { values, .. }
        | Filter::ContainsAll { values, .. } => {
            for v in values {
                prescan_field_path_cache(v, interner, cache);
            }
        }
        Filter::Between { from, to, .. } => {
            prescan_field_path_cache(from, interner, cache);
            prescan_field_path_cache(to, interner, cache);
        }
        Filter::ValueCompare { left, right, .. } => {
            prescan_field_path_cache(left, interner, cache);
            prescan_field_path_cache(right, interner, cache);
        }
        Filter::Computed {
            expr_args, value, ..
        } => {
            if let Some(args) = expr_args {
                for a in args {
                    prescan_field_path_cache(a, interner, cache);
                }
            }
            prescan_field_path_cache(value, interner, cache);
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
