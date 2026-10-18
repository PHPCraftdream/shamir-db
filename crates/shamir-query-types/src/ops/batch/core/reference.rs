//! Query reference parsing.
//!
//! Parses `$query` references like `@users[0].id` or `@orders[].status`.
//!
//! # Syntax
//!
//! Query references use the `@alias` prefix followed by an optional path:
//!
//! | Syntax | Description | Example |
//! |--------|-------------|---------|
//! | `@alias` | Entire result | `@users` |
//! | `@alias[n]` | Index access | `@users[0]` |
//! | `@alias[]` | All items | `@users[]` |
//! | `@alias.field` | Field access | `@users.name` |
//! | `@alias.count` | Result count | `@users.count` |
//! | `@alias.length` | Result count (alias) | `@users.length` |
//!
//! Paths can be chained:
//!
//! | Syntax | Description |
//! |--------|-------------|
//! | `@users[0].name` | First user's name |
//! | `@users[].id` | All user IDs |
//! | `@data[0].items[].value` | Nested extraction |
//!
//! # Usage in Filters
//!
//! References are used in filter values with the `$query` key:
//!
//! ```text
//! {
//!   "op": "eq",
//!   "field": "user_id",
//!   "value": { "$query": "users[0].id" }
//! }
//! ```
//!
//! Or with explicit path:
//!
//! ```text
//! {
//!   "value": {
//!     "$query": "users",
//!     "path": "[0].id"
//!   }
//! }
//! ```

use std::fmt;

/// Parsed query reference: `@alias[...].field...`
///
/// # Examples
///
/// ```ignore
/// use shamir_db::query::batch::QueryReference;
///
/// let r = QueryReference::parse("@users[0].name").unwrap();
/// assert_eq!(r.alias, "users");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct QueryReference {
    /// Target alias (the query name).
    pub alias: String,
    /// Path into the result.
    pub path: QueryPath,
}

/// Path into a query result.
///
/// Represents the navigation path from the root result array
/// to the target value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum QueryPath {
    /// Root: `@alias` (entire result array).
    Root,

    /// Index: `@alias[0]` (single record).
    Index(usize),

    /// All items: `@alias[]` (for column extraction).
    All,

    /// Field access: `@alias.field`.
    Field(String),

    /// Chain: `@alias[0].address.city`.
    Chain(Vec<QueryPath>),

    /// Count/length: `@alias.count` or `@alias.length`.
    Count,
}

impl QueryReference {
    /// Parse a query reference string.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use shamir_db::query::batch::{QueryReference, QueryPath};
    ///
    /// // Simple alias
    /// let r = QueryReference::parse("@users").unwrap();
    /// assert_eq!(r.alias, "users");
    /// assert_eq!(r.path, QueryPath::Root);
    ///
    /// // With index
    /// let r = QueryReference::parse("@users[0]").unwrap();
    /// assert_eq!(r.path, QueryPath::Index(0));
    ///
    /// // With field
    /// let r = QueryReference::parse("@users.name").unwrap();
    /// assert_eq!(r.path, QueryPath::Field("name".to_string()));
    ///
    /// // Complex path
    /// let r = QueryReference::parse("@users[0].address.city").unwrap();
    /// ```
    pub fn parse(s: &str) -> Result<Self, ReferenceParseError> {
        let s = s.trim();

        // Must start with @
        let s = s.strip_prefix('@').ok_or(ReferenceParseError::MissingAt)?;

        if s.is_empty() {
            return Err(ReferenceParseError::EmptyAlias);
        }

        // Find where alias ends (at '[' or '.')
        let (alias, rest) = split_alias(s);

        if alias.is_empty() {
            return Err(ReferenceParseError::EmptyAlias);
        }

        // Validate alias (alphanumeric + underscore)
        if !alias.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(ReferenceParseError::InvalidAlias(alias.to_string()));
        }

