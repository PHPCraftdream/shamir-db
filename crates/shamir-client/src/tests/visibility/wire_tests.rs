use std::sync::{Arc, Mutex};
use std::time::Duration;

use shamir_collections::TFxMap;
use tokio::sync::oneshot;

use crate::client::{await_pending_response, PendingMap};
use crate::{
    ClientError, DbRequest, DbResponse, VisibilityState, VisibilityWait, CURRENT_QUERY_LANG_VERSION,
};

#[test]
fn bounded_zero_and_u64_positions_roundtrip() {
    for version in [9_007_199_254_740_993, u64::MAX] {
        let request = DbRequest::WaitForVisibility {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: "db".into(),
            repo: "main".into(),
            version,
            visibility_wait: VisibilityWait::Bounded { timeout_ms: 0 },
        };
        let bytes = rmp_serde::to_vec_named(&request).unwrap();
        let decoded: DbRequest = rmp_serde::from_slice(&bytes).unwrap();
        assert!(
            matches!(decoded, DbRequest::WaitForVisibility {
                version: actual,
                visibility_wait: VisibilityWait::Bounded { timeout_ms: 0 },
                query_version: CURRENT_QUERY_LANG_VERSION,
                ..
            } if actual == version),
            "{decoded:?}",
        );
        let response = DbResponse::VisibilityObserved {
            version,
            visibility: VisibilityState::Visible,
        };
        let bytes = rmp_serde::to_vec_named(&response).unwrap();
        let decoded: DbResponse = rmp_serde::from_slice(&bytes).unwrap();
        assert!(
            matches!(decoded, DbResponse::VisibilityObserved {
                version: actual,
                visibility: VisibilityState::Visible,
            } if actual == version),
            "{decoded:?}",
        );
    }
}

#[tokio::test(start_paused = true)]
async fn commit_transport_timeout_has_one_request_and_no_replay() {
    // Setup-only log lock; never held across await.
    let log = Arc::new(Mutex::new(Vec::new()));
    let request = DbRequest::TxCommit {
        query_version: CURRENT_QUERY_LANG_VERSION,
        visibility_wait: Some(VisibilityWait::Bounded { timeout_ms: 0 }),
        db: "db".into(),
        tx_handle: u64::MAX,
    };
    let send = async {
        log.lock()
            .unwrap()
            .push(rmp_serde::to_vec_named(&request).unwrap());
        let pending: PendingMap = Arc::new(Mutex::new(TFxMap::default()));
        let (tx, rx) = oneshot::channel();
        pending.lock().unwrap().insert(1, tx);
        let response =
            await_pending_response(rx, Some(Duration::from_millis(50)), &pending, 1).await?;
        rmp_serde::from_slice(&response).map_err(ClientError::from)
    };
    let result = crate::visibility::commit_once(&request, send).await;
    assert!(
        matches!(result, Err(ClientError::RequestTimeout(_))),
        "{result:?}",
    );
    let log = log.lock().unwrap();
    assert_eq!(log.len(), 1);
    let decoded: DbRequest = rmp_serde::from_slice(&log[0]).unwrap();
    assert!(
        matches!(
            decoded,
            DbRequest::TxCommit {
                tx_handle: u64::MAX,
                query_version: CURRENT_QUERY_LANG_VERSION,
                ..
            }
        ),
        "{decoded:?}",
    );
}

#[tokio::test]
async fn reconciliation_validates_target_and_prefix() {
    let observed = crate::visibility::observe_once(u64::MAX, async {
        Ok(DbResponse::VisibilityObserved {
            version: u64::MAX,
            visibility: VisibilityState::Visible,
        })
    })
    .await
    .unwrap();
    assert_eq!(observed, VisibilityState::Visible);
    let mismatch = crate::visibility::observe_once(1, async {
        Ok(DbResponse::VisibilityObserved {
            version: 2,
            visibility: VisibilityState::Visible,
        })
    })
    .await;
    assert!(
        matches!(mismatch, Err(ClientError::Protocol(_))),
        "{mismatch:?}",
    );
}

#[test]
fn unsupported_protocol_error_remains_classified() {
    let result = crate::visibility::map_response(DbResponse::Error {
        code: "unsupported_query_version".into(),
        message: "legacy".into(),
    });
    assert!(
        matches!(&result, Err(ClientError::Db { code, .. }) if code == "unsupported_query_version"),
        "{result:?}",
    );
}
