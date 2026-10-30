//! RU8 (RV-TYPES-2EB39DFD) acceptance 8: `pre_intern_select_keys` must
//! surface the typed interner quota error (a HAVING on a new alias at the
//! cap fails with `interner_quota_exceeded`), not silently absorb it.

use crate::query::filter::eval_context::FilterContext;
use crate::query::read::aggregation::group_execution::try_apply_group_by;
use crate::query::read::tests::query_results::select_order_page::helpers::{
    make_records, to_bytes_records,
};
use crate::query::read::GroupBy;
use shamir_query_builder::select;
use shamir_types::types::common::new_map;

use crate::query::read::exec::pre_intern_select_keys;
use shamir_query_types::read::select::{Select, SelectItem};
use shamir_storage::error::DbError;
use shamir_types::core::interner::{Interner, InternerQuota};
use shamir_types::{mpack, types::value::QueryValue};

#[test]
fn downstream_mpack_negative_object_values() {
    let objects = [
        mpack!({
            "first": -1,
            "middle": -2.5,
            "last": -3,
            "nested": {
                "first": -4.5,
                "middle": -5,
                "last": -6.5
            }
        }),
        mpack!({
            "first": -1,
            "middle": -2.5,
            "last": -3,
            "nested": {
                "first": -4.5,
                "middle": -5,
                "last": -6.5,
            },
        }),
        mpack!({
            "first": -1.5,
            "middle": -2,
            "last": -3.5,
            "nested": {
                "first": -4,
                "middle": -5.5,
                "last": -6
            }
        }),
        mpack!({
            "first": -1.5,
            "middle": -2,
            "last": -3.5,
            "nested": {
                "first": -4,
                "middle": -5.5,
                "last": -6,
            },
        }),
    ];
    let final_values = [
        mpack!({
            "other": true,
            "last": -7
        }),
        mpack!({
            "other": true,
            "last": -7,
        }),
        mpack!({
            "other": true,
            "last": -7.5
        }),
        mpack!({
            "other": true,
            "last": -7.5,
        }),
    ];
    for (index, object) in final_values.iter().enumerate() {
        let expected = if index < 2 {
            QueryValue::Int(-7)
        } else {
            QueryValue::F64(-7.5)
        };
        assert_eq!(object.get_path("last"), Some(&expected));
    }
    for (index, object) in objects.iter().enumerate() {
        let expected = if index < 2 {
            [
                QueryValue::Int(-1),
                QueryValue::F64(-2.5),
                QueryValue::Int(-3),
                QueryValue::F64(-4.5),
                QueryValue::Int(-5),
                QueryValue::F64(-6.5),
            ]
        } else {
            [
                QueryValue::F64(-1.5),
                QueryValue::Int(-2),
                QueryValue::F64(-3.5),
                QueryValue::Int(-4),
                QueryValue::F64(-5.5),
                QueryValue::Int(-6),
            ]
        };
        for (path, value) in [
            "first",
            "middle",
            "last",
            "nested.first",
            "nested.middle",
            "nested.last",
        ]
        .iter()
        .zip(expected.iter())
        {
            assert_eq!(object.get_path(path), Some(value));
        }
    }
}

fn select_with_alias(alias: &str) -> Select {
    Select {
        items: vec![SelectItem::Field {
            path: vec!["value".to_string()],
            alias: Some(alias.to_string()),
        }],
        distinct: false,
    }
}

#[test]
fn rv_types_2eb39dfd_pre_intern_select_keys_at_cap_returns_typed_error() {
    let interner = Interner::with_quota(
        Vec::new(),
        InternerQuota {
            max_names: 1,
            max_bytes: 1,
        },
    )
    .unwrap();

    // Alias interned first consumes the whole quota budget.
    let occupied = select_with_alias("a");
    pre_intern_select_keys(&occupied, &interner, |name| {
        interner.get_ind(name).map(|id| id.id())
    })
    .unwrap();

    // The next new alias is metered and must fail with the wire code.
    let over = select_with_alias("toolong");
    let err = pre_intern_select_keys(&over, &interner, |name| {
        interner.get_ind(name).map(|id| id.id())
    })
    .unwrap_err();
    assert!(matches!(err, DbError::InternerQuotaExceeded { .. }));
    assert_eq!(err.code(), Some("interner_quota_exceeded"));
}

#[test]
fn rv_types_2eb39dfd_pre_intern_select_keys_under_cap_ok() {
    let interner = Interner::with_quota(
        Vec::new(),
        InternerQuota {
            max_names: 16,
            max_bytes: 1024,
        },
    )
    .unwrap();
    pre_intern_select_keys(&select_with_alias("total"), &interner, |name| {
        interner.get_ind(name).map(|id| id.id())
    })
    .unwrap();
}

#[test]
fn rv_types_2eb39dfd_having_new_alias_at_cap_surfaces_code() {
    // Quota admits exactly the three record field names; the HAVING alias
    // ("total_age") is the next NEW name and must be rejected.
    let interner = Interner::with_quota(
        Vec::new(),
        InternerQuota {
            max_names: 3,
            max_bytes: 1_048_576,
        },
    )
    .unwrap();
    let records = make_records(&interner);
    assert_eq!(interner.quota_usage().0, 3);
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);

    let group_by = GroupBy {
        fields: vec![vec!["city".into()]],
        having: Some(shamir_query_builder::filter::gt("total_age", 0)),
    };
    let select = Select {
        items: vec![select::field("city"), select::sum("age", "total_age")],
        distinct: false,
    };

    let err = try_apply_group_by(
        &to_bytes_records(&records),
        &group_by,
        &select,
        &interner,
        &ctx,
    )
    .expect_err("HAVING on a new alias at the cap must fail");
    assert!(matches!(err, DbError::InternerQuotaExceeded { .. }));
    assert_eq!(err.code(), Some("interner_quota_exceeded"));
}
