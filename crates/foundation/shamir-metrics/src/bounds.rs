//! Static bucket bounds (design `engine-metrics.md` §3.2).
//!
//! Bounds are `&'static [u64]` so `Histogram::new` can stay `const` and
//! allocation-free. `MAX_BOUNDS` is the fixed cell-array capacity; a larger
//! bound slice is rejected at construction.

/// Fixed atomic cell capacity (largest bound set + the implicit overflow
/// cell: 19 latency bounds → 20 cells).
pub const MAX_BOUNDS: usize = 19;

/// Latency bounds in nanoseconds, ~powers pattern 100 µs … 60 s.
pub const LATENCY_NS_BOUNDS: &[u64] = &[
    100_000,
    250_000,
    500_000,
    1_000_000,
    2_500_000,
    5_000_000,
    10_000_000,
    25_000_000,
    50_000_000,
    100_000_000,
    250_000_000,
    500_000_000,
    1_000_000_000,
    2_500_000_000,
    5_000_000_000,
    10_000_000_000,
    20_000_000_000,
    40_000_000_000,
    60_000_000_000,
];

/// Byte bounds, 12 powers of two spanning 1 KiB → 1 GiB (1–4 KiB consecutive,
/// then ×4 steps — extra granularity where payload sizes cluster).
pub const BYTES_BOUNDS: &[u64] = &[
    1_024,
    2_048,
    4_096,
    16_384,
    65_536,
    262_144,
    1_048_576,
    4_194_304,
    16_777_216,
    67_108_864,
    268_435_456,
    1_073_741_824,
];
