use crate::security::perimeter::http_rest::AllowedOperation;
use serde::Serialize;
use shamir_collections::new_map;
use shamir_connect::server::token::TokenMode;
use shamir_query_builder::batch::{Batch, IntoBatchOp, Subscribe};
use shamir_query_builder::ddl::list::list_tables;
use shamir_query_builder::ddl::{access_tree, describe_table, get_table_schema, list_databases};
use shamir_query_builder::write::{delete, doc, insert, update, upsert};
use shamir_query_types::batch::{BatchOp, BatchRequest};
use shamir_query_types::read::ReadQuery;
use shamir_query_types::wire::DbRequest;
use shamir_types::types::value::QueryValue;

use super::super::scope::{classify_request, Decision};

fn batch_with(op: impl shamir_query_builder::batch::IntoBatchOp) -> BatchRequest {
    let mut batch = Batch::new();
    batch.op("q", op);
    batch.build()
}

fn execute(op: impl shamir_query_builder::batch::IntoBatchOp, db: &str) -> DbRequest {
    DbRequest::Execute {
        query_version: 1,
        db: db.into(),
        batch: batch_with(op),
    }
}

#[test]
fn rest_request_allowlist_and_rest_only_transaction_class_are_distinct() {
    for request in [
        DbRequest::Ping,
        DbRequest::WaitForVisibility {
            query_version: 1,
            db: "db".into(),
            repo: "main".into(),
            version: 1,
            visibility_wait: shamir_query_types::batch::VisibilityWait::Strict,
        },
        DbRequest::GetDdlOpStatus {
            db: "db".into(),
            repo: "main".into(),
            table: "t".into(),
            op_id: "ddl".into(),
        },
        execute(shamir_query_builder::query::Query::new("t"), "db"),
    ] {
        assert!(matches!(
            classify_request(&request, TokenMode::ReadOnly, None),
            Decision::Allowed(_)
        ));
    }
    let tx_outside_scope = DbRequest::TxBegin {
        query_version: 1,
        db: "outside".into(),
        repo: "main".into(),
        isolation: None,
    };
    assert_eq!(
        classify_request(
            &tx_outside_scope,
            TokenMode::ReadOnly,
            Some(&["inside".into()])
        ),
        Decision::NotSupported("not_supported_over_rest")
    );
}

#[test]
fn readonly_denies_each_data_write_class_and_readwrite_admits_it() {
    let value = doc().set_value("x", QueryValue::Int(1)).build();
    let writes = [
        execute(insert("t").row(value.clone()), "db"),
        execute(
            update("t")
                .set(value.clone())
                .where_(shamir_query_types::filter::Filter::And {
                    filters: Vec::new(),
                })
                .build()
                .expect("update builder"),
            "db",
        ),
        execute(
            upsert("t")
                .key(value.clone())
                .value(value.clone())
                .build()
                .expect("set builder"),
            "db",
        ),
        execute(
            delete("t")
                .where_(shamir_query_types::filter::Filter::And {
                    filters: Vec::new(),
                })
                .build()
                .expect("delete builder"),
            "db",
        ),
        {
            let mut batch = Batch::new();
            batch.call(
                "q",
                "f",
                Vec::<shamir_query_types::filter::FilterValue>::new(),
            );
            DbRequest::Execute {
                query_version: 1,
                db: "db".into(),
                batch: batch.build(),
            }
        },
    ];
    for write in &writes {
        assert_eq!(
            classify_request(write, TokenMode::ReadOnly, None),
            Decision::Denied("token_read_only")
        );
        assert_eq!(
            classify_request(write, TokenMode::ReadWrite, None),
            Decision::Allowed(AllowedOperation::Execute)
        );
    }
}

