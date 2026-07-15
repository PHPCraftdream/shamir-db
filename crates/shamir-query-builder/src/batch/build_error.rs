/// Client-side validation error from [`super::Batch::try_build`].
#[derive(Debug, Clone, PartialEq)]
pub enum BuildError {
    /// A `$query` ref points to an alias not present in the batch.
    UnknownAlias {
        /// The alias that was referenced.
        alias: String,
        /// The alias of the entry that contains the bad reference.
        referenced_by: String,
    },
    /// A `$query` ref inside an entry points back to itself.
    SelfReference {
        /// The alias that references itself.
        alias: String,
    },
    /// An `after` entry carried a value-path tail (e.g. `"mk[0].id"`,
    /// `"mk.id"`) that `after` silently ignores.
    ///
    /// `after` is alias-only ordering — it never resolves a value path the
    /// way `$query` does. A path tail here is almost always a developer
    /// mistake, so the builder rejects it up front (mirrors
    /// `shamir_query_types::batch::BatchError::AfterPathIgnored`).
    AfterPathIgnored {
        /// The alias of the entry whose `after` list carries the bad ref.
        alias: String,
        /// The raw `after` string that carried the path tail.
        raw: String,
    },
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::UnknownAlias {
                alias,
                referenced_by,
            } => write!(
                f,
                "unknown alias '{}' referenced by '{}'",
                alias, referenced_by
            ),
            BuildError::SelfReference { alias } => {
                write!(f, "alias '{}' references itself", alias)
            }
            BuildError::AfterPathIgnored { alias, raw } => {
                write!(
                    f,
                    "'after' entry '{}' on '{}' carries a value-path tail, but 'after' is \
                     alias-only ordering and never resolves a path; use a bare alias, or a \
                     '$query' reference if you need the value",
                    raw, alias
                )
            }
        }
    }
}

impl std::error::Error for BuildError {}
