//! [`select_window`] paging-selection tests (R4b-1 follow-up, P1 hard
//! paging cap): the PRODUCTION keyset selection helper used by the
//! requests and transactions sections, exercised directly.
//!
//! Coverage: duplicate ids from a torn pinned-scc iteration are
//! deduplicated before the cap is applied (a double visit can never
//! shrink the page or spuriously overflow it); the limit is EXACT (a page
//! fills at exactly `remaining` keys without declaring overflow); the
//! overflow flag is decided against the full deduplicated count (the
//! `remaining+1`-th key is a lookahead only, never selected); and the
//! continuation boundary — the last SELECTED key — resumes the walk
//! without dropping or repeating any key.

use super::super::list::select_window;

/// Exact fill: exactly `remaining` distinct keys selects ALL of them in
/// ascending order, deduplicates duplicates, and does NOT overflow (the
/// page filled exactly at the section boundary — the chain continues at
/// the top of the next section, never skipping a key).
#[test]
fn exact_limit_fills_without_overflow() {
    let keys = vec![30, 10, 20, 10, 30, 20]; // duplicates from a torn iteration
    let (selected, overflowed) = select_window(keys, 3);
    assert!(!overflowed);
    assert_eq!(selected, vec![10, 20, 30]);
}

/// Overflow: more distinct keys than the budget — the selection is the
/// first `remaining` ascending keys, overflow is declared against the
/// DEDUPLICATED count (duplicates never trigger a false overflow).
#[test]
fn overflow_selects_first_keys_and_flags() {
    let keys = vec![50, 10, 50, 20, 10, 40, 20]; // 4 distinct: 10, 20, 40, 50
    let (selected, overflowed) = select_window(keys, 3);
    assert!(overflowed);
    assert_eq!(selected, vec![10, 20, 40]);
    // Duplicates alone never overflow: 2 distinct keys, budget 3.
    let (selected, overflowed) = select_window(vec![7, 7, 7, 7], 3);
    assert!(!overflowed);
    assert_eq!(selected, vec![7]);
}

/// Duplicate ids from a pinned-scc double visit can never shrink the
/// page: the distinct count, not the raw visit count, is what the cap
/// applies to, and no distinct key is lost.
#[test]
fn duplicates_never_shrink_or_reorder_the_page() {
    let keys: Vec<u64> = [1, 2, 3, 4, 5].iter().flat_map(|&k| [k, k, k]).collect();
    let (selected, overflowed) = select_window(keys, 5);
    assert!(!overflowed);
    assert_eq!(selected, vec![1, 2, 3, 4, 5]);
}

/// Continuation walk with duplicates feeding the production helper: the
/// boundary is the LAST SELECTED key, resuming strictly above it renders
/// the next window, and the two pages together cover every distinct key
/// exactly once — no dropped, repeated or skipped key.
#[test]
fn continuation_from_last_selected_key_resumes_exactly() {
    // Unsorted with duplicates — the shape a pinned scc iteration hands
    // the worker.
    let keys = vec![9, 2, 9, 5, 2, 8, 5, 1, 8]; // distinct: 1, 2, 5, 8, 9

    // Page 1: budget 2.
    let (page1, overflowed1) = select_window(keys.clone(), 2);
    assert!(overflowed1);
    assert_eq!(page1, vec![1, 2]);

    // The continuation anchors at the last SELECTED key (2): the resumed
    // scan only considers keys strictly greater.
    let boundary = page1[page1.len() - 1];
    assert_eq!(boundary, 2);
    let resumed: Vec<u64> = keys.iter().copied().filter(|&k| k > boundary).collect();
    let (page2, overflowed2) = select_window(resumed, 2);
    assert!(overflowed2);
    assert_eq!(page2, vec![5, 8]);

    // Page 3: one key left, no overflow, chain reaches exhaustion.
    let boundary2 = page2[page2.len() - 1];
    let resumed2: Vec<u64> = keys.iter().copied().filter(|&k| k > boundary2).collect();
    let (page3, overflowed3) = select_window(resumed2, 2);
    assert!(!overflowed3);
    assert_eq!(page3, vec![9]);

    // Whole-chain invariant: every distinct key appears exactly once.
    let mut walked = page1;
    walked.extend(page2);
    walked.extend(page3);
    walked.sort_unstable();
    walked.dedup();
    assert_eq!(walked, vec![1, 2, 5, 8, 9]);
}

/// The cap cannot be exceeded even by an oversized budget input: an empty
/// candidate set selects nothing, declares no overflow, and yields no
/// boundary key (the section contributes nothing to the page).
#[test]
fn empty_section_selects_nothing() {
    let (selected, overflowed) = select_window(Vec::new(), 4);
    assert!(!overflowed);
    assert!(selected.is_empty());
}
