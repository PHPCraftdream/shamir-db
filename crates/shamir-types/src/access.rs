//! Shomer access-control primitive types.
//!
//! These types model *who* is acting (`Actor`), *what* they target
//! (`ResourcePath`), and *how* (`Action`). The [`authorize`] gate is
//! transparent during the pure-refactoring track (R1–R3); P4 seats the
//! real POSIX check here later.

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

/// Uniform address of a securable resource in the database tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourcePath {
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
    Function {
        name: String,
    },
    /// The "directory" under which user-defined functions are created.
    FunctionNamespace,
}

impl fmt::Display for ResourcePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResourcePath::Database { db } => write!(f, "db://{db}"),
            ResourcePath::Store { db, store } => write!(f, "db://{db}/{store}"),
            ResourcePath::Table { db, store, table } => write!(f, "db://{db}/{store}/{table}"),
            ResourcePath::Function { name } => write!(f, "fn://{name}"),
            ResourcePath::FunctionNamespace => f.write_str("fn://"),
        }
    }
}

/// The class of operation being performed on a resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Read,
    Write,
    Execute,
    Create,
    Delete,
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Action::Read => "READ",
            Action::Write => "WRITE",
            Action::Execute => "EXECUTE",
            Action::Create => "CREATE",
            Action::Delete => "DELETE",
        })
    }
}

/// Access denied (will be constructed by the real policy check in P4).
#[derive(Debug, thiserror::Error)]
#[error("access denied: {actor} cannot {action} on {path}")]
pub struct AccessError {
    pub actor: Actor,
    pub path: ResourcePath,
    pub action: Action,
}

/// Transparent authorization gate.
///
/// Always returns `Ok(())` and emits a `log::trace!` access line.
/// The real POSIX-style check will be seated here in P4.
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
    fn authorize_system_function_execute_ok() {
        let result = authorize(
            &Actor::System,
            &ResourcePath::Function {
                name: "test_fn".into(),
            },
            Action::Execute,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn authorize_system_database_read_ok() {
        let result = authorize(
            &Actor::System,
            &ResourcePath::Database {
                db: "test_db".into(),
            },
            Action::Read,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn authorize_system_function_namespace_create_ok() {
        let result = authorize(
            &Actor::System,
            &ResourcePath::FunctionNamespace,
            Action::Create,
        );
        assert!(result.is_ok());
    }
}
