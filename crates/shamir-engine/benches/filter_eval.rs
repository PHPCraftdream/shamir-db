//! Hot-loop bench for filter `matches()` on 1000 records.
//!
//! Each callback walks the record via `resolve_field` for each path. The
//! baseline implementation returns `Option<InnerValue>`, cloning the
//! resolved value before the comparison even runs. The intended
//! optimisation returns a borrow (`Option<&InnerValue>`), avoiding the
//! clone entirely on a path that runs once per record per predicate.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};

use shamir_engine::query::filter::Filter;
use shamir_engine::query::filter::{compile_filter, FilterContext};
use shamir_query_types::filter::FilterValue;

use shamir_types::core::interner::{Interner, TouchInd};
use shamir_types::types::common::{new_map_wc, TMap};
use shamir_types::types::value::InnerValue;

fn intern(i: &Interner, s: &str) {
    let _ = i.touch_ind(s);
}

fn make_record(interner: &Interner, idx: u32) -> InnerValue {
    let touch = |s: &str| match interner.touch_ind(s).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k,
    };
    let mut m = new_map_wc(10);
    m.insert(touch("id"), InnerValue::Int(idx as i64));
    m.insert(touch("name"), InnerValue::Str(format!("user-{}", idx)));
    m.insert(touch("age"), InnerValue::Int((idx % 100) as i64));
    m.insert(touch("score"), InnerValue::F64(idx as f64 * 1.5));
    m.insert(touch("active"), InnerValue::Bool(idx % 2 == 0));
    m.insert(
        touch("email"),
        InnerValue::Str(format!("u{}@example.com", idx)),
    );
    m.insert(touch("tags"), {
        InnerValue::List(vec![
            InnerValue::Str("alpha".into()),
            InnerValue::Str("beta".into()),
            InnerValue::Str("gamma".into()),
        ])
    });
    m.insert(touch("address"), {
        let mut a = new_map_wc(3);
        a.insert(touch("city"), InnerValue::Str("Jerusalem".into()));
        a.insert(touch("zip"), InnerValue::Str("9100000".into()));
        a.insert(touch("country"), InnerValue::Str("IL".into()));
        InnerValue::Map(a)
    });
    InnerValue::Map(m)
}

