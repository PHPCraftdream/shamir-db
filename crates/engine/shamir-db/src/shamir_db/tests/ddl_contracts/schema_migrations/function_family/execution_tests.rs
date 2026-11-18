use shamir_query_builder::ddl::{self, WriteOp};

use super::super::helpers::on_big_stack;
use super::helpers::{
    apply_request, body, bounded, create_function, create_validator, history, identity,
    persistent_owner_db, run_apply, run_op, wasm, DB, REPO,
};
use crate::shamir_db::system_store::JournalEventKind;
use crate::types::value::QueryValue;

#[test]
fn creates_registrations_and_journals_ordered_check_then_retries_as_noop() {
    on_big_stack(|| async {
        let (_dir, db) = persistent_owner_db().await;
        let target = identity(&db).await;
        let code = wasm();
        let migration = body(
            "register",
            1,
            [
                create_function("greeter"),
                ddl::check_function().wasm(code.as_str()).build().unwrap(),
                create_validator("row_guard"),
            ],
        );
        let request = apply_request(target, None, 0, &migration);
        let applied = run_apply(&db, request.clone()).await.unwrap();
        assert_eq!(applied["status"].as_str(), Some("applied"));
        let stored = bounded(db.system_store().load_function("greeter"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored["wasm_b64"].as_str(), Some(code.as_str()));
        let stored = bounded(db.system_store().load_validator("row_guard"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored["wasm_b64"].as_str(), Some(code.as_str()));
        assert!(db.validators().id_for_name("row_guard").is_some());

        let before = history(&db, target).await;
        assert_eq!(before.events[0].body.as_ref(), Some(&migration));
        let steps: Vec<_> = before
            .events
            .iter()
            .map(|event| (event.kind, event.operation_index))
            .collect();
        assert_eq!(
            steps,
            [
                (JournalEventKind::Started, None),
                (JournalEventKind::OperationIntent, Some(0)),
                (JournalEventKind::OperationComplete, Some(0)),
                (JournalEventKind::OperationIntent, Some(1)),
                (JournalEventKind::OperationComplete, Some(1)),
                (JournalEventKind::OperationIntent, Some(2)),
                (JournalEventKind::OperationComplete, Some(2)),
                (JournalEventKind::Applied, None),
                (JournalEventKind::FenceReleased, None),
            ]
        );
        assert_eq!(run_apply(&db, request).await.unwrap(), applied);
        assert_eq!(history(&db, target).await, before);
        assert!(db.schema_migration_gate_generation(DB).is_none());
    });
}

#[test]
fn alters_only_optimization_and_renames_both_registrations() {
    on_big_stack(|| async {
        let (_dir, db) = persistent_owner_db().await;
        let target = identity(&db).await;
        run_op(
            &db,
            ddl::create_function("greeter")
                .wasm(wasm())
                .optimize("auto")
                .build()
                .unwrap(),
        )
        .await
        .unwrap();
        run_op(&db, create_validator("row_guard")).await.unwrap();
        let original = bounded(db.system_store().load_function("greeter"))
            .await
            .unwrap()
            .unwrap();
        let validator = bounded(db.system_store().load_validator("row_guard"))
            .await
            .unwrap()
            .unwrap();
        let validator_id = db.validators().id_for_name("row_guard").unwrap();

        // End-state verification rechecks each operation, so alter and rename
        // the same function in successive migrations.
        let alter = body(
            "alter-greeter",
            1,
            [ddl::alter_function("greeter", "never")],
        );
        let applied = run_apply(&db, apply_request(target, None, 0, &alter))
            .await
            .unwrap();
        assert_eq!(applied["status"].as_str(), Some("applied"));
        let altered = bounded(db.system_store().load_function("greeter"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(altered["optimize"].as_str(), Some("never"));
        for key in ["wasm_b64", "artifact_id", "lang"] {
            assert_eq!(altered.get(key), original.get(key), "function field {key}");
        }

        let rename = body(
            "rename-registrations",
            2,
            [
                ddl::rename_function("greeter", "salutation"),
                ddl::rename_validator("row_guard", "renamed_guard"),
            ],
        );
        // The first migration released its fence: the durable generation is 2.
        let applied = run_apply(&db, apply_request(target, Some(&alter), 2, &rename))
            .await
            .unwrap();
        assert_eq!(applied["status"].as_str(), Some("applied"));
        assert!(bounded(db.system_store().load_function("greeter"))
            .await
            .unwrap()
            .is_none());
        assert!(bounded(db.system_store().load_validator("row_guard"))
            .await
            .unwrap()
            .is_none());
        let renamed = bounded(db.system_store().load_function("salutation"))
            .await
            .unwrap()
            .unwrap();
        for key in ["wasm_b64", "optimize", "lang"] {
            assert_eq!(renamed.get(key), altered.get(key), "function field {key}");
        }
        let renamed = bounded(db.system_store().load_validator("renamed_guard"))
            .await
            .unwrap()
            .unwrap();
        for key in ["_id", "wasm_b64", "module_hash"] {
            assert_eq!(
                renamed.get(key),
                validator.get(key),
                "validator field {key}"
            );
        }
        assert_eq!(
            db.validators().id_for_name("renamed_guard"),
            Some(validator_id)
        );
        assert!(db.validators().id_for_name("row_guard").is_none());
    });
}

#[test]
fn unbind_removes_an_existing_binding_but_keeps_the_validator() {
    on_big_stack(|| async {
        let (_dir, db) = persistent_owner_db().await;
        let target = identity(&db).await;
        run_op(&db, ddl::create_table("items").repo(REPO).build())
            .await
            .unwrap();
        run_op(&db, create_validator("row_guard")).await.unwrap();
        bounded(db.check_seed_catalogue()).await.unwrap();
        let id = db.validators().id_for_name("row_guard").unwrap();
        run_op(
            &db,
            ddl::bind_validator("row_guard", "items")
                .db(DB)
                .ops([WriteOp::Insert])
                .build(),
        )
        .await
        .unwrap();
        let bindings = bounded(db.list_validator_bindings(DB, REPO, "items"))
            .await
            .unwrap();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].validator_id, id);

        let migration = body(
            "unbind-guard",
            1,
            [ddl::unbind_validator("row_guard", "items")
                .db(DB)
                .repo(REPO)
                .build()],
        );
        let applied = run_apply(&db, apply_request(target, None, 0, &migration))
            .await
            .unwrap();
        assert_eq!(applied["status"].as_str(), Some("applied"));
        assert!(bounded(db.list_validator_bindings(DB, REPO, "items"))
            .await
            .unwrap()
            .is_empty());
        assert_eq!(db.validators().id_for_name("row_guard"), Some(id));
        let row = bounded(db.system_store().load_validator("row_guard"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row["bound_in"], QueryValue::List(vec![]));
    });
}
