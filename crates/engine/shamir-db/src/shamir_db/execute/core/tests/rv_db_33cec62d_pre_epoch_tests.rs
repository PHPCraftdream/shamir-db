use crate::query::batch::BatchError;
use crate::shamir_db::execute::helpers::{admin_result_with_op_id_at, system_time_to_unix_millis};
use shamir_types::types::record_id::RecordId;

#[test]
fn rv_db_33cec62d_pre_epoch_clock_is_reported_as_error() {
    let before_epoch = std::time::UNIX_EPOCH - std::time::Duration::from_millis(1);
    let error = system_time_to_unix_millis(before_epoch)
        .expect_err("a pre-epoch clock reading must not be unwrapped or accepted");
    assert!(matches!(
        error,
        BatchError::QueryError { code: Some(code), .. } if code == "internal_error"
    ));
}

#[test]
fn rv_db_33cec62d_committed_ddl_keeps_op_id_without_clock_status() {
    // The system clock is sampled after DDL commit; the result must not turn
    // a committed operation into an error if that clock predates the epoch.
    let op_id = RecordId::system("pre_epoch_committed_ddl");
    let before_epoch = std::time::UNIX_EPOCH - std::time::Duration::from_millis(1);
    let result =
        admin_result_with_op_id_at(crate::types::value::QueryValue::Null, op_id, before_epoch)
            .expect("committed DDL response remains successful");
    assert_eq!(result.op_id, Some(op_id));
    assert!(result.ddl_status.is_none());
}

#[test]
fn rv_db_33cec62d_epoch_clock_converts_to_zero() {
    assert_eq!(
        system_time_to_unix_millis(std::time::UNIX_EPOCH).unwrap(),
        0
    );
}
