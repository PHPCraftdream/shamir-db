use crate::query::batch::BatchError;
use crate::shamir_db::execute::helpers::system_time_to_unix_millis;

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
fn rv_db_33cec62d_epoch_clock_converts_to_zero() {
    assert_eq!(
        system_time_to_unix_millis(std::time::UNIX_EPOCH).unwrap(),
        0
    );
}
