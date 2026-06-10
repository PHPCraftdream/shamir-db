//! Write-operation builders: [`Doc`], [`Insert`], [`Update`], [`Upsert`],
//! [`Delete`].
//!
//! Each builder produces exactly the corresponding wire DTO from
//! `shamir_query_types::write` — no parallel model, no extra
//! serialization layer.
//!
//! # `Doc` — record-value builder
//!
//! A write record is a JSON object whose field values are **either**
//! literal JSON **or** expressions (computed `{"$fn":...}`,
//! `{"$ref":...}`, `{"$query":...}`). Expressions are produced by
//! serializing a [`FilterValue`] to JSON.
//!
//! ```ignore
//! use shamir_query_builder::{write::doc, val::*};
//!
//! let d = doc()
//!     .set("email", "Alice@X.COM")
//!     .set("email_norm", func("strings/lower", [col("email")]));
//! ```
//!
//! # Op builders
//!
//! ```ignore
//! use shamir_query_builder::write::*;
//! use shamir_query_builder::{val::*, filter::*};
//!
//! // Insert
//! let ins = insert("users")
//!     .row(doc().set("name", "Alice"))
//!     .build();
//!
//! // Update
//! let upd = update("users")
//!     .where_(eq("id", 1))
//!     .set(doc().set("name", "Bob"))
//!     .returning(UpdateReturnMode::All)
//!     .build();
//!
//! // Upsert (SetOp)
//! let ups = upsert("cache")
//!     .key(serde_json::json!("k1"))
//!     .value(doc().set("v", 42))
//!     .build();
//!
//! // Delete
//! let del = delete("sessions")
//!     .where_(eq("expired", true))
//!     .build();
//! ```

mod delete;
mod doc;
mod insert;
mod update;
mod upsert;

pub use delete::*;
pub use doc::*;
pub use insert::*;
pub use update::*;
pub use upsert::*;

// Re-export `UpdateReturnMode` for ergonomic `use write::*` imports.
pub use shamir_query_types::write::UpdateReturnMode;

#[cfg(test)]
mod tests;
