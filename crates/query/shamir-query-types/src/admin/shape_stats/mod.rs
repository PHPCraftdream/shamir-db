//! Admin DTOs for per-shape execution statistics (R4b step 4): the typed
//! `BatchOp::ListShapeStats` / `BatchOp::ResetShapeStats` requests and the
//! `ShapeStatsPage` response rows.
//!
//! Pure wire/serde types; classification (read-only introspection, local
//! in-memory telemetry, follower-safe) lives in `BatchOp`, authorization and
//! rendering in the server's admin handler. Rows carry canonical shape
//! hashes and counters only: never field names, literals, user identity or
//! table names (the key is a hash).

mod list_shape_stats_req;
mod reset_shape_stats_req;
mod shape_stats_page;
mod shape_stats_row;
mod shape_stats_totals;

pub use list_shape_stats_req::ListShapeStatsReq;
pub use reset_shape_stats_req::ResetShapeStatsReq;
pub use shape_stats_page::ShapeStatsPage;
pub use shape_stats_row::ShapeStatsRow;
pub use shape_stats_totals::ShapeStatsTotals;

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
