pub mod engine;
pub mod net;
pub mod query;
pub mod shamir_db;

// `db::storage` is now a separate crate (`shamir-storage`). Re-exported
// here so existing `shamir_db::db::storage::storage_redb::*` paths keep
// resolving without any caller-side changes. The re-export is a pure
// pass-through — there is no `crates/shamir-db/src/db/storage/`
// directory anymore.
pub use shamir_storage as storage;

// Re-export error for convenience and backwards compatibility (it lives
// inside shamir-storage now).
pub use shamir_db::{ShamirDb, SystemStoreConfig};
pub use shamir_storage::error::{DbError, DbResult};
