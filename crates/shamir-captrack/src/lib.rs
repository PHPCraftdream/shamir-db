//! `shamir-captrack` — capacity-telemetry infrastructure for ShamirDB.
//!
//! # Overview
//!
//! This crate provides 13 `macro_rules!` macros (`tvec!`, `tmap!`, …) that
//! wrap every major collection constructor.  In off-feature mode (default)
//! each macro expands to the bare constructor with **zero overhead** — the
//! compiler sees exactly `Vec::with_capacity(n)` etc.  When the
//! `capacity-telemetry` feature is enabled, each macro instead returns a
//! `Tracked*` wrapper that records two counters in a global lock-free
//! registry:
//!
//! * `peak_capacity` — maximum capacity observed across all instances of that
//!   name (updated in `Drop` via `fetch_max`).
//! * `creation_count` — total number of instances created (updated in ctor
//!   via `fetch_add`).
//!
//! Call `dump_capacity_stats("path/to/stats.json")` at any point (e.g. end of
//! a benchmark) to write the accumulated stats as pretty-printed JSON.
//!
//! # Feature flags
//!
//! * `capacity-telemetry` (default: **off**) — enables `Tracked*` wrappers and
//!   the global registry.  In production builds leave this off; enable it only
//!   in benchmarks that need capacity data.
//!
//! # Hasher
//!
//! All hash-keyed macros (`tfxmap!`, `tmap!`, `tfxset!`, `tset!`,
//! `tdashmap!`, `tsccmap!`, `tsccset!`) expand to
//! `…::with_capacity_and_hasher(cap, ShamirHasher::default())` —
//! `ShamirHasher = BuildHasherDefault<FxHasher>` — matching the workspace
//! ideology (CLAUDE.md §4).

// ---------------------------------------------------------------------------
// ShamirHasher — workspace-standard FxHasher alias.
//
// Defined here because shamir-captrack cannot take a hard dep on
// shamir-collections (that would create a dependency that is wrong for a
// leaf telemetry crate).  Both crates define the same alias over the same
// fxhash crate; Rust's type system treats them as distinct types but the
// semantics are identical.
//
// Only needed when hash-collection features are active.
// ---------------------------------------------------------------------------

/// `BuildHasherDefault<FxHasher>` — the workspace-standard fast hasher.
///
/// Used by all hash-keyed macros in both on-feature and off-feature mode so
/// the expanded code always uses `FxHasher` rather than `RandomState`.
/// `fxhash` is a non-optional dep of shamir-captrack so that this type alias
/// is available regardless of whether `capacity-telemetry` is enabled.
pub type ShamirHasher = std::hash::BuildHasherDefault<fxhash::FxHasher>;

// ---------------------------------------------------------------------------
// Sub-modules (cfg-gated where they need feature deps)
// ---------------------------------------------------------------------------

#[cfg(feature = "capacity-telemetry")]
pub mod registry;

pub mod dump;

#[cfg(feature = "capacity-telemetry")]
mod tracked;

// ---------------------------------------------------------------------------
// Public re-exports
// ---------------------------------------------------------------------------

pub use dump::dump_capacity_stats;

#[cfg(feature = "capacity-telemetry")]
pub use tracked::{
    TrackedBTreeMap, TrackedBTreeSet, TrackedBytesMut, TrackedDashMap, TrackedFxHashMap,
    TrackedHashMap, TrackedHashSet, TrackedIndexMap, TrackedIndexSet, TrackedSccHashMap,
    TrackedSccHashSet, TrackedSccTreeIndex, TrackedVec, TrackedVecDeque,
};

// ---------------------------------------------------------------------------
// 13 call-site macros
//
// CRITICAL: every off-feature expansion is wrapped in `{ #[allow(...)] expr }`
// so that when Faza 2 (#293) adds `disallowed-methods` bans on bare
// constructors, the macros themselves don't trigger those lints on call-sites.
// ---------------------------------------------------------------------------

// ── tvec! ────────────────────────────────────────────────────────────────────

/// Create a `Vec<T>` (off-feature) or `TrackedVec<T>` (on-feature) with the
/// given capacity.
///
/// ```
/// # use shamir_captrack::tvec;
/// let mut v = tvec!("my/vec", 16);
/// v.push(1u32);
/// assert_eq!(v.len(), 1);
/// ```
#[cfg(not(feature = "capacity-telemetry"))]
#[macro_export]
macro_rules! tvec {
    ($name:literal, $cap:expr) => {{
        let _: &'static str = $name;
        {
            #[allow(clippy::disallowed_methods)]
            ::std::vec::Vec::with_capacity($cap)
        }
    }};
}

#[cfg(feature = "capacity-telemetry")]
#[macro_export]
macro_rules! tvec {
    ($name:literal, $cap:expr) => {
        $crate::TrackedVec::with_capacity_named($cap, $name)
    };
}

