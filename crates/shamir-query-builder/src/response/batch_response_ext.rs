//! [`ResponseError`] and the [`BatchResponseExt`] extension trait for
//! typed extraction from [`BatchResponse`].

use shamir_query_types::batch::{BatchResponse, TransactionInfo};
use shamir_query_types::read::{QueryRecord, QueryResult};

use crate::batch::Handle;

// ============================================================================
// ResponseError
// ============================================================================

/// Errors returned by typed extraction methods on `BatchResponseExt`.
#[derive(Debug)]
pub enum ResponseError {
    /// The requested alias is not present in the response `results` map.
    MissingAlias(String),
    /// The requested row index is out of range for the alias's records.
    RowOutOfRange {
        /// Alias whose records were indexed.
        alias: String,
        /// The index that was requested.
        index: usize,
        /// The actual number of records.
        len: usize,
    },
    /// A record failed to deserialize into the requested type `T`.
    ///
    /// The source is an `rmp_serde` decode error produced when the msgpack
    /// round-trip through `QueryValue` cannot be mapped onto `T`.
    Deserialize {
        /// Alias whose record failed.
        alias: String,
        /// The underlying rmp_serde decode error.
        source: rmp_serde::decode::Error,
    },
}

impl std::fmt::Display for ResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResponseError::MissingAlias(alias) => {
                write!(f, "alias '{}' not found in response results", alias)
            }
            ResponseError::RowOutOfRange { alias, index, len } => {
                write!(
                    f,
                    "row index {} out of range for alias '{}' (len {})",
                    index, alias, len
                )
            }
            ResponseError::Deserialize { alias, source } => {
                write!(
                    f,
                    "failed to deserialize record for alias '{}': {}",
                    alias, source
                )
            }
        }
    }
}

impl std::error::Error for ResponseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ResponseError::Deserialize { source, .. } => Some(source),
            _ => None,
        }
    }
}

// ============================================================================
// Empty-slice sentinel
// ============================================================================

/// Returned by `rows` / `get_rows` when the alias is absent.
const EMPTY_SLICE: &[QueryRecord] = &[];

// ============================================================================
// Internal deserialization helper
// ============================================================================

/// Deserialize a single `QueryRecord` into `T` via a msgpack round-trip
/// through its `QueryValue` representation.
///
/// Encodes the record as msgpack (via `rmp_serde::to_vec_named`) and decodes
/// it into `T` (via `rmp_serde::from_slice`).  Because `QueryValue`'s
/// `Serialize` impl is byte-identical to the msgpack wire encoding, this
/// round-trip is lossless.
fn deserialize_record<T: serde::de::DeserializeOwned>(
    alias: &str,
    record: &QueryRecord,
) -> Result<T, ResponseError> {
    // Serializing a QueryValue to msgpack is infallible: no I/O, no
    // non-finite floats, no unknown-length sequences. Propagate as Syntax
    // to avoid unwrap in library code.
    let bytes = rmp_serde::to_vec_named(record.as_value().as_ref()).map_err(|e| {
        ResponseError::Deserialize {
            alias: alias.to_owned(),
            source: rmp_serde::decode::Error::Syntax(e.to_string()),
        }
    })?;
    rmp_serde::from_slice(&bytes).map_err(|e| ResponseError::Deserialize {
        alias: alias.to_owned(),
        source: e,
    })
}

// ============================================================================
// BatchResponseExt trait
// ============================================================================

/// Extension trait that adds typed extraction helpers to `BatchResponse`.
pub trait BatchResponseExt {
    /// The `QueryResult` for an alias, or `None` if absent.
    fn result(&self, alias: &str) -> Option<&QueryResult>;

    /// The raw records for an alias (empty slice if absent).
    fn rows(&self, alias: &str) -> &[QueryRecord];

