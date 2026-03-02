//! Tests for the read query execution pipeline (exec.rs).

use serde_json::json;

use crate::core::interner::{InternerKey, Interner, TouchInd};
use crate::db::query::filter::eval_context::FilterContext;
use crate::db::query::filter::{Filter, FilterValue};
use crate::db::query::read::exec::*;
use crate::db::query::read::*;
use crate::types::common::new_map;
use crate::types::record_id::RecordId;
use crate::types::value::InnerValue;

/// Helper: intern a string and return its u64 id.
fn intern(interner: &Interner, s: &str) -> u64 {
    match interner.touch_ind(s) {
        Ok(TouchInd::New(k)) | Ok(TouchInd::Exists(k)) => k.id(),
        Err(e) => panic!("intern failed: {}", e),
    }
}

/// Build a simple record: `{ "name": Str, "age": Int, "city": Str }`.
fn make_record(interner: &Interner, name: &str, age: i64, city: &str) -> InnerValue {
    let mut map = new_map();
    map.insert(InternerKey::new(intern(interner, "name")), InnerValue::Str(name.into()));
    map.insert(InternerKey::new(intern(interner, "age")), InnerValue::Int(age));
    map.insert(InternerKey::new(intern(interner, "city")), InnerValue::Str(city.into()));
    InnerValue::Map(map)
}

fn make_records(interner: &Interner) -> Vec<(RecordId, InnerValue)> {
    vec![
        (RecordId::new(), make_record(interner, "Alice", 30, "NYC")),
        (RecordId::new(), make_record(interner, "Bob", 25, "LA")),
        (RecordId::new(), make_record(interner, "Carol", 35, "NYC")),
        (RecordId::new(), make_record(interner, "Dave", 25, "LA")),
    ]
}

// ============================================================================
// apply_select tests
// ============================================================================

#[test]
fn select_all() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let select = Select::all();

    let result = apply_select(&records, &select, &interner);
    assert_eq!(result.len(), 4);
    assert_eq!(result[0]["name"], "Alice");
    assert_eq!(result[0]["age"], 30);
}

#[test]
fn select_specific_fields() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let select = Select::fields(["name", "age"]);

    let result = apply_select(&records, &select, &interner);
    assert_eq!(result.len(), 4);
    assert_eq!(result[0]["name"], "Alice");
    assert_eq!(result[0]["age"], 30);
    // city should not be present
    assert!(result[0].get("city").is_none());
}

#[test]
fn select_with_alias() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let select = Select {
        items: vec![SelectItem::Field {
            path: "name".into(),
            alias: Some("user_name".into()),
        }],
        distinct: false,
    };

    let result = apply_select(&records, &select, &interner);
    assert_eq!(result[0]["user_name"], "Alice");
    assert!(result[0].get("name").is_none());
}

#[test]
fn select_nonexistent_field_returns_null() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let select = Select::fields(["name", "nonexistent"]);

    let result = apply_select(&records, &select, &interner);
    assert_eq!(result[0]["name"], "Alice");
    assert!(result[0]["nonexistent"].is_null());
}

// ============================================================================
// apply_group_by tests
// ============================================================================

#[test]
fn group_by_count() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);

    let group_by = GroupBy::new(["city"]);
    let select = Select {
        items: vec![
            SelectItem::Field {
                path: "city".into(),
                alias: None,
            },
            SelectItem::CountAll {
                alias: Some("cnt".into()),
            },
        ],
        distinct: false,
    };

    let result = apply_group_by(&records, &group_by, &select, &interner, &ctx);
    assert_eq!(result.len(), 2);

    // BTreeMap is sorted by serialized key, so "LA" < "NYC"
    assert_eq!(result[0]["city"], "LA");
    assert_eq!(result[0]["cnt"], 2);
    assert_eq!(result[1]["city"], "NYC");
    assert_eq!(result[1]["cnt"], 2);
}

#[test]
fn group_by_sum_avg() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);

    let group_by = GroupBy::new(["city"]);
    let select = Select {
        items: vec![
            SelectItem::Field {
                path: "city".into(),
                alias: None,
            },
            SelectItem::Aggregate {
                func: AggFunc::Sum,
                field: AggregateField::Field("age".into()),
                alias: Some("total_age".into()),
                distinct: false,
            },
            SelectItem::Aggregate {
                func: AggFunc::Avg,
                field: AggregateField::Field("age".into()),
                alias: Some("avg_age".into()),
                distinct: false,
            },
        ],
        distinct: false,
    };

    let result = apply_group_by(&records, &group_by, &select, &interner, &ctx);
    // LA: Bob(25) + Dave(25) = 50, avg = 25
    assert_eq!(result[0]["city"], "LA");
    assert_eq!(result[0]["total_age"], 50);
    assert_eq!(result[0]["avg_age"], 25.0);

    // NYC: Alice(30) + Carol(35) = 65, avg = 32.5
    assert_eq!(result[1]["city"], "NYC");
    assert_eq!(result[1]["total_age"], 65);
    assert_eq!(result[1]["avg_age"], 32.5);
}

