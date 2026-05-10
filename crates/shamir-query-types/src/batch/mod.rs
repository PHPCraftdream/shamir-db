//! Batch-request DTOs (BatchRequest, BatchResponse, BatchOp,
//! BatchLimits, BatchError). Planner / executor / `$query` reference
//! resolver live in shamir-engine.

pub mod types;

pub use types::{
    BatchError, BatchLimits, BatchOp, BatchPlan, BatchRequest, BatchResponse, QueryEntry,
    TransactionInfo,
};
