use super::super::dedup::hash_value;
use crate::agg::{self, AggRegistry, Aggregator, DistinctWrapper};
use crate::compare::compare;
use num_bigint::BigInt;
use rust_decimal::Decimal;
use shamir_types::types::common::{new_map, new_set};
use shamir_types::types::value::QueryValue as V;
use std::cmp::Ordering;

fn decimal(s: &str) -> V {
    V::Dec(Decimal::from_str_exact(s).unwrap())
}

fn registry() -> AggRegistry {
    let mut registry = AggRegistry::new();
    agg::register(&mut registry);
    registry
}

#[test]
fn permutation_counts_and_first_occurrence() {
    let values = [
        V::Int(9_007_199_254_740_992),
        V::F64(9_007_199_254_740_992.0),
        V::Int(9_007_199_254_740_993),
    ];
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let registry = registry();
        let mut count = registry.make("count_distinct").unwrap();
        let mut array = Box::new(DistinctWrapper::new(registry.make("array_agg").unwrap()));
        for i in order {
            count.accumulate(&values[i]).unwrap();
            array.accumulate(&values[i]).unwrap();
        }
        assert_eq!(count.finalize().unwrap(), V::Int(2));
        let expected = if order[0] == 2 {
            vec![values[2].clone(), values[order[1]].clone()]
        } else {
            vec![values[order[0]].clone(), values[2].clone()]
        };
        assert_eq!(array.finalize().unwrap(), V::List(expected));
    }
}

#[test]
fn recursive_equal_values_have_equal_hashes() {
    let equivalents = [
        V::Int(1),
        V::Big(BigInt::from(1)),
        decimal("1.000"),
        V::F64(1.0),
    ];
    for a in &equivalents {
        for b in &equivalents {
            let nested = |v: &V, reverse: bool| {
                let mut set = new_set();
                if reverse {
                    set.insert(V::Int(7));
                    set.insert(v.clone());
                } else {
                    set.insert(v.clone());
                    set.insert(V::Int(7));
                }
                let mut map = new_map();
                if reverse {
                    map.insert("z".to_owned(), V::Int(4));
                    map.insert("a".to_owned(), V::Set(set));
                } else {
                    map.insert("a".to_owned(), V::Set(set));
                    map.insert("z".to_owned(), V::Int(4));
                }
                V::List(vec![V::Map(map)])
            };
            let x = nested(a, false);
            let y = nested(b, true);
            assert_eq!(compare(&x, &y), Ordering::Equal);
            assert_eq!(hash_value(&x), hash_value(&y));
        }
    }
    for (a, b) in [
        (V::F64(-0.0), decimal("0.00")),
        (
            V::F64(f64::NAN),
            V::F64(f64::from_bits(0xfff8_0000_0000_0001)),
        ),
        (decimal("0.125"), V::F64(0.125)),
    ] {
        assert_eq!(compare(&a, &b), Ordering::Equal);
        assert_eq!(hash_value(&a), hash_value(&b));
    }
}
