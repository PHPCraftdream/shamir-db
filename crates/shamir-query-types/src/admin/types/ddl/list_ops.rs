//! List operation — enumerate databases, repos, tables, indexes, functions, validators, etc.

use serde::{Deserialize, Serialize};

fn default_repo() -> String {
    "main".to_string()
}

/// List databases / repos / tables / indexes / functions / validators / function_folders.
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
    /// List all registered functions. Optionally filter by folder prefix.
    #[serde(rename = "functions")]
    Functions {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        folder: Option<String>,
    },
    /// List all registered validators (id + name + bound tables).
    #[serde(rename = "validators")]
    Validators,
    /// List explicitly created function folders. Optionally filter by parent.
    #[serde(rename = "function_folders")]
    FunctionFolders {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
    },
}
