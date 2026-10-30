use std::fmt;
use std::hash::{Hash, Hasher};

use self::ResourcePathRef::*;
use super::access::ResourcePath;

/// A resource address borrowing its components from an existing path or name.
/// Folder prefixes represent the segments produced by splitting on `/`, including
/// empty segments; they do not represent an empty segment slice.
#[derive(Clone, Copy, Debug)]
pub enum ResourcePathRef<'a> {
    Root,
    Database {
        db: &'a str,
    },
    Store {
        db: &'a str,
        store: &'a str,
    },
    Table {
        db: &'a str,
        store: &'a str,
        table: &'a str,
    },
    Record {
        db: &'a str,
        store: &'a str,
        table: &'a str,
        key: &'a str,
    },
    Index {
        db: &'a str,
        store: &'a str,
        table: &'a str,
        index: &'a str,
    },
    FunctionNamespace,
    WasmCompiler,
    FunctionFolder {
        path: &'a [String],
    },
    FunctionFolderPrefix {
        prefix: &'a str,
    },
    Function {
        name: &'a str,
    },
    User {
        name: &'a str,
    },
    Group {
        name: &'a str,
    },
}

impl<'a> ResourcePathRef<'a> {
    /// Materialize this borrowed address, preserving the original path identity.
    pub fn to_owned(self) -> ResourcePath {
        match self {
            Self::Root => ResourcePath::Root,
            Self::Database { db } => ResourcePath::database(db),
            Self::Store { db, store } => ResourcePath::store(db, store),
            Self::Table { db, store, table } => ResourcePath::table(db, store, table),
            Self::Record {
                db,
                store,
                table,
                key,
            } => ResourcePath::record(db, store, table, key),
            Self::Index {
                db,
                store,
                table,
                index,
            } => ResourcePath::index(db, store, table, index),
            Self::FunctionNamespace => ResourcePath::FunctionNamespace,
            Self::WasmCompiler => ResourcePath::WasmCompiler,
            Self::FunctionFolder { path } => ResourcePath::function_folder(path.to_vec()),
            Self::FunctionFolderPrefix { prefix } => {
                ResourcePath::function_folder(prefix.split('/').map(str::to_owned).collect())
            }
            Self::Function { name } => ResourcePath::function(name),
            Self::User { name } => ResourcePath::user(name),
            Self::Group { name } => ResourcePath::group(name),
        }
    }

    /// Return the containing address, borrowing the same input as this view.
    pub fn parent(self) -> Option<Self> {
        match self {
            Self::Root => None,
            Self::Database { .. }
            | Self::FunctionNamespace
            | Self::WasmCompiler
            | Self::User { .. }
            | Self::Group { .. } => Some(Self::Root),
            Self::Store { db, .. } => Some(Self::Database { db }),
            Self::Table { db, store, .. } => Some(Self::Store { db, store }),
            Self::Record {
                db, store, table, ..
            }
            | Self::Index {
                db, store, table, ..
            } => Some(Self::Table { db, store, table }),
            Self::FunctionFolder { path } => {
                if path.len() > 1 {
                    Some(Self::FunctionFolder {
                        path: &path[..path.len() - 1],
                    })
                } else {
                    Some(Self::FunctionNamespace)
                }
            }
            Self::FunctionFolderPrefix { prefix } => match prefix.rfind('/') {
                Some(pos) => Some(Self::FunctionFolderPrefix {
                    prefix: &prefix[..pos],
                }),
                None => Some(Self::FunctionNamespace),
            },
            Self::Function { name } => match name.rfind('/') {
                Some(pos) => Some(Self::FunctionFolderPrefix {
                    prefix: &name[..pos],
                }),
                None => Some(Self::FunctionNamespace),
            },
        }
    }

    /// Iterate ancestors nearest first without allocating; excludes this path.
    pub fn ancestors(self) -> ResourcePathAncestors<'a> {
        ResourcePathAncestors {
            next: self.parent(),
        }
    }
}

/// Allocation-free nearest-first traversal of borrowed resource ancestors.
#[derive(Clone, Debug)]
pub struct ResourcePathAncestors<'a> {
    next: Option<ResourcePathRef<'a>>,
}

impl<'a> Iterator for ResourcePathAncestors<'a> {
    type Item = ResourcePathRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.next?;
        self.next = current.parent();
        Some(current)
    }
}

