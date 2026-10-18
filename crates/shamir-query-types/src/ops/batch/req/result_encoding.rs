//! [`ResultEncoding`] — controls whether the server de-interns result rows.

use serde::{Deserialize, Serialize};

/// Controls the encoding of rows returned in a [`BatchResponse`].
///
/// Name (default, legacy) = server de-interns rows to name-keyed QueryValue;
/// Id = server returns id-keyed [`QueryRecord::IdBytes`], client de-interns.
///
/// [`BatchResponse`]: super::batch_response::BatchResponse
/// [`QueryRecord::IdBytes`]: crate::read::QueryRecord
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResultEncoding {
    /// Server de-interns field ids to names before returning rows (legacy default).
    #[default]
    Name,
    /// Server returns raw id-keyed storage msgpack; the client de-interns via
    /// its FieldMap.
    Id,
}
