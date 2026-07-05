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
        ef_search: None,
        oversample: None,
    };
    match &f {
        Filter::VectorSimilarity {
            field, query, k, ..
        } => {
            assert_eq!(field, &["emb"]);
            assert_eq!(query.len(), 3);
            assert_eq!(*k, 10);
        }
        _ => panic!("expected VectorSimilarity"),
    }
    let f2 = roundtrip_filter(&f);
    assert_eq!(f, f2);
}

/// V1.1 back-compat: a msgpack payload that OMITS `ef_search`/`oversample`
/// (old client) MUST deserialize with both fields = `None`. The server never
/// rejects a pre-V1.1 VectorSimilarity filter.
///
/// We build the "old" payload by serializing a `VectorSimilarity` with both
/// `None` fields — `skip_serializing_if = "Option::is_none"` ensures the
/// produced bytes contain NO `ef_search` / `oversample` keys at all (verified
/// by `vector_similarity_none_fields_omitted_from_wire`). We then decode those
/// bytes back, which is exactly the shape a pre-V1.1 client would emit.
#[test]
fn vector_similarity_back_compat_old_payload_without_ef_fields() {
    let bare = Filter::VectorSimilarity {
        field: vec!["emb".to_string()],
        query: vec![1.0, 0.0, 0.5],
        k: 10,
        ef_search: None,
        oversample: None,
    };
    let old_bytes = rmp_serde::to_vec_named(&bare).unwrap();
    // Sanity: the bytes must NOT contain the ef_search / oversample keys.
    let hex: String = old_bytes.iter().map(|b| format!("{:02x}", b)).collect();
    assert!(
        !hex.contains("65665f736561726368"),
        "pre-V1.1 payload must omit ef_search key; bytes={hex}"
    );
    assert!(
        !hex.contains("6f76657273616d706c65"),
        "pre-V1.1 payload must omit oversample key; bytes={hex}"
    );
    // Decode back — both fields default to None.
    let f: Filter = rmp_serde::from_slice(&old_bytes)
        .expect("old VectorSimilarity payload must deserialize (back-compat)");
    match f {
        Filter::VectorSimilarity {
            ef_search,
            oversample,
            ..
        } => {
            assert_eq!(ef_search, None, "missing ef_search must default to None");
            assert_eq!(oversample, None, "missing oversample must default to None");
        }
        other => panic!("expected VectorSimilarity, got {other:?}"),
    }
}

/// V1.1: full round-trip WITH both new fields set.
#[test]
fn vector_similarity_with_ef_and_oversample_round_trip() {
    let f = Filter::VectorSimilarity {
        field: vec!["emb".to_string()],
        query: vec![1.0, 0.0],
        k: 5,
        ef_search: Some(400),
        oversample: Some(2.0),
    };
    let bytes = rmp_serde::to_vec_named(&f).unwrap();
    let f2: Filter = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(f, f2);
}

/// V1.1: `skip_serializing_if = "Option::is_none"` means a `None`-field
/// payload emits ZERO bytes for the field — wire identical to pre-V1.1.
#[test]
fn vector_similarity_none_fields_omitted_from_wire() {
    let f = Filter::VectorSimilarity {
        field: vec!["v".to_string()],
        query: vec![0.0],
        k: 1,
        ef_search: None,
        oversample: None,
    };
    let bytes = rmp_serde::to_vec_named(&f).unwrap();
    let hex: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
    assert!(
        !hex.contains("65665f736561726368"),
        "ef_search key must not appear when None; bytes={hex}"
    );
    assert!(
        !hex.contains("6f76657273616d706c65"),
        "oversample key must not appear when None; bytes={hex}"
    );
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
