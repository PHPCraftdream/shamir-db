//! Admin (DDL) operation types.

use serde::{Deserialize, Serialize};

/// Create a new database.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateDbOp {
    pub create_db: String,
}

/// Drop a database.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropDbOp {
    pub drop_db: String,
}

/// Create a new repository within the current database.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateRepoOp {
    pub create_repo: String,
    #[serde(default = "default_engine")]
    pub engine: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tables: Vec<String>,
}

fn default_engine() -> String {
    "in_memory".to_string()
}

/// Drop a repository.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropRepoOp {
    pub drop_repo: String,
}

/// Create a table in a repository.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateTableOp {
    pub create_table: String,
    #[serde(default = "default_repo")]
    pub repo: String,
}

fn default_repo() -> String {
    "main".to_string()
}

/// Drop a table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropTableOp {
    pub drop_table: String,
    #[serde(default = "default_repo")]
    pub repo: String,
}

/// Create an index on a table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateIndexOp {
    pub create_index: String,
    pub table: String,
    pub fields: Vec<Vec<String>>,
    #[serde(default)]
    pub unique: bool,
    #[serde(default = "default_repo")]
    pub repo: String,
}

/// Drop an index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropIndexOp {
    pub drop_index: String,
    pub table: String,
    #[serde(default)]
    pub unique: bool,
    #[serde(default = "default_repo")]
    pub repo: String,
}

/// List databases / repos / tables / indexes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "list")]
pub enum ListOp {
    #[serde(rename = "databases")]
    Databases,
    #[serde(rename = "repos")]
    Repos,
    #[serde(rename = "tables")]
    Tables {
        #[serde(default = "default_repo")]
        repo: String,
    },
    #[serde(rename = "indexes")]
    Indexes {
        table: String,
        #[serde(default = "default_repo")]
        repo: String,
    },
    #[serde(rename = "users")]
    Users,
    #[serde(rename = "roles")]
    Roles,
}
