//! `DbRequest` / `DbResponse` — the application-layer payload of
//! `RequestEnvelope.req` / `ResponseEnvelope.res` after the SCRAM
//! handshake completes.

use serde::{Deserialize, Serialize};

use crate::batch::{BatchRequest, BatchResponse, TransactionInfo};

/// Current query-language version. Bumped when the on-the-wire
/// `BatchRequest` schema changes incompatibly. The server keeps a
/// hardcoded supported list (`SUPPORTED_QUERY_LANG_VERSIONS`) and
/// rejects unknown versions before invoking the DB layer.
pub const CURRENT_QUERY_LANG_VERSION: u32 = 1;

fn default_query_version() -> u32 {
    CURRENT_QUERY_LANG_VERSION
}

/// Application-layer DB request (msgpack-encoded payload of
/// `RequestEnvelope.req`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum DbRequest {
    /// Health check — no DB lookup, no version negotiation.
    Ping,
    /// Execute a [`BatchRequest`] against the named database. The batch
    /// payload is forwarded verbatim to `ShamirDb::execute`; the full
    /// [`BatchResponse`] (records, stats, pagination, plan, transaction
    /// info) is returned to the client.
    Execute {
        /// Query-language version. Default: [`CURRENT_QUERY_LANG_VERSION`].
        #[serde(default = "default_query_version")]
        query_version: u32,
        /// Target database name.
        db: String,
        /// Batch payload — see [`crate::batch::BatchRequest`].
        batch: BatchRequest,
    },
    /// Create a SCRAM-authenticatable user (the kind that can log in
    /// over the wire). Distinct from `BatchOp::CreateUser` (DB-level
    /// user for table permissions). Server runs Argon2id with its
    /// configured KDF defaults and writes the record to the durable
    /// user directory. Requires superuser session.
    CreateScramUser {
        /// Username (will be NFC + UsernameCaseMapped normalised on
        /// the server write path).
        name: String,
        /// Plaintext password. Hashed server-side; the server wraps the
        /// received `String` in `Zeroizing<Vec<u8>>` before deriving keys
        /// so it is zeroized on drop. The `String` here is the on-the-wire
        /// carrier — callers should avoid retaining it longer than needed.
        password: String,
        /// Roles to grant. `["superuser"]` for admin powers; other
        /// strings are opaque to the protocol (RBAC is app-defined).
        #[serde(default)]
        roles: Vec<String>,
    },

    // --- Phase B: interactive (multi-call) transactions ---
    /// Open an interactive transaction scoped to a single `repo`. The server
    /// mints an opaque `tx_handle`, opens an MVCC snapshot, and parks the
    /// live transaction bound to this session. Subsequent `TxExecute`s run
    /// against it until `TxCommit`/`TxRollback`. Cross-repo transactions stay
    /// out of scope (one repo per handle).
    TxBegin {
        /// Query-language version. Default: [`CURRENT_QUERY_LANG_VERSION`].
        #[serde(default = "default_query_version")]
        query_version: u32,
        /// Target database name.
        db: String,
        /// The single repo the transaction is scoped to.
        repo: String,
        /// `"snapshot"` (default) | `"serializable"` — same vocabulary as
        /// [`crate::batch::BatchRequest::isolation`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        isolation: Option<String>,
    },
    /// Run a batch inside an already-open interactive transaction. The tx is
    /// NOT committed — state accumulates in the parked transaction. The
    /// batch's `transactional` flag is ignored here (the handle already
    /// establishes tx mode).
    TxExecute {
        /// Query-language version. Default: [`CURRENT_QUERY_LANG_VERSION`].
        #[serde(default = "default_query_version")]
        query_version: u32,
        /// Target database name (must match the handle's pinned db).
        db: String,
        /// The handle minted by [`DbRequest::TxBegin`].
        tx_handle: u64,
        /// Batch payload — see [`crate::batch::BatchRequest`].
        batch: BatchRequest,
    },
    /// Commit an open interactive transaction (runs the full commit
    /// pipeline). The reply carries the [`TransactionInfo`].
    TxCommit {
        /// Target database name.
        db: String,
        /// The handle to commit.
        tx_handle: u64,
    },
    /// Roll back (abort) an open interactive transaction. Drops the parked
    /// transaction — RAII rollback, no storage side effects.
    TxRollback {
        /// Target database name.
        db: String,
        /// The handle to roll back.
        tx_handle: u64,
    },
}

