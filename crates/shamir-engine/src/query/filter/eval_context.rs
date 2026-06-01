//! FilterContext — evaluation context for filter callbacks.

use shamir_types::access::Actor;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::TMap;

use crate::query::read::QueryResult;

/// Context passed to filter callbacks during evaluation.
///
/// Contains the interner for resolving field paths,
/// a map of resolved query results for QueryRef support,
/// and the [`Actor`] that initiated the operation.
pub struct FilterContext<'a> {
    pub interner: &'a Interner,
    pub resolved_refs: &'a TMap<String, QueryResult>,
    pub actor: Actor,
}

impl<'a> FilterContext<'a> {
    pub fn new(interner: &'a Interner, resolved_refs: &'a TMap<String, QueryResult>) -> Self {
        Self {
            interner,
            resolved_refs,
            actor: Actor::System,
        }
    }

    /// Builder: set the actor that initiated this operation.
    pub fn with_actor(mut self, actor: Actor) -> Self {
        self.actor = actor;
        self
    }
}
