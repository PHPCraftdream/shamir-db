//! Factory arguments are execution constants, never aggregate row inputs.

use std::sync::Arc;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::{select, val, Query};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{MvccStore, RepoTxGate, Retention};
use shamir_types::access::Actor;
use shamir_types::types::common::new_map;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::batch::{execute_batch_unchecked, TableResolver};
use crate::query::filter::{FilterContext, FilterValue};
use crate::query::TableRef;
use crate::repo::RepoInstance;
use crate::table::TableManager;

struct Resolver(TableManager, u64);

#[async_trait::async_trait]
impl TableResolver for Resolver {
    async fn resolve(&self, table: &TableRef) -> DbResult<TableManager> {
        assert_eq!(table.table, "rows");
        Ok(self.0.clone())
    }

    async fn resolve_repo(&self, _: &str) -> DbResult<RepoInstance> {
        // Latest unchecked, nontransactional reads do not require a repo.
        Err(DbError::NotFound("standalone fixture".into()))
    }
}

async fn setup() -> Resolver {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    mvcc.set_retention(Retention::keep_history()).unwrap();
    let table = TableManager::create("rows".into(), data, info)
        .await
        .unwrap()
        .with_mvcc_store(Arc::clone(&mvcc))
        .with_changefeed(gate);
    let mut version = 0;
    let interner = table.interner().get().await.unwrap();
    for (v, text) in [(10, "a"), (30, "b")] {
        let mut row = new_map();
        for (name, value) in [
            ("v", InnerValue::Int(v)),
            ("g", InnerValue::Int(1)),
            ("p", InnerValue::F64(0.5)),
            ("sep", InnerValue::Str("|".into())),
            ("text", InnerValue::Str(text.into())),
        ] {
            row.insert(interner.touch_ind(name).unwrap().into_key(), value);
        }
        table.interner().persist().await.unwrap();
        let id = table.insert(&InnerValue::Map(row)).await.unwrap();
        version = version.max(mvcc.version_of(id.as_bytes()));
    }
    Resolver(table, version)
}

fn aggregate(grouped: bool, p: FilterValue, sep: FilterValue) -> Query {
    let query = Query::from("rows").select([
        select::agg_fn_with_args("percentile", "v", "pct", [p]),
        select::agg_fn_with_args("string_agg", "text", "joined", [sep]),
    ]);
    if grouped {
        query.group_by("g")
    } else {
        query
    }
}

fn assert_values(row: &QueryValue, pct: i64, separator: &str) {
    let QueryValue::Map(row) = row else {
        panic!("expected aggregate map")
    };
    assert_eq!(row["pct"], QueryValue::Int(pct));
    // Storage traversal order is not a string_agg ordering guarantee.
    let QueryValue::Str(joined) = &row["joined"] else {
        panic!("expected string")
    };
    let mut parts: Vec<_> = joined.split(separator).collect();
    parts.sort_unstable();
    assert_eq!(parts, ["a", "b"]);
}

/// Red before this fix: QueryRef factory arguments are rejected as nonliteral.
#[tokio::test]
async fn aggregate_batch_query_refs_grouped_and_ungrouped() {
    let resolver = setup().await;
    resolver.0.create_index("g_idx", &["g"]).await.unwrap();
    resolver
        .0
        .create_sorted_index("v_idx", &["v"])
        .await
        .unwrap();
    for grouped in [false, true] {
        for mode in 0..4 {
            let mut consumer = aggregate(
                grouped,
                val::qref("q1", "[0].p"),
                val::qref("q1", "[0].sep"),
            );
            if mode == 1 {
                consumer = consumer.where_eq("g", 1);
            } else if mode == 2 {
                consumer = consumer.where_between("v", 0, 40);
            } else if mode == 3 {
                consumer = consumer.as_of_version(resolver.1);
            }
            let mut batch = Batch::new();
            batch.query("q2", consumer);
            batch.query(
                "q1",
                Query::from("rows")
                    .select([select::field("p"), select::field("sep")])
                    .limit(1),
            );
            let response = execute_batch_unchecked(
                &batch.build(),
                &resolver,
                None,
                None,
                Actor::System,
                "test",
            )
            .await
            .unwrap();
            let result = &response.results["q2"];
            assert_eq!(result.records.len(), 1);
            assert_values(&result.records[0].as_value(), 10, "|");
            if mode == 1 {
                assert_eq!(
                    result.stats.as_ref().unwrap().index_used.as_deref(),
                    Some("g_idx"),
                );
            } else if mode == 2 {
                assert!(result
                    .stats
                    .as_ref()
                    .unwrap()
                    .index_used
                    .as_ref()
                    .unwrap()
                    .starts_with("sorted_idx_"));
            } else if mode == 3 {
                assert_eq!(
                    result.stats.as_ref().unwrap().index_used.as_deref(),
                    Some("temporal_asof"),
                );
            }
        }
    }
}

