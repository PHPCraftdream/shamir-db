//! Replication bookmark metadata stored in a versioned envelope.
//!
//! Legacy fixed-width bookmark records remain readable; version 1 binds a
//! bookmark to its upstream fence, while version 2 also tracks the follower's
//! local clean-history horizon independently. `J` is monotonic and never
//! overwrites the upstream history epoch.
#[path = "../tests/follower_bookmark/v2.rs"]
mod v2;
pub use v2::{
    load_clean_horizon, load_fence_record, load_replication_bookmark,
    save_bookmark_and_clean_horizon, save_fence_record, save_replication_bookmark, FenceRecord,
};

#[cfg(test)]
#[path = "../tests/follower_bookmark/mod.rs"]
mod tests;
