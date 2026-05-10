//! Admin (DDL) operation DTOs.

pub mod types;

pub use types::{
    CreateDbOp, CreateIndexOp, CreateRepoOp, CreateTableOp, DropDbOp, DropIndexOp,
    DropRepoOp, DropTableOp, ListOp,
};
