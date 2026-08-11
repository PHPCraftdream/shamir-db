use shamir_query_types::admin::DropIndexOp;
use shamir_query_types::batch::BatchOp;
use shamir_types::types::record_id::RecordId;

use crate::batch::IntoBatchOp;

/// Drop an index from a table. Returns a builder for optional fields.
pub fn drop_index(name: impl Into<String>, table: impl Into<String>) -> DropIndex {
    DropIndex {
        name: name.into(),
        table: table.into(),
        repo: "main".to_owned(),
        hmac: None,
        if_exists: false,
        request_id: None,
    }
}

/// Builder for [`DropIndexOp`].
pub struct DropIndex {
    name: String,
    table: String,
    repo: String,
    hmac: Option<String>,
    if_exists: bool,
    request_id: Option<RecordId>,
}

impl DropIndex {
    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Attach the hex-encoded HMAC tag.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Enable `IF EXISTS` semantics: dropping a non-existent index is
    /// a silent no-op (`existed: false`) instead of an error.
    pub fn if_exists(mut self) -> Self {
        self.if_exists = true;
        self
    }

    /// Set a client-supplied correlation ID. If present, the server uses this as
    /// the `op_id` for the operation, enabling the client to poll by this ID
    /// even if the response is lost (crash or disconnect). If absent, the
    /// server mints a fresh `RecordId::new()`.
    ///
    /// Idempotent retry: if this field is set and a status record already
    /// exists for this `request_id`, the server short-circuits and returns
    /// the existing status instead of re-executing the mutation.
    pub fn request_id(mut self, request_id: RecordId) -> Self {
        self.request_id = Some(request_id);
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::DropIndex(DropIndexOp {
            drop_index: self.name,
            table: self.table,
            repo: self.repo,
            hmac: self.hmac,
            if_exists: self.if_exists,
            request_id: self.request_id,
        })
    }
}

impl From<DropIndex> for BatchOp {
    fn from(b: DropIndex) -> Self {
        b.build()
    }
}

impl IntoBatchOp for DropIndex {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
