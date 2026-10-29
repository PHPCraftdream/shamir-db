//! FilterContext — evaluation context for filter callbacks.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use shamir_funclib::scalar_resolver::ScalarResolver;
use shamir_query_types::batch::{BatchLimits, VisibilityWait};
use shamir_tx::OVERLAY_ID_BASE;
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::types::common::TMap;
use shamir_types::types::value::QueryValue;

use crate::query::filter::caches::cond_cache::{new_local_cond_cache, CondCache, LocalCondCache};
use crate::query::filter::caches::field_path_cache::FieldPathCache;
use crate::query::filter::caches::query_ref_cache::QueryRefCache;
use crate::query::filter::read_budget::{ReadByteMeter, ReadVisibilityScope};
use crate::query::read::QueryResult;

/// Context passed to filter callbacks during evaluation.
///
/// Contains the interner for resolving field paths,
/// a map of resolved query results for QueryRef support,
/// and the scalar function resolver used to evaluate `FilterValue::FnCall`
/// nodes.
///
/// **No `actor` field, deliberately (#1199).** A prior `actor: Actor`
/// field defaulted to `Actor::System` in [`new`](Self::new) and was set via
/// a `with_actor` builder — but nothing in filter evaluation ever READ it
/// (verified: no `ctx.actor` reference anywhere in `query/filter/`,
/// `query/read/`, or `shamir-funclib`), so it was a pure write-only
/// landmine: any future feature that reads it (row-level security keyed
/// on actor, a `current_user()` scalar) would silently see `Actor::System`
/// on every call site that forgot to set it explicitly. Removing the field
/// entirely (rather than keeping the default) closes that class of bug
/// structurally — there is nothing left to forget. If/when a real reader
/// is added, thread `actor: Actor` through as a REQUIRED parameter of
/// [`new`](Self::new), not a builder-default.
///
/// `params` is the injected sub-batch parameter scope — populated when
/// this context belongs to a nested `BatchOp::Batch` execution. At the
/// top level it is an empty shared map (zero allocation on the common
/// path). Used to resolve `FilterValue::Param { name }` references.
pub struct FilterContext<'a> {
    pub(crate) read_visibility_budget: Option<Duration>,
    pub(crate) read_visibility_scope: Option<&'a dyn ReadVisibilityScope>,
    pub(crate) row_byte_meter: Option<&'a dyn ReadByteMeter>,
    max_accumulator_bytes: Option<usize>,
    /// Interner for resolving field paths. `pub(crate)`, not `pub` (S5c-X
    /// Round 7): the `Interner` is interior-mutable (`touch_ind`), so a
    /// `pub` field would let read-view holders mutate through it. Engine
    /// readers use the field directly; no cross-crate reader exists.
    pub(crate) interner: &'a Interner,
    /// O(1) reverse resolver snapshot for this transaction's overlay names.
    pub(crate) interner_overlay: Option<Arc<shamir_collections::TFxMap<u64, String>>>,
    pub(crate) interner_overlay_ids: Option<Arc<shamir_collections::TFxMap<String, u64>>>,
    pub resolved_refs: &'a TMap<String, QueryResult>,
    /// Scalar function resolver for `FnCall` dispatch. Defaults to
    /// built-ins only ([`ScalarResolver::builtins_only`]); a per-DB
    /// resolver with user scalars is injected via [`with_scalars`](Self::with_scalars).
    pub scalars: ScalarResolver,
    /// Injected sub-batch parameters (`$param` bindings). Empty at the
    /// top level; populated by the recursive sub-batch executor (P3).
    pub params: &'a TMap<String, QueryValue>,
    /// Optional pre-compiled `$cond` condition cache (#643). Defaults to
    /// `None` — callers that never opt in (WHERE, `when`, `for_each`'s
    /// `over`, write-value resolution) fall back to `local_cond_cache`
    /// below (group 13 Defect 1), not to an unconditional recompile. Only
    /// callers that build a [`CondCache`] once (e.g. `SelectProjection::new`)
    /// and inject it via [`with_cond_cache`](Self::with_cond_cache) skip
    /// `local_cond_cache`'s per-lookup content-hash cost too, trading it for
    /// an eager, address-independent prescan.
    pub cond_cache: Option<&'a CondCache>,
    /// Optional pre-interned `FieldRef` path cache (F1). Defaults to `None`
    /// — every EXISTING caller (WHERE, `when`, `for_each`'s `over`,
    /// write-value resolution) is completely unaffected: `resolve_filter_query`
    /// re-interns each `FieldRef`'s path via `intern_field_path` on every
    /// evaluation exactly as before. Only callers that build a
    /// [`FieldPathCache`] once (e.g. `SelectProjection::new`) and inject it
    /// via [`with_field_path_cache`](Self::with_field_path_cache) skip the
    /// per-row `Vec` alloc + per-segment `DashMap` lookup.
    pub field_path_cache: Option<&'a FieldPathCache>,
    /// Optional lazily-populated `$query`/`QueryRef` resolution cache (F2).
    /// Defaults to `None` — every EXISTING caller (WHERE, `when`, `for_each`'s
    /// `over`, write-value resolution) is completely unaffected:
    /// `resolve_filter_query` re-parses the `path` string and re-walks the
    /// referenced `QueryResult` on every `QueryRef` evaluation exactly as
    /// before. Only callers that build a [`QueryRefCache`] once (e.g.
    /// `SelectProjection::new`) and inject it via
    /// [`with_query_ref_cache`](Self::with_query_ref_cache) skip the per-row
    /// path parsing + Map/List navigation walk (the slot is filled lazily
    /// on the first row, since the resolved value depends on
    /// `resolved_refs`, which is not available at prescan time — unlike
    /// F1's eagerly-populated `FieldPathCache`).
    pub query_ref_cache: Option<&'a QueryRefCache>,
    /// Lazily-populated fallback `$cond` cache (group 13 Defect 1). Unlike
    /// `cond_cache` above (an eagerly pre-scanned, externally-owned cache
    /// only `SelectProjection::new` opts into today), this cache is owned
    /// BY the context itself, starts empty, and is populated on demand by
    /// `resolve_filter_query`'s `Cond` arm the first time it evaluates a
    /// given `$cond` against THIS context — then reused for every
    /// subsequent evaluation sharing this context (e.g. every row of one
    /// WHERE/HAVING scan, since the caller builds one `FilterContext` per
    /// query/op and reuses it across the whole row loop). This closes the
    /// gap `cond_cache` left open: WHERE, `when`, `for_each`'s `over`, and
    /// write-value resolution never call `with_cond_cache`, so before this
    /// field existed they recompiled `cond.condition` (e.g. re-running
    /// `Regex::new`) on every single evaluation.
    ///
    /// `LocalCondCache` (`scc::HashMap`), not `RefCell<CondCache>`: several
    /// batch-executor futures capture `&FilterContext` across an `.await`
    /// and are boxed as `Pin<Box<dyn Future<..> + Send>>`, which requires
    /// `FilterContext: Sync` (`&T: Send` needs `T: Sync`) —
    /// `std::cell::RefCell` is unconditionally `!Sync` and broke that bound
    /// the first time this was tried; `scc::HashMap` gives the same
    /// interior mutability while staying `Sync`. See `cond_cache.rs`'s
    /// `LocalCondCache` doc for the full story, and this module's tests for
    /// the reuse-across-rows proof.
    pub(crate) local_cond_cache: LocalCondCache,
}

