//! Candidate identity and ordering, independent of query postprocessing.

use crate::index2::backend::IndexResult;
use crate::query::read::{exec, ReadQuery};
use shamir_types::types::record_id::RecordId;

#[derive(Debug)]
pub(super) struct CandidateSet {
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

pub(super) fn index2_shortcut_eligible(query: &ReadQuery) -> bool {
    !exec::has_aggregates(&query.select)
        && query.group_by.is_none()
        && !query.select.distinct
        && query.order_by.is_none()
}