/// Red before this fix: Param is rejected; sticky bindings would return 10 twice.
#[tokio::test]
async fn aggregate_foreach_factory_params_change_per_iteration() {
    let resolver = setup().await;
    for grouped in [false, true] {
        let mut body = Batch::new();
        body.query(
            "q2",
            aggregate(
                grouped,
                val::add(val::param("p"), val::lit(0)),
                val::lit("|"),
            ),
        );
        let mut outer = Batch::new();
        outer.for_each("loop", vec![val::lit(0), val::lit(1)], "p", body.build());
        let response =
            execute_batch_unchecked(&outer.build(), &resolver, None, None, Actor::System, "test")
                .await
                .unwrap();
        let QueryValue::List(iterations) = response.results["loop"].value.as_ref().unwrap() else {
            panic!("expected iterations")
        };
        assert_eq!(iterations.len(), 2);
        for (iteration, pct) in iterations.iter().zip([10, 30]) {
            let QueryValue::Map(results) = iteration else {
                panic!("expected results")
            };
            let QueryValue::Map(result) = &results["q2"] else {
                panic!("expected result")
            };
            let QueryValue::List(rows) = &result["records"] else {
                panic!("expected rows")
            };
            assert_eq!(rows.len(), 1);
            assert_values(&rows[0], pct, "|");
        }

        let mut body = Batch::new();
        body.query("q2", aggregate(grouped, val::lit(0.5), val::param("sep")));
        let mut outer = Batch::new();
        outer.for_each(
            "loop",
            vec![val::lit("|"), val::lit(";")],
            "sep",
            body.build(),
        );
        let response =
            execute_batch_unchecked(&outer.build(), &resolver, None, None, Actor::System, "test")
                .await
                .unwrap();
        let QueryValue::List(iterations) = response.results["loop"].value.as_ref().unwrap() else {
            panic!("expected iterations")
        };
        assert_eq!(iterations.len(), 2);
        for (iteration, separator) in iterations.iter().zip(["|", ";"]) {
            let QueryValue::Map(results) = iteration else {
                panic!("expected results")
            };
            let QueryValue::Map(result) = &results["q2"] else {
                panic!("expected result")
            };
            let QueryValue::List(rows) = &result["records"] else {
                panic!("expected rows")
            };
            assert_eq!(rows.len(), 1);
            assert_values(&rows[0], 10, separator);
        }
    }
}

/// Direct read asserts the typed DbError; batch currently erases its code.
#[tokio::test]
async fn aggregate_factory_errors_are_typed_even_without_rows() {
    let resolver = setup().await;
    let interner = resolver.0.interner().get().await.unwrap();
    let refs = new_map();
    let mut params = new_map();
    params.insert("bad_type".into(), QueryValue::Str("bad".into()));
    params.insert("bad_range".into(), QueryValue::F64(2.0));
    let ctx = FilterContext::new(interner, &refs).with_params(&params);
    for grouped in [false, true] {
        for (arg, expected) in [
            (val::param("missing"), "agg_param_unresolved"),
            (val::qref("missing", "[0].p"), "agg_param_unresolved"),
            (val::param("bad_type"), "type_mismatch"),
            (val::param("bad_range"), "out_of_range"),
            (
                val::add(val::col("v"), val::lit(0)),
                "agg_param_must_be_literal",
            ),
            (
                val::add(val::param("missing"), val::lit(0)),
                "agg_param_unresolved",
            ),
            (val::lit(f64::NAN), "out_of_range"),
            (val::lit(f64::INFINITY), "out_of_range"),
            (val::col("v"), "agg_param_must_be_literal"),
        ] {
            let query = aggregate(grouped, arg, val::lit("|"))
                .where_eq("v", -1)
                .build();
            let error = resolver.0.read(&query, &ctx).await.unwrap_err();
            assert!(
                matches!(&error, DbError::Validation(code) if code == expected),
                "{error:?}",
            );
        }
        let query = Query::from("rows")
            .select([select::agg_fn_with_args(
                "percentile",
                "v",
                "pct",
                [val::lit(0.5), val::lit(1)],
            )])
            .build();
        let error = resolver.0.read(&query, &ctx).await.unwrap_err();
        assert!(
            matches!(&error, DbError::Validation(code) if code == "agg_param_arity"),
            "{error:?}",
        );
        let query = aggregate(grouped, val::lit(0.5), val::param("bad_range")).build();
        let error = resolver.0.read(&query, &ctx).await.unwrap_err();
        assert!(
            matches!(&error, DbError::Validation(code) if code == "type_mismatch"),
            "{error:?}",
        );
    }
}
