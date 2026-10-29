//! Table reference with optional repository qualifier.
//!
//! Supports three input wire formats:
//! - `"users"` → repo="main", table="users"
//! - `["hot", "sessions"]` → repo="hot", table="sessions"
//! - `{ "repo": "hot", "table": "sessions" }` → explicit map (repo optional)

use serde::de;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

const DEFAULT_REPO: &str = "main";

/// Reference to a table within a repository.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TableRef {
    pub repo: String,
    pub table: String,
}

impl TableRef {
    pub fn new(table: impl Into<String>) -> Self {
        Self {
            repo: DEFAULT_REPO.to_string(),
            table: table.into(),
        }
    }

    pub fn with_repo(repo: impl Into<String>, table: impl Into<String>) -> Self {
        Self {
            repo: repo.into(),
            table: table.into(),
        }
    }
}

impl<S: Into<String>> From<S> for TableRef {
    fn from(table: S) -> Self {
        Self::new(table)
    }
}

impl Serialize for TableRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.repo == DEFAULT_REPO {
            self.table.serialize(serializer)
        } else {
            (&self.repo, &self.table).serialize(serializer)
        }
    }
}

impl<'de> Deserialize<'de> for TableRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TableRefVisitor;

        impl<'de> de::Visitor<'de> for TableRefVisitor {
            type Value = TableRef;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a table string, [repo, table] array, or {repo, table} map")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<TableRef, E> {
                Ok(TableRef::new(v))
            }

            fn visit_string<E: de::Error>(self, v: String) -> Result<TableRef, E> {
                Ok(TableRef::new(v))
            }

            fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<TableRef, A::Error> {
                let mut repo = None;
                let mut table = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "repo" => {
                            if repo.is_some() {
                                return Err(de::Error::duplicate_field("repo"));
                            }
                            repo = Some(map.next_value::<String>()?);
                        }
                        "table" => {
                            if table.is_some() {
                                return Err(de::Error::duplicate_field("table"));
                            }
                            table = Some(map.next_value::<String>()?);
                        }
                        _ => return Err(de::Error::unknown_field(&key, &["repo", "table"])),
                    }
                }
                let table = table.ok_or_else(|| de::Error::missing_field("table"))?;
                Ok(TableRef::with_repo(
                    repo.unwrap_or_else(|| DEFAULT_REPO.to_string()),
                    table,
                ))
            }

            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<TableRef, A::Error> {
                let repo: String = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &"2"))?;
                let table: String = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(1, &"2"))?;
                Ok(TableRef::with_repo(repo, table))
            }
        }

        deserializer.deserialize_any(TableRefVisitor)
    }
}

impl std::fmt::Display for TableRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.repo == DEFAULT_REPO {
            write!(f, "{}", self.table)
        } else {
            write!(f, "{}.{}", self.repo, self.table)
        }
    }
}
