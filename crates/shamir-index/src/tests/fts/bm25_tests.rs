use crate::bm25::{idf, term_score, Bm25Params, FtsPostingValue, FtsStats, FtsStatsSnapshot};
use std::sync::{Arc, Barrier};
use std::thread;

#[test]
fn idf_basic() {
    // 1000 docs, 10 contain the term
    let v = idf(1000, 10);
    assert!(v > 4.0 && v < 5.0, "idf={v}");
}

#[test]
fn idf_rare_term() {
    // 1000 docs, 1 contain the term
    let v = idf(1000, 1);
    assert!(v > 6.0, "idf={v}");
}

#[test]
fn idf_common_term() {
    // 1000 docs, 900 contain the term
    let v = idf(1000, 900);
    assert!(v > 0.0 && v < 1.0, "idf={v}");
}

#[test]
fn term_score_basic() {
    let params = Bm25Params::default();
    let idf_val = idf(1000, 10);
    let s = term_score(&params, 3, 100, 80.0, idf_val);
    assert!(s > 0.0, "score={s}");
}

#[test]
fn term_score_higher_tf_higher_score() {
    let params = Bm25Params::default();
    let idf_val = idf(1000, 10);
    let s1 = term_score(&params, 1, 100, 80.0, idf_val);
    let s2 = term_score(&params, 5, 100, 80.0, idf_val);
    assert!(s2 > s1);
}

#[test]
fn term_score_longer_doc_lower_score() {
    let params = Bm25Params::default();
    let idf_val = idf(1000, 10);
    let s_short = term_score(&params, 2, 50, 80.0, idf_val);
    let s_long = term_score(&params, 2, 200, 80.0, idf_val);
    assert!(s_short > s_long);
}

#[test]
fn stats_avg_doc_len() {
    let stats = FtsStats::new();
    stats.on_insert(100);
    stats.on_insert(200);
    assert_eq!(stats.doc_count(), 2);
    assert!((stats.avg_doc_len() - 150.0).abs() < 0.001);
    stats.on_delete(100);
    assert_eq!(stats.doc_count(), 1);
    assert!((stats.avg_doc_len() - 200.0).abs() < 0.001);
}

#[test]
fn stats_empty_returns_1() {
    let stats = FtsStats::new();
    assert_eq!(stats.avg_doc_len(), 1.0);
}

#[test]
fn rv_index_cf7d545b_delete_from_zero_does_not_underflow() {
    let stats = FtsStats::new();
    stats.on_delete(5);
    assert_eq!(stats.snapshot(), FtsStatsSnapshot::default());
}

#[test]
fn rv_index_aeedb51b_zero_length_documents_keep_average_positive() {
    let stats = FtsStats::new();
    stats.on_insert(0);
    assert_eq!(stats.doc_count(), 1);
    assert_eq!(stats.sum_doc_len(), 0);
    assert_eq!(stats.avg_doc_len(), 1.0);
}

#[test]
fn rv_index_cf7d545b_rejected_and_double_deletes_preserve_pair() {
    let stats = FtsStats::new();
    stats.on_insert(3);
    stats.on_delete(9);
    assert_eq!(
        stats.snapshot(),
        FtsStatsSnapshot {
            doc_count: 1,
            sum_doc_len: 3,
        }
    );
    stats.on_delete(3);
    assert_eq!(stats.snapshot(), FtsStatsSnapshot::default());
    stats.on_insert(4);
    stats.on_delete(4);
    stats.on_delete(4);
    assert_eq!(stats.snapshot(), FtsStatsSnapshot::default());
}

#[test]
fn rv_index_aeedb51b_snapshot_stays_paired_across_updates() {
    let stats = Arc::new(FtsStats::new());
    let barrier = Arc::new(Barrier::new(2));
    let writer_stats = Arc::clone(&stats);
    let writer_barrier = Arc::clone(&barrier);
    let writer = thread::spawn(move || {
        writer_barrier.wait();
        for _ in 0..10_000 {
            writer_stats.on_insert(3);
            writer_stats.on_delete(3);
        }
    });
    barrier.wait();
    for _ in 0..10_000 {
        let snapshot = stats.snapshot();
        assert_eq!(snapshot.sum_doc_len, snapshot.doc_count * 3);
        assert!(stats.avg_doc_len().is_finite() && stats.avg_doc_len() > 0.0);
    }
    writer.join().unwrap();
    assert_eq!(stats.snapshot(), FtsStatsSnapshot::default());
}

#[test]
fn rv_index_aeedb51b_term_score_guards_nonpositive_average() {
    let params = Bm25Params::default();
    let score = term_score(&params, 2, 3, 0.0, idf(4, 1));
    assert!(score.is_finite() && score > 0.0);
}

#[test]
fn posting_value_serde() {
    let pv = FtsPostingValue {
        tf: 5,
        doc_len: 100,
    };
    let bytes = bincode::serialize(&pv).unwrap();
    assert_eq!(bytes.len(), 8); // u32 + u32
    let got: FtsPostingValue = bincode::deserialize(&bytes).unwrap();
    assert_eq!(got.tf, 5);
    assert_eq!(got.doc_len, 100);
}
