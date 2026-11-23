/// Raw counters of one shape (or of the untracked `other` bucket).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShapeCounters {
    pub executions: u64,
    pub total_nanos: u64,
    pub errors: u64,
}

/// Cold-path copy of a registry's shape counters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShapeStatsSnapshot {
    pub rows: Vec<([u8; 16], ShapeCounters)>,
    pub other: ShapeCounters,
    pub tracked: u64,
}
