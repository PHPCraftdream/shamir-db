pub mod net;
pub mod shamir_db;

// `db::engine` and `db::query` are now in the `shamir-engine` crate
// (engine + query share an internal cycle — table-manager evaluates
// query filters and builds query results — so they ship together).
// Re-exported here so existing `crate::db::engine::*` and
// `crate::db::query::*` paths keep resolving with no caller-side
// changes. There is no `db/engine/` or `db/query/` directory anymore.
pub use shamir_engine as engine;
pub use shamir_engine::query;

// `db::storage` lives in the `shamir-storage` crate.
pub use shamir_storage as storage;

// `ShamirDb` + `SystemStoreConfig` live in the local `shamir_db` *child
// module* (path: `crate::db::shamir_db`). The historical line
// `pub use shamir_db::{...}` worked in the monocrate where there was no
// outer crate of the same name; after the split it resolves to the
// outer crate root and finds nothing. `self::shamir_db::*` is the
// unambiguous form.
pub use self::shamir_db::{ShamirDb, SystemStoreConfig};

// Re-export error for convenience and backwards compatibility (it lives
// inside shamir-storage now).
pub use shamir_storage::error::{DbError, DbResult};
