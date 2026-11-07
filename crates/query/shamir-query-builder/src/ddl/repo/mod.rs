//! Repository DDL builders, resource-reference helpers, and the string
//! interner used by repo DDL.

pub mod alter_repo;
#[path = "alter_repo/create_repo.rs"]
pub mod create_repo;
pub mod drop_repo;
pub mod interner;
pub mod interner_resolve;
pub mod rename_repo;
pub mod res;
