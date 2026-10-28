//! Production host-import missing-result classifier contract test.

use crate::error::ErrorKind;
use crate::host_imports::{with_native_imports, NativeReply};

#[test]
fn rv_sdk_6d2497ca_missing_host_import_results_are_host_errors() {
    with_native_imports(
        |operation, _, _| {
            assert_eq!(operation, "db_insert");
            NativeReply::Packed(0, Vec::new())
        },
        || {
            let error = crate::Db::new()
                .table("items")
                .insert(crate::Value::Map(Vec::new()))
                .unwrap_err();
            assert_eq!(error.kind(), ErrorKind::Host);
            assert!(error.message().starts_with("db_insert"));
        },
    );
}
