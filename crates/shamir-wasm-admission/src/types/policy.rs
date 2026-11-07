use super::limits::AdmissionLimits;

/// Which gates [`admit_module`](crate::wasm::admit_module) applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionPolicy {
    /// Every gate: the deterministic ones plus the numeric caps and the
    /// export contract check.
    Admission(AdmissionLimits),
    /// Deterministic gates only: parse, feature set (including the
    /// reference-type operator scan), import allowlist, ABI/origin sections.
    /// No caps, no contract check, no size or time limits.
    Load,
}
