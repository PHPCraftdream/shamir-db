mod body;
mod ddl;
mod function;
#[cfg(test)]
mod value;

pub use body::{canonical_body, canonical_body_checksum, SchemaMigrationCanonicalError};
#[cfg(test)]
pub(crate) use ddl::canonical_query_value;
#[cfg(test)]
pub(crate) use value::canonical_value;