#[test]
fn group_by_min_max() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);

    let group_by = GroupBy::new(["city"]);
    let select = Select {
        items: vec![
            SelectItem::Field {
                path: "city".into(),
                alias: None,
            },
            SelectItem::Aggregate {
                func: AggFunc::Min,
                field: AggregateField::Field("age".into()),
                alias: Some("min_age".into()),
                distinct: false,
            },
            SelectItem::Aggregate {
                func: AggFunc::Max,
                field: AggregateField::Field("age".into()),
                alias: Some("max_age".into()),
                distinct: false,
            },
        ],
        distinct: false,
    };

    let result = apply_group_by(&records, &group_by, &select, &interner, &ctx);
    // LA: min=25, max=25
    assert_eq!(result[0]["min_age"], 25);
    assert_eq!(result[0]["max_age"], 25);
    // NYC: min=30, max=35
    assert_eq!(result[1]["min_age"], 30);
    assert_eq!(result[1]["max_age"], 35);
}

#[test]
fn group_by_having() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);

    // HAVING total_age > 55
    let group_by = GroupBy::new(["city"]).having(Filter::Gt {
        field: "total_age".into(),
        value: FilterValue::Int(55),
    });

    let select = Select {
        items: vec![
            SelectItem::Field {
                path: "city".into(),
                alias: None,
            },
            SelectItem::Aggregate {
                func: AggFunc::Sum,
                field: AggregateField::Field("age".into()),
                alias: Some("total_age".into()),
                distinct: false,
            },
        ],
        distinct: false,
    };

    let result = apply_group_by(&records, &group_by, &select, &interner, &ctx);
    // Only NYC (65) passes, LA (50) doesn't
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["city"], "NYC");
}

#[test]
fn group_by_multiple_fields() {
    let interner = Interner::default();
    let records = vec![
        (RecordId::new(), make_record(&interner, "Alice", 25, "NYC")),
        (RecordId::new(), make_record(&interner, "Bob", 25, "NYC")),
        (RecordId::new(), make_record(&interner, "Carol", 30, "NYC")),
    ];
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);

    let group_by = GroupBy::new(["city", "age"]);
    let select = Select {
        items: vec![
            SelectItem::Field {
                path: "city".into(),
                alias: None,
            },
            SelectItem::Field {
                path: "age".into(),
                alias: None,
            },
            SelectItem::CountAll {
                alias: Some("cnt".into()),
            },
        ],
        distinct: false,
    };

    let result = apply_group_by(&records, &group_by, &select, &interner, &ctx);
    assert_eq!(result.len(), 2); // (NYC,25) and (NYC,30)
}

#[test]
fn group_by_empty_input() {
    let interner = Interner::default();
    let records: Vec<(RecordId, InnerValue)> = vec![];
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);
    let group_by = GroupBy::new(["city"]);
    let select = Select {
        items: vec![SelectItem::CountAll { alias: None }],
        distinct: false,
    };

    let result = apply_group_by(&records, &group_by, &select, &interner, &ctx);
    assert!(result.is_empty());
}

// ============================================================================
// apply_aggregate_all tests
// ============================================================================

#[test]
fn aggregate_all_count_sum() {
    let interner = Interner::default();
    let records = make_records(&interner);

    let select = Select {
        items: vec![
            SelectItem::CountAll {
                alias: Some("total".into()),
            },
            SelectItem::Aggregate {
                func: AggFunc::Sum,
                field: AggregateField::Field("age".into()),
                alias: Some("sum_age".into()),
                distinct: false,
            },
        ],
        distinct: false,
    };

    let result = apply_aggregate_all(&records, &select, &interner);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["total"], 4);
    assert_eq!(result[0]["sum_age"], 115); // 30+25+35+25
}

// ============================================================================
// apply_order_by tests
// ============================================================================

#[test]
fn order_by_asc() {
    let mut records = vec![
        json!({"name": "Carol", "age": 35}),
        json!({"name": "Alice", "age": 30}),
        json!({"name": "Bob", "age": 25}),
    ];

    apply_order_by(&mut records, &OrderBy::asc("age"));
    assert_eq!(records[0]["age"], 25);
    assert_eq!(records[1]["age"], 30);
    assert_eq!(records[2]["age"], 35);
}

#[test]
fn order_by_desc() {
    let mut records = vec![
        json!({"name": "Alice", "age": 30}),
        json!({"name": "Bob", "age": 25}),
        json!({"name": "Carol", "age": 35}),
    ];

    apply_order_by(&mut records, &OrderBy::desc("age"));
    assert_eq!(records[0]["age"], 35);
    assert_eq!(records[1]["age"], 30);
    assert_eq!(records[2]["age"], 25);
}

