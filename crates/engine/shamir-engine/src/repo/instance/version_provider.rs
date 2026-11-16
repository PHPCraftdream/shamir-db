/// Convert the wire DTO [`shamir_query_types::admin::Retention`] into the
/// engine-internal [`shamir_tx::Retention`]. The orphan rule forbids a
/// cross-crate `From` impl, so this free fn bridges the two types.
///
/// Call `Retention::validate()` on the DTO first; this fn performs no
/// validation (it is a pure field copy).
pub fn to_mvcc_retention(r: &shamir_query_types::admin::Retention) -> shamir_tx::Retention {
    shamir_tx::Retention {
        max_age_secs: r.max_age_secs,
        max_count: r.max_count,
        min_count: r.min_count,
    }
}

/// ENGINE-LAYERING R2d: the implementation moved DOWN to the repo layer
/// (`crate::repo::meta::repo_identity::repo_token` — the identity token of a
/// repository); re-exported so `crate::repo::version_provider::repo_token`
/// and `crate::repo::repo_token` keep working for all callers and tests.
pub use crate::repo::meta::repo_identity::repo_token;