#[test]
fn nested_write_in_zero_iteration_for_each_is_read_only_denied() {
    let write = batch_with(insert("t").row(doc()));
    let for_each = batch_with(BatchOp::ForEach(shamir_query_types::batch::ForEachOp {
        over: shamir_query_types::filter::FilterValue::Array(Vec::new()),
        bind_row: "item".into(),
        batch: write,
    }));
    let nested = batch_with(BatchOp::Batch(shamir_query_types::batch::SubBatchOp {
        batch: for_each,
        bind: new_map(),
    }));
    let request = DbRequest::Execute {
        query_version: 1,
        db: "db".into(),
        batch: nested,
    };
    assert_eq!(
        classify_request(&request, TokenMode::ReadOnly, None),
        Decision::Denied("token_read_only")
    );
    assert_eq!(
        classify_request(&request, TokenMode::ReadWrite, None),
        Decision::Allowed(AllowedOperation::Execute)
    );
}

#[test]
fn database_allowlist_applies_to_requests_and_access_tree() {
    let allowed = ["one".to_owned()];
    let outside = DbRequest::WaitForVisibility {
        query_version: 1,
        db: "two".into(),
        repo: "main".into(),
        version: 1,
        visibility_wait: shamir_query_types::batch::VisibilityWait::Strict,
    };
    let inside = DbRequest::WaitForVisibility {
        query_version: 1,
        db: "one".into(),
        repo: "main".into(),
        version: 1,
        visibility_wait: shamir_query_types::batch::VisibilityWait::Strict,
    };
    assert_eq!(
        classify_request(&outside, TokenMode::ReadOnly, Some(&allowed)),
        Decision::Denied("token_database_denied")
    );
    assert_eq!(
        classify_request(&inside, TokenMode::ReadOnly, Some(&allowed)),
        Decision::Allowed(AllowedOperation::WaitForVisibility)
    );
    for db in [Some("one".to_owned()), Some("two".to_owned()), None] {
        let access_op = match db.as_ref() {
            Some(db) => access_tree().db(db),
            None => access_tree(),
        };
        let access = execute(access_op, "one");
        let expected = if db.as_deref() == Some("one") {
            Decision::Allowed(AllowedOperation::Execute)
        } else {
            Decision::Denied("token_database_denied")
        };
        assert_eq!(
            classify_request(&access, TokenMode::ReadOnly, Some(&allowed)),
            expected
        );
    }
    let list = execute(list_databases(), "one");
    assert_eq!(
        classify_request(&list, TokenMode::ReadOnly, Some(&allowed)),
        Decision::Denied("token_database_denied")
    );
}

#[test]
fn top_level_introspection_is_allowed_and_nested_introspection_denied() {
    let introspection = [
        list_tables().into_batch_op(),
        access_tree().into_batch_op(),
        describe_table("t").into_batch_op(),
        get_table_schema("t").into_batch_op(),
    ];
    for op in introspection {
        let batch = batch_with(op);
        let request = DbRequest::Execute {
            query_version: 1,
            db: "db".into(),
            batch: batch.clone(),
        };
        assert_eq!(
            classify_request(&request, TokenMode::ReadOnly, None),
            Decision::Allowed(AllowedOperation::Execute)
        );
        let nested = batch_with(BatchOp::Batch(shamir_query_types::batch::SubBatchOp {
            batch,
            bind: new_map(),
        }));
        let request = DbRequest::Execute {
            query_version: 1,
            db: "db".into(),
            batch: nested,
        };
        assert_eq!(
            classify_request(&request, TokenMode::ReadOnly, None),
            Decision::Denied("token_request_denied")
        );
    }
    let nested_for_each = batch_with(BatchOp::ForEach(shamir_query_types::batch::ForEachOp {
        over: shamir_query_types::filter::FilterValue::Array(Vec::new()),
        bind_row: "item".into(),
        batch: batch_with(list_tables()),
    }));
    let request = DbRequest::Execute {
        query_version: 1,
        db: "db".into(),
        batch: nested_for_each,
    };
    assert_eq!(
        classify_request(&request, TokenMode::ReadWrite, None),
        Decision::Denied("token_request_denied")
    );
}

#[test]
fn ddl_representative_is_denied_for_both_token_modes() {
    let ddl = execute(shamir_query_builder::ddl::create_db("x").build(), "db");
    assert_eq!(
        classify_request(&ddl, TokenMode::ReadOnly, None),
        Decision::Denied("token_read_only")
    );
    assert_eq!(
        classify_request(&ddl, TokenMode::ReadWrite, None),
        Decision::Denied("token_request_denied")
    );
}

