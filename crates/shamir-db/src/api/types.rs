#![allow(deprecated)]

use crate::types::value::UserValue;
use serde::{Deserialize, Serialize};

/// Compatibility-only DTOs retained for source compatibility.
///
/// This legacy API is not used by the production client/server transport;
/// new integrations should use the query and wire types instead.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub enum Command {
    /// Inserts or updates a value for a given key.
    Put { key: String, value: UserValue },

    /// Retrieves a value for a given key.
    Get { key: String },

    /// Deletes a key-value pair.
    Del { key: String },

    /// Executes a WASM function. (For future use)
    Execute { func: String, args: Vec<UserValue> },
}

/// Compatibility-only request DTO; no production transport sends this type.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Request {
    /// A unique ID to correlate requests with responses.
    pub request_id: u64,
    /// The command to be executed.
    pub command: Command,
}

/// Compatibility-only response DTO; no production transport sends this type.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Response {
    /// The ID of the request this response corresponds to.
    pub request_id: u64,
    /// The result of the operation.
    /// Ok(Some(value)) for successful Get.
    /// Ok(None) for successful Put, Del, or Get on a non-existent key.
    /// Err(message) for any failure.
    pub result: Result<Option<UserValue>, String>,
}
