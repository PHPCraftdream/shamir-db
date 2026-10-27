use crate::query::batch::BatchError;
use crate::shamir_db::execute::helpers::to_qv;

/// A value whose serialization always fails.
struct UnsupportedValue;

impl serde::Serialize for UnsupportedValue {
    fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("unsupported value"))
    }
}

#[test]
fn rv_db_3fbfc9a8_serialization_failure_is_an_error() {
    let error =
        to_qv(&UnsupportedValue).expect_err("unsupported MessagePack integer must not become Null");
    assert!(
        matches!(error, BatchError::QueryError { code: Some(code), .. } if code == "internal_error")
    );
}
