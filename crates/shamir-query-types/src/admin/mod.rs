//! Admin (DDL) operation DTOs.

pub mod types;

pub use types::{
    AlterBufferConfigOp, BufferConfigDto, BufferConfigPatch, CreateDbOp, CreateIndexOp,
    CreateRepoOp, CreateTableOp, DropDbOp, DropIndexOp, DropRepoOp, DropTableOp,
    GetBufferConfigOp, ListOp, SetBufferConfigOp,
};
