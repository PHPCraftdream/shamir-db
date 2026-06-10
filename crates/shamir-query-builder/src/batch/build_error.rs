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
        }
    }
}

impl std::error::Error for BuildError {}
