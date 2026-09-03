//! FilterContext — evaluation context for filter callbacks.

use std::sync::OnceLock;

use shamir_funclib::scalar_resolver::ScalarResolver;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::TMap;
use shamir_types::types::value::QueryValue;

use super::cond_cache::{new_local_cond_cache, CondCache, LocalCondCache};
use super::field_path_cache::FieldPathCache;
use super::query_ref_cache::QueryRefCache;
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
    pub interner: &'a Interner,
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
    pub fn new(interner: &'a Interner, resolved_refs: &'a TMap<String, QueryResult>) -> Self {
        Self {
            interner,
            resolved_refs,
            scalars: builtins_only_resolver(),
            params: empty_params(),
            cond_cache: None,
            field_path_cache: None,
            query_ref_cache: None,
            local_cond_cache: new_local_cond_cache(),
        }
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
