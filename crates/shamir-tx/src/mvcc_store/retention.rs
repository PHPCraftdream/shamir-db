/// Per-store history retention — three ORTHOGONAL optional knobs
/// (TEMPORAL.md §3). Default = CurrentOnly (`max_count: Some(0)`): keep only
/// current + versions pinned by live snapshots. All three knobs are enforced
/// by [`MvccStore::vacuum_key`] (T1c wired `max_age_secs` once versions
/// carry a per-version commit timestamp).
///
/// Stored on [`MvccStore`] via `ArcSwap<Retention>` (lock-free swappable —
/// three fields can't be one atomic).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retention {
    /// AGE cap: reclaim versions whose commit timestamp is older than
    /// `max_age_secs` seconds (a version with no recorded ts is treated as
    /// "unknown age" and conservatively KEPT by the age axis).
    pub max_age_secs: Option<u64>,
    /// COUNT cap: keep at most N old versions per key (`None` = unlimited).
    pub max_count: Option<u64>,
    /// COUNT floor: always keep ≥ M newest old versions per key, EVEN IF
    /// older than `max_age_secs` (this is `min_count`'s real job — protect
    /// recent versions from the age cap). Inert against the count cap
    /// (validation guarantees `min_count ≤ max_count`, so the cap already
    /// keeps ≥ min_count).
    pub min_count: Option<u64>,
}

impl Default for Retention {
    fn default() -> Self {
        // CurrentOnly: keep 0 old versions (current + live-snapshot-pinned only).
        Self {
            max_age_secs: None,
            max_count: Some(0),
            min_count: None,
        }
    }
}

impl Retention {
    /// CurrentOnly: keep only current + versions pinned by live snapshots.
    pub fn current_only() -> Self {
        Self::default()
    }

    /// KeepHistory (Forever): retain all versions — no count cap.
    pub fn keep_history() -> Self {
        Self {
            max_age_secs: None,
            max_count: None,
            min_count: None,
        }
    }

    /// Validate: `min_count` must be `<= max_count` when both are `Some`.
    /// Returns `Err(message)` on violation.
    pub fn validate(&self) -> Result<(), String> {
        if let (Some(mc), Some(maxc)) = (self.min_count, self.max_count) {
            if mc > maxc {
                return Err(format!(
                    "retention: min_count ({mc}) must be <= max_count ({maxc})"
                ));
            }
        }
        Ok(())
    }
}
