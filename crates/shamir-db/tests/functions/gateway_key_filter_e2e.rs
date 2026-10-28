//! Regression tests for fail-closed guest database key filters.

use std::sync::Arc;

use async_trait::async_trait;
use shamir_collections::{TMap, TSet};
use shamir_db::ShamirDb;
use shamir_engine::function::{FnBatch, FnCtx, FunctionError, Params, ShamirFunction};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_types::filter::FilterValue;
use shamir_types::types::value::QueryValue;

/// Probes `get` with a fixed key list and returns one outcome string per key.
struct KeyProbe;

fn probe_keys() -> Vec<QueryValue> {
    let map_of = |entries: Vec<(&str, QueryValue)>| -> QueryValue {
        QueryValue::Map(
            entries
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect::<TMap<String, QueryValue>>(),
        )
    };
    vec![
        map_of(vec![("tag", QueryValue::List(vec![QueryValue::Int(1)]))]),
        map_of(vec![("tag", map_of(vec![("k", QueryValue::Int(1))]))]),
        map_of(vec![(
            "tag",
            QueryValue::Set(
                [QueryValue::Int(1)]
                    .into_iter()
                    .collect::<TSet<QueryValue>>(),
            ),
        )]),
        map_of(vec![("tag", QueryValue::Dec(1.into()))]),
        map_of(vec![("tag", QueryValue::Big(1.into()))]),
        QueryValue::Big(1.into()),
        QueryValue::List(vec![QueryValue::Int(1)]),
        map_of(vec![("tag", QueryValue::Null)]),
        map_of(vec![("tag", QueryValue::Str("x".into()))]),
    ]
}

#[async_trait]
impl ShamirFunction for KeyProbe {
    async fn call(
        &self,
        ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        let gateway = ctx
            .db_gateway()
            .ok_or_else(|| FunctionError::Compute("no db gateway".into()))?;
        let mut outcomes = Vec::new();
        for key in probe_keys() {
            let outcome = match gateway.get("main", "items", key).await {
                Ok(Some(row)) => format!("row:{row:?}"),
                Ok(None) => "none".to_string(),
                Err(error) => format!("err:{error}"),
            };
            outcomes.push(QueryValue::Str(outcome));
        }
        Ok(QueryValue::List(outcomes))
    }
}

/// Exercises query conversion and supported scalar/match-all semantics.
struct QueryProbe;

#[async_trait]
impl ShamirFunction for QueryProbe {
    async fn call(
        &self,
        ctx: &FnCtx,
        _batch: &FnBatch,
        _params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        let gateway = ctx
            .db_gateway()
            .ok_or_else(|| FunctionError::Compute("no db gateway".into()))?;
        let map_of = |entries: Vec<(&str, QueryValue)>| -> QueryValue {
            QueryValue::Map(
                entries
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect(),
            )
        };
        let list_filter = map_of(vec![("tag", QueryValue::List(vec![QueryValue::Int(1)]))]);
        let list_error = gateway.query("main", "items", Some(list_filter)).await;
        let scalar_list_error = gateway
            .query(
                "main",
                "items",
                Some(QueryValue::List(vec![QueryValue::Int(1)])),
            )
            .await;
        let scalar_int = gateway.get("main", "items", QueryValue::Int(1)).await;
        let empty_map_rows = gateway
            .query("main", "items", Some(map_of(Vec::new())))
            .await;
        let all_rows = gateway.query("main", "items", None).await;
        let mut outcomes = Vec::new();
        outcomes.push(QueryValue::Str(match list_error {
            Err(error) => format!("err:{error}"),
            Ok(rows) => format!("rows:{}", rows.len()),
        }));
        outcomes.push(QueryValue::Str(match scalar_list_error {
            Err(error) => format!("err:{error}"),
            Ok(rows) => format!("rows:{}", rows.len()),
        }));
        outcomes.push(QueryValue::Str(match scalar_int {
            Ok(Some(row)) => format!("row:{row:?}"),
            Ok(None) => "none".to_string(),
            Err(error) => format!("err:{error}"),
        }));
        outcomes.push(QueryValue::Str(match empty_map_rows {
            Ok(rows) => format!("rows:{}", rows.len()),
            Err(error) => format!("err:{error}"),
        }));
        outcomes.push(QueryValue::Str(match all_rows {
            Ok(rows) => format!("rows:{}", rows.len()),
            Err(error) => format!("err:{error}"),
        }));
        Ok(QueryValue::List(outcomes))
    }
}

