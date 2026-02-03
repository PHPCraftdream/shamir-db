//! Index engine module
//!
//! # Current Implementation (2025-02-03)
//!
//! This module provides **index configuration management** and **unique constraint enforcement**.
//!
//! ## What's Implemented
//!
//! - **IndexTarget**: Three-state indexing configuration (Disabled/All/Selective)
//! - **IndexDef**: Index definition with path and unique flag
//! - **IndexOp**: Journal operation types (for future async indexing)
//! - **IndexChange**: Index change entries (for future async indexing)
//!
//! ## API Available on Table
//!
//! ```ignore
//! // Index management
//! table.add_index(&["email"]).await?;
//! table.add_unique_index(&["user", "id"]).await?;
//! table.remove_index(&["email"]).await?;
//! table.enable_indexing_all().await?;
//! table.disable_indexing().await?;
//!
//! // Unique constraints are automatically enforced on:
//! // - table.insert()
//! // - table.update()
//! // - table.set()
//! ```
//!
//! ## What's NOT Yet Implemented
//!
//! - Actual index storage (hash keys to record ID lists)
//! - Query-by-index functionality
//! - Journal-based async index updates
//! - Global indexer thread
//!
//! See `index_engine.md` for the full architecture design (deferred).
//! See `milestones.md` for implementation status and roadmap.

mod types;

pub use types::{IndexChange, IndexDef, IndexOp, IndexTarget, OpType};
