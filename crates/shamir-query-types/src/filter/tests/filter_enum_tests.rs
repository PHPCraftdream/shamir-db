use crate::filter::filter_enum::{check_filter_depth, Filter};
use crate::filter::FilterValue;

#[test]
fn fts_serde_round_trip() {
    let json = r#"{"op":"fts","field":["body"],"query":"hello world","mode":"and"}"#;
    let f: Filter = serde_json::from_str(json).unwrap();
    match &f {
        Filter::Fts { field, query, mode } => {
            assert_eq!(field, &["body"]);
            assert_eq!(query, "hello world");
            assert_eq!(mode, "and");
        }
        _ => panic!("expected Fts"),
    }
    let back = serde_json::to_string(&f).unwrap();
    let f2: Filter = serde_json::from_str(&back).unwrap();
    assert_eq!(f, f2);
}

#[test]
fn fts_default_mode_is_and() {
    let json = r#"{"op":"fts","field":["body"],"query":"test"}"#;
    let f: Filter = serde_json::from_str(json).unwrap();
    match &f {
        Filter::Fts { mode, .. } => assert_eq!(mode, "and"),
        _ => panic!("expected Fts"),
    }
}

#[test]
fn vector_similarity_serde_round_trip() {
    let json = r#"{"op":"vector_similarity","field":["emb"],"query":[1.0,0.0,0.5],"k":10}"#;
    let f: Filter = serde_json::from_str(json).unwrap();
    match &f {
        Filter::VectorSimilarity { field, query, k } => {
            assert_eq!(field, &["emb"]);
            assert_eq!(query.len(), 3);
            assert_eq!(*k, 10);
        }
        _ => panic!("expected VectorSimilarity"),
    }
    let back = serde_json::to_string(&f).unwrap();
    let f2: Filter = serde_json::from_str(&back).unwrap();
    assert_eq!(f, f2);
}

#[test]
fn computed_serde_round_trip() {
    let json =
        r#"{"op":"computed","expr_op":"lower","field":["email"],"cmp":"eq","value":"alice"}"#;
    let f: Filter = serde_json::from_str(json).unwrap();
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
    let back = serde_json::to_string(&f).unwrap();
    let f2: Filter = serde_json::from_str(&back).unwrap();
    assert_eq!(f, f2);
}

#[test]
fn existing_eq_still_works() {
    let json = r#"{"op":"eq","field":["age"],"value":30}"#;
    let f: Filter = serde_json::from_str(json).unwrap();
    assert!(matches!(f, Filter::Eq { .. }));
}

#[test]
fn field_accepts_bare_string_as_single_segment() {
    // Ergonomic: a top-level field may be written as a bare string.
    let json = r#"{"op":"eq","field":"id","value":"user:42"}"#;
    let f: Filter = serde_json::from_str(json).unwrap();
    match &f {
        Filter::Eq { field, .. } => assert_eq!(field, &["id"]),
        _ => panic!("expected Eq"),
    }
    // Serializes back to the canonical array form, which re-parses.
    let back = serde_json::to_string(&f).unwrap();
    assert!(back.contains(r#""field":["id"]"#), "serialized: {back}");
    let f2: Filter = serde_json::from_str(&back).unwrap();
    assert_eq!(f, f2);
}

#[test]
fn field_accepts_nested_path_array() {
    let json = r#"{"op":"eq","field":["address","city"],"value":"NY"}"#;
    let f: Filter = serde_json::from_str(json).unwrap();
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
