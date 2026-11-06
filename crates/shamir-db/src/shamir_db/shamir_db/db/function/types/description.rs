use shamir_engine::function::{TierMode, TierReport};

use super::FunctionState;

/// Introspection snapshot of one function.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDescription {
    /// Function name.
    pub name: String,
    /// Server-derived language tag: `"wasm"`, `"rust"` or `"native"`.
    pub lang: String,
    /// Origin string claimed by the module, if any.
    pub origin_claim: Option<String>,
    /// ABI version declared by the module, if known.
    pub abi_version: Option<u32>,
    /// Composite module hash in 32 hex chars; `None` when unknown.
    pub hash: Option<String>,
    /// Size of the stored module in bytes, if known.
    pub byte_size: Option<usize>,
    /// Stored optimization mode.
    pub optimize: TierMode,
    /// Live tier snapshot; `None` when not WASM-backed or unavailable.
    pub tier_report: Option<TierReport>,
    /// Runtime availability.
    pub state: FunctionState,
}