#[tokio::test]
async fn rv_sdk_f956c637_unsupported_key_shapes_fail_closed() {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("testdb").await.unwrap();
    let mut setup = Batch::new();
    setup.id("setup");
    setup.create_repo(
        "repo",
        ddl::create_repo("main")
            .engine("in_memory")
            .tables(["items"]),
    );
    db.execute("testdb", &setup.to_request_via_msgpack().unwrap())
        .await
        .unwrap();

    let mut seed = Batch::new();
    seed.id("seed");
    seed.insert(
        "rows",
        insert("items").rows([
            doc! { "id" => QueryValue::Int(1), "tag" => QueryValue::Null },
            doc! { "id" => QueryValue::Int(2), "tag" => "x" },
        ]),
    );
    db.execute("testdb", &seed.to_request_via_msgpack().unwrap())
        .await
        .unwrap();

    let wasm = wat::parse_str("(module)").unwrap();
    db.create_function_from_wasm("key_probe", &wasm, false)
        .await
        .unwrap();
    db.functions().replace("key_probe", Arc::new(KeyProbe));
    db.create_function_from_wasm("query_probe", &wasm, false)
        .await
        .unwrap();
    db.functions().replace("query_probe", Arc::new(QueryProbe));

    let mut call = Batch::new();
    call.id("invoke");
    call.call("result", "key_probe", [] as [FilterValue; 0]);
    let response = db
        .execute("testdb", &call.to_request_via_msgpack().unwrap())
        .await
        .unwrap();
    let Some(QueryValue::List(outcomes)) = response.results["result"].value.as_ref() else {
        panic!("probe must return a list of outcomes");
    };
    let text: Vec<String> = outcomes
        .iter()
        .map(|o| match o {
            QueryValue::Str(s) => s.clone(),
            other => panic!("unexpected outcome {other:?}"),
        })
        .collect();

    assert!(
        text[0].starts_with("err:") && text[0].contains("unsupported key value List for field tag")
    );
    assert!(
        text[1].starts_with("err:") && text[1].contains("unsupported key value Map for field tag")
    );
    assert!(
        text[2].starts_with("err:") && text[2].contains("unsupported key value Set for field tag")
    );
    assert!(text[3].contains("unsupported key value Dec for field tag"));
    assert!(text[4].contains("unsupported key value Big for field tag"));
    assert!(text[5].contains("unsupported key value Big for field id"));
    assert!(text[6].contains("unsupported key value List for field id"));
    assert!(
        text[7].starts_with("row:") && text[7].contains("Null"),
        "tag Null must select row A: {}",
        text[7]
    );
    assert!(
        text[8].starts_with("row:") && text[8].contains("Str(\"x\")"),
        "tag x must select row B: {}",
        text[8]
    );

    let mut query_call = Batch::new();
    query_call.id("query-invoke");
    query_call.call("result", "query_probe", [] as [FilterValue; 0]);
    let query_response = db
        .execute("testdb", &query_call.to_request_via_msgpack().unwrap())
        .await
        .unwrap();
    let Some(QueryValue::List(query_outcomes)) = query_response.results["result"].value.as_ref()
    else {
        panic!("query probe must return a list of outcomes");
    };
    let query_text: Vec<String> = query_outcomes
        .iter()
        .map(|outcome| match outcome {
            QueryValue::Str(value) => value.clone(),
            other => panic!("unexpected query outcome {other:?}"),
        })
        .collect();
    assert!(
        query_text[0].starts_with("err:")
            && query_text[0].contains("unsupported key value List for field tag")
    );
    assert!(
        query_text[1].starts_with("err:")
            && query_text[1].contains("unsupported key value List for field id")
    );
    assert!(query_text[2].starts_with("row:") && query_text[2].contains("Int(1)"));
    assert_eq!(query_text[3], "rows:2", "empty map should match both rows");
    assert_eq!(query_text[4], "rows:2", "no filter should match both rows");
}
