//! Instance-layer half of the FK reverse-cache split: the cache TYPE lives in
//! `crate::table::fk_reverse_cache`; the rebuild SCAN (`build_reverse_fk_entries`,
//! which takes the query-layer `BatchResolver`) moved to the query layer in
//! ENGINE-LAYERING R2d (`crate::query::batch::fk::reverse_fk_scan`) and is
//! re-exported below so every historical
//! `crate::repo::<...>::build_reverse_fk_entries` path keeps resolving
//! (instance -> query is the allowed direction).

pub use crate::query::batch::build_reverse_fk_entries;
pub use crate::table::fk_reverse_cache::{
    FkReverseCache, FkSnapshot, ReverseFkEntry, TaggedReverseFkEntry,
};
#[cfg(test)]
pub(crate) use crate::table::fk_reverse_cache::{
    PostGenCheckPrePublishHook, TEST_POST_GENCHECK_PRE_PUBLISH_HOOK,
    TEST_POST_PUBLISH_PRE_RETURN_HOOK,
};
