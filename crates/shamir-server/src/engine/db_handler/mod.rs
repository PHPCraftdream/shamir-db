mod config;
mod cursor_handlers;
#[path = "repo_admin/cursor_support.rs"]
mod cursor_support;
mod handler;
mod handlers;
#[path = "repo_admin/repo_audit.rs"]
mod repo_audit;
#[path = "repo_admin/tx_handlers.rs"]
mod tx_handlers;

#[cfg(test)]
mod tests;

// Module-name re-exports so `super::admin` / `super::subscribe_handler` /
// `super::visibility_handlers` / `super::repl_handler` keep resolving for
// sibling modules (`handler.rs`, `cursor_handlers.rs`, `repo_admin/*`)
// and for the tests under `tests/`.
pub(crate) use handlers::{admin, subscribe_handler, visibility_handlers};

pub use config::{CursorLimitsCap, NodeMode, QueryLimitsCap, SlowQueryConfig, TxLimitsCap};
pub use handler::{DbRequest, DbResponse, ShamirDbHandler};
pub(crate) use handlers::admin::derive_scram_record;
pub use handlers::admin::AdminGlue;
