//! What `check_data_dir` did or observed (design 2.7, 3.4).

/// Outcome of a startup check. Inspect mode reports observations
/// (`InspectedStampless`, `InspectedOlder`); the caller decides whether an
/// observation is a refusal for its subcommand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StampOutcome {
    /// Row 3: stamp written into an empty directory.
    Created,
    /// Row 5a: stampless data adopted at the base epoch.
    Adopted,
    /// Rows 7/10: valid stamp, current epoch, nothing rewritten.
    Unchanged,
    /// D6: `last_opened_by` rewritten (binary version changed).
    VersionUpdated,
    /// Inspect on a stampless directory.
    InspectedStampless,
    /// Inspect on an older-epoch directory (row 9/12 observe).
    InspectedOlder,
}