// ── tvecdeque! ───────────────────────────────────────────────────────────────

#[cfg(not(feature = "capacity-telemetry"))]
#[macro_export]
macro_rules! tvecdeque {
    ($name:literal, $cap:expr) => {{
        let _: &'static str = $name;
        {
            #[allow(clippy::disallowed_methods)]
            ::std::collections::VecDeque::with_capacity($cap)
        }
    }};
}

#[cfg(feature = "capacity-telemetry")]
#[macro_export]
macro_rules! tvecdeque {
    ($name:literal, $cap:expr) => {
        $crate::TrackedVecDeque::with_capacity_named($cap, $name)
    };
}

// ── tbtreemap! ───────────────────────────────────────────────────────────────

/// Cap hint is accepted for API uniformity; BTreeMap has no `with_capacity`.
#[cfg(not(feature = "capacity-telemetry"))]
#[macro_export]
macro_rules! tbtreemap {
    ($name:literal, $_cap:expr) => {{
        let _: &'static str = $name;
        {
            #[allow(clippy::disallowed_methods)]
            ::std::collections::BTreeMap::new()
        }
    }};
}

#[cfg(feature = "capacity-telemetry")]
#[macro_export]
macro_rules! tbtreemap {
    ($name:literal, $cap:expr) => {
        $crate::TrackedBTreeMap::new_named($cap, $name)
    };
}

// ── tbtreeset! ───────────────────────────────────────────────────────────────

#[cfg(not(feature = "capacity-telemetry"))]
#[macro_export]
macro_rules! tbtreeset {
    ($name:literal, $_cap:expr) => {{
        let _: &'static str = $name;
        {
            #[allow(clippy::disallowed_methods)]
            ::std::collections::BTreeSet::new()
        }
    }};
}

#[cfg(feature = "capacity-telemetry")]
#[macro_export]
macro_rules! tbtreeset {
    ($name:literal, $cap:expr) => {
        $crate::TrackedBTreeSet::new_named($cap, $name)
    };
}

// ── tbytesmut! ───────────────────────────────────────────────────────────────

#[cfg(not(feature = "capacity-telemetry"))]
#[macro_export]
macro_rules! tbytesmut {
    ($name:literal, $cap:expr) => {{
        let _: &'static str = $name;
        {
            #[allow(clippy::disallowed_methods)]
            ::bytes::BytesMut::with_capacity($cap)
        }
    }};
}

#[cfg(feature = "capacity-telemetry")]
#[macro_export]
macro_rules! tbytesmut {
    ($name:literal, $cap:expr) => {
        $crate::TrackedBytesMut::with_capacity_named($cap, $name)
    };
}

// ── tfxmap! ──────────────────────────────────────────────────────────────────

/// `std::HashMap` with `FxHasher` (workspace default for order-agnostic maps).
#[cfg(not(feature = "capacity-telemetry"))]
#[macro_export]
macro_rules! tfxmap {
    ($name:literal, $cap:expr) => {{
        let _: &'static str = $name;
        {
            // disallowed_types: std::HashMap is the correct type here — the
            // workspace ban targets bare HashMap without FxHasher; the macro
            // enforces FxHasher so this site is intentional.
            // disallowed_methods: allow the bare constructor — we wrap it.
            #[allow(clippy::disallowed_types, clippy::disallowed_methods)]
            ::std::collections::HashMap::with_capacity_and_hasher(
                $cap,
                <$crate::ShamirHasher as ::std::default::Default>::default(),
            )
        }
    }};
}

#[cfg(feature = "capacity-telemetry")]
#[macro_export]
macro_rules! tfxmap {
    ($name:literal, $cap:expr) => {
        $crate::TrackedFxHashMap::with_capacity_named($cap, $name)
    };
}

// ── tfxset! ──────────────────────────────────────────────────────────────────

/// `std::HashSet` with `FxHasher` (workspace default for order-agnostic sets).
#[cfg(not(feature = "capacity-telemetry"))]
#[macro_export]
macro_rules! tfxset {
    ($name:literal, $cap:expr) => {{
        let _: &'static str = $name;
        {
            // disallowed_types: std::HashSet is the correct type here (with
            // FxHasher enforced by this macro).
            #[allow(clippy::disallowed_types, clippy::disallowed_methods)]
            ::std::collections::HashSet::with_capacity_and_hasher(
                $cap,
                <$crate::ShamirHasher as ::std::default::Default>::default(),
            )
        }
    }};
}

#[cfg(feature = "capacity-telemetry")]
#[macro_export]
macro_rules! tfxset {
    ($name:literal, $cap:expr) => {
        $crate::TrackedHashSet::with_capacity_named($cap, $name)
    };
}

// ── tmap! ────────────────────────────────────────────────────────────────────

