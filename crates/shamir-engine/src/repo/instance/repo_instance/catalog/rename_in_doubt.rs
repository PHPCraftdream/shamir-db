use super::rename_freeze::RenameFreeze;

/// Owned source state retained while a rename outcome is unresolved.
pub(crate) struct RenameInDoubt {
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) freeze: RenameFreeze,
}

/// Name-indexed value for an in-doubt rename pair.
pub(crate) enum RenameFenceEntry {
    /// Source entry owns the retained freeze.
    Source(RenameInDoubt),
    /// Source entry is temporarily held in place while an exact-pair retry runs.
    InFlight { from: String, to: String },
    /// Destination entry points to the source name.
    Destination { from: String },
}
