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
//! * **Single-node degradation** — on a single-socket box (most dev machines,
//!   Windows, CI runners) [`FallbackSingleNodeTopology`] reports one node,
//!   [`NodeReplicated`] holds one replica, and behaviour is identical to a
//!   bare `ArcSwap` — zero overhead, no `cfg`-gates at the call site.
//! * **False-sharing defence** ([`CachePadded`]) — per-node `ArcSwap` cells
//!   are aligned to a cache line so independent nodes never ping-pong a shared
//!   line (Herlihy & Shavit §8; Drepper §6.4).
//!
//! # Scope of this version (Фаза 1)
//!
//! Platform-independent skeleton only: the trait, the mock, the single-node
//! fallback, the `NodeReplicated` primitive, the cache-padding helper, and the
//! Linux `cpulist` parser ([`parse_cpulist`]). The real `LinuxTopology`
//! (`/sys` probe + `sched_setaffinity`) and QEMU-based integration tests land
//! in Фаза 1b — see `docs/research/NUMA-DESIGN-2026-06-29.md`.

mod cache_padded;
mod cpulist;
mod detect;
mod error;
mod fallback;
#[cfg(target_os = "linux")]
mod linux;
mod mock;
mod node;
mod node_replicated;
mod topology;

pub use cache_padded::CachePadded;
pub use cpulist::parse_cpulist;
pub use detect::detect;
pub use error::AffinityError;
pub use fallback::FallbackSingleNodeTopology;
#[cfg(target_os = "linux")]
pub use linux::LinuxTopology;
pub use mock::MockTopology;
pub use node::{CpuId, NodeId};
pub use node_replicated::NodeReplicated;
pub use topology::Topology;

#[cfg(test)]
mod tests;
