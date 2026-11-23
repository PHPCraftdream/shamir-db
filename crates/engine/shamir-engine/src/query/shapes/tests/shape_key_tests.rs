use shamir_query_types::filter::{Filter, FilterValue, ValueCompareOp};
use shamir_query_types::read::ReadQuery;
use shamir_query_types::write::UpdateOp;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::value::QueryValue;

use super::super::ShapeKey;
use crate::query::batch::BatchOp;

fn key(query: &ReadQuery, interner: &Interner) -> [u8; 16] {
    ShapeKey::fold(&BatchOp::Read(query.clone()), "t", interner).into_bytes()
}

fn eq(field: &str, value: FilterValue) -> Filter {
    Filter::Eq {
        field: vec![field.to_string()],
        value,
    }
}

#[test]
fn shape_folding_same_shape_diff_literals() {
    let interner = Interner::default();
    interner.touch_ind("age").unwrap();
    interner.touch_ind("other").unwrap();

    let low = ReadQuery::new("t").filter(eq("age", FilterValue::Int(5)));
    let high = ReadQuery::new("t").filter(eq("age", FilterValue::Int(999)));
    assert_eq!(key(&low, &interner), key(&high, &interner));

    let greater = ReadQuery::new("t").filter(Filter::Gt {
        field: vec!["age".to_string()],
        value: FilterValue::Int(5),
    });
    assert_ne!(key(&greater, &interner), key(&low, &interner));

    let ordered = ReadQuery::new("t").filter(Filter::And {
        filters: vec![
            eq("age", FilterValue::Int(5)),
            eq("other", FilterValue::Int(1)),
        ],
    });
    let reversed = ReadQuery::new("t").filter(Filter::And {
        filters: vec![
            eq("other", FilterValue::Int(1)),
            eq("age", FilterValue::Int(5)),
        ],
    });
    assert_ne!(key(&ordered, &interner), key(&reversed, &interner));

    let first_text = ReadQuery::new("t").filter(eq("age", FilterValue::String("alpha".into())));
    let second_text = ReadQuery::new("t").filter(eq("age", FilterValue::String("beta".into())));
    assert_eq!(key(&first_text, &interner), key(&second_text, &interner));

    let first_param = ReadQuery::new("t").filter(eq(
        "age",
        FilterValue::Param {
            name: "first".into(),
        },
    ));
    let second_param = ReadQuery::new("t").filter(eq(
        "age",
        FilterValue::Param {
            name: "second".into(),
        },
    ));
    assert_eq!(key(&first_param, &interner), key(&second_param, &interner));
}

#[test]
fn golden_value_compare_read_shape_is_pinned() {
    let interner = Interner::default();
    let query = ReadQuery::new("orders").filter(Filter::ValueCompare {
        left: FilterValue::Int(5),
        cmp: ValueCompareOp::Eq,
        right: FilterValue::Int(6),
    });
    let folded = ShapeKey::fold(&BatchOp::Read(query), "orders", &interner);
    assert_eq!(folded.to_hex(), "ed003f45996653fa69dc7b2be1af78af");
}

#[test]
fn unknown_fields_collapse_and_never_intern() {
    let interner = Interner::default();
    let before = interner.len();
    let mut keys = Vec::new();
    for index in 0..10 {
        let name = format!("missing_{index}");
        let query = ReadQuery::new("t").filter(eq(&name, FilterValue::Int(1)));
        keys.push(key(&query, &interner));
        assert!(interner.get_ind(&name).is_none());
    }
    assert!(keys.windows(2).all(|pair| pair[0] == pair[1]));
    assert_eq!(interner.len(), before);
}

fn update_key(update: UpdateOp, interner: &Interner) -> [u8; 16] {
    ShapeKey::fold(&BatchOp::Update(update), "t", interner).into_bytes()
}

fn update_with(fields: TMap<String, QueryValue>) -> UpdateOp {
    UpdateOp {
        update: shamir_query_types::TableRef::new("t"),
        where_clause: None,
        set: QueryValue::Map(fields),
        select: None,
        expected_version: None,
    }
}

/// The update fold uses the `set` map's insertion order (the map is an
/// order-preserving `IndexMap`) and never folds the values; the third map
/// differs from `ordered` only in VALUES.
#[test]
fn update_set_keys_fold_in_map_insertion_order_without_values() {
    let interner = Interner::default();
    interner.touch_ind("alpha").unwrap();
    interner.touch_ind("beta").unwrap();

    let mut unordered: TMap<String, QueryValue> = new_map();
    unordered.insert("beta".to_string(), QueryValue::Int(1));
    unordered.insert("alpha".to_string(), QueryValue::Int(2));

    let mut ordered: TMap<String, QueryValue> = new_map();
    ordered.insert("alpha".to_string(), QueryValue::Int(9));
    ordered.insert("beta".to_string(), QueryValue::Int(8));

    let mut same_order_new_values: TMap<String, QueryValue> = new_map();
    same_order_new_values.insert("alpha".to_string(), QueryValue::Int(70));
    same_order_new_values.insert("beta".to_string(), QueryValue::Int(80));

    assert_ne!(
        update_key(update_with(unordered), &interner),
        update_key(update_with(ordered.clone()), &interner)
    );
    assert_eq!(
        update_key(update_with(ordered), &interner),
        update_key(update_with(same_order_new_values), &interner)
    );
}

/// Kills the mutant that interns an unknown `set` key (and the one that emits a
/// per-key unknown tag instead of the single `tag(1)`).
#[test]
fn update_set_unknown_keys_collapse_and_never_intern() {
    let interner = Interner::default();
    let before = interner.len();
    let first = {
        let mut fields: TMap<String, QueryValue> = new_map();
        fields.insert("missing_alpha".to_string(), QueryValue::Int(1));
        update_key(update_with(fields), &interner)
    };
    let second = {
        let mut fields: TMap<String, QueryValue> = new_map();
        fields.insert("missing_beta".to_string(), QueryValue::Int(2));
        update_key(update_with(fields), &interner)
    };
    assert_eq!(first, second, "unknown set keys collapse to one tag");
    assert_eq!(interner.len(), before, "the fold never interns a set key");
    assert!(interner.get_ind("missing_alpha").is_none());
    assert!(interner.get_ind("missing_beta").is_none());
}

/// Kills the mutant that folds a field reference as an unknown path or drops the
/// interned segment id from a known path.
#[test]
fn golden_read_shape_with_a_field_reference_is_pinned() {
    let interner = Interner::default();
    interner.touch_ind("age").unwrap();
    assert_eq!(
        interner.get_ind("age").expect("age interned").id(),
        1,
        "the golden hex below assumes the interner's first id is 1"
    );
    let query = ReadQuery::new("orders").filter(Filter::Eq {
        field: vec!["age".to_string()],
        value: FilterValue::Int(5),
    });
    let folded = ShapeKey::fold(&BatchOp::Read(query), "orders", &interner);
    assert_eq!(folded.to_hex(), "1e041bd074979079dd633fe5fe57ca13");
}