#[test]
fn order_by_multiple_fields() {
    let mut records = vec![
        json!({"city": "NYC", "age": 35}),
        json!({"city": "LA", "age": 30}),
        json!({"city": "LA", "age": 25}),
        json!({"city": "NYC", "age": 30}),
    ];

    let order = OrderBy::new([OrderByItem::asc("city"), OrderByItem::asc("age")]);
    apply_order_by(&mut records, &order);

    assert_eq!(records[0]["city"], "LA");
    assert_eq!(records[0]["age"], 25);
    assert_eq!(records[1]["city"], "LA");
    assert_eq!(records[1]["age"], 30);
    assert_eq!(records[2]["city"], "NYC");
    assert_eq!(records[2]["age"], 30);
    assert_eq!(records[3]["city"], "NYC");
    assert_eq!(records[3]["age"], 35);
}

#[test]
fn order_by_nulls_first() {
    let mut records = vec![
        json!({"name": "Alice", "age": 30}),
        json!({"name": "Bob"}),
        json!({"name": "Carol", "age": 25}),
    ];

    let order = OrderBy::new([OrderByItem::asc("age").nulls_first()]);
    apply_order_by(&mut records, &order);

    assert!(records[0].get("age").is_none() || records[0]["age"].is_null());
    assert_eq!(records[1]["age"], 25);
    assert_eq!(records[2]["age"], 30);
}

#[test]
fn order_by_nulls_last() {
    let mut records = vec![
        json!({"name": "Bob"}),
        json!({"name": "Alice", "age": 30}),
        json!({"name": "Carol", "age": 25}),
    ];

    let order = OrderBy::new([OrderByItem::asc("age").nulls_last()]);
    apply_order_by(&mut records, &order);

    assert_eq!(records[0]["age"], 25);
    assert_eq!(records[1]["age"], 30);
    assert!(records[2].get("age").is_none() || records[2]["age"].is_null());
}

// ============================================================================
// apply_pagination tests
// ============================================================================

#[test]
fn pagination_limit_offset() {
    let records = vec![json!(1), json!(2), json!(3), json!(4), json!(5)];

    let pagination = Pagination::LimitOffset {
        limit: Some(2),
        offset: 1,
    };
    let (result, info) = apply_pagination(records, &pagination, true);

    assert_eq!(result, vec![json!(2), json!(3)]);
    let info = info.unwrap();
    assert_eq!(info.total_count, Some(5));
    assert!(info.has_next);
    assert!(info.has_prev);
}

#[test]
fn pagination_page_based() {
    let records = vec![json!(1), json!(2), json!(3), json!(4), json!(5)];

    let pagination = Pagination::page(2, 2);
    let (result, info) = apply_pagination(records, &pagination, true);

    assert_eq!(result, vec![json!(3), json!(4)]);
    let info = info.unwrap();
    assert_eq!(info.total_count, Some(5));
    assert_eq!(info.current_page, Some(2));
    assert!(info.has_next);
    assert!(info.has_prev);
}

#[test]
fn pagination_count_total_false() {
    let records = vec![json!(1), json!(2), json!(3)];

    let pagination = Pagination::LimitOffset {
        limit: Some(2),
        offset: 0,
    };
    let (result, info) = apply_pagination(records, &pagination, false);

    assert_eq!(result, vec![json!(1), json!(2)]);
    let info = info.unwrap();
    assert_eq!(info.total_count, None);
}

#[test]
fn pagination_none_no_count() {
    let records = vec![json!(1), json!(2)];
    let (result, info) = apply_pagination(records, &Pagination::None, false);
    assert_eq!(result, vec![json!(1), json!(2)]);
    assert!(info.is_none());
}

// ============================================================================
// apply_distinct tests
// ============================================================================

#[test]
fn distinct_removes_duplicates() {
    let records = vec![
        json!({"a": 1}),
        json!({"a": 2}),
        json!({"a": 1}),
        json!({"a": 3}),
        json!({"a": 2}),
    ];

    let result = apply_distinct(records);
    assert_eq!(result.len(), 3);
    assert_eq!(result[0], json!({"a": 1}));
    assert_eq!(result[1], json!({"a": 2}));
    assert_eq!(result[2], json!({"a": 3}));
}

// ============================================================================
// has_aggregates tests
// ============================================================================

#[test]
fn has_aggregates_true() {
    let select = Select {
        items: vec![
            SelectItem::Field {
                path: "name".into(),
                alias: None,
            },
            SelectItem::CountAll { alias: None },
        ],
        distinct: false,
    };
    assert!(has_aggregates(&select));
}

#[test]
fn has_aggregates_false() {
    let select = Select::fields(["name", "age"]);
    assert!(!has_aggregates(&select));
}
