//! Pre-resolved SELECT projection — avoids re-interning paths per record.

use smallvec::SmallVec;

use crate::query::filter::eval::{intern_field_path, resolve_filter_query};
use crate::query::filter::{prescan_cond_cache, CondCache, FilterContext, FilterValue, FnCall};
use crate::query::read::{QueryResult, Select, SelectItem};
use shamir_funclib::scalar_resolver::ScalarResolver;
use shamir_types::codecs::interned::inner_value_to_query_value;
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::record_view::RecordRef;
use shamir_types::types::common::{new_map_wc, TMap};
use shamir_types::types::value::QueryValue;

/// Pre-resolved select projection info (avoids re-interning paths per record).
///
/// Output keys (alias or last path segment) are pre-allocated as
/// `String` at compile time — `project_value()` clones them per record
/// instead of paying `to_string()` for each field on each row.
pub struct SelectProjection {
    /// true → just convert whole record to QueryValue
    pub(super) is_all: bool,
    /// (interned_path, pre-built output key)
    pub(super) fields: Vec<(Option<Vec<u64>>, String)>,
    /// Scalar-function projections: (output key, FnCall-shaped FilterValue).
    /// Evaluated per record via `resolve_filter_value`, reusing the filter
    /// value model (`$ref` / literals / nested `$fn`).
    pub(super) funcs: Vec<(String, FilterValue)>,
    /// Empty resolved-refs map so `project_value` can build a `FilterContext`
    /// without `$query` support (projection scalar fns see only the row).
    pub(super) empty_refs: TMap<String, QueryResult>,
    /// Pre-compiled `$cond` condition cache (#643) — populated once in
    /// `new()` by pre-scanning every `FilterValue` in `funcs` for nested
    /// `Cond` nodes. `project_value` threads this into the per-record
    /// `FilterContext` so `resolve_filter_query`'s `Cond` arm reuses the
    /// compiled `FilterNode` instead of recompiling `cond.condition` on
    /// every record. `SelectProjection` is built once per query and
    /// `funcs`/this cache are never cloned — the pointer-identity cache key
    /// stays valid for the projection's whole lifetime.
    pub(super) funcs_cond_cache: CondCache,
    /// Scalar resolver (user + builtin layers) for `$fn` projections.
    /// Stored once in `new()`, cloned per-record into the `FilterContext`
    /// (cheap — `ScalarResolver` wraps an `Arc`).
    pub(super) scalars: ScalarResolver,
}

impl SelectProjection {
    /// Build a reusable projection from a Select + Interner.
    pub fn new(select: &Select, interner: &Interner, scalars: ScalarResolver) -> Self {
        let is_all =
            select.items.is_empty() || select.items.iter().any(|i| matches!(i, SelectItem::All));

        let (fields, funcs) = if is_all {
            (Vec::new(), Vec::new())
        } else {
            let mut fields = Vec::new();
            let mut funcs = Vec::new();
            for item in &select.items {
                match item {
                    SelectItem::Field { path, alias } => {
                        let interned = intern_field_path(path, interner);
                        let key = alias
                            .clone()
                            .unwrap_or_else(|| path.last().cloned().unwrap_or_default());
                        fields.push((interned, key));
                    }
                    SelectItem::Function { name, args, alias } => {
                        let key = alias.clone().unwrap_or_else(|| name.clone());
                        let fv = FilterValue::FnCall {
                            call: FnCall::complex(name.clone(), args.clone()),
                        };
                        funcs.push((key, fv));
                    }
                    _ => {}
                }
            }
            (fields, funcs)
        };

        // #643: pre-scan every projected FilterValue once (at query-compile
        // time, NOT per record) for nested `$cond` conditions, and compile
        // each one now. `project_value` reuses this cache for every record
        // instead of re-running `compile_filter` per row per `$cond`.
        let mut funcs_cond_cache: CondCache = shamir_types::types::common::new_map();
        for (_, fv) in &funcs {
            prescan_cond_cache(fv, interner, &mut funcs_cond_cache);
        }

        Self {
            is_all,
            fields,
            funcs,
            empty_refs: new_map_wc(0),
            funcs_cond_cache,
            scalars,
        }
    }

    /// Project a single record to QueryValue.
    ///
    /// Mirrors the deleted `project` exactly — same branching, same field/func
    /// handling — but builds a `QueryValue` (string-keyed) map.
    pub fn project_value(
        &self,
        record: &(impl RecordRef + ?Sized),
        interner: &Interner,
    ) -> QueryValue {
        if self.is_all {
            return record.to_query_value(interner);
        }
        if self.fields.is_empty() && self.funcs.is_empty() {
            return QueryValue::Map(shamir_types::types::common::new_map_wc(0));
        }
        let mut obj = shamir_types::types::common::new_map_wc(self.fields.len() + self.funcs.len());
        for (interned_path, key) in &self.fields {
            let val = interned_path
                .as_ref()
                .and_then(|p| {
                    let ipath: SmallVec<[InternerKey; 4]> =
                        p.iter().map(|&id| InternerKey::new(id)).collect();
                    record.materialize_at(&ipath)
                })
                .map(|v| inner_value_to_query_value(&v, interner).unwrap_or(QueryValue::Null))
                .unwrap_or(QueryValue::Null);
            obj.insert(key.clone(), val);
        }
        if !self.funcs.is_empty() {
            let ctx = FilterContext::new(interner, &self.empty_refs)
                .with_scalars(self.scalars.clone())
                .with_cond_cache(&self.funcs_cond_cache);
            for (key, fv) in &self.funcs {
                let val = resolve_filter_query(fv, record, &ctx).unwrap_or(QueryValue::Null);
                obj.insert(key.clone(), val);
            }
        }
        QueryValue::Map(obj)
    }
}
