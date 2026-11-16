//! One change-log entry (decision D12, design 4.3).

/// A format change. Static strings so the table is a `const`.
pub struct FormatChange {
    /// Stable short id (first entry: `pre-release-absorbed`).
    pub id: &'static str,
    /// Whether the format epoch was raised by this change (design 4.2).
    pub bumps_epoch: bool,
    /// Epoch in force after this change.
    pub epoch_after: u64,
    /// Component versions after this change.
    pub components: &'static [(&'static str, u32)],
    /// What changed, one sentence.
    pub summary: &'static str,
    /// What the previous binary does with the new data: reads / writes / loses.
    pub previous_binary_effect: &'static str,
    /// For `bumps_epoch = false`: why the previous binary is safe.
    pub reason: &'static str,
    /// First commit carrying the change.
    pub commit: &'static str,
    /// R1b fills this; `None` in R1a (design 4.3).
    pub migration_module: Option<&'static str>,
}
