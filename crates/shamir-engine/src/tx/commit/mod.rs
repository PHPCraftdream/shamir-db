mod entry;
mod legacy_and_lockfree;

#[cfg(test)]
pub(crate) use crate::tx::ack::apply_backpressure;
#[allow(unused_imports)] // Compatibility paths for the moved VIS-3 API.
pub(crate) use crate::tx::ack::{
    acknowledge_known_outcome, apply_backpressure_with_budget, commit_tx_with_options,
};
#[cfg(test)]
pub(crate) use entry::TEST_SYNCED_WAL_REPO;
pub use entry::{commit_tx, wal_ops_from_tx, TxError};
pub(in crate::tx) use entry::{
    commit_tx_inner, commit_wal_durability, fire_post_wal_pre_publish_test_hook, maybe_crash,
    wal_completion,
};
#[cfg(test)]
pub(crate) use entry::{
    AsyncIndexTailHook, PostValidatePrePublishHook, TEST_ASYNC_INDEX_TAIL_HOOK,
    TEST_MAX_TX_LIFETIME_OVERRIDE, TEST_POST_VALIDATE_PRE_PUBLISH_HOOK,
};
#[cfg(any(test, feature = "test-util"))]
pub use entry::{PostWalPrePublishHook, TEST_POST_WAL_PRE_PUBLISH_HOOK};
pub(crate) use legacy_and_lockfree::release_pessimistic_locks;