/// `IndexMap` with `FxHasher` — insertion-ordered (workspace TMap equivalent).
#[cfg(not(feature = "capacity-telemetry"))]
#[macro_export]
macro_rules! tmap {
    ($name:literal, $cap:expr) => {{
        let _: &'static str = $name;
        {
            #[allow(clippy::disallowed_methods)]
            ::indexmap::IndexMap::with_capacity_and_hasher(
                $cap,
                <$crate::ShamirHasher as ::std::default::Default>::default(),
            )
        }
    }};
}

#[cfg(feature = "capacity-telemetry")]
#[macro_export]
macro_rules! tmap {
    ($name:literal, $cap:expr) => {
        $crate::TrackedIndexMap::with_capacity_named($cap, $name)
    };
}

// ── tset! ────────────────────────────────────────────────────────────────────

/// `IndexSet` with `FxHasher` — insertion-ordered (workspace TSet equivalent).
#[cfg(not(feature = "capacity-telemetry"))]
#[macro_export]
macro_rules! tset {
    ($name:literal, $cap:expr) => {{
        let _: &'static str = $name;
        {
            #[allow(clippy::disallowed_methods)]
            ::indexmap::IndexSet::with_capacity_and_hasher(
                $cap,
                <$crate::ShamirHasher as ::std::default::Default>::default(),
            )
        }
    }};
}

#[cfg(feature = "capacity-telemetry")]
#[macro_export]
macro_rules! tset {
    ($name:literal, $cap:expr) => {
        $crate::TrackedIndexSet::with_capacity_named($cap, $name)
    };
}

// ── tdashmap! ────────────────────────────────────────────────────────────────

/// `DashMap` with `FxHasher` (workspace default for sharded concurrent maps).
#[cfg(not(feature = "capacity-telemetry"))]
#[macro_export]
macro_rules! tdashmap {
    ($name:literal, $cap:expr) => {{
        let _: &'static str = $name;
        {
            #[allow(clippy::disallowed_methods)]
            ::dashmap::DashMap::with_capacity_and_hasher(
                $cap,
                <$crate::ShamirHasher as ::std::default::Default>::default(),
            )
        }
    }};
}

#[cfg(feature = "capacity-telemetry")]
#[macro_export]
macro_rules! tdashmap {
    ($name:literal, $cap:expr) => {
        $crate::TrackedDashMap::with_capacity_named($cap, $name)
    };
}

// ── tsccmap! ─────────────────────────────────────────────────────────────────

/// `scc::HashMap` with `FxHasher` (workspace default for lock-free concurrent
/// maps).
#[cfg(not(feature = "capacity-telemetry"))]
#[macro_export]
macro_rules! tsccmap {
    ($name:literal, $cap:expr) => {{
        let _: &'static str = $name;
        {
            #[allow(clippy::disallowed_methods)]
            ::scc::HashMap::with_capacity_and_hasher(
                $cap,
                <$crate::ShamirHasher as ::std::default::Default>::default(),
            )
        }
    }};
}

#[cfg(feature = "capacity-telemetry")]
#[macro_export]
macro_rules! tsccmap {
    ($name:literal, $cap:expr) => {
        $crate::TrackedSccHashMap::with_capacity_named($cap, $name)
    };
}

// ── tsccset! ─────────────────────────────────────────────────────────────────

/// `scc::HashSet` with `FxHasher`.
#[cfg(not(feature = "capacity-telemetry"))]
#[macro_export]
macro_rules! tsccset {
    ($name:literal, $cap:expr) => {{
        let _: &'static str = $name;
        {
            #[allow(clippy::disallowed_methods)]
            ::scc::HashSet::with_capacity_and_hasher(
                $cap,
                <$crate::ShamirHasher as ::std::default::Default>::default(),
            )
        }
    }};
}

#[cfg(feature = "capacity-telemetry")]
#[macro_export]
macro_rules! tsccset {
    ($name:literal, $cap:expr) => {
        $crate::TrackedSccHashSet::with_capacity_named($cap, $name)
    };
}

// ── tscctree! ────────────────────────────────────────────────────────────────

/// `scc::TreeIndex` — sorted lock-free B+ tree.  Cap hint is accepted for API
/// uniformity; `TreeIndex::new()` takes no capacity argument.
#[cfg(not(feature = "capacity-telemetry"))]
#[macro_export]
macro_rules! tscctree {
    ($name:literal, $_cap:expr) => {{
        let _: &'static str = $name;
        {
            #[allow(clippy::disallowed_methods)]
            ::scc::TreeIndex::new()
        }
    }};
}

#[cfg(feature = "capacity-telemetry")]
#[macro_export]
macro_rules! tscctree {
    ($name:literal, $cap:expr) => {
        $crate::TrackedSccTreeIndex::new_named($cap, $name)
    };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
