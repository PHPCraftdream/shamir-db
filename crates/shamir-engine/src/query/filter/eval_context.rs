//! FilterContext — evaluation context for filter callbacks.

use crate::query::read::QueryResult;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::TMap;

/// Context passed to filter callbacks during evaluation.
///
/// Contains the interner for resolving field paths and
/// a map of resolved query results for QueryRef support.
pub struct FilterContext<'a> {
    pub interner: &'a Interner,
    pub resolved_refs: &'a TMap<String, QueryResult>,
}

impl<'a> FilterContext<'a> {
    pub fn new(interner: &'a Interner, resolved_refs: &'a TMap<String, QueryResult>) -> Self {
        Self {
            interner,
            resolved_refs,
        }
    }
}
