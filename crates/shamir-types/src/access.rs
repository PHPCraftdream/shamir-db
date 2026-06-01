//! Shomer access-control primitive types.
//!
//! These types model *who* is acting ([`Actor`]), *what* they target
//! ([`ResourcePath`]), and *how* ([`Action`]). The [`authorize`] gate is
//! transparent during the pure-refactoring track (R1–R3); P4 seats the
//! real POSIX-style check here later.
//!
//! The full object & operation hierarchy is specified in
//! `docs/roadmap/ACCESS_HIERARCHY.md`.

use std::fmt;

/// The identity performing an operation.
///
/// `System` is the all-bypassing default used while the authentication
/// wire path is not yet plumbed through.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Actor {
    #[default]
    System,
    User(u64),
}

impl fmt::Display for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Actor::System => f.write_str("System"),
            Actor::User(id) => write!(f, "User({id})"),
        }
    }
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
        ResourcePath::Database { db: db.into() }
    }
    /// Construct a store path.
    pub fn store(db: impl Into<String>, store: impl Into<String>) -> Self {
        ResourcePath::Store {
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
        ResourcePath::Table {
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
        ResourcePath::Record {
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
        ResourcePath::Index {
            db: db.into(),
            store: store.into(),
            table: table.into(),
            index: index.into(),
        }
    }
    /// Construct a function path.
    pub fn function(name: impl Into<String>) -> Self {
        ResourcePath::Function { name: name.into() }
    }
    /// Construct a user path.
    pub fn user(name: impl Into<String>) -> Self {
        ResourcePath::User { name: name.into() }
    }
    /// Construct a group path.
    pub fn group(name: impl Into<String>) -> Self {
        ResourcePath::Group { name: name.into() }
    }

    /// The containing resource, or `None` for the root.
    ///
    /// Record/Index resolve to their Table (inheritance); the top-level
    /// containers (Database, FunctionNamespace, User, Group) resolve to Root.
    pub fn parent(&self) -> Option<ResourcePath> {
        match self {
            ResourcePath::Root => None,
            ResourcePath::Database { .. } => Some(ResourcePath::Root),
            ResourcePath::Store { db, .. } => Some(ResourcePath::database(db.clone())),
            ResourcePath::Table { db, store, .. } => {
                Some(ResourcePath::store(db.clone(), store.clone()))
            }
            ResourcePath::Record {
                db, store, table, ..
            }
            | ResourcePath::Index {
                db, store, table, ..
            } => Some(ResourcePath::table(
                db.clone(),
                store.clone(),
                table.clone(),
            )),
            ResourcePath::FunctionNamespace => Some(ResourcePath::Root),
            ResourcePath::Function { .. } => Some(ResourcePath::FunctionNamespace),
            ResourcePath::User { .. } | ResourcePath::Group { .. } => Some(ResourcePath::Root),
        }
    }

    /// Ancestor containers, nearest first, up to and including `Root`
    /// (excludes `self`). The gate requires `Execute` (traverse) on each.
    pub fn ancestors(&self) -> Vec<ResourcePath> {
        let mut out = Vec::new();
        let mut cur = self.parent();
        while let Some(p) = cur {
            cur = p.parent();
            out.push(p);
        }
        out
    }
}

impl fmt::Display for ResourcePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResourcePath::Root => f.write_str("/"),
            ResourcePath::Database { db } => write!(f, "db://{db}"),
            ResourcePath::Store { db, store } => write!(f, "db://{db}/{store}"),
            ResourcePath::Table { db, store, table } => write!(f, "db://{db}/{store}/{table}"),
            ResourcePath::Record {
                db,
                store,
                table,
                key,
            } => write!(f, "db://{db}/{store}/{table}#{key}"),
            ResourcePath::Index {
                db,
                store,
                table,
                index,
            } => write!(f, "db://{db}/{store}/{table}.idx/{index}"),
            ResourcePath::FunctionNamespace => f.write_str("fn://"),
            ResourcePath::Function { name } => write!(f, "fn://{name}"),
            ResourcePath::User { name } => write!(f, "user://{name}"),
            ResourcePath::Group { name } => write!(f, "group://{name}"),
        }
    }
}

