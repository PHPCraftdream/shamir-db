//! FilterContext — evaluation context for filter callbacks.

use shamir_funclib::scalar_resolver::ScalarResolver;
use shamir_types::access::Actor;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::TMap;
use shamir_types::types::value::QueryValue;

use super::cond_cache::CondCache;
use crate::query::read::QueryResult;

/// Context passed to filter callbacks during evaluation.
///
/// Contains the interner for resolving field paths,
/// a map of resolved query results for QueryRef support,
/// the [`Actor`] that initiated the operation, and the scalar function
/// resolver used to evaluate `FilterValue::FnCall` nodes.
///
/// `params` is the injected sub-batch parameter scope — populated when
/// this context belongs to a nested `BatchOp::Batch` execution. At the
/// top level it is an empty shared map (zero allocation on the common
/// path). Used to resolve `FilterValue::Param { name }` references.
pub struct FilterContext<'a> {
    pub interner: &'a Interner,
    pub resolved_refs: &'a TMap<String, QueryResult>,
    pub actor: Actor,
    /// Scalar function resolver for `FnCall` dispatch. Defaults to
    /// built-ins only ([`ScalarResolver::builtins_only`]); a per-DB
    /// resolver with user scalars is injected via [`with_scalars`](Self::with_scalars).
    pub scalars: ScalarResolver,
    /// Injected sub-batch parameters (`$param` bindings). Empty at the
    /// top level; populated by the recursive sub-batch executor (P3).
    pub params: &'a TMap<String, QueryValue>,
    /// Optional pre-compiled `$cond` condition cache (#643). Defaults to
    /// `None` — every EXISTING caller (WHERE, `when`, `for_each`'s `over`,
    /// write-value resolution) is completely unaffected: `resolve_filter_query`
    /// falls back to `compile_filter` on every `Cond` evaluation exactly as
    /// before. Only callers that build a [`CondCache`] once (e.g.
    /// `SelectProjection::new`) and inject it via
    /// [`with_cond_cache`](Self::with_cond_cache) skip the per-row recompile.
    pub cond_cache: Option<&'a CondCache>,
}

/// A permanently empty params map, shared across all top-level contexts
/// so `FilterContext::new` never allocates.
fn empty_params() -> &'static TMap<String, QueryValue> {
    use std::sync::OnceLock;
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
            actor: Actor::System,
            scalars: builtins_only_resolver(),
            params: empty_params(),
            cond_cache: None,
        }
    }

    /// Builder: set the actor that initiated this operation.
    pub fn with_actor(mut self, actor: Actor) -> Self {
        self.actor = actor;
        self
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
}
