use super::{ArrayAggAgg, CountDistinctAgg};
use crate::agg::{Aggregator, DistinctWrapper};
use shamir_types::types::value::QueryValue;

#[test]
fn rv_funclib_a1b1a620_count_distinct_compare_count_is_linear_for_unique_ints() {
    let mut agg = CountDistinctAgg::new();
    for value in 0..4096 {
        agg.accumulate(&QueryValue::Int(value)).unwrap();
    }
    assert!(agg.comparison_count() < 4096 * 4);
}

#[test]
fn rv_funclib_b30dc9ac_distinct_wrapper_compare_count_is_linear_for_unique_ints() {
    let mut wrapped = DistinctWrapper::new(Box::new(ArrayAggAgg::new()));
    for value in 0..4096 {
        wrapped.accumulate(&QueryValue::Int(value)).unwrap();
    }
    assert!(wrapped.comparison_count() < 4096 * 4);
}