        let path = if rest.is_empty() {
            QueryPath::Root
        } else {
            Self::parse_path(rest)?
        };

        Ok(QueryReference {
            alias: alias.to_string(),
            path,
        })
    }

    fn parse_path(s: &str) -> Result<QueryPath, ReferenceParseError> {
        let mut segments = Vec::new();
        let mut current = s;

        while !current.is_empty() {
            if current.starts_with('[') {
                // Array access
                let end = current
                    .find(']')
                    .ok_or(ReferenceParseError::UnclosedBracket)?;
                let inner = &current[1..end];

                if inner.is_empty() {
                    segments.push(QueryPath::All);
                } else {
                    let idx: usize = inner
                        .parse()
                        .map_err(|_| ReferenceParseError::InvalidIndex(inner.to_string()))?;
                    segments.push(QueryPath::Index(idx));
                }

                current = &current[end + 1..];
            } else if current.starts_with('.') {
                // Field access
                current = &current[1..];

                if current.is_empty() {
                    return Err(ReferenceParseError::TrailingDot);
                }

                // Find end of field name
                let end = current.find(['.', '[']).unwrap_or(current.len());
                let field = &current[..end];

                // Special fields
                if field == "count" || field == "length" {
                    segments.push(QueryPath::Count);
                } else {
                    segments.push(QueryPath::Field(field.to_string()));
                }

                current = &current[end..];
            } else {
                return Err(ReferenceParseError::UnexpectedChar(
                    current.chars().next().unwrap(),
                ));
            }
        }

        if segments.is_empty() {
            Ok(QueryPath::Root)
        } else if segments.len() == 1 {
            Ok(segments.into_iter().next().unwrap())
        } else {
            Ok(QueryPath::Chain(segments))
        }
    }
}

/// Split string into (alias, rest).
fn split_alias(s: &str) -> (&str, &str) {
    let pos = s.find(['[', '.']).unwrap_or(s.len());
    (&s[..pos], &s[pos..])
}

impl fmt::Display for QueryReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}", self.alias)?;
        Self::fmt_path(&self.path, f)
    }
}

impl QueryReference {
    fn fmt_path(path: &QueryPath, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match path {
            QueryPath::Root => Ok(()),
            QueryPath::Index(i) => write!(f, "[{}]", i),
            QueryPath::All => write!(f, "[]"),
            QueryPath::Field(name) => write!(f, ".{}", name),
            QueryPath::Count => write!(f, ".count"),
            QueryPath::Chain(segments) => {
                for seg in segments {
                    Self::fmt_path(seg, f)?;
                }
                Ok(())
            }
        }
    }
}

/// Error parsing query reference.
#[derive(Debug, Clone, PartialEq)]
pub enum ReferenceParseError {
    /// Missing '@' prefix.
    MissingAt,
    /// Empty alias.
    EmptyAlias,
    /// Invalid alias characters (must be alphanumeric + underscore).
    InvalidAlias(String),
    /// Unclosed bracket.
    UnclosedBracket,
    /// Invalid array index (must be a non-negative integer).
    InvalidIndex(String),
    /// Trailing dot.
    TrailingDot,
    /// Unexpected character.
    UnexpectedChar(char),
}

impl fmt::Display for ReferenceParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReferenceParseError::MissingAt => write!(f, "Missing '@' prefix"),
            ReferenceParseError::EmptyAlias => write!(f, "Empty alias"),
            ReferenceParseError::InvalidAlias(a) => write!(f, "Invalid alias: '{}'", a),
            ReferenceParseError::UnclosedBracket => write!(f, "Unclosed bracket"),
            ReferenceParseError::InvalidIndex(i) => write!(f, "Invalid index: '{}'", i),
            ReferenceParseError::TrailingDot => write!(f, "Trailing dot"),
            ReferenceParseError::UnexpectedChar(c) => write!(f, "Unexpected character: '{}'", c),
        }
    }
}

impl std::error::Error for ReferenceParseError {}
