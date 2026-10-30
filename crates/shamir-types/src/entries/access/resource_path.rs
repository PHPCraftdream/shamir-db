use std::fmt;

use crate::entries::resource_path_ref::ResourcePathRef;

/// Escape control characters at diagnostic boundaries without changing path identity.
pub fn escape_resource_diagnostic(value: &str) -> String {
    let mut escaped = String::new();
    for ch in value.chars() {
        if ch.is_control() {
            escaped.extend(ch.escape_default());
        } else {
            escaped.push(ch);
        }
    }
    escaped
}

/// Uniform, traversable address of a securable resource in the tree.
///
/// The tree (see `ACCESS_HIERARCHY.md`):
/// ```text
/// Root
/// ├── databases/<db>/<store>/<table>/{<record>, indexes/<index>}
/// ├── functions/<function>            (FunctionNamespace → Function)
/// ├── users/<user>
/// └── groups/<group>
/// ```
/// [`parent`](Self::parent) walks toward the root so the gate can require
/// traversal (`Execute`) on every ancestor container.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ResourcePath {
    /// The system root — the admin domain.
    Root,
    Database {
        db: String,
    },
    Store {
        db: String,
        store: String,
    },
    Table {
        db: String,
        store: String,
        table: String,
    },
    /// A single row. Leaf; inherits its table's owner/mode (row-level
    /// metadata only when row-level security is enabled).
    Record {
        db: String,
        store: String,
        table: String,
        key: String,
    },
    /// A secondary index — derived; inherits its table.
    Index {
        db: String,
        store: String,
        table: String,
        index: String,
    },
    /// The container under which user-defined functions are created.
    FunctionNamespace,
    /// Global singleton gating the "compile Rust source into WASM"
    /// capability (task #607) — separate from `FunctionNamespace`'s
    /// bare Create right. Mode-bearing, POSIX-style ("x" bit = may
    /// trigger host compilation), default `0o755` (mirrors `Root`'s
    /// default — see `resource_meta`'s `ResourcePath::Root` arm).
    WasmCompiler,
    /// A folder within the function namespace, addressed by path segments.
    ///
    /// For example, the folder `reports/daily` is represented as
    /// `FunctionFolder { path: vec!["reports".into(), "daily".into()] }`.
    /// A function `reports/daily/orders` lives *inside* this folder.
    FunctionFolder {
        path: Vec<String>,
    },
    Function {
        name: String,
    },
    User {
        name: String,
    },
    Group {
        name: String,
    },
}

impl ResourcePath {
    /// Construct a database path.
    pub fn database(db: impl Into<String>) -> Self {
        Self::Database { db: db.into() }
    }
    /// Construct a store path.
    pub fn store(db: impl Into<String>, store: impl Into<String>) -> Self {
        Self::Store {
            db: db.into(),
            store: store.into(),
        }
    }
    /// Construct a table path.
    pub fn table(
        db: impl Into<String>,
        store: impl Into<String>,
        table: impl Into<String>,
    ) -> Self {
        Self::Table {
            db: db.into(),
            store: store.into(),
            table: table.into(),
        }
    }
    /// Construct a record path.
    pub fn record(
        db: impl Into<String>,
        store: impl Into<String>,
        table: impl Into<String>,
        key: impl Into<String>,
    ) -> Self {
        Self::Record {
            db: db.into(),
            store: store.into(),
            table: table.into(),
            key: key.into(),
        }
    }
    /// Construct an index path.
    pub fn index(
        db: impl Into<String>,
        store: impl Into<String>,
        table: impl Into<String>,
        index: impl Into<String>,
    ) -> Self {
        Self::Index {
            db: db.into(),
            store: store.into(),
            table: table.into(),
            index: index.into(),
        }
    }
    /// Construct a function path.
    pub fn function(name: impl Into<String>) -> Self {
        Self::Function { name: name.into() }
    }
    /// Construct a function-folder path from path segments. An empty vector
    /// denotes the namespace root; prefer `FunctionNamespace` directly.
    pub fn function_folder(segments: Vec<String>) -> Self {
        Self::FunctionFolder { path: segments }
    }
    /// Construct a user path.
    pub fn user(name: impl Into<String>) -> Self {
        Self::User { name: name.into() }
    }
    /// Construct a group path.
    pub fn group(name: impl Into<String>) -> Self {
        Self::Group { name: name.into() }
    }

    /// Borrow this address without allocating.
    pub fn as_ref(&self) -> ResourcePathRef<'_> {
        match self {
            Self::Root => ResourcePathRef::Root,
            Self::Database { db } => ResourcePathRef::Database { db },
            Self::Store { db, store } => ResourcePathRef::Store { db, store },
            Self::Table { db, store, table } => ResourcePathRef::Table { db, store, table },
            Self::Record {
                db,
                store,
                table,
                key,
            } => ResourcePathRef::Record {
                db,
                store,
                table,
                key,
            },
            Self::Index {
                db,
                store,
                table,
                index,
            } => ResourcePathRef::Index {
                db,
                store,
                table,
                index,
            },
            Self::FunctionNamespace => ResourcePathRef::FunctionNamespace,
            Self::WasmCompiler => ResourcePathRef::WasmCompiler,
            Self::FunctionFolder { path } => ResourcePathRef::FunctionFolder { path },
            Self::Function { name } => ResourcePathRef::Function { name },
            Self::User { name } => ResourcePathRef::User { name },
            Self::Group { name } => ResourcePathRef::Group { name },
        }
    }

    /// The containing resource, or `None` for the root.
    pub fn parent(&self) -> Option<Self> {
        self.as_ref().parent().map(ResourcePathRef::to_owned)
    }

    /// Ancestor containers, nearest first, up to and including `Root`.
    pub fn ancestors(&self) -> Vec<Self> {
        self.as_ref()
            .ancestors()
            .map(ResourcePathRef::to_owned)
            .collect()
    }
}

impl fmt::Display for ResourcePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Root => f.write_str("/"),
            Self::Database { db } => write!(f, "db://{db}"),
            Self::Store { db, store } => write!(f, "db://{db}/{store}"),
            Self::Table { db, store, table } => write!(f, "db://{db}/{store}/{table}"),
            Self::Record {
                db,
                store,
                table,
                key,
            } => write!(f, "db://{db}/{store}/{table}#{key}"),
            Self::Index {
                db,
                store,
                table,
                index,
            } => write!(f, "db://{db}/{store}/{table}.idx/{index}"),
            Self::FunctionNamespace => f.write_str("fn://"),
            Self::WasmCompiler => f.write_str("compiler://"),
            Self::FunctionFolder { path } => write!(f, "fn://{}/", path.join("/")),
            Self::Function { name } => write!(f, "fn://{name}"),
            Self::User { name } => write!(f, "user://{name}"),
            Self::Group { name } => write!(f, "group://{name}"),
        }
    }
}
