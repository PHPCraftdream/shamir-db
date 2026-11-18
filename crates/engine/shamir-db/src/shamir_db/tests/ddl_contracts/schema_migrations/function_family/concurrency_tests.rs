//! Server-wide function-family admission while a registration holds the gate.

use std::collections::BTreeSet;

use shamir_query_builder::{apply_application_schema_migration, batch::Batch};
use shamir_query_types::admin::types::schema_migration::SchemaMigrationApplyRequest;
use shamir_types::access::{ResourceMeta, ResourcePath};

use super::super::helpers::on_big_stack;
use super::helpers::{
    apply_request, body, bounded, create_function, history, identity, persistent_owner_db,
    run_apply, DB, OWNER,
};
use crate::query::batch::BatchError;
use crate::shamir_db::shamir_db::pause_next_function_registration_for_test;
use crate::types::value::QueryValue;
use crate::ShamirDb;

const OTHER_DB: &str = "otherdb";
const OTHER_FUNCTION: &str = "other_function";

async fn apply_on_other_db(
    db: &ShamirDb,
    request: SchemaMigrationApplyRequest,
) -> Result<QueryValue, BatchError> {
    let mut batch = Batch::new();
    batch.op("apply", apply_application_schema_migration(request));
    let request = batch.to_request_via_msgpack().unwrap();
    let response = bounded(db.execute_as(OWNER, OTHER_DB, &request)).await?;
    Ok(response.results["apply"].records[0].as_value().into_owned())
}

fn outcome(result: &Result<QueryValue, BatchError>) -> &str {
    match result {
        Ok(value) => value["status"].as_str().expect("apply status"),
        Err(error) => error.code().expect("stable apply error code"),
    }
}

#[test]
fn function_migrations_on_two_databases_share_a_server_wide_gate() {
    on_big_stack(|| async {
        let (_dir, db) = bounded(persistent_owner_db()).await;
        bounded(db.create_db(OTHER_DB)).await.unwrap();
        bounded(db.set_resource_meta(
            &ResourcePath::database(OTHER_DB),
            &ResourceMeta {
                owner: OWNER,
                group: None,
                mode: 0o700,
            },
        ))
        .await
        .unwrap();
        // Both migrations contain only function operations; neither needs a repo on OTHER_DB.
        let first_target = bounded(identity(&db)).await;
        let other_target = bounded(db.system_store().schema_migration_identity(OTHER_DB))
            .await
            .unwrap();
        assert_ne!(first_target, other_target);
        let first = body("first-function", 1, [create_function("first_function")]);
        let other = body("other-function", 1, [create_function(OTHER_FUNCTION)]);
        let first_request = apply_request(first_target, None, 0, &first);
        let other_request = apply_request(other_target, None, 0, &other);
        let pristine = bounded(history(&db, other_target)).await;

        let (entered, release) = pause_next_function_registration_for_test(&db);
        let (first_result, (competing, other_during_pause, other_row_during_pause)) =
            bounded(async {
                tokio::join!(
                    bounded(run_apply(&db, first_request)),
                    bounded(async {
                        bounded(entered.notified()).await;
                        assert_eq!(db.schema_migration_gate_generation(DB), Some(1));
                        let competing =
                            bounded(apply_on_other_db(&db, other_request.clone())).await;
                        let other_during_pause = bounded(history(&db, other_target)).await;
                        let other_row_during_pause =
                            bounded(db.system_store().load_function(OTHER_FUNCTION))
                                .await
                                .unwrap();
                        release.notify_one();
                        (competing, other_during_pause, other_row_during_pause)
                    })
                )
            })
            .await;

        assert_eq!(
            BTreeSet::from([outcome(&first_result), outcome(&competing)]),
            BTreeSet::from(["applied", "schema_migration_in_progress"]),
        );
        assert_eq!(other_during_pause, pristine);
        assert_eq!(bounded(history(&db, other_target)).await, pristine);
        assert!(other_row_during_pause.is_none());
        assert!(db.schema_migration_gate_generation(OTHER_DB).is_none());
        assert_eq!(bounded(history(&db, first_target)).await.applied.len(), 1);

        let retry = bounded(apply_on_other_db(&db, other_request)).await;
        assert_eq!(outcome(&retry), "applied");
        assert_eq!(bounded(history(&db, other_target)).await.applied.len(), 1);
        assert!(bounded(db.system_store().load_function(OTHER_FUNCTION))
            .await
            .unwrap()
            .is_some());
    });
}
