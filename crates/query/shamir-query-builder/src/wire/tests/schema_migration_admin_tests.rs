//! Wire round trip for the typed application-schema admin batch operation
//! (R1c 2b-1): built with the builder, encoded and decoded through the
//! normal msgpack batch entry point — never raw JSON construction.

use std::str::FromStr;

use shamir_query_builder::application_schema_status;
use shamir_query_builder::batch::Batch;
use shamir_query_types::admin::types::schema_migration::{
    SchemaMigrationAdminOp, SchemaMigrationStatusRequest, SchemaMigrationTarget,
};
use shamir_query_types::batch::BatchOp;
use shamir_types::types::record_id::RecordId;

fn status_request(identity: &str) -> SchemaMigrationStatusRequest {
    SchemaMigrationStatusRequest {
        protocol_version: 1,
        target: SchemaMigrationTarget {
            database_identity: RecordId::from_str(identity).expect("test identity parses"),
        },
    }
}

#[test]
fn application_schema_status_round_trips_via_msgpack_batch() {
    let request = status_request("0197f36a-1b2c-7cc3-8a44-3d6f5a9b0e17");

    let mut b = Batch::new();
    b.op("status", application_schema_status(request.clone()));

    let via_build = b.build();
    let via_msgpack = b.to_request_via_msgpack().expect("batch encodes");
    assert_eq!(via_build, via_msgpack, "round trip preserves the batch");

    let entry = via_msgpack
        .queries
        .values()
        .next()
        .expect("the status entry survives the round trip");
    let BatchOp::ApplicationSchema(SchemaMigrationAdminOp::ApplicationSchemaStatus(decoded)) =
        &entry.op
    else {
        panic!(
            "expected the ApplicationSchema status arm, got {:?}",
            entry.op
        );
    };
    assert_eq!(decoded, &request);
}
