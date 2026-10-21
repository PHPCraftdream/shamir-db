//! Batch query executor — `QueryRunner` and supporting free functions.
//!
//! Executes a BatchPlan stage by stage, passing results between
//! dependent queries via FilterContext::resolved_refs.

mod arms_read;
mod arms_write;
mod fk_isolation;
mod helpers;
mod nested;
mod runner;

#[cfg(test)]
pub(crate) use fk_isolation::{
    implicit_tx_isolation_for_fk_parent, require_footprint_if_fk_child, FkParentOpKind,
};
pub(in crate::query::batch) use helpers::{
    build_resolved_refs, resolve_skip, skipped_query_result,
};
#[cfg(test)]
pub(crate) use helpers::{effective_max_iterations, ABSOLUTE_MAX_FOR_EACH_ITERATIONS};
pub use runner::QueryRunner;

#[cfg(test)]
pub(crate) use fk_isolation::FkRoleDecisionHook;
#[cfg(test)]
pub(crate) use fk_isolation::{
    TEST_CHILD_PRE_ROLE_DECISION_HOOK, TEST_PARENT_PRE_ROLE_DECISION_HOOK,
};
#[cfg(test)]
pub(in crate::query::batch) use runner::execute_single_impl;
