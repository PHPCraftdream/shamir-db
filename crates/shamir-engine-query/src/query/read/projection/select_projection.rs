//! Pre-resolved SELECT projection — avoids re-interning paths per record.

use std::sync::Arc;

use crate::query::filter::eval::resolve_filter_query;
use crate::query::filter::{
    prescan_cond_cache, prescan_field_path_cache, prescan_query_ref_cache, CondCache,
    FieldPathCache, FilterContext, FilterValue, FnCall, QueryRefCache,
};
use crate::query::read::{QueryResult, Select, SelectItem};
use shamir_funclib::scalar_resolver::ScalarResolver;
use shamir_storage::error::DbResult;
use shamir_types::codecs::interned::inner_value_to_query_value;
use shamir_types::codecs::CodecError;
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
    pub(in crate::query::read) is_all: bool,
    /// (interned_path, pre-built output key). F10: path segments are stored
    /// as `InternerKey` (not raw `u64`) so `project_value` can pass the path
    /// straight to `RecordRef::materialize_at` without re-wrapping each
    /// segment per record.
    pub(in crate::query::read) fields: Vec<(Option<Vec<InternerKey>>, String)>,
    /// Scalar-function projections: (output key, FnCall-shaped FilterValue).
    /// Evaluated per record via `resolve_filter_value`, reusing the filter
    /// value model (`$ref` / literals / nested `$fn`).
    pub(in crate::query::read) funcs: Vec<(String, FilterValue)>,
    /// Execution-local resolved references and parameter bindings.
    pub(in crate::query::read) resolved_refs: TMap<String, QueryResult>,
    pub(in crate::query::read) params: TMap<String, QueryValue>,
    /// Pre-compiled `$cond` condition cache (#643) — populated once in
    /// `new()` by pre-scanning every `FilterValue` in `funcs` for nested
    /// `Cond` nodes. `project_value` threads this into the per-record
    /// `FilterContext` so `resolve_filter_query`'s `Cond` arm reuses the
    /// compiled `FilterNode` instead of recompiling `cond.condition` on
    /// every record. The cache key is derived from `cond.condition`'s
    /// content (see `cond_cache.rs`'s doc), so it stays correct regardless
    /// of `funcs`'s address — `SelectProjection` is still built once per
    /// query and this cache is scoped to its lifetime.
    pub(in crate::query::read) funcs_cond_cache: CondCache,
    /// Pre-interned `FieldRef` path cache (F1) — populated once in `new()`
    /// by pre-scanning the same `funcs` tree for nested `FieldRef` nodes
    /// (e.g. `SELECT upper(name)`'s `$ref name`). `project_value` threads
    /// this into the per-record `FilterContext` so `resolve_filter_query`'s
    /// `FieldRef` arm reuses the interned `SmallVec<InternerKey>` instead
    /// of re-allocating a `Vec<u64>` + re-issuing a per-segment `Interner::
    /// get_ind` lookup on every record. Content-keyed like
    /// `funcs_cond_cache` (see `field_path_cache.rs`'s doc).
    pub(in crate::query::read) funcs_field_path_cache: FieldPathCache,
    /// Lazily-populated `$query`/`QueryRef` resolution cache (F2) — slots
    /// RESERVED once in `new()` by pre-scanning the same `funcs` tree for
    /// nested `QueryRef` nodes (e.g. `SELECT upper(@q.id)`'s `$query` ref);
    /// the VALUE is filled lazily on the first `project_value` call that
    /// hits each node (via `OnceLock::get_or_init`), because the resolved
    /// value depends on `resolved_refs` runtime scan data that does NOT
    /// exist at `new()` time — unlike `funcs_field_path_cache`/
    /// `funcs_cond_cache` (eagerly populated). `project_value` threads this
    /// into the per-record `FilterContext` so `resolve_filter_query`'s
    /// `QueryRef` arm reuses the cached `Option<QueryValue>` instead of
    /// re-parsing the path string + re-walking the referenced `QueryResult`
    /// on every record. Content-keyed like `funcs_cond_cache` (see
    /// `query_ref_cache.rs`'s doc).
    pub(in crate::query::read) funcs_query_ref_cache: QueryRefCache,
    /// Scalar resolver (user + builtin layers) for `$fn` projections.
    /// Stored once in `new()`, cloned per-record into the `FilterContext`
    /// (cheap — `ScalarResolver` wraps an `Arc`).
    pub(in crate::query::read) scalars: ScalarResolver,
    pub(in crate::query::read) interner_overlay:
        Option<Arc<shamir_collections::TFxMap<u64, String>>>,
    pub(in crate::query::read) interner_overlay_ids:
        Option<Arc<shamir_collections::TFxMap<String, u64>>>,
}

