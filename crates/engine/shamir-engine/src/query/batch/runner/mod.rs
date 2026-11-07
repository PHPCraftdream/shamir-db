//! Query execution: the stage-runner internals (`query_runner`), `$param`
//! substitution, result-size accounting and sub-batch value serialization.

pub(super) mod param_subst;
pub(super) mod query_runner;
pub(super) mod query_value_serializer;
pub(super) mod result_size;
