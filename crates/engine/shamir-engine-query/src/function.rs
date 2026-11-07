//! Immutable built-in aggregate registry.

use shamir_funclib::agg::AggRegistry;
use std::sync::OnceLock;

/// Built-in aggregate functions.
pub(crate) fn builtin_aggs() -> &'static AggRegistry {
    static REG: OnceLock<AggRegistry> = OnceLock::new();
    REG.get_or_init(shamir_funclib::agg_builtins)
}
