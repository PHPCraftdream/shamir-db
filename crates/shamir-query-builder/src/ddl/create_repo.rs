use shamir_query_types::admin::CreateRepoOp;
use shamir_query_types::batch::BatchOp;

use crate::batch::IntoBatchOp;

/// Typed storage-engine selector for [`create_repo`].
///
/// The server (see `handle_create_repo`'s engine match arms in
/// `shamir-db`'s `admin_db_repo.rs`) recognises exactly three engine
/// strings — [`InMemory`](RepoEngine::InMemory),
/// [`Fjall`](RepoEngine::Fjall) (the default when `engine` is unset), and
/// [`Hybrid`](RepoEngine::Hybrid) — and rejects anything else. Pass this enum
/// to [`CreateRepo::engine`] instead of a raw string to get a compile-checked
/// engine choice while staying wire-compatible: [`RepoEngine`] converts into
/// the same `String` the existing `impl Into<String>` bound already accepts, so
/// every existing caller passing a plain `&str` keeps working unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoEngine {
    /// Fully ephemeral — table data and the table-config mirror live in
    /// memory only and never touch disk. Wire string: `"in_memory"`.
    InMemory,
    /// Durable default — table data and config persisted to a fjall directory
    /// under `data_root/<db>/<repo>` (falls back to in-memory when the home has
    /// no `data_root`). Wire string: `"fjall"`.
    Fjall,
    /// Hybrid — table DATA is ephemeral in-memory, but the table-config mirror
    /// (`__info__`/`__interner__`) is durably persisted to fjall so indexes and
    /// schema validators survive a restart. Requires a `data_root`. Wire string:
    /// `"hybrid"`.
    Hybrid,
    /// Escape hatch for an engine value not yet known to this builder version
    /// (forward-compat with a server that supports more engines than this
    /// client library does). The wrapped string is sent on the wire verbatim;
    /// the server rejects it if it is not a real engine.
    Other(String),
}

impl From<RepoEngine> for String {
    fn from(engine: RepoEngine) -> Self {
        match engine {
            RepoEngine::InMemory => "in_memory".to_string(),
            RepoEngine::Fjall => "fjall".to_string(),
            RepoEngine::Hybrid => "hybrid".to_string(),
            RepoEngine::Other(s) => s,
        }
    }
}

/// Create a new repository. Returns a builder for optional fields.
pub fn create_repo(name: impl Into<String>) -> CreateRepo {
    CreateRepo {
        name: name.into(),
        engine: None,
        path: None,
        tables: Vec::new(),
        if_not_exists: false,
    }
}

/// Builder for [`CreateRepoOp`].
pub struct CreateRepo {
    name: String,
    engine: Option<String>,
    path: Option<String>,
    tables: Vec<String>,
    if_not_exists: bool,
}

impl CreateRepo {
    /// Set the storage engine (e.g. [`RepoEngine::InMemory`],
    /// [`RepoEngine::Fjall`], [`RepoEngine::Hybrid`], or a raw string like
    /// `"in_memory"`, `"fjall"`, `"hybrid"`). Accepts any `impl Into<String>`,
    /// so both a plain `&str` and a typed [`RepoEngine`] value work. The server
    /// resolves the on-disk repo directory itself; this only selects the engine
    /// kind.
    pub fn engine(mut self, engine: impl Into<String>) -> Self {
        self.engine = Some(engine.into());
        self
    }

    /// Set the data path.
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Pre-create these tables inside the repo.
    pub fn tables(mut self, tables: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.tables = tables.into_iter().map(Into::into).collect();
        self
    }

    /// Skip error if the repo already exists.
    pub fn if_not_exists(mut self) -> Self {
        self.if_not_exists = true;
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::CreateRepo(CreateRepoOp {
            create_repo: self.name,
            engine: self.engine,
            path: self.path,
            tables: self.tables,
            if_not_exists: self.if_not_exists,
        })
    }
}

impl From<CreateRepo> for BatchOp {
    fn from(b: CreateRepo) -> Self {
        b.build()
    }
}

impl IntoBatchOp for CreateRepo {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
