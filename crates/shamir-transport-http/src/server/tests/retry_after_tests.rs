#[test]
fn retry_delay_is_carried_by_exec_outcome_and_status_maps() {
    let retry = crate::gate::ExecOutcome {
        status: crate::gate::ExecStatus::RateLimited,
        body: bytes::Bytes::new(),
        retry_after_secs: Some(7),
        hold: None,
    };
    assert_eq!(retry.retry_after_secs, Some(7));
    assert_eq!(
        super::super::service::exec_status(retry.status),
        http::StatusCode::TOO_MANY_REQUESTS
    );
}
