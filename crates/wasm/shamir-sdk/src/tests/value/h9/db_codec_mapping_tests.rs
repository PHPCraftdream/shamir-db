//! Production-site DB batch response codec contract tests.

#![cfg(feature = "query-builder")]

use crate::error::ErrorKind;
use shamir_query_builder::batch::Batch;

#[test]
fn rv_sdk_6d2497ca_db_batch_validation_classifies_user_error() {
    let mut batch = Batch::new();
    batch.id("bad");
    let error = crate::db::batch_request(&batch).expect_err("invalid batch");
    assert_eq!(error.kind(), ErrorKind::User);
}

#[test]
fn rv_sdk_6d2497ca_db_codec_and_host_categories_are_distinct() {
    assert_eq!(
        crate::Error::codec("batch encode/decode").kind(),
        ErrorKind::Codec
    );
    assert_eq!(
        crate::Error::host("empty host response").kind(),
        ErrorKind::Host
    );
}