#[test]
fn top_level_admin_requests_are_denied_using_typed_variants() {
    let requests = [
        DbRequest::SetSuperuser {
            user: "u".into(),
            on: true,
            hmac: None,
        },
        DbRequest::SetReplicator {
            user: "u".into(),
            on: true,
            hmac: None,
        },
        DbRequest::CreateScramUser {
            name: "u".into(),
            password: "p".into(),
            roles: Vec::new(),
            hmac: None,
        },
        DbRequest::ChangePasswordChallenge {
            client_nonce_cp: vec![1],
        },
        DbRequest::ChangePasswordVerify(shamir_query_types::wire::ChangePasswordSecrets {
            client_proof_old: vec![1],
            new_salt: vec![2],
            new_stored_key: vec![3],
            new_server_key: vec![4],
        }),
        DbRequest::Repl(shamir_query_types::wire::ReplRequest::Hello {
            proto_ver: 1,
            node_id: "n".into(),
            capabilities: None,
        }),
    ];
    for request in requests {
        assert_eq!(
            classify_request(&request, TokenMode::ReadOnly, None),
            Decision::Denied("token_request_denied")
        );
    }
}

#[test]
fn subscribe_and_unsubscribe_are_not_supported_over_rest() {
    let requests = [
        execute(BatchOp::Subscribe(Subscribe::table("t").build()), "db"),
        {
            let mut batch = Batch::new();
            batch.unsubscribe("q", 1);
            DbRequest::Execute {
                query_version: 1,
                db: "db".into(),
                batch: batch.build(),
            }
        },
    ];
    for request in requests {
        assert_eq!(
            classify_request(&request, TokenMode::ReadWrite, None),
            Decision::NotSupported("not_supported_over_rest")
        );
    }
}

#[test]
fn unsupported_transaction_and_cursor_requests_precede_database_denial() {
    let requests = [
        DbRequest::TxBegin {
            query_version: 1,
            db: "outside".into(),
            repo: "main".into(),
            isolation: None,
        },
        DbRequest::TxExecute {
            query_version: 1,
            db: "outside".into(),
            tx_handle: 1,
            batch: batch_with(shamir_query_builder::query::Query::new("t")),
        },
        DbRequest::TxCommit {
            query_version: 1,
            visibility_wait: None,
            db: "outside".into(),
            tx_handle: 1,
        },
        DbRequest::TxRollback {
            db: "outside".into(),
            tx_handle: 1,
        },
        DbRequest::CreateCursor {
            db: "outside".into(),
            query_version: 1,
            query: ReadQuery::new("t"),
            page_size: 10,
        },
        DbRequest::FetchNext {
            cursor_id: shamir_query_types::wire::CursorId(1),
            page_size: None,
        },
        DbRequest::CancelCursor {
            cursor_id: shamir_query_types::wire::CursorId(1),
        },
    ];
    for request in requests {
        assert_eq!(
            classify_request(&request, TokenMode::ReadOnly, Some(&["inside".into()])),
            Decision::NotSupported("not_supported_over_rest")
        );
    }
}

// These management variants are deliberately modeled as a test-only wire
// fixture: DbRequest has no such variants yet, so decoding must reject them.
#[derive(Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum ApiTokenManagementWireFixture {
    CreateApiToken { name: String },
    ListApiTokens,
    RevokeApiToken { token_id: String },
    RevokeUserTokens { user: String },
}

#[test]
fn api_token_management_wire_variants_are_not_available_yet() {
    let fixtures = [
        ApiTokenManagementWireFixture::CreateApiToken {
            name: "fixture".into(),
        },
        ApiTokenManagementWireFixture::ListApiTokens,
        ApiTokenManagementWireFixture::RevokeApiToken {
            token_id: "fixture".into(),
        },
        ApiTokenManagementWireFixture::RevokeUserTokens {
            user: "fixture".into(),
        },
    ];
    for fixture in fixtures {
        let wire = rmp_serde::to_vec_named(&fixture).expect("typed MessagePack fixture");
        assert!(rmp_serde::from_slice::<DbRequest>(&wire).is_err());
    }
}