fn bench(c: &mut Criterion) {
    let interner = Interner::new();
    // Make sure all field-names are interned before compile_filter runs.
    for k in [
        "id", "name", "age", "score", "active", "email", "tags", "address", "city", "zip",
        "country",
    ] {
        intern(&interner, k);
    }
    let records: Vec<InnerValue> = (0..1000).map(|i| make_record(&interner, i)).collect();
    let empty_refs: TMap<String, _> = new_map_wc(0);
    let ctx = FilterContext::new(&interner, &empty_refs);

    let eq_age = compile_filter(
        &Filter::Eq {
            field: vec!["age".to_string()],
            value: FilterValue::Int(50),
        },
        &interner,
    );

    let eq_nested = compile_filter(
        &Filter::Eq {
            field: vec!["address".to_string(), "city".to_string()],
            value: FilterValue::String("Jerusalem".to_string()),
        },
        &interner,
    );

    let mut group = c.benchmark_group("filter_eval");
    group.throughput(Throughput::Elements(records.len() as u64));

    group.bench_function("eq_int_top_level_1000", |b| {
        b.iter(|| {
            let mut n = 0usize;
            for r in &records {
                if eq_age.matches(r, &ctx) {
                    n += 1;
                }
            }
            black_box(n);
        })
    });

    group.bench_function("eq_str_nested_path_1000", |b| {
        b.iter(|| {
            let mut n = 0usize;
            for r in &records {
                if eq_nested.matches(r, &ctx) {
                    n += 1;
                }
            }
            black_box(n);
        })
    });

    // ── Compound AND: age > 20 AND active = true ──────────────
    let compound_and = compile_filter(
        &Filter::And {
            filters: vec![
                Filter::Gt {
                    field: vec!["age".to_string()],
                    value: FilterValue::Int(20),
                },
                Filter::Eq {
                    field: vec!["active".to_string()],
                    value: FilterValue::Bool(true),
                },
            ],
        },
        &interner,
    );
    group.bench_function("compound_and_2_1000", |b| {
        b.iter(|| {
            let mut n = 0usize;
            for r in &records {
                if compound_and.matches(r, &ctx) {
                    n += 1;
                }
            }
            black_box(n);
        })
    });

    // ── Compound AND(3): age > 20 AND active = true AND score < 500 ─
    let compound_and3 = compile_filter(
        &Filter::And {
            filters: vec![
                Filter::Gt {
                    field: vec!["age".to_string()],
                    value: FilterValue::Int(20),
                },
                Filter::Eq {
                    field: vec!["active".to_string()],
                    value: FilterValue::Bool(true),
                },
                Filter::Lt {
                    field: vec!["score".to_string()],
                    value: FilterValue::Float(500.0),
                },
            ],
        },
        &interner,
    );
    group.bench_function("compound_and_3_1000", |b| {
        b.iter(|| {
            let mut n = 0usize;
            for r in &records {
                if compound_and3.matches(r, &ctx) {
                    n += 1;
                }
            }
            black_box(n);
        })
    });

    // ── Compound OR: age = 50 OR age = 30 ──────────────────────
    let compound_or = compile_filter(
        &Filter::Or {
            filters: vec![
                Filter::Eq {
                    field: vec!["age".to_string()],
                    value: FilterValue::Int(50),
                },
                Filter::Eq {
                    field: vec!["age".to_string()],
                    value: FilterValue::Int(30),
                },
            ],
        },
        &interner,
    );
    group.bench_function("compound_or_2_1000", |b| {
        b.iter(|| {
            let mut n = 0usize;
            for r in &records {
                if compound_or.matches(r, &ctx) {
                    n += 1;
                }
            }
            black_box(n);
        })
    });

    // ── Regex match on name ────────────────────────────────────
    let regex_filter = compile_filter(
        &Filter::Regex {
            field: vec!["name".to_string()],
            pattern: "user-[0-9]{2}$".to_string(),
        },
        &interner,
    );
    group.bench_function("regex_match_1000", |b| {
        b.iter(|| {
            let mut n = 0usize;
            for r in &records {
                if regex_filter.matches(r, &ctx) {
                    n += 1;
                }
            }
            black_box(n);
        })
    });

    // ── FTS brute-force AND: 2 tokens on 1000 text records ──────
    let fts_and = compile_filter(
        &Filter::Fts {
            field: vec!["name".to_string()],
            query: "user alpha".to_string(),
            mode: "and".to_string(),
        },
        &interner,
    );
    group.bench_function("fts_brute_and_1000", |b| {
        b.iter(|| {
            let mut n = 0usize;
            for r in &records {
                if fts_and.matches(r, &ctx) {
                    n += 1;
                }
            }
            black_box(n);
        })
    });

    // ── FTS brute-force OR: 2 tokens on 1000 text records ────
    let fts_or = compile_filter(
        &Filter::Fts {
            field: vec!["name".to_string()],
            query: "user alpha".to_string(),
            mode: "or".to_string(),
        },
        &interner,
    );
    group.bench_function("fts_brute_or_1000", |b| {
        b.iter(|| {
            let mut n = 0usize;
            for r in &records {
                if fts_or.matches(r, &ctx) {
                    n += 1;
                }
            }
            black_box(n);
        })
    });

    // ── Computed: LOWER(name) == "user-50" on 1000 records ────
    let computed_lower = compile_filter(
        &Filter::Computed {
            expr_op: "lower".to_string(),
            field: vec!["name".to_string()],
            expr_args: None,
            cmp: "eq".to_string(),
            value: FilterValue::String("user-50".to_string()),
        },
        &interner,
    );
    group.bench_function("computed_lower_eq_1000", |b| {
        b.iter(|| {
            let mut n = 0usize;
            for r in &records {
                if computed_lower.matches(r, &ctx) {
                    n += 1;
                }
            }
            black_box(n);
        })
    });

    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