/// A permanently empty params map, shared across all top-level contexts
/// so `FilterContext::new` never allocates.
fn empty_params() -> &'static TMap<String, QueryValue> {
    static EMPTY: OnceLock<TMap<String, QueryValue>> = OnceLock::new();
    EMPTY.get_or_init(shamir_types::types::common::new_map)
}

/// A permanently empty ScalarResolver (builtins only), shared across all
/// top-level contexts so `FilterContext::new` never allocates an Arc.
fn builtins_only_resolver() -> ScalarResolver {
    ScalarResolver::builtins_only()
}

impl<'a> FilterContext<'a> {
    pub(crate) fn resolve_query_value(
        &self,
        value: &shamir_types::types::value::InnerValue,
    ) -> Result<QueryValue, shamir_types::codecs::CodecError> {
        match &self.interner_overlay {
            Some(overlay) => {
                shamir_types::codecs::interned::inner_value_to_query_value_with(value, &|id| {
                    if id >= OVERLAY_ID_BASE {
                        overlay.get(&id).cloned()
                    } else {
                        self.interner
                            .get_str(&InternerKey::new(id))
                            .map(|name| name.to_string())
                    }
                })
            }
            None => {
                shamir_types::codecs::interned::inner_value_to_query_value(value, self.interner)
            }
        }
    }

    pub(crate) fn with_overlay_snapshot(
        mut self,
        reverse: Option<Arc<shamir_collections::TFxMap<u64, String>>>,
    ) -> Self {
        self.interner_overlay = reverse;
        self
    }

    pub(crate) fn with_overlay_ids_snapshot(
        mut self,
        ids: Option<Arc<shamir_collections::TFxMap<String, u64>>>,
    ) -> Self {
        self.interner_overlay_ids = ids;
        self
    }

    pub(crate) fn with_read_overlay(
        mut self,
        overlay: &scc::HashMap<String, u64, shamir_collections::THasher>,
    ) -> Self {
        if overlay.is_empty() {
            return self;
        }
        let mut reverse = shamir_collections::new_fx_map_wc(0);
        let mut ids = shamir_collections::new_fx_map();
        overlay.iter_sync(|name, id| {
            reverse.insert(*id, name.clone());
            ids.insert(name.clone(), *id);
            true
        });
        self.interner_overlay_ids = Some(Arc::new(ids));
        self.interner_overlay = Some(Arc::new(reverse));
        self
    }

