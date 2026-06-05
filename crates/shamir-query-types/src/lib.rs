//! Pure DTOs for ShamirDB's SDBQL query language.
//!
//! Holds the **data shapes** (Filter, ReadQuery, BatchRequest etc.) that
//! both the engine layer (which consumes a Filter to scan a table) and
//! the query layer (which compiles, plans, executes) share. Logic —
//! `compile_filter`, `eval`, `exec`, `BatchPlanner`, `executor`,
//! `parser`, `SessionPermissions::check_batch` — lives in
//! `shamir-engine` (today; potentially a future `shamir-query` crate).
//!
//! # Why a separate crate
//!
//! Engine code needs Filter + ReadQuery + QueryResult shapes to receive
//! and return them via TableManager APIs. The same shapes are produced
//! by query code (planner / parser / executor). Putting them in a
//! shared crate prevents the engine from having to depend on the full
//! query stack and lets external consumers (e.g. shamir-server's wire
//! handler) deserialise BatchRequest without pulling the whole engine.
//!
//! # What's NOT here
//!
//! * `FilterContext` — runtime; holds Interner refs.
//! * `FilterCallback` / `CompiledFilter` — runtime closure types.
//! * `compile_filter`, `intern_field_path`, `filter_value_to_inner` —
//!   functions that touch interner state.
//! * `read::exec`, `read::parser` — execution / parsing logic.
//! * `batch::executor`, `batch::planner`, `batch::reference` — batch
//!   planning + execution logic.
//! * `auth::session::SessionPermissions::check_batch` — permission logic.
//! * `common::filter_from_value` — JSON → Filter parser.
//! * Note: `batch::planner` and `batch::reference` were lifted in here
//!   from `shamir-engine` once it became clear they only consume DTOs.
//!   `batch::executor` still lives in the engine since it drives a
//!   `TableManager`.

pub mod admin;
pub mod auth;
pub mod batch;
pub mod call;
pub mod filter;
pub mod hmac;
pub mod read;
pub mod table_ref;
pub mod validator;
pub mod wire;
pub mod write;

pub use call::CallOp;
pub use table_ref::TableRef;
pub use validator::{ValidationError, WriteOp};