/// The class of operation being performed on a resource.
///
/// POSIX-flavoured: `Read`/`Write`/`Execute` map to `r`/`w`/`x`; `Create`
/// and `Delete` are writes on a container; `List` is read on a container;
/// `Manage` is the owner/admin-only class (chmod/chown/chgrp/grant).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Read,
    Write,
    Create,
    Delete,
    Execute,
    List,
    Manage,
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Action::Read => "READ",
            Action::Write => "WRITE",
            Action::Create => "CREATE",
            Action::Delete => "DELETE",
            Action::Execute => "EXECUTE",
            Action::List => "LIST",
            Action::Manage => "MANAGE",
        })
    }
}

/// Access denied (constructed by the real policy check in P4).
#[derive(Debug, thiserror::Error)]
#[error("access denied: {actor} cannot {action} on {path}")]
pub struct AccessError {
    pub actor: Actor,
    /// The rendered resource path (kept as a `String`, not the full
    /// `ResourcePath` enum, so the error stays small — `Result<_, AccessError>`
    /// is on the hot path and `clippy::result_large_err` would fire otherwise).
    pub path: String,
    pub action: Action,
}

/// Transparent authorization gate.
///
/// Always returns `Ok(())` and emits a `log::trace!` access line. The real
/// POSIX-style check (the object × operation matrix in
/// `ACCESS_HIERARCHY.md`) will be seated here in P4.
pub fn authorize(actor: &Actor, path: &ResourcePath, action: Action) -> Result<(), AccessError> {
    log::trace!("shomer: {actor} {action} on {path}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_is_default() {
        assert_eq!(Actor::default(), Actor::System);
    }

    #[test]
    fn authorize_transparent_for_all_variants() {
        for path in [
            ResourcePath::Root,
            ResourcePath::database("d"),
            ResourcePath::store("d", "s"),
            ResourcePath::table("d", "s", "t"),
            ResourcePath::record("d", "s", "t", "k"),
            ResourcePath::index("d", "s", "t", "i"),
            ResourcePath::FunctionNamespace,
            ResourcePath::function("f"),
            ResourcePath::user("u"),
            ResourcePath::group("g"),
        ] {
            for action in [
                Action::Read,
                Action::Write,
                Action::Create,
                Action::Delete,
                Action::Execute,
                Action::List,
                Action::Manage,
            ] {
                assert!(authorize(&Actor::System, &path, action).is_ok());
            }
        }
    }

    #[test]
    fn parent_walks_to_root() {
        // record → table → store → database → root → None
        let rec = ResourcePath::record("d", "s", "t", "k");
        let table = rec.parent().unwrap();
        assert_eq!(table, ResourcePath::table("d", "s", "t"));
        let store = table.parent().unwrap();
        assert_eq!(store, ResourcePath::store("d", "s"));
        let db = store.parent().unwrap();
        assert_eq!(db, ResourcePath::database("d"));
        let root = db.parent().unwrap();
        assert_eq!(root, ResourcePath::Root);
        assert_eq!(root.parent(), None);
    }

    #[test]
    fn index_inherits_table_as_parent() {
        let idx = ResourcePath::index("d", "s", "t", "i");
        assert_eq!(idx.parent().unwrap(), ResourcePath::table("d", "s", "t"));
    }

    #[test]
    fn function_parent_is_namespace() {
        assert_eq!(
            ResourcePath::function("f").parent().unwrap(),
            ResourcePath::FunctionNamespace
        );
        assert_eq!(
            ResourcePath::FunctionNamespace.parent().unwrap(),
            ResourcePath::Root
        );
    }

    #[test]
    fn ancestors_nearest_first_to_root() {
        let rec = ResourcePath::record("d", "s", "t", "k");
        assert_eq!(
            rec.ancestors(),
            vec![
                ResourcePath::table("d", "s", "t"),
                ResourcePath::store("d", "s"),
                ResourcePath::database("d"),
                ResourcePath::Root,
            ]
        );
    }

    #[test]
    fn user_and_group_under_root() {
        assert_eq!(
            ResourcePath::user("u").parent().unwrap(),
            ResourcePath::Root
        );
        assert_eq!(
            ResourcePath::group("g").parent().unwrap(),
            ResourcePath::Root
        );
    }
}