impl PartialEq for ResourcePathRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (*self, *other) {
            (Root, Root)
            | (FunctionNamespace, FunctionNamespace)
            | (WasmCompiler, WasmCompiler) => true,
            (Database { db: a }, Database { db: b }) => a == b,
            (Store { db: a, store: b }, Store { db: c, store: d }) => (a, b) == (c, d),
            (
                Table {
                    db: a,
                    store: b,
                    table: c,
                },
                Table {
                    db: d,
                    store: e,
                    table: f,
                },
            ) => (a, b, c) == (d, e, f),
            (
                Record {
                    db: a,
                    store: b,
                    table: c,
                    key: d,
                },
                Record {
                    db: e,
                    store: f,
                    table: g,
                    key: h,
                },
            )
            | (
                Index {
                    db: a,
                    store: b,
                    table: c,
                    index: d,
                },
                Index {
                    db: e,
                    store: f,
                    table: g,
                    index: h,
                },
            ) => (a, b, c, d) == (e, f, g, h),
            (FunctionFolder { path: a }, FunctionFolder { path: b }) => a == b,
            (FunctionFolderPrefix { prefix: a }, FunctionFolderPrefix { prefix: b }) => a == b,
            (FunctionFolder { path }, FunctionFolderPrefix { prefix })
            | (FunctionFolderPrefix { prefix }, FunctionFolder { path }) => {
                path.iter().map(String::as_str).eq(prefix.split('/'))
            }
            (Function { name: a }, Function { name: b })
            | (User { name: a }, User { name: b })
            | (Group { name: a }, Group { name: b }) => a == b,
            _ => false,
        }
    }
}

impl Eq for ResourcePathRef<'_> {}

impl Hash for ResourcePathRef<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match *self {
            Root => 0u8.hash(state),
            Database { db } => {
                1u8.hash(state);
                db.hash(state);
            }
            Store { db, store } => {
                2u8.hash(state);
                (db, store).hash(state);
            }
            Table { db, store, table } => {
                3u8.hash(state);
                (db, store, table).hash(state);
            }
            Record {
                db,
                store,
                table,
                key,
            } => {
                4u8.hash(state);
                (db, store, table, key).hash(state);
            }
            Index {
                db,
                store,
                table,
                index,
            } => {
                5u8.hash(state);
                (db, store, table, index).hash(state);
            }
            FunctionNamespace => 6u8.hash(state),
            WasmCompiler => 7u8.hash(state),
            FunctionFolder { path } => {
                8u8.hash(state);
                for part in path {
                    part.hash(state);
                }
                path.len().hash(state);
            }
            FunctionFolderPrefix { prefix } => {
                8u8.hash(state);
                let mut count = 0usize;
                for part in prefix.split('/') {
                    part.hash(state);
                    count += 1;
                }
                count.hash(state);
            }
            Function { name } => {
                9u8.hash(state);
                name.hash(state);
            }
            User { name } => {
                10u8.hash(state);
                name.hash(state);
            }
            Group { name } => {
                11u8.hash(state);
                name.hash(state);
            }
        }
    }
}

impl fmt::Display for ResourcePathRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Root => f.write_str("/"),
            Database { db } => write!(f, "db://{db}"),
            Store { db, store } => write!(f, "db://{db}/{store}"),
            Table { db, store, table } => write!(f, "db://{db}/{store}/{table}"),
            Record {
                db,
                store,
                table,
                key,
            } => write!(f, "db://{db}/{store}/{table}#{key}"),
            Index {
                db,
                store,
                table,
                index,
            } => write!(f, "db://{db}/{store}/{table}.idx/{index}"),
            FunctionNamespace => f.write_str("fn://"),
            WasmCompiler => f.write_str("compiler://"),
            FunctionFolder { path } => {
                f.write_str("fn://")?;
                for (i, segment) in path.iter().enumerate() {
                    if i != 0 {
                        f.write_str("/")?;
                    }
                    f.write_str(segment)?;
                }
                f.write_str("/")
            }
            FunctionFolderPrefix { prefix } => write!(f, "fn://{prefix}/"),
            Function { name } => write!(f, "fn://{name}"),
            User { name } => write!(f, "user://{name}"),
            Group { name } => write!(f, "group://{name}"),
        }
    }
}
