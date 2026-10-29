//! `shamir-numa` — NUMA-topology abstraction and per-node replicated
//! read-mostly state for S.H.A.M.I.R.
//!
//! # Why this crate exists
//!
//! On a **NUMA** (Non-Uniform Memory Access) machine — any modern 2+ socket
//! server — the latency and bandwidth of a memory access depend on which
//! socket owns the cache line and which core issues the load. Local access on
//! Sapphire Rapids / Genoa is ~80 ns at full per-socket bandwidth; a remote
//! access crosses the inter-socket interconnect (UPI / Infinity Fabric) at
//! ~140–200 ns and shares that link with every other cross-socket stream
//! (Hennessy & Patterson §5.2–5.3; Drepper, *What Every Programmer Should
//! Know About Memory*, §5).
//!
//! S.H.A.M.I.R.'s hot read-mostly registries (index definitions, validator
//! bindings, interner snapshots) already live in `ArcSwap` cells after the
//! #292 / #304 campaign. This crate adds the NUMA dimension: a
//! [`NodeReplicated<T>`] keeps **one `ArcSwap<T>` per NUMA node**, so a reader
//! on node *N* loads its node-local replica without reaching across a socket.
//!
//! # Design pillars (formal grounding)
//!
//! * **Topology behind a trait** ([`Topology`]) — production code depends on
//!   `Arc<dyn Topology>`, tests inject [`MockTopology`]. NUMA-aware logic is
//!   testable without real multi-socket hardware (DI mock).
//! * **Single-node degradation** — a one-node topology gives one replica and
//!   no write mirror loop. Local loads still query the topology and resolve a
//!   replica before loading the `ArcSwap`, with constant-time routing.
//! * **False-sharing defence** ([`CachePadded`]) — per-node `ArcSwap` cells
//!   are aligned to a cache line so independent nodes never ping-pong a shared
//!   line (Herlihy & Shavit §8; Drepper §6.4).
//!
//! # Scope
//!
//! Linux discovery and affinity, the single-node fallback, mock topology,
//! replication, cache padding, and cpulist parsing are implemented. CI has a
//! QEMU topology smoke harness; full Rust integration inside the guest remains
//! future work.

mod core;
mod error;
mod platform;

pub use core::{CachePadded, CpuId, NodeId, NodeReplicated, Topology};
pub use error::AffinityError;
#[cfg(target_os = "linux")]
pub use platform::LinuxTopology;
pub use platform::{
    detect, detect_cached, parse_cpulist, FallbackSingleNodeTopology, MockTopology,
};

#[cfg(test)]
mod tests;
