use std::fmt;

use serde::{Deserialize, Serialize};

/// Stable control-plane and terminal follower refusal codes (SEED-4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedRefusal {
    IncompleteInstall,
    OrdinaryRepository,
    WrongScope,
    WrongPublication,
    MissingIdentity,
    WrongIdentity,
    WrongHistoryEpoch,
    WrongSchema,
    HorizonAboveBookmark,
    SourceRegression,
    BookmarkOverflow,
    CompetingOwner,
    UpstreamUnavailable,
    ReopenRequired,
    AtomicBatchUnsupported,
    ConflictingBinding,
}

impl SeedRefusal {
    pub fn code(self) -> &'static str {
        match self {
            Self::IncompleteInstall => "seed_incomplete_install",
            Self::OrdinaryRepository => "seed_ordinary_repository",
            Self::WrongScope => "seed_wrong_scope",
            Self::WrongPublication => "seed_wrong_publication",
            Self::MissingIdentity => "seed_missing_identity",
            Self::WrongIdentity => "seed_wrong_identity",
            Self::WrongHistoryEpoch => "seed_wrong_history_epoch",
            Self::WrongSchema => "seed_wrong_schema",
            Self::HorizonAboveBookmark => "seed_horizon_above_bookmark",
            Self::SourceRegression => "seed_source_regression",
            Self::BookmarkOverflow => "seed_bookmark_overflow",
            Self::CompetingOwner => "seed_competing_owner",
            Self::UpstreamUnavailable => "seed_upstream_unavailable",
            Self::ReopenRequired => "reopen_required",
            Self::AtomicBatchUnsupported => "atomic_batch_unsupported",
            Self::ConflictingBinding => "seed_conflicting_binding",
        }
    }
}

impl fmt::Display for SeedRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for SeedRefusal {}
