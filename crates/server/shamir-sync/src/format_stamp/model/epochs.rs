//! Base and current format epochs (design 4.1). Both are `1` until the
//! release commit; no change bumps the epoch before that (design 3.6).

/// Lowest epoch any shipped binary may stamp; adoption target (row 5a).
pub const BASE_FORMAT_EPOCH: u64 = 1;

/// Epoch this binary writes and accepts as up to date.
pub const CURRENT_FORMAT_EPOCH: u64 = 1;
