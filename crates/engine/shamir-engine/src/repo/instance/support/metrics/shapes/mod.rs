mod shape_registry;
mod shape_row;
mod shape_stats_snapshot;

#[cfg(test)]
mod tests;

pub use shape_registry::{ShapeRegistry, SHAPE_ROWS_CAP, SHAPE_ROWS_LOW_WATER};
pub use shape_stats_snapshot::{ShapeCounters, ShapeStatsSnapshot};
