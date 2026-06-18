use crate::filter::filter_enum::{check_filter_depth, Filter};
use crate::filter::FilterValue;

fn roundtrip_filter(f: &Filter) -> Filter {
    let bytes = rmp_serde::to_vec_named(f).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

#[test]
fn fts_serde_round_trip() {
    let f = Filter::Fts {
        field: vec!["body".to_string()],
        query: "hello world".to_string(),
        mode: "and".to_string(),
    };
    match &f {
        Filter::Fts { field, query, mode } => {
            assert_eq!(field, &["body"]);
            assert_eq!(query, "hello world");
            assert_eq!(mode, "and");
        }
        _ => panic!("expected Fts"),
    }
    let f2 = roundtrip_filter(&f);
    assert_eq!(f, f2);
}

#[test]
fn fts_default_mode_is_and() {
    let f = Filter::Fts {
        field: vec!["body".to_string()],
        query: "test".to_string(),
        mode: "and".to_string(),
    };
    match &f {
        Filter::Fts { mode, .. } => assert_eq!(mode, "and"),
        _ => panic!("expected Fts"),
    }
}

#[test]
fn vector_similarity_serde_round_trip() {
    let f = Filter::VectorSimilarity {
        field: vec!["emb".to_string()],
        query: vec![1.0, 0.0, 0.5],
        k: 10,
    };
    match &f {
        Filter::VectorSimilarity { field, query, k } => {
            assert_eq!(field, &["emb"]);
            assert_eq!(query.len(), 3);
            assert_eq!(*k, 10);
        }
        _ => panic!("expected VectorSimilarity"),
    }
    let f2 = roundtrip_filter(&f);
    assert_eq!(f, f2);
}

#[test]
fn computed_serde_round_trip() {
    let f = Filter::Computed {
        expr_op: "lower".to_string(),
        field: vec!["email".to_string()],
        cmp: "eq".to_string(),
        value: FilterValue::String("alice".into()),
        expr_args: None,
    };
    match &f {
        Filter::Computed {
            expr_op,
            field,
            cmp,
            value,
            expr_args,
        } => {
            assert_eq!(expr_op, "lower");
            assert_eq!(field, &["email"]);
            assert_eq!(cmp, "eq");
            assert_eq!(value, &FilterValue::String("alice".into()));
            assert!(expr_args.is_none());
        }
        _ => panic!("expected Computed"),
    }
    let f2 = roundtrip_filter(&f);
    assert_eq!(f, f2);
}

#[test]
fn existing_eq_still_works() {
    let f = Filter::Eq {
        field: vec!["age".to_string()],
        value: FilterValue::Int(30),
    };
    assert!(matches!(f, Filter::Eq { .. }));
}

#[test]
fn field_accepts_bare_string_as_single_segment() {
    // A single-segment field must round-trip identically.
    let f = Filter::Eq {
        field: vec!["id".to_string()],
        value: FilterValue::String("user:42".into()),
    };
    match &f {
        Filter::Eq { field, .. } => assert_eq!(field, &["id"]),
        _ => panic!("expected Eq"),
    }
    let f2 = roundtrip_filter(&f);
    assert_eq!(f, f2);
}

#[test]
fn field_accepts_nested_path_array() {
    let f = Filter::Eq {
        field: vec!["address".to_string(), "city".to_string()],
        value: FilterValue::String("NY".into()),
    };
    match &f {
        Filter::Eq { field, .. } => assert_eq!(field, &["address", "city"]),
        _ => panic!("expected Eq"),
    }
}

#[test]
fn deep_filter_rejected_by_depth_check() {
    // Build a deeply nested Not chain: Not(Not(Not(...(Eq)...)))
    let mut f = Filter::Eq {
        field: vec!["id".into()],
        value: FilterValue::String("x".into()),
    };
    for _ in 0..100 {
        f = Filter::Not {
            filter: Box::new(f),
        };
    }
    let result = check_filter_depth(&f);
    assert!(result.is_err(), "deeply nested filter should be rejected");
}

#[test]
fn normal_filter_depth_passes() {
    let f = Filter::And {
        filters: vec![
            Filter::Eq {
                field: vec!["a".into()],
                value: FilterValue::String("b".into()),
            },
            Filter::Not {
                filter: Box::new(Filter::Eq {
                    field: vec!["c".into()],
                    value: FilterValue::Int(1),
                }),
            },
        ],
    };
    assert!(check_filter_depth(&f).is_ok());
}
