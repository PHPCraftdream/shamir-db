//! Sync subsystem: replication (leader/follower), replication seeds, the
//! connection transport stack, durability primitives and the trait ports
//! they consume.

pub mod connection;
pub mod durability;
pub mod framer;
pub mod ports;
pub mod replication;
pub mod replication_seed;