    /// Deserialize every record of an alias into `T`.
    ///
    /// Returns `Err(MissingAlias)` if the alias is absent, or
    /// `Err(Deserialize)` if any record fails.
    fn rows_as<T: serde::de::DeserializeOwned>(&self, alias: &str)
        -> Result<Vec<T>, ResponseError>;

    /// Deserialize the `index`-th record of an alias into `T`.
    ///
    /// Returns `Err(MissingAlias)` if the alias is absent, or
    /// `Err(RowOutOfRange)` if `index >= len`, or `Err(Deserialize)` on
    /// serde failure.
    fn row_as<T: serde::de::DeserializeOwned>(
        &self,
        alias: &str,
        index: usize,
    ) -> Result<T, ResponseError>;

    /// `result` keyed by a `Handle` (delegates to `handle.alias()`).
    fn get(&self, handle: &Handle) -> Option<&QueryResult>;

    /// `rows` keyed by a `Handle`.
    fn get_rows(&self, handle: &Handle) -> &[QueryRecord];

    /// `rows_as` keyed by a `Handle`.
    fn get_as<T: serde::de::DeserializeOwned>(
        &self,
        handle: &Handle,
    ) -> Result<Vec<T>, ResponseError>;

    /// The execution plan (parallel stages).
    fn execution_plan(&self) -> &[Vec<String>];

    /// Transaction info, if this was a transactional batch.
    fn transaction(&self) -> Option<&TransactionInfo>;

    /// True if non-transactional OR the tx committed.
    ///
    /// A present tx with `status != "committed"` is the only false case.
    fn is_committed(&self) -> bool;

    /// The abort reason, if the tx aborted.
    fn abort_reason(&self) -> Option<&str>;
}

impl BatchResponseExt for BatchResponse {
    fn result(&self, alias: &str) -> Option<&QueryResult> {
        self.results.get(alias)
    }

    fn rows(&self, alias: &str) -> &[QueryRecord] {
        self.results
            .get(alias)
            .map(|qr| qr.records.as_slice())
            .unwrap_or(EMPTY_SLICE)
    }

    fn rows_as<T: serde::de::DeserializeOwned>(
        &self,
        alias: &str,
    ) -> Result<Vec<T>, ResponseError> {
        let qr = self
            .results
            .get(alias)
            .ok_or_else(|| ResponseError::MissingAlias(alias.to_owned()))?;
        qr.records
            .iter()
            .map(|v| deserialize_record(alias, v))
            .collect()
    }

    fn row_as<T: serde::de::DeserializeOwned>(
        &self,
        alias: &str,
        index: usize,
    ) -> Result<T, ResponseError> {
        let qr = self
            .results
            .get(alias)
            .ok_or_else(|| ResponseError::MissingAlias(alias.to_owned()))?;
        let val = qr
            .records
            .get(index)
            .ok_or_else(|| ResponseError::RowOutOfRange {
                alias: alias.to_owned(),
                index,
                len: qr.records.len(),
            })?;
        deserialize_record(alias, val)
    }

    fn get(&self, handle: &Handle) -> Option<&QueryResult> {
        self.result(handle.alias())
    }

    fn get_rows(&self, handle: &Handle) -> &[QueryRecord] {
        self.rows(handle.alias())
    }

    fn get_as<T: serde::de::DeserializeOwned>(
        &self,
        handle: &Handle,
    ) -> Result<Vec<T>, ResponseError> {
        self.rows_as(handle.alias())
    }

    fn execution_plan(&self) -> &[Vec<String>] {
        &self.execution_plan
    }

    fn transaction(&self) -> Option<&TransactionInfo> {
        self.transaction.as_ref()
    }

    fn is_committed(&self) -> bool {
        match &self.transaction {
            None => true,
            Some(tx) => tx.is_committed(),
        }
    }

    fn abort_reason(&self) -> Option<&str> {
        self.transaction
            .as_ref()
            .and_then(|tx| tx.reason.as_deref())
    }
}