impl SelectProjection {
    /// Build a reusable projection from a Select + Interner.
    ///
    /// #1024 (follow-up to F-26 / #819): `SelectItem::Expression` (computed
    /// SELECT expressions) is evaluated by translating its `SelectExpr` tree
    /// into the equivalent `FilterValue::Expr` shape
    /// (`SelectExpr::to_filter_value`) and feeding it into the SAME `funcs`
    /// vec `SelectItem::Function` already populates — this is the single
    /// production choke point every read plan (full scan, index2, temporal,
    /// cursor) funnels through, so every entry point gets the same
    /// evaluation, including a `Select` built directly by Rust code that
    /// bypasses the wire parser.
    pub fn new(select: &Select, interner: &Interner, scalars: ScalarResolver) -> DbResult<Self> {
        Self::new_with_context(select, interner, scalars, None)
    }

    pub fn new_with_context(
        select: &Select,
        interner: &Interner,
        scalars: ScalarResolver,
        ctx: Option<&FilterContext<'_>>,
    ) -> DbResult<Self> {
        // #1069 Defect 1: `SELECT *` combined with other projected items used
        // to silently drop the extras — `is_all` went true the moment ANY
        // item was `All`, and the branch below then discarded `fields`/
        // `funcs` wholesale, so `[All, Expression(price * qty AS total)]`
        // returned just the raw record with `total` gone, no error. `*` has
        // no defined merge semantics with named/computed columns (does it
        // mean "the whole record plus these", nested or flattened?), so —
        // mirroring F-26 (#819)'s precedent of rejecting an ambiguous/
        // unsupported SELECT shape outright instead of guessing — this is a
        // validation error, not a best-effort merge.
        let has_all = select.items.iter().any(|i| matches!(i, SelectItem::All));
        if has_all && select.items.len() > 1 {
            return Err(shamir_storage::error::DbError::Validation(format!(
                "SELECT * cannot be combined with other projected columns or expressions \
                 (found {} additional item(s) alongside '*') — '*' has no defined merge \
                 semantics with named columns. Select only '*' for the whole record, or \
                 remove '*' and list the specific columns/expressions you want.",
                select.items.len() - 1
            )));
        }
        let is_all = select.items.is_empty() || has_all;

        let interner_overlay = ctx.and_then(|ctx| ctx.interner_overlay.clone());
        let interner_overlay_ids = ctx.and_then(|ctx| ctx.interner_overlay_ids.clone());
        let (fields, funcs) = if is_all {
            (Vec::new(), Vec::new())
        } else {
            let mut fields = Vec::new();
            let mut funcs = Vec::new();
            for item in &select.items {
                match item {
                    SelectItem::Field { path, alias } => {
                        // F10: convert raw `u64` IDs to `InternerKey` ONCE at
                        // projection-build time so `project_value` avoids
                        // re-wrapping every segment per record.
                        let interned = path
                            .iter()
                            .map(|part| {
                                ctx.and_then(|ctx| ctx.resolve_interner_id(part))
                                    .or_else(|| interner.get_ind(part).map(|key| key.id()))
                            })
                            .collect::<Option<Vec<_>>>()
                            .map(|ids| ids.iter().map(|&id| InternerKey::new(id)).collect());
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
                    SelectItem::Expression { expr, alias } => {
                        // #1024: computed SELECT expressions are translated
                        // into the equivalent `FilterValue::Expr` tree
                        // (`SelectExpr::to_filter_value`) and pushed into the
                        // SAME `funcs` vec `SelectItem::Function` populates —
                        // reusing 100% of the existing, already-tested
                        // arithmetic/field-resolution evaluation logic
                        // (`resolve_filter_query`) instead of a bespoke
                        // evaluator. `SelectExpr` has no natural "name" like
                        // `Function`'s `name` field, so the no-alias default
                        // key is the literal `"expr"` — mirrors the wire
                        // item's own `#[serde(rename = "expr")]` tag
                        // (`SelectItem::Expression`, `read/select.rs`).
                        let key = alias.clone().unwrap_or_else(|| "expr".to_string());
                        funcs.push((key, expr.to_filter_value()));
                    }
                    _ => {}
                }
            }

            // #1069 Defect 2: every unaliased `SelectItem::Expression`
            // defaulted to the SAME literal key `"expr"` (and any two items
            // — aliased or not, field/function/expression — could collide on
            // an explicit or defaulted key), so `obj.insert` in
            // `project_value` silently last-write-wins over the earlier
            // column. Validate output-key uniqueness across `fields` + `funcs`
            // combined, up front, once per query compile — not per record.
            let mut seen_keys: shamir_types::types::common::TSet<&str> =
                shamir_types::types::common::new_set_wc(fields.len() + funcs.len());
            for (_, key) in &fields {
                if !seen_keys.insert(key.as_str()) {
                    return Err(shamir_storage::error::DbError::Validation(format!(
                        "SELECT projection has a duplicate output column name '{key}' — \
                         add an explicit alias (AS <name>) to disambiguate."
                    )));
                }
            }
            for (key, _) in &funcs {
                if !seen_keys.insert(key.as_str()) {
                    return Err(shamir_storage::error::DbError::Validation(format!(
                        "SELECT projection has a duplicate output column name '{key}' — \
                         add an explicit alias (AS <name>) to disambiguate."
                    )));
                }
            }

            (fields, funcs)
        };

        // #643 / F1 / F2: pre-scan every projected FilterValue once (at
        // query-compile time, NOT per record) for nested `$cond` conditions
        // (compiled into `funcs_cond_cache`), nested `FieldRef` paths
        // (interned into `funcs_field_path_cache`), and nested `QueryRef`
        // nodes (`OnceLock` slots RESERVED into `funcs_query_ref_cache` —
        // values filled lazily per scan, since they depend on `resolved_refs`
        // runtime data not available here). `project_value` reuses all three
        // caches for every record instead of re-running `compile_filter` /
        // `intern_field_path` / path-parsing per row per node.
        let mut funcs_cond_cache: CondCache = shamir_types::types::common::new_map();
        let funcs_field_path_cache: FieldPathCache = if interner_overlay.is_none() {
            let mut cache = shamir_types::types::common::new_map();
            for (_, fv) in &funcs {
                prescan_field_path_cache(fv, interner, &mut cache);
            }
            cache
        } else {
            shamir_types::types::common::new_map()
        };
        let mut funcs_query_ref_cache: QueryRefCache = shamir_types::types::common::new_map();
        for (_, fv) in &funcs {
            prescan_cond_cache(fv, interner, &mut funcs_cond_cache);
            prescan_query_ref_cache(fv, &mut funcs_query_ref_cache);
        }

        let mut projection = Self {
            is_all,
            fields,
            funcs,
            resolved_refs: new_map_wc(0),
            params: new_map_wc(0),
            funcs_cond_cache,
            funcs_field_path_cache,
            funcs_query_ref_cache,
            scalars,
            interner_overlay,
            interner_overlay_ids,
        };
        if let Some(ctx) = ctx {
            projection.resolved_refs = ctx.resolved_refs.clone();
            projection.params = ctx.params.clone();
            projection.scalars = ctx.scalars.clone();
            for cell in projection.funcs_query_ref_cache.values_mut() {
                *cell = std::sync::OnceLock::new();
            }
        }
        Ok(projection)
    }

    /// Bind this fresh projection to one execution. Owned snapshots cannot
    /// borrow a previous ForEach iteration; reset lazy cells on rebinding.
    pub(crate) fn with_context(mut self, ctx: &FilterContext<'_>) -> Self {
        self.resolved_refs = ctx.resolved_refs.clone();
        self.params = ctx.params.clone();
        self.scalars = ctx.scalars.clone();
        self.interner_overlay = ctx.interner_overlay.clone();
        self.interner_overlay_ids = ctx.interner_overlay_ids.clone();
        for cell in self.funcs_query_ref_cache.values_mut() {
            *cell = std::sync::OnceLock::new();
        }
        self
    }

    /// Project a single record to QueryValue.
    ///
    /// Mirrors the deleted `project` exactly — same branching, same field/func
    /// handling — but builds a `QueryValue` (string-keyed) map.
    fn resolve_interner_name(
        interner: &Interner,
        overlay: &shamir_collections::TFxMap<u64, String>,
        id: u64,
    ) -> Option<String> {
        if id >= shamir_tx::OVERLAY_ID_BASE {
            return overlay.get(&id).cloned();
        }
        interner
            .get_str(&InternerKey::new(id))
            .map(|name| name.to_string())
    }

    pub fn project_value(
        &self,
        record: &(impl RecordRef + ?Sized),
        interner: &Interner,
    ) -> Result<QueryValue, CodecError> {
        if self.is_all {
            if let Some(overlay) = &self.interner_overlay {
                let mut inner = shamir_types::types::common::new_map_wc(0);
                record.for_each_field(&mut |key, value| {
                    inner.insert(key, value);
                });
                return shamir_types::codecs::interned::inner_value_to_query_value_with(
                    &shamir_types::types::value::InnerValue::Map(inner),
                    &|id| {
                        if id >= shamir_tx::OVERLAY_ID_BASE {
                            overlay.get(&id).cloned()
                        } else {
                            interner
                                .get_str(&InternerKey::new(id))
                                .map(|name| name.to_string())
                        }
                    },
                );
            }
            return record.to_query_value(interner);
        }
        if self.fields.is_empty() && self.funcs.is_empty() {
            return Ok(QueryValue::Map(shamir_types::types::common::new_map_wc(0)));
        }
        let mut obj = shamir_types::types::common::new_map_wc(self.fields.len() + self.funcs.len());
        for (interned_path, key) in &self.fields {
            // F10: `interned_path` is already `&Vec<InternerKey>` — pass
            // directly to `materialize_at` (no per-row `SmallVec` rebuild).
            let val = match interned_path
                .as_ref()
                .and_then(|path| record.materialize_at(path))
            {
                Some(value) => {
                    if let Some(overlay) = &self.interner_overlay {
                        shamir_types::codecs::interned::inner_value_to_query_value_with(
                            &value,
                            &|id| Self::resolve_interner_name(interner, overlay, id),
                        )?
                    } else {
                        inner_value_to_query_value(&value, interner)?
                    }
                }
                None => QueryValue::Null,
            };
            obj.insert(key.clone(), val);
        }
        if !self.funcs.is_empty() {
            let ctx = FilterContext::new(interner, &self.resolved_refs)
                .with_params(&self.params)
                .with_scalars(self.scalars.clone())
                .with_overlay_snapshot(self.interner_overlay.clone())
                .with_overlay_ids_snapshot(self.interner_overlay_ids.clone())
                .with_cond_cache(&self.funcs_cond_cache)
                .with_field_path_cache(&self.funcs_field_path_cache)
                .with_query_ref_cache(&self.funcs_query_ref_cache);
            for (key, fv) in &self.funcs {
                // #1069 Defect 3 — DOCUMENTED, DELIBERATE alpha-scope
                // decision (not an accidental fallthrough): `resolve_filter_query`
                // returns `Option<QueryValue>` by design, and folds EVERY
                // failure class into `None` uniformly across the WHOLE filter
                // evaluator — unresolvable field/query refs, unknown/erroring
                // scalar functions (`ctx.scalars.call(..).ok()` in
                // `resolve.rs`'s `FnCall` arm), division-by-zero and integer
                // overflow (`resolve.rs`'s `FilterExprOp::Div`/`Mod`/checked-
                // arithmetic arms), and arity/type mismatches. This is the
                // SAME "silent-miss" semantics WHERE/`when`/`for_each` already
                // rely on (see `resolve.rs`'s `Cond` arm doc, "Silent-miss
                // inheritance"), not something specific to SELECT projection.
                // A "strict mode" that surfaces WASM-trap/scalar errors
                // distinctly from ordinary null-propagation (division by
                // zero, missing field) would require giving
                // `resolve_filter_query` a typed `Result` return —a
                // cross-cutting change to the shared evaluator affecting
                // WHERE/`when`/`for_each` as much as SELECT, not a
                // projection-local fix, and out of scope for this task's
                // single-context slice. Until that redesign lands, SELECT
                // projection maps `None` to `QueryValue::Null` as its
                // documented, intentional behavior — division by zero, a
                // missing field, and an erroring scalar function are all
                // indistinguishable `Null` in the output today, by design,
                // not by accident.
                let val = resolve_filter_query(fv, record, &ctx).unwrap_or(QueryValue::Null);
                obj.insert(key.clone(), val);
            }
        }
        Ok(QueryValue::Map(obj))
    }
}
