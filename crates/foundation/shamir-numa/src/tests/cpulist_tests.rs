//! `parse_cpulist` — Linux `/sys` cpulist format.

use crate::{parse_cpulist, CpuId};

fn ids(xs: &[usize]) -> Vec<CpuId> {
    xs.iter().copied().map(CpuId).collect()
}

#[test]
fn single_range() {
    assert_eq!(parse_cpulist("0-3"), ids(&[0, 1, 2, 3]));
}

#[test]
fn single_index() {
    assert_eq!(parse_cpulist("5"), ids(&[5]));
}

#[test]
fn comma_separated_indices() {
    assert_eq!(parse_cpulist("0,2,4,6"), ids(&[0, 2, 4, 6]));
}

#[test]
fn mixed_ranges_and_indices() {
    assert_eq!(parse_cpulist("0-1,8-9,15"), ids(&[0, 1, 8, 9, 15]));
}

#[test]
fn trailing_newline_and_whitespace_tolerated() {
    // /sys files end in a newline; tolerate it and stray spaces.
    assert_eq!(parse_cpulist(" 0 - 2 , 4 \n"), ids(&[0, 1, 2, 4]));
}

#[test]
fn empty_is_empty() {
    assert_eq!(parse_cpulist(""), Vec::<CpuId>::new());
    assert_eq!(parse_cpulist("  \n"), Vec::<CpuId>::new());
}

#[test]
fn result_is_sorted_and_deduplicated() {
    assert_eq!(parse_cpulist("4,2,2,0-1,1"), ids(&[0, 1, 2, 4]));
}

#[test]
fn reversed_range_is_skipped_not_panicked() {
    // /sys never emits "7-2"; a best-effort parse drops it rather than abort.
    assert_eq!(parse_cpulist("7-2,3"), ids(&[3]));
}

#[test]
fn garbage_tokens_are_skipped() {
    assert_eq!(parse_cpulist("0,foo,2,-,3"), ids(&[0, 2, 3]));
}

#[test]
fn rv_numa_cf3796d2_accepts_exact_expansion_boundary() {
    let parsed = parse_cpulist("0-999999");
    assert_eq!(parsed.len(), 1_000_000);
    assert_eq!(parsed.first(), Some(&CpuId(0)));
    assert_eq!(parsed.last(), Some(&CpuId(999_999)));
}

#[test]
fn rv_numa_cf3796d2_cumulative_distinct_token_obeys_budget() {
    let parsed = parse_cpulist("0-999999,1000000");
    assert_eq!(parsed.len(), 1_000_000);
    assert_eq!(parsed.last(), Some(&CpuId(999_999)));
}

#[test]
fn rv_numa_cf3796d2_oversized_and_overflow_ranges_are_skipped() {
    assert!(parse_cpulist("0-1000000").is_empty());
    assert_eq!(parse_cpulist("0-18446744073709551615"), Vec::<CpuId>::new());
}
