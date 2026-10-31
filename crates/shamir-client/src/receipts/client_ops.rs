//! `Client` entry points for implicit-batch receipts and the typed
//! `changes_since` read, plus the injectable-send cores the mock-transport
//! tests exercise (the closure IS the transport seam: request construction,
//! one attempt, and response decoding are identical to production).

use std::future::Future;
use std::sync::Arc;

use shamir_query_types::admin::ChangesSinceResponse;
use shamir_query_types::batch::{BatchRequest, BatchResponse, VisibilityState, VisibilityWait};
use shamir_query_types::read::QueryRecord;
use shamir_query_types::wire::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};

use super::alias_receipt::AliasReceipt;
use super::extract::extract_receipts;
use crate::error::client_batch_error;
use crate::interner_cache::InternerCacheRegistry;
use crate::{client::should_advertise_interner_epochs, Client, ClientError};

const CHANGES_SINCE_ALIAS: &str = "changes_since";

impl Client {
    /// Execute a NON-transactional batch and extract per-alias implicit
    /// commit receipts (pending is a committed result — never an error,
    /// never resent; `batch.transactional` is never touched). Interner
    /// epoch advertisement and delta merging behave exactly like
    /// [`Client::execute`].
    pub async fn execute_with_receipts(
        &self,
        db: &str,
        batch: BatchRequest,
        visibility_wait: Option<VisibilityWait>,
    ) -> Result<(BatchResponse, Vec<AliasReceipt>), ClientError> {
        // L4-RV-CLIENT-1C219006: gate the interner registry exactly like
        // `Client::execute` (api.rs) — a legacy peer (server_query_version
        // < 2) never negotiates interner epochs, so it must not get
        // pre-touched cache entries either. Passing `None` also disables the
        // post-response merge, which is correct: a legacy peer never sends
        // deltas.
        let interner = if should_advertise_interner_epochs(
            self.server_query_version(),
            !batch.interner_epochs.is_empty(),
        ) {
            Some(self.interner_cache())
        } else {
            None
        };
        execute_with_receipts_via(
            |req| async move { self.roundtrip(&req).await },
            interner.map(Arc::as_ref),
            db,
            batch,
            visibility_wait,
        )
        .await
    }

    /// One-shot `changes_since` journal read, built with the query
    /// builder, typed response includes the sampled `visible_through`
    /// (a settlement sample, NOT a coverage certificate).
    pub async fn changes_since(
        &self,
        db: &str,
        repo: &str,
        cursor: u64,
        limit: Option<u64>,
    ) -> Result<ChangesSinceResponse, ClientError> {
        changes_since_via(
            |req| async move { self.roundtrip(&req).await },
            db,
            repo,
            cursor,
            limit,
        )
        .await
    }
}

/// Production core for [`Client::execute_with_receipts`]: the WHOLE entry
/// point minus the transport call. With `interner = Some(..)` it performs
/// the same interner-epoch advertisement `Client::execute` does before
/// building the wire request and merges the response's `interner_delta`
/// afterwards (mirroring [`crate::interner_cache::ops`] `merge_interner_delta`
/// call for call). It sets the visibility wait, sends the exact
/// `DbRequest::Execute` through `send` exactly once (the transport owns
/// deadlines; a transport failure is never retried or replayed), and
/// extracts the receipts. Tests pass a fresh registry to exercise the
/// interner sections, or `None` to opt out.
pub(crate) async fn execute_with_receipts_via<F, Fut>(
    send: F,
    interner: Option<&InternerCacheRegistry>,
    db: &str,
    mut batch: BatchRequest,
    visibility_wait: Option<VisibilityWait>,
) -> Result<(BatchResponse, Vec<AliasReceipt>), ClientError>
where
    F: FnOnce(DbRequest) -> Fut,
    Fut: Future<Output = Result<DbResponse, ClientError>>,
{
    if let Some(cache) = interner {
        if batch.interner_epochs.is_empty() {
            for repo in shamir_query_types::batch::distinct_repos(&batch.queries) {
                let epoch = cache.get_or_create(db, &repo).epoch();
                batch.interner_epochs.insert(repo, epoch);
            }
        }
    }
    batch.visibility_wait = visibility_wait;
    let request = batch.clone();
    let response = send(DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.to_string(),
        batch,
    })
    .await?;
    let out = match response {
        DbResponse::Batch { response } => {
            let receipts = extract_receipts(&request, &response)?;
            Ok((response, receipts))
        }
        other => Err(ClientError::Protocol(format!(
            "expected Batch, got {other:?}"
        ))),
    };
    if let (Some(cache), Ok((response, _))) = (interner, &out) {
        // Mirror of `Client::merge_interner_delta` (identical calls).
        for (repo, delta) in &response.interner_delta {
            let fm = cache.get_or_create(db, repo);
            for (id, name) in &delta.entries {
                fm.insert_entry(name, *id);
            }
            fm.set_epoch(delta.epoch);
        }
    }
    out
}

/// Production core for [`Client::changes_since`]: builds the request with
/// the query builder, sends the exact `DbRequest::Execute` through `send`
/// once, and decodes the typed response.
pub(crate) async fn changes_since_via<F, Fut>(
    send: F,
    db: &str,
    repo: &str,
    cursor: u64,
    limit: Option<u64>,
) -> Result<ChangesSinceResponse, ClientError>
where
    F: FnOnce(DbRequest) -> Fut,
    Fut: Future<Output = Result<DbResponse, ClientError>>,
{
    let op = crate::builder::ddl::changes_since(cursor).repo(repo);
    let op = match limit {
        Some(limit) => op.limit(limit),
        None => op,
    };
    let mut batch = crate::builder::batch::Batch::new();
    batch
        .id(CHANGES_SINCE_ALIAS)
        .changes_since(CHANGES_SINCE_ALIAS, op);
    let response = send(DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.to_string(),
        batch: batch.try_build().map_err(client_batch_error)?,
    })
    .await?;
    match response {
        DbResponse::Batch { response } => decode_changes_since(&response),
        other => Err(ClientError::Protocol(format!(
            "expected Batch, got {other:?}"
        ))),
    }
}

/// Production core for [`Client::wait_for_visibility`]: builds the exact
/// `DbRequest::WaitForVisibility` and validates one observation.
pub(crate) async fn wait_for_visibility_via<F, Fut>(
    send: F,
    db: &str,
    repo: &str,
    version: u64,
    wait: VisibilityWait,
) -> Result<VisibilityState, ClientError>
where
    F: FnOnce(DbRequest) -> Fut,
    Fut: Future<Output = Result<DbResponse, ClientError>>,
{
    let request = DbRequest::WaitForVisibility {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.to_string(),
        repo: repo.to_string(),
        version,
        visibility_wait: wait,
    };
    crate::visibility::observe_once(version, send(request)).await
}

/// Decode the admin payload out of the response's single result record.
pub(crate) fn decode_changes_since(
    response: &BatchResponse,
) -> Result<ChangesSinceResponse, ClientError> {
    let result = response.results.get(CHANGES_SINCE_ALIAS).ok_or_else(|| {
        ClientError::Protocol("changes_since result missing from batch response".into())
    })?;
    let record = result.records.first().ok_or_else(|| {
        ClientError::Protocol("changes_since result carries no payload record".into())
    })?;
    let QueryRecord::Direct(payload) = record else {
        return Err(ClientError::Protocol(
            "changes_since payload is not a direct record".into(),
        ));
    };
    ChangesSinceResponse::from_query_value(payload)
        .map_err(|e| ClientError::Protocol(format!("changes_since payload: {e}")))
}