    pub(crate) fn clone_with_read_overlay(
        &self,
        overlay: &scc::HashMap<String, u64, shamir_collections::THasher>,
    ) -> Self {
        let mut cloned = Self::new(self.interner, self.resolved_refs)
            .with_scalars(self.scalars.clone())
            .with_params(self.params);
        cloned.max_accumulator_bytes = self.max_accumulator_bytes;
        cloned.read_visibility_budget = self.read_visibility_budget;
        cloned.read_visibility_scope = self.read_visibility_scope;
        cloned.row_byte_meter = self.row_byte_meter;
        cloned.cond_cache = self.cond_cache;
        cloned.field_path_cache = self.field_path_cache;
        cloned.query_ref_cache = self.query_ref_cache;
        cloned.with_read_overlay(overlay)
    }

    pub(crate) fn resolve_interner_id(&self, name: &str) -> Option<u64> {
        self.interner
            .get_ind(name)
            .map(|key| key.id())
            .or_else(|| self.interner_overlay_ids.as_ref()?.get(name).copied())
    }

    pub fn new(interner: &'a Interner, resolved_refs: &'a TMap<String, QueryResult>) -> Self {
        Self {
            read_visibility_budget: Some(Duration::from_millis(5000)),
            read_visibility_scope: None,
            row_byte_meter: None,
            max_accumulator_bytes: None,
            interner,
            interner_overlay: None,
            interner_overlay_ids: None,
            resolved_refs,
            scalars: builtins_only_resolver(),
            params: empty_params(),
            cond_cache: None,
            field_path_cache: None,
            query_ref_cache: None,
            local_cond_cache: new_local_cond_cache(),
        }
    }

    pub(crate) fn with_row_byte_meter(mut self, meter: Option<&'a dyn ReadByteMeter>) -> Self {
        self.row_byte_meter = meter;
        self
    }

    /// Explicit allocated-version read budget for embedded callers.
    pub fn with_read_visibility_budget(mut self, budget: Duration) -> Self {
        self.read_visibility_budget = Some(budget);
        self
    }

    /// Select strict or bounded allocated-version waits for embedded reads.
    pub fn with_read_visibility_wait(mut self, wait: VisibilityWait) -> Self {
        self.read_visibility_budget = match wait {
            VisibilityWait::Strict => None,
            VisibilityWait::Bounded { timeout_ms } => Some(Duration::from_millis(timeout_ms)),
        };
        self
    }

    pub(crate) fn with_read_visibility_scope(mut self, scope: &'a dyn ReadVisibilityScope) -> Self {
        self.read_visibility_scope = Some(scope);
        self
    }

    pub(crate) fn remaining_read_visibility_budget(&self) -> Option<Duration> {
        match self.read_visibility_scope {
            Some(scope) => Some(scope.read_visibility_budget()),
            None => self.read_visibility_budget,
        }
    }

    pub fn with_max_accumulator_bytes(mut self, max: usize) -> Self {
        self.max_accumulator_bytes = Some(max.min(BatchLimits::CEILING.max_result_size));
        self
    }

    pub fn max_accumulator_bytes(&self) -> usize {
        self.max_accumulator_bytes
            .unwrap_or(BatchLimits::CEILING.max_result_size)
    }

    /// Builder: inject a per-DB scalar resolver with user-registered scalars.
    pub fn with_scalars(mut self, resolver: ScalarResolver) -> Self {
        self.scalars = resolver;
        self
    }

    /// Builder: inject sub-batch params for `$param` resolution.
    pub fn with_params(mut self, params: &'a TMap<String, QueryValue>) -> Self {
        self.params = params;
        self
    }

    /// Builder: inject a pre-compiled `$cond` condition cache (#643).
    /// Only meaningful for callers that pre-scan a static `FilterValue` tree
    /// once (e.g. `SelectProjection::new`) and reuse it across many records —
    /// one-off evaluation contexts (WHERE, `when`, `for_each`, write-value
    /// resolution) should leave this unset.
    pub fn with_cond_cache(mut self, cache: &'a CondCache) -> Self {
        self.cond_cache = Some(cache);
        self
    }

    /// Builder: inject a pre-interned `FieldRef` path cache (F1).
    /// Only meaningful for callers that pre-scan a static `FilterValue` tree
    /// once (e.g. `SelectProjection::new`) and reuse it across many records —
    /// one-off evaluation contexts (WHERE, `when`, `for_each`, write-value
    /// resolution) should leave this unset.
    pub fn with_field_path_cache(mut self, cache: &'a FieldPathCache) -> Self {
        self.field_path_cache = Some(cache);
        self
    }

    /// Builder: inject a lazily-populated `$query`/`QueryRef` resolution
    /// cache (F2). Only meaningful for callers that pre-scan a static
    /// `FilterValue` tree once (e.g. `SelectProjection::new`) and reuse it
    /// across many records — one-off evaluation contexts (WHERE, `when`,
    /// `for_each`, write-value resolution) should leave this unset. The
    /// cache slots are filled lazily on the first row of each scan (not
    /// eagerly at prescan time), because the resolved value depends on
    /// `resolved_refs` runtime data.
    pub fn with_query_ref_cache(mut self, cache: &'a QueryRefCache) -> Self {
        self.query_ref_cache = Some(cache);
        self
    }
}
