use crate::query::batch::receipts::policy::body_metadata_test_probe;
use crate::query::batch::{BatchLimits, BatchOp, BatchRequest, QueryEntry};
use shamir_collections::{TFxSet, TMap};
use shamir_query_builder::query::Query;
use shamir_query_types::batch::{
    distinct_repos, BatchRequest as Request, ResultEncoding, SubBatchOp,
};
use shamir_types::types::value::QueryValue;

fn request(queries: TMap<String, QueryEntry>) -> BatchRequest {
    Request {
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
        interner_epochs: TMap::default(),
        result_encoding: ResultEncoding::default(),
    }
}

fn entry(op: BatchOp) -> QueryEntry {
    QueryEntry {
        op,
        return_result: true,
        after: Vec::new(),
        when: None,
    }
}

const DEPTH: usize = 30;

// Every level reads its own repository and nests the next level; the deepest
// level only reads.
fn chain() -> BatchRequest {
    let leaf = |level: usize| {
        let mut queries = TMap::default();
        queries.insert(
            format!("read-{level}"),
            entry(
                Query::with_repo(format!("r{level}"), "items")
                    .build()
                    .into(),
            ),
        );
        queries
    };
    let mut body = request(leaf(DEPTH - 1));
    for level in (0..DEPTH - 1).rev() {
        let mut queries = leaf(level);
        queries.insert(
            "nested".into(),
            entry(BatchOp::Batch(SubBatchOp {
                batch: body,
                bind: TMap::default(),
            })),
        );
        body = request(queries);
    }
    body
}

#[test]
fn body_summaries_match_distinct_repos_at_every_depth_with_linear_entry_visits() {
    let root = chain();
    let (visits, summaries) = body_metadata_test_probe(&root);
    let entries = 2 * (DEPTH - 1) + 1;
    // One enumeration finds the bodies, one summarises them; a per-body
    // recollection of descendants would grow with DEPTH * entries.
    assert_eq!(visits, 2 * entries);
    assert_eq!(summaries.len(), DEPTH);

    let mut body = &root;
    for level in 0..DEPTH {
        let expected: TFxSet<String> = distinct_repos(&body.queries);
        assert_eq!(expected.len(), DEPTH - level);
        assert_eq!(summaries[&(body as *const Request as usize)], expected);
        match body.queries.get("nested").map(|entry| &entry.op) {
            Some(BatchOp::Batch(sub)) => body = &sub.batch,
            Some(_) => unreachable!("only batch bodies are nested"),
            None => assert_eq!(level, DEPTH - 1),
        }
    }
}
