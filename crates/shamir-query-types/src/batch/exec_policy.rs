//! Server-owned execution policy, separate from request limits.

use super::BatchLimits;

/// Server-owned policy passed separately from the request DTO.
/// Clients cannot influence this policy through their request limits.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecPolicy {
    pub caps: BatchLimits,
    pub enforce_result_size: bool,
}

impl ExecPolicy {
    /// Embedded execution uses hard engine caps without result-size enforcement.
    pub const EMBEDDED: ExecPolicy = ExecPolicy {
        caps: BatchLimits::CEILING,
        enforce_result_size: false,
    };
}
