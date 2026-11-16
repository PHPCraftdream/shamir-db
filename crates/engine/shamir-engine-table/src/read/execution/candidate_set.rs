//! Candidate identity and ordering, independent of query postprocessing.

use shamir_engine_query::query::read::{exec, ReadQuery};
use shamir_index::backend::IndexResult;
use shamir_types::types::record_id::RecordId;

#[derive(Debug)]
#[doc(hidden)]
pub struct CandidateSet {
    pub rids: Vec<RecordId>,
    pub ranked: bool,
}

impl From<IndexResult> for CandidateSet {
    fn from(result: IndexResult) -> Self {
        match result {
            IndexResult::Set(set) => Self {
                rids: set.into_iter().collect(),
                ranked: false,
            },
            IndexResult::Ranked(rows) => Self {
                rids: rows.into_iter().map(|(id, _)| id).collect(),
                ranked: true,
            },
        }
    }
}

pub(crate) fn index2_shortcut_eligible(query: &ReadQuery) -> bool {
    !exec::has_aggregates(&query.select)
        && query.group_by.is_none()
        && !query.select.distinct
        && query.order_by.is_none()
}
