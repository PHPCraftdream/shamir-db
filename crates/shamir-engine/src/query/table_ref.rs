//! Table reference with optional repository qualifier.
//!
//! Supports two JSON formats:
//! - `"users"` → repo="main", table="users"
//! - `["hot", "sessions"]` → repo="hot", table="sessions"

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
        use serde::de;

        struct TableRefVisitor;

        impl<'de> de::Visitor<'de> for TableRefVisitor {
            type Value = TableRef;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a string \"table\" or array [\"repo\", \"table\"]")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<TableRef, E> {
                Ok(TableRef::new(v))
            }

            fn visit_string<E: de::Error>(self, v: String) -> Result<TableRef, E> {
                Ok(TableRef::new(v))
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
