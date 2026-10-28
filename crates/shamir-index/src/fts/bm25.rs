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
    let avg_doc_len = if avg_doc_len.is_finite() && avg_doc_len > 0.0 {
        avg_doc_len
    } else {
        1.0
    };
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FtsStatsSnapshot {
    pub doc_count: u64,
    pub sum_doc_len: u64,
}

/// Global FTS index stats. Each update publishes a coherent count/length pair.
pub struct FtsStats {
    state: arc_swap::ArcSwap<FtsStatsSnapshot>,
}

impl Default for FtsStats {
    fn default() -> Self {
        Self::new()
    }
}

impl FtsStats {
    pub fn new() -> Self {
        Self {
            state: arc_swap::ArcSwap::from_pointee(FtsStatsSnapshot::default()),
        }
    }

    pub fn snapshot(&self) -> FtsStatsSnapshot {
        **self.state.load()
    }

    pub fn doc_count(&self) -> u64 {
        self.snapshot().doc_count
    }

    pub fn sum_doc_len(&self) -> u64 {
        self.snapshot().sum_doc_len
    }

    pub fn reset(&self) {
        self.update(|_| Some(FtsStatsSnapshot::default()));
    }

    pub fn avg_doc_len(&self) -> f64 {
        let snapshot = self.snapshot();
        if snapshot.doc_count == 0 || snapshot.sum_doc_len == 0 {
            return 1.0;
        }
        snapshot.sum_doc_len as f64 / snapshot.doc_count as f64
    }

    pub fn on_insert(&self, doc_len: u32) {
        self.update(|current| {
            Some(FtsStatsSnapshot {
                doc_count: current.doc_count.checked_add(1)?,
                sum_doc_len: current.sum_doc_len.checked_add(doc_len as u64)?,
            })
        });
    }

    pub fn on_delete(&self, doc_len: u32) {
        self.update(|current| {
            Some(FtsStatsSnapshot {
                doc_count: current.doc_count.checked_sub(1)?,
                sum_doc_len: current.sum_doc_len.checked_sub(doc_len as u64)?,
            })
        });
    }

    fn update(&self, update: impl Fn(FtsStatsSnapshot) -> Option<FtsStatsSnapshot>) {
        loop {
            let current = self.state.load_full();
            let Some(next) = update(*current) else {
                return;
            };
            let previous = self
                .state
                .compare_and_swap(&current, std::sync::Arc::new(next));
            if std::sync::Arc::ptr_eq(&previous, &current) {
                return;
            }
        }
    }
}
