//! [`Tier`] — the compilation tier a function slot is resident on.

/// The compilation tier backing a function slot.
///
/// * `Fast` — the single-pass Winch engine: cheap compile, slower code.
/// * `Optimized` — the Cranelift engine: slower compile, faster code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Fast,
    Optimized,
}
