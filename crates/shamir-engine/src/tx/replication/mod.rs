// `tx::replication` — follower apply + replicated events (stage E1 regroup).
// Declarations and re-exports only. `follower_durability` stays mounted as a
// child of `apply_replicated` (via its `#[path]` line) exactly as before.
pub mod apply_replicated;
pub mod replicated_event;

pub use apply_replicated::follower_durability;
