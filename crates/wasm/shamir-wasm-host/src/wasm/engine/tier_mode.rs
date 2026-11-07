//! [`TierMode`] — when a function may leave the fast tier.

/// Creation-time policy selecting when a function may leave the fast tier.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TierMode {
    /// Compile the fast tier synchronously; one background upgrade to the
    /// optimized tier once the call counter crosses `TierOptions::hot_calls`.
    #[default]
    Auto,
    /// Compile the optimized tier synchronously at creation; never upgraded.
    Now,
    /// Fast tier only; the upgrade is never scheduled.
    Never,
}
