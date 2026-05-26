//! BM25 scoring for FTS ranked queries.
//!
//! Classic Okapi BM25 formula:
//!   score(d, q) = Σ_t∈q  idf(t) · ((k1+1)·tf) / (tf + k1·(1 - b + b·dl/avgdl))
//!
//! Parameters: k1 = 1.2, b = 0.75 (standard defaults).

/// BM25 parameters.
pub struct Bm25Params {
    pub k1: f64,
    pub b: f64,
}

impl Default for Bm25Params {
    fn default() -> Self {
        Self { k1: 1.2, b: 0.75 }
    }
}

/// Inverse document frequency.
///   idf(t) = ln((N - df + 0.5) / (df + 0.5) + 1)
pub fn idf(total_docs: u64, doc_freq: u64) -> f64 {
    let n = total_docs as f64;
    let df = doc_freq as f64;
    ((n - df + 0.5) / (df + 0.5) + 1.0).ln()
}

/// Per-term BM25 contribution for one document.
pub fn term_score(
    params: &Bm25Params,
    tf: u32,
    doc_len: u32,
    avg_doc_len: f64,
    idf_val: f64,
) -> f64 {
    let tf = tf as f64;
    let dl = doc_len as f64;
    let norm = 1.0 - params.b + params.b * dl / avg_doc_len;
    idf_val * (params.k1 + 1.0) * tf / (tf + params.k1 * norm)
}

/// Posting value stored alongside each (token, record_id) posting.
/// Bincode-serialized into the posting value bytes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FtsPostingValue {
    /// Term frequency in this document for this token.
    pub tf: u32,
    /// Total token count in this document (all tokens, not just this one).
    pub doc_len: u32,
}

/// Global FTS index stats. Updated atomically on insert/delete.
pub struct FtsStats {
    pub doc_count: std::sync::atomic::AtomicU64,
    pub sum_doc_len: std::sync::atomic::AtomicU64,
}

impl Default for FtsStats {
    fn default() -> Self {
        Self::new()
    }
}

impl FtsStats {
    pub fn new() -> Self {
        Self {
            doc_count: std::sync::atomic::AtomicU64::new(0),
            sum_doc_len: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn avg_doc_len(&self) -> f64 {
        let count = self.doc_count.load(std::sync::atomic::Ordering::Relaxed);
        if count == 0 {
            return 1.0;
        }
        self.sum_doc_len.load(std::sync::atomic::Ordering::Relaxed) as f64 / count as f64
    }

    pub fn on_insert(&self, doc_len: u32) {
        self.doc_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.sum_doc_len
            .fetch_add(doc_len as u64, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn on_delete(&self, doc_len: u32) {
        self.doc_count
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        self.sum_doc_len
            .fetch_sub(doc_len as u64, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(
            stats.doc_count.load(std::sync::atomic::Ordering::Relaxed),
            2
        );
        assert!((stats.avg_doc_len() - 150.0).abs() < 0.001);
        stats.on_delete(100);
        assert!((stats.avg_doc_len() - 200.0).abs() < 0.001);
    }

    #[test]
    fn stats_empty_returns_1() {
        let stats = FtsStats::new();
        assert_eq!(stats.avg_doc_len(), 1.0);
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
}
