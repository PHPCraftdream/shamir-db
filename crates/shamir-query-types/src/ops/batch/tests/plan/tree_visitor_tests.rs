//! Shared static-tree traversal and policy regressions.

use shamir_collections::TMap;
use shamir_types::access::{Action, ResourcePath};
use shamir_types::types::value::QueryValue;

use crate::admin::DropTableOp;
use crate::batch::{
    collect_required_access, distinct_repos, BatchError, BatchLimits, BatchOp, BatchPlanner,
    BatchRequest, BatchTreeVisitor, ForEachOp, QueryEntry, ResultEncoding, SubBatchOp,
};
use crate::filter::FilterValue;
use crate::read::{Pagination, ReadQuery, Select, Temporal};
use crate::TableRef;

// Typed DTO fixtures: the query builder depends on query-types, so using it
// here would introduce a dependency cycle. No JSON query construction.
fn entry(op: BatchOp) -> QueryEntry {
    QueryEntry {
        op,
        return_result: false,
        after: Vec::new(),
        when: None,
    }
}

fn read(repo: &str, table: &str) -> QueryEntry {
    entry(BatchOp::Read(ReadQuery {
        from: TableRef::with_repo(repo, table),
        r#where: None,
        select: Select::all(),
        order_by: None,
        pagination: Pagination::default(),
        group_by: None,
        count_total: false,
        temporal: Temporal::default(),
        with_version: false,
        explain: false,
    }))
}

fn batch(queries: TMap<String, QueryEntry>) -> BatchRequest {
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
        interner_epochs: TMap::default(),
        result_encoding: ResultEncoding::default(),
    }
}

fn wrap(queries: TMap<String, QueryEntry>, loop_body: bool) -> QueryEntry {
    if loop_body {
        entry(BatchOp::ForEach(ForEachOp {
            over: FilterValue::Array(Vec::new()),
            bind_row: "row".into(),
            batch: batch(queries),
        }))
    } else {
        entry(BatchOp::Batch(SubBatchOp {
            batch: batch(queries),
            bind: TMap::default(),
        }))
    }
}

fn singleton(alias: &str, entry: QueryEntry) -> TMap<String, QueryEntry> {
    let mut queries = TMap::default();
    queries.insert(alias.into(), entry);
    queries
}

// Both construction and teardown are iterative, including during unwinding
// from an assertion. 20,000 levels stress a recursive 2 MiB walker without
// relying on recursive DTO Drop or leaking the test tree.
struct Tree(TMap<String, QueryEntry>);

impl Drop for Tree {
    fn drop(&mut self) {
        let mut pending = vec![std::mem::take(&mut self.0)];
        while let Some(queries) = pending.pop() {
            for (_, entry) in queries {
                match entry.op {
                    BatchOp::Batch(mut sub) => {
                        pending.push(std::mem::take(&mut sub.batch.queries));
                    }
                    BatchOp::ForEach(mut fe) => {
                        pending.push(std::mem::take(&mut fe.batch.queries));
                    }
                    _ => {}
                }
            }
        }
    }
}

fn chain(depth: usize, leaf: QueryEntry) -> Tree {
    let mut queries = singleton("leaf", leaf);
    for level in 0..depth {
        queries = singleton("body", wrap(queries, level % 2 == 0));
    }
    Tree(queries)
}

#[test]
fn mixed_tree_preorder_aliases_depth_and_collectors() {
    let mut inner = singleton("same", read("cold", "events"));
    inner.insert("empty".into(), wrap(TMap::default(), false));
    let mut middle = singleton("same", read("hot", "users"));
    middle.insert("loop".into(), wrap(inner, true));
    let mut queries = singleton("same", read("main", "top"));
    queries.insert("nested".into(), wrap(middle, false));
    queries.insert("last".into(), read("cold", "events"));
    let tree = Tree(queries);
    let visits: Vec<_> = BatchTreeVisitor::new(&tree.0)
        .map(|visit| (visit.alias, visit.depth, visit.body_depth()))
        .collect();
    assert_eq!(
        visits,
        vec![
            ("same", 0, None),
            ("nested", 0, Some(1)),
            ("same", 1, None),
            ("loop", 1, Some(2)),
            ("same", 2, None),
            ("empty", 2, Some(3)),
            ("last", 0, None),
        ],
    );
    let repos = distinct_repos(&tree.0);
    assert_eq!(repos.len(), 3);
    for repo in ["main", "hot", "cold"] {
        assert!(repos.contains(repo));
    }
    let expected: Vec<_> = [
        ("main", "top"),
        ("hot", "users"),
        ("cold", "events"),
        ("cold", "events"),
    ]
    .into_iter()
    .map(|(repo, table)| {
        (
            Action::Read,
            ResourcePath::Table {
                db: "db".into(),
                store: repo.into(),
                table: table.into(),
            },
        )
    })
    .collect();
    assert_eq!(collect_required_access(&tree.0, "db"), expected);
}

#[test]
fn containers_classify_descendant_leaves_not_runtime_iterations() {
    let destructive = entry(BatchOp::DropTable(DropTableOp {
        drop_table: "secret".into(),
        repo: "main".into(),
        hmac: None,
        if_exists: false,
        cascade: false,
    }));
    assert!(destructive.op.is_admin());
    assert!(destructive.op.is_write());
    for depth in [1, 2, 3] {
        let tree = chain(depth, destructive.clone());
        let root = &tree.0["body"].op;
        assert!(root.is_admin());
        assert!(root.is_write());
    }
    for loop_body in [false, true] {
        let empty = wrap(TMap::default(), loop_body);
        assert!(!empty.op.is_admin());
        assert!(!empty.op.is_write());
        let data = wrap(singleton("read", read("main", "users")), loop_body);
        assert!(!data.op.is_admin());
        assert!(!data.op.is_write());
    }
}

#[test]
fn deep_chain_walk_and_all_policy_helpers_are_iterative() {
    let depth = 20_000;
    let tree = chain(depth, read("deep", "users"));
    assert_eq!(BatchTreeVisitor::new(&tree.0).count(), depth + 1);
    assert_eq!(BatchTreeVisitor::new(&tree.0).last().unwrap().depth, depth);
    assert!(!tree.0["body"].op.is_admin());
    assert!(!tree.0["body"].op.is_write());
    assert!(distinct_repos(&tree.0).contains("deep"));
    assert_eq!(collect_required_access(&tree.0, "db").len(), 1);
    let limits = BatchLimits {
        max_nesting_depth: 64,
        ..BatchLimits::default()
    };
    assert!(matches!(
        BatchPlanner::plan(&tree.0, &limits),
        Err(BatchError::NestingTooDeep { depth: actual, max: 64 }) if actual == depth
    ));
}

#[test]
fn planner_counts_empty_bodies_and_does_not_truncate_at_64() {
    let mut queries = TMap::default();
    for level in 0..80 {
        queries = singleton("body", wrap(queries, level % 2 == 0));
    }
    let tree = Tree(queries);
    let limits = BatchLimits {
        max_nesting_depth: 79,
        ..BatchLimits::default()
    };
    assert!(matches!(
        BatchPlanner::plan(&tree.0, &limits),
        Err(BatchError::NestingTooDeep { depth: 80, max: 79 })
    ));
    let empty = Tree(TMap::default());
    assert_eq!(BatchTreeVisitor::new(&empty.0).count(), 0);
    assert!(collect_required_access(&empty.0, "db").is_empty());
    assert!(distinct_repos(&empty.0).is_empty());
}
