use crate::query::batch::BatchError;
use crate::shamir_db::execute::helpers::to_qv;

#[derive(serde::Serialize)]
struct UnsupportedValue {
    value: i128,
}

#[test]
fn rv_db_3fbfc9a8_serialization_failure_is_an_error() {
    let error = to_qv(&UnsupportedValue { value: i128::MAX })
        .expect_err("unsupported MessagePack integer must not become Null");
    assert!(
        matches!(error, BatchError::QueryError { code: Some(code), .. } if code == "internal_error")
    );
}
