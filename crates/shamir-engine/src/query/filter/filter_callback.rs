//! `FilterCallback` — thin compat trait for callers that hold `&dyn FilterCallback`.
//!
//! New code should use `&FilterNode` directly; the trait exists so existing
//! callers keep working without changing signatures.

use shamir_types::types::value::InnerValue;

use super::eval_context::FilterContext;

pub trait FilterCallback: Send + Sync {
    fn matches(&self, record: &InnerValue, ctx: &FilterContext) -> bool;
}