/// Application-layer DB response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DbResponse {
    /// Reply to [`DbRequest::Ping`].
    Pong,
    /// Successful batch execution.
    Batch {
        /// Full [`BatchResponse`] — no fields dropped.
        response: BatchResponse,
    },
    /// Successful [`DbRequest::CreateScramUser`].
    UserCreated {
        /// Echoed user name (post-normalisation).
        name: String,
        /// Stable 16-byte user_id assigned by the directory.
        #[serde(with = "serde_bytes")]
        user_id: Vec<u8>,
    },
    /// DB-layer failure (permission, planner, query, lock-timeout, …).
    /// Not a protocol error; the wire frame is a normal `ResponseEnvelope`.
    Error {
        /// Coarse classification: `permission_denied`, `validation`,
        /// `limits`, `query`, `timeout`, `lock_timeout`, `unknown_db`,
        /// `not_supported`, `user_exists`.
        code: String,
        /// Human-readable detail.
        message: String,
    },

    // --- Phase B: interactive (multi-call) transactions ---
    /// Reply to [`DbRequest::TxBegin`] — the minted handle + the snapshot
    /// version the transaction reads at.
    TxOpened {
        /// Opaque handle for subsequent `TxExecute`/`TxCommit`/`TxRollback`.
        tx_handle: u64,
        /// MVCC version the transaction's snapshot reads at.
        snapshot_version: u64,
        /// Effective isolation (`"snapshot"` | `"serializable"`).
        isolation: String,
    },
    /// Reply to [`DbRequest::TxExecute`] — the same [`BatchResponse`] a non-tx
    /// `Execute` returns, except `BatchResponse.transaction` stays `None`
    /// (the tx is still open; there is no per-call commit outcome yet).
    TxBatch {
        /// Full [`BatchResponse`] for this call.
        response: BatchResponse,
    },
    /// Reply to [`DbRequest::TxCommit`] — the commit outcome, produced at
    /// COMMIT time rather than per batch.
    TxCommitted {
        /// Committed-or-aborted transaction info.
        transaction: TransactionInfo,
    },
    /// Reply to [`DbRequest::TxRollback`].
    TxRolledBack {
        /// The handle that was rolled back.
        tx_handle: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tx_begin_request_roundtrip_and_tag() {
        let req = DbRequest::TxBegin {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: "app".into(),
            repo: "main".into(),
            isolation: Some("serializable".into()),
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["op"], "tx_begin");
        assert_eq!(v["repo"], "main");
        assert_eq!(v["isolation"], "serializable");

        let back: DbRequest = serde_json::from_value(v).unwrap();
        assert!(matches!(back, DbRequest::TxBegin { repo, .. } if repo == "main"));
    }

    #[test]
    fn tx_begin_isolation_optional_and_query_version_defaults() {
        // Minimal payload — no isolation, no query_version (older/min client).
        let v = json!({
            "op": "tx_begin",
            "db": "app",
            "repo": "main"
        });
        let req: DbRequest = serde_json::from_value(v).unwrap();
        match req {
            DbRequest::TxBegin {
                query_version,
                isolation,
                ..
            } => {
                assert_eq!(
                    query_version, CURRENT_QUERY_LANG_VERSION,
                    "absent query_version must default"
                );
                assert!(isolation.is_none(), "absent isolation decodes to None");
            }
            _ => panic!("expected TxBegin"),
        }
    }

    #[test]
    fn tx_execute_request_roundtrip() {
        let v = json!({
            "op": "tx_execute",
            "db": "app",
            "tx_handle": 42,
            "batch": {
                "id": 1,
                "queries": {}
            }
        });
        let req: DbRequest = serde_json::from_value(v).unwrap();
        assert!(matches!(req, DbRequest::TxExecute { tx_handle, .. } if tx_handle == 42));
    }

    #[test]
    fn tx_commit_and_rollback_request_tags() {
        let commit = serde_json::to_value(&DbRequest::TxCommit {
            db: "app".into(),
            tx_handle: 7,
        })
        .unwrap();
        assert_eq!(commit["op"], "tx_commit");
        assert_eq!(commit["tx_handle"], 7);

        let rollback = serde_json::to_value(&DbRequest::TxRollback {
            db: "app".into(),
            tx_handle: 7,
        })
        .unwrap();
        assert_eq!(rollback["op"], "tx_rollback");
        assert_eq!(rollback["tx_handle"], 7);
    }

    #[test]
    fn tx_opened_response_roundtrip_and_tag() {
        let resp = DbResponse::TxOpened {
            tx_handle: 99,
            snapshot_version: 1234,
            isolation: "snapshot".into(),
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["kind"], "tx_opened");
        assert_eq!(v["tx_handle"], 99);

        let back: DbResponse = serde_json::from_value(v).unwrap();
        assert!(matches!(
            back,
            DbResponse::TxOpened { snapshot_version, .. } if snapshot_version == 1234
        ));
    }

    #[test]
    fn tx_committed_response_carries_transaction_info() {
        let info = TransactionInfo::committed(5, 100, 105, true);
        let resp = DbResponse::TxCommitted { transaction: info };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["kind"], "tx_committed");
        assert_eq!(v["transaction"]["status"], "committed");

        let back: DbResponse = serde_json::from_value(v).unwrap();
        assert!(matches!(
            back,
            DbResponse::TxCommitted { transaction } if transaction.is_committed()
        ));
    }

    #[test]
    fn tx_rolled_back_response_tag() {
        let v = serde_json::to_value(&DbResponse::TxRolledBack { tx_handle: 3 }).unwrap();
        assert_eq!(v["kind"], "tx_rolled_back");
        assert_eq!(v["tx_handle"], 3);
    }
}
