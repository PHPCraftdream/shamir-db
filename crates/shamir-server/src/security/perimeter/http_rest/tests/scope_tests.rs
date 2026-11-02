use crate::security::perimeter::http_rest::AllowedOperation;
use shamir_collections::new_map;
use shamir_connect::server::token::TokenMode;
use shamir_query_types::batch::{BatchLimits, BatchOp, BatchRequest, QueryEntry, ResultEncoding};
use shamir_query_types::read::ReadQuery;
use shamir_query_types::wire::DbRequest;
use shamir_query_types::write::InsertOp;
use shamir_query_types::TableRef;
use shamir_types::types::value::QueryValue;

use super::super::scope::{classify_request, Decision};

fn batch_with(op: BatchOp) -> BatchRequest {
    let mut queries = new_map();
    queries.insert(
        "q".to_owned(),
        QueryEntry {
            op,
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    BatchRequest {
        id: QueryValue::Int(1),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: new_map(),
        result_encoding: ResultEncoding::default(),
    }
}

#[test]
fn rest_request_allowlist_and_rest_only_transaction_class_are_distinct() {
    let read = DbRequest::Execute {
        query_version: 1,
        db: "db".into(),
        batch: batch_with(BatchOp::Read(ReadQuery::new("t"))),
    };
    assert_eq!(
        classify_request(&read, TokenMode::ReadOnly, None),
        Decision::Allowed(AllowedOperation::Execute)
    );
    let tx = DbRequest::TxRollback {
        db: "db".into(),
        tx_handle: 1,
    };
    assert_eq!(
        classify_request(&tx, TokenMode::ReadOnly, None),
        Decision::NotSupported("not_supported_over_rest")
    );
    let admin = DbRequest::CreateScramUser {
        name: "u".into(),
        password: "p".into(),
        roles: Vec::new(),
        hmac: None,
    };
    assert_eq!(
        classify_request(&admin, TokenMode::ReadOnly, None),
        Decision::Denied("token_request_denied")
    );
}

#[test]
fn read_only_mode_rejects_write_op_but_allows_read_op() {
    let write = DbRequest::Execute {
        query_version: 1,
        db: "db".into(),
        batch: batch_with(BatchOp::Insert(InsertOp {
            insert_into: TableRef::new("t"),
            values: vec![QueryValue::Map(new_map())],
            records_idmsgpack: Vec::new(),
            select: None,
        })),
    };
    assert_eq!(
        classify_request(&write, TokenMode::ReadOnly, None),
        Decision::Denied("token_read_only")
    );
    assert!(matches!(
        classify_request(&write, TokenMode::ReadWrite, None),
        Decision::Allowed(_)
    ));
}

#[test]
fn database_scope_denies_outside_and_allows_inside() {
    let allowed = ["one".to_owned()];
    let request = DbRequest::WaitForVisibility {
        query_version: 1,
        db: "two".into(),
        repo: "main".into(),
        version: 1,
        visibility_wait: shamir_query_types::batch::VisibilityWait::Strict,
    };
    let access = batch_with(BatchOp::AccessTree(
        shamir_query_types::admin::AccessTreeOp {
            access_tree: true,
            depth: None,
            db: Some("two".into()),
        },
    ));
    let scoped_request = DbRequest::Execute {
        query_version: 1,
        db: "one".into(),
        batch: access,
    };
    let inside = DbRequest::WaitForVisibility {
        query_version: 1,
        db: "one".into(),
        repo: "main".into(),
        version: 1,
        visibility_wait: shamir_query_types::batch::VisibilityWait::Strict,
    };
    let access_inside = DbRequest::Execute {
        query_version: 1,
        db: "one".into(),
        batch: batch_with(BatchOp::AccessTree(
            shamir_query_types::admin::AccessTreeOp {
                access_tree: true,
                depth: None,
                db: Some("one".into()),
            },
        )),
    };
    let access_missing = DbRequest::Execute {
        query_version: 1,
        db: "one".into(),
        batch: batch_with(BatchOp::AccessTree(
            shamir_query_types::admin::AccessTreeOp {
                access_tree: true,
                depth: None,
                db: None,
            },
        )),
    };
    assert_eq!(
        classify_request(&request, TokenMode::ReadOnly, Some(&allowed)),
        Decision::Denied("token_database_denied")
    );
    assert_eq!(
        classify_request(&inside, TokenMode::ReadOnly, Some(&allowed)),
        Decision::Allowed(AllowedOperation::WaitForVisibility)
    );
    assert_eq!(
        classify_request(&scoped_request, TokenMode::ReadOnly, Some(&allowed)),
        Decision::Denied("token_database_denied")
    );
    assert_eq!(
        classify_request(&access_inside, TokenMode::ReadOnly, Some(&allowed)),
        Decision::Allowed(AllowedOperation::Execute)
    );
    assert_eq!(
        classify_request(&access_missing, TokenMode::ReadOnly, Some(&allowed)),
        Decision::Denied("token_database_denied")
    );
}
