//! FTS backend with BM25 ranking (Phase 3).
//!
//! Extends the MVP FtsBackend: posting values carry `{ tf, doc_len }`,
//! global FtsStats track doc_count/sum_doc_len atomically. Lookup
//! returns `IndexResult::Ranked` with BM25 scores sorted descending.

use crate::backend::{FtsMode, IndexBackend, IndexError, IndexQuery, IndexResult};
use crate::bm25::{self, Bm25Params, FtsPostingValue, FtsStats};
use crate::descriptor::IndexDescriptor;
use crate::posting_layout::{build_posting_key, type_tag, PostingKeyRef};
use crate::tokenizer::{self, token_identity, TokenizerEnum, WhitespaceTokenizer};
use crate::write_ops::IndexWriteOp;
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use shamir_collections::{TFxMap, TFxSet};
use shamir_storage::types::Store;
use shamir_types::core::interner::InternerKey;
use shamir_types::record_view::RecordRef;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use smallvec::SmallVec;
use std::collections::{BTreeSet, BinaryHeap};
use std::sync::Arc;

#[derive(Clone, Copy, Debug)]
struct RankedCandidate(RecordId, f32);

impl PartialEq for RankedCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0 && self.1.to_bits() == other.1.to_bits()
    }
}

impl Eq for RankedCandidate {}

impl PartialOrd for RankedCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RankedCandidate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .1
            .total_cmp(&self.1)
            .then_with(|| self.0.cmp(&other.0))
    }
}

/// Keep at most k candidates. Higher scores win; ties use ascending RecordId.
pub(crate) fn ranked_top_k(
    scores: impl IntoIterator<Item = (RecordId, f64)>,
    limit: usize,
) -> (Vec<(RecordId, f32)>, usize) {
    let mut max_retained = 0;
    let mut heap = BinaryHeap::new();
    for (rid, score) in scores {
        let candidate = RankedCandidate(rid, score as f32);
        if heap.len() < limit {
            heap.push(candidate);
            max_retained = max_retained.max(heap.len());
        } else if heap.peek().is_some_and(|worst| candidate < *worst) {
            heap.pop();
            heap.push(candidate);
        }
    }
    let mut ranked: Vec<_> = heap
        .into_iter()
        .map(|candidate| (candidate.0, candidate.1))
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    (ranked, max_retained)
}

pub struct FtsRankedBackend {
    descriptor: IndexDescriptor,
    field_path: Vec<u64>,
    tokenizer: TokenizerEnum,
    store: Arc<dyn Store>,
    pub(crate) stats: Arc<FtsStats>,
    params: Bm25Params,
    #[cfg(test)]
    high_water: std::sync::atomic::AtomicUsize,
}

impl FtsRankedBackend {
    pub fn new(descriptor: IndexDescriptor, field_path: Vec<u64>, store: Arc<dyn Store>) -> Self {
        let tokenizer = match &descriptor.kind {
            crate::kind::IndexKind::Fts { tokenizer: tk, .. } => tokenizer::build_tokenizer(tk),
            _ => TokenizerEnum::Whitespace(WhitespaceTokenizer),
        };
        Self {
            descriptor,
            field_path,
            tokenizer,
            store,
            stats: Arc::new(FtsStats::new()),
            params: Bm25Params::default(),
            #[cfg(test)]
            high_water: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// F-50 Step 2 (#870, Part C): current document count tracked by the
    /// in-memory BM25 stats. Exposed so engine-side tests can assert
    /// `BumpFtsStats` was applied exactly once (not double-counted) for a
    /// quiescent tx that planned the FTS insert at stage time.
    pub fn doc_count(&self) -> u64 {
        self.stats.doc_count()
    }

    /// #1063: sum of all document lengths tracked by the in-memory BM25 stats.
    /// Exposed so engine-side tests can assert `BumpFtsStats` didn't pollute
    /// one FTS backend's stats with another's when multiple FTS indexes exist
    /// on the same table (cross-field `doc_len` contamination).
    pub fn sum_doc_len(&self) -> u64 {
        self.stats.sum_doc_len()
    }

    #[cfg(test)]
    pub(crate) fn collector_high_water(&self) -> usize {
        self.high_water.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Resolve `self.field_path` to its interned-key form (see
    /// `FtsBackend::ipath`).
    fn ipath(&self) -> SmallVec<[InternerKey; 4]> {
        self.field_path
            .iter()
            .map(|&id| InternerKey::new(id))
            .collect()
    }

    fn tokenize_with_freq(&self, rec: &dyn RecordRef) -> (TFxMap<Vec<u8>, u32>, u32) {
        let ipath = self.ipath();
        match rec.str_at(&ipath) {
            Some(text) => {
                let mut freq: TFxMap<Vec<u8>, u32> = TFxMap::default();
                let mut doc_len = 0_u32;
                self.tokenizer.for_each_token_identity(text, |identity| {
                    doc_len = doc_len.wrapping_add(1);
                    if let Some(count) = freq.get_mut(identity) {
                        *count += 1;
                    } else {
                        freq.insert(identity.to_vec(), 1);
                    }
                });
                (freq, doc_len)
            }
            None => (TFxMap::default(), 0),
        }
    }

    fn try_tokenize_with_freq(
        &self,
        rec: &dyn RecordRef,
    ) -> Result<(TFxMap<Vec<u8>, u32>, u32), IndexError> {
        let ipath = self.ipath();
        let Some(text) = rec.str_at(&ipath) else {
            return Ok((TFxMap::default(), 0));
        };
        self.tokenizer
            .try_token_frequencies_with(text, token_identity)
    }

    fn posting_key_for_token(&self, canonical: &[u8], rid: &RecordId) -> Vec<u8> {
        let identity = crate::model::identity::identity_bytes::identity_bytes(canonical);
        build_posting_key(self.descriptor.id, type_tag::FTS_V2, &identity, rid)
    }

    fn prefix_for_token(&self, canonical: &[u8]) -> Vec<u8> {
        let identity = crate::model::identity::identity_bytes::identity_bytes(canonical);
        let mut prefix = Vec::with_capacity(5 + identity.len());
        prefix.extend_from_slice(&self.descriptor.id.to_le_bytes());
        prefix.push(type_tag::FTS_V2);
        prefix.extend_from_slice(&identity);
        prefix
    }

    async fn scan_token_with_values(
        &self,
        canonical: &[u8],
    ) -> Result<Vec<(RecordId, FtsPostingValue)>, IndexError> {
        let identity = crate::model::identity::identity_bytes::identity_bytes(canonical);
        let prefix = self.prefix_for_token(canonical);
        let mut stream = self.store.scan_prefix_stream(Bytes::from(prefix), 1024);
        let mut results = Vec::with_capacity(16);
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result.map_err(IndexError::from)?;
            for (key_bytes, val_bytes) in batch {
                if let Some(pk) = PostingKeyRef::decode(&key_bytes) {
                    if pk.index_id == self.descriptor.id
                        && pk.type_tag == type_tag::FTS_V2
                        && pk.value_bytes == identity
                    {
                        if canonical.len() > crate::model::identity::identity_bytes::INLINE_MAX {
                            if val_bytes.len() <= 8 || val_bytes[8..] != canonical[..] {
                                continue;
                            }
                        } else if val_bytes.len() != 8 {
                            log::warn!(
                                "corrupt FTS posting for index {}: expected 8-byte payload",
                                self.descriptor.id
                            );
                            return Err(IndexError::Backend(format!(
                                "corrupt FTS posting for index {}: expected 8-byte payload",
                                self.descriptor.id
                            )));
                        }
                        let payload = &val_bytes[..8];
                        let pv: FtsPostingValue =
                            bincode::deserialize(payload).map_err(|error| {
                                IndexError::Backend(format!(
                                    "corrupt FTS posting for index {}: {error}",
                                    self.descriptor.id
                                ))
                            })?;
                        if pv.tf == 0 || pv.doc_len == 0 || pv.tf > pv.doc_len {
                            log::warn!(
                                "corrupt FTS posting values for index {}: tf={}, doc_len={}",
                                self.descriptor.id,
                                pv.tf,
                                pv.doc_len
                            );
                            return Err(IndexError::Backend(format!(
                                "corrupt FTS posting for index {}: invalid tf/doc_len",
                                self.descriptor.id
                            )));
                        }
                        results.push((pk.record_id_owned(), pv));
                    }
                }
            }
        }
        Ok(results)
    }
}

#[async_trait]
impl IndexBackend for FtsRankedBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn descriptor(&self) -> &IndexDescriptor {
        &self.descriptor
    }

    fn tokenize_query(&self, query: &str) -> Result<Vec<Vec<u8>>, IndexError> {
        Ok(self
            .tokenizer
            .try_tokenize(query)?
            .iter()
            .map(|t| token_identity(t))
            .collect())
    }

    async fn plan_insert(
        &self,
        rid: RecordId,
        rec: &(dyn RecordRef + Sync + '_),
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        let (freq, doc_len) = self.try_tokenize_with_freq(rec)?;
        if doc_len == 0 {
            return Ok(Vec::new());
        }
        let provenance = crate::write_ops::index2_provenance(&self.descriptor);
        let mut ops = Vec::with_capacity(freq.len() + 1);
        for (identity, tf) in &freq {
            let key = self.posting_key_for_token(identity, &rid);
            let pv = FtsPostingValue { tf: *tf, doc_len };
            let mut val =
                bincode::serialize(&pv).map_err(|e| IndexError::Backend(e.to_string()))?;
            if identity.len() > crate::model::identity::identity_bytes::INLINE_MAX {
                val.extend_from_slice(identity);
            }
            ops.push(IndexWriteOp::SetPosting {
                key: Bytes::from(key),
                value: Bytes::from(val),
                provenance,
            });
        }
        ops.push(IndexWriteOp::BumpFtsStats {
            doc_len,
            sign: 1,
            provenance,
        });
        Ok(ops)
    }

    async fn plan_update(
        &self,
        rid: RecordId,
        old: &(dyn RecordRef + Sync + '_),
        new: &(dyn RecordRef + Sync + '_),
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        let (old_freq, old_doc_len) = self.tokenize_with_freq(old);
        let old_set: TFxSet<Vec<u8>> = old_freq.keys().cloned().collect();
        let (new_freq, new_doc_len) = self.try_tokenize_with_freq(new)?;
        let new_set: TFxSet<Vec<u8>> = new_freq.keys().cloned().collect();

        let provenance = crate::write_ops::index2_provenance(&self.descriptor);
        let mut ops = Vec::new();
        // Remove disappeared tokens.
        for th in old_set.difference(&new_set) {
            let key = self.posting_key_for_token(th, &rid);
            ops.push(IndexWriteOp::RemovePosting {
                key: Bytes::from(key),
                provenance,
                owner: None,
            });
        }
        // Add/update all tokens in new (tf or doc_len may have changed).
        for (identity, tf) in &new_freq {
            let key = self.posting_key_for_token(identity, &rid);
            let pv = FtsPostingValue {
                tf: *tf,
                doc_len: new_doc_len,
            };
            let mut val =
                bincode::serialize(&pv).map_err(|e| IndexError::Backend(e.to_string()))?;
            if identity.len() > crate::model::identity::identity_bytes::INLINE_MAX {
                val.extend_from_slice(identity);
            }
            ops.push(IndexWriteOp::SetPosting {
                key: Bytes::from(key),
                value: Bytes::from(val),
                provenance,
            });
        }
        if old_doc_len > 0 {
            ops.push(IndexWriteOp::BumpFtsStats {
                doc_len: old_doc_len,
                sign: -1,
                provenance,
            });
        }
        if new_doc_len > 0 {
            ops.push(IndexWriteOp::BumpFtsStats {
                doc_len: new_doc_len,
                sign: 1,
                provenance,
            });
        }
        Ok(ops)
    }

    async fn plan_delete(
        &self,
        rid: RecordId,
        rec: &(dyn RecordRef + Sync + '_),
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        let (freq, doc_len) = self.tokenize_with_freq(rec);
        let provenance = crate::write_ops::index2_provenance(&self.descriptor);
        let mut ops = Vec::with_capacity(freq.len() + 1);
        for th in freq.keys() {
            let key = self.posting_key_for_token(th, &rid);
            ops.push(IndexWriteOp::RemovePosting {
                key: Bytes::from(key),
                provenance,
                owner: None,
            });
        }
        if doc_len > 0 {
            ops.push(IndexWriteOp::BumpFtsStats {
                doc_len,
                sign: -1,
                provenance,
            });
        }
        Ok(ops)
    }

    /// Apply BM25 doc-length aggregates (`BumpFtsStats`) to the live
    /// in-memory `stats`.
    ///
    /// ## Why these stats are NOT WAL-durable (by design, not a gap)
    ///
    /// `BumpFtsStats` is a *derived* aggregate — `doc_count` and
    /// `sum_doc_len` are fully reconstructible from the durable postings
    /// (which DO go through the WAL as `IndexPut`/`IndexDel`). It is
    /// therefore deliberately excluded from `wal_ops_from_tx`, for the
    /// same reason `CounterDelta` replay is skipped in
    /// `recovery::replay_v2_op`: replaying a derived value alongside its
    /// already-replayed source would double-count.
    ///
    /// The authoritative recovery path is `rebuild()` (below), which
    /// re-derives `stats` from the data store on open. This keeps the
    /// aggregate provably consistent with the postings at all times —
    /// see `rebuild_restores_stats_from_data_store` for the guarantee.
    ///
    /// (A future cold-start optimisation could persist a `stats`
    /// snapshot — mirroring the lockout / rate-limiter snapshots — to
    /// skip the full rebuild scan on large tables. That is an
    /// optimisation, not a correctness fix.)
    async fn apply_in_memory(&self, ops: &[IndexWriteOp]) -> Result<(), IndexError> {
        for op in ops {
            if let IndexWriteOp::BumpFtsStats { doc_len, sign, .. } = op {
                if *doc_len > 0 {
                    if *sign > 0 {
                        self.stats.on_insert(*doc_len);
                    } else if *sign < 0 {
                        self.stats.on_delete(*doc_len);
                    }
                }
            }
        }
        Ok(())
    }

    async fn lookup(&self, query: IndexQuery) -> Result<IndexResult, IndexError> {
        match query {
            IndexQuery::Fts {
                tokens,
                mode,
                limit,
            } => {
                if tokens.is_empty() {
                    return Ok(IndexResult::Ranked(Vec::new()));
                }
                let snapshot = self.stats.snapshot();
                let total_docs = snapshot.doc_count;
                let avg_dl = if total_docs == 0 || snapshot.sum_doc_len == 0 {
                    1.0
                } else {
                    snapshot.sum_doc_len as f64 / total_docs as f64
                };
                let mut per_token: Vec<Vec<(RecordId, FtsPostingValue)>> =
                    Vec::with_capacity(tokens.len());
                for th in &tokens {
                    per_token.push(self.scan_token_with_values(th).await?);
                }

                let rank = |scores: TFxMap<RecordId, f64>, limit: Option<usize>| match limit {
                    Some(k) => {
                        let (ranked, high_water) = ranked_top_k(scores, k);
                        #[cfg(test)]
                        self.high_water
                            .store(high_water, std::sync::atomic::Ordering::Relaxed);
                        debug_assert!(high_water <= k);
                        ranked
                    }
                    None => {
                        let mut ranked: Vec<_> = scores
                            .into_iter()
                            .map(|(rid, score)| (rid, score as f32))
                            .collect();
                        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                        ranked
                    }
                };

                let scores = match mode {
                    FtsMode::AndAll => {
                        let mut iter = per_token.iter().map(|entries| {
                            entries.iter().map(|(rid, _)| *rid).collect::<BTreeSet<_>>()
                        });
                        let Some(first) = iter.next() else {
                            return Ok(IndexResult::Ranked(Vec::new()));
                        };
                        let intersection = iter.fold(first, |acc, set| &acc & &set);
                        let mut scores: TFxMap<RecordId, f64> = TFxMap::default();
                        for entries in &per_token {
                            let idf_val = bm25::idf(total_docs, entries.len() as u64);
                            for (rid, posting) in entries {
                                if intersection.contains(rid) {
                                    *scores.entry(*rid).or_insert(0.0) += bm25::term_score(
                                        &self.params,
                                        posting.tf,
                                        posting.doc_len,
                                        avg_dl,
                                        idf_val,
                                    );
                                }
                            }
                        }
                        scores
                    }
                    FtsMode::OrAny => {
                        let mut scores: TFxMap<RecordId, f64> = TFxMap::default();
                        for entries in &per_token {
                            let idf_val = bm25::idf(total_docs, entries.len() as u64);
                            for (rid, posting) in entries {
                                *scores.entry(*rid).or_insert(0.0) += bm25::term_score(
                                    &self.params,
                                    posting.tf,
                                    posting.doc_len,
                                    avg_dl,
                                    idf_val,
                                );
                            }
                        }
                        scores
                    }
                };
                Ok(IndexResult::Ranked(rank(scores, limit)))
            }
            _ => Err(IndexError::Backend(
                "FtsRankedBackend only supports Fts queries".into(),
            )),
        }
    }

    async fn rebuild(&self, source: Arc<dyn Store>) -> Result<(), IndexError> {
        let batch_size = 1000usize;
        let mut stream = source.iter_stream(batch_size);
        while let Some(batch_res) = stream.next().await {
            let batch = batch_res.map_err(IndexError::from)?;
            for (_key_bytes, val_bytes) in batch {
                let rec = match InnerValue::from_bytes(&val_bytes) {
                    Ok(r) => r,
                    Err(error) => {
                        log::warn!(
                            "skipping malformed FTS rebuild row for index {}: {error}",
                            self.descriptor.id
                        );
                        continue;
                    }
                };
                let (_, doc_len) = self.tokenize_with_freq(&rec);
                if doc_len > 0 {
                    self.stats.on_insert(doc_len);
                }
            }
        }
        Ok(())
    }

    async fn drop_all(&self) -> Result<(), IndexError> {
        let prefix = self.descriptor.id.to_le_bytes();
        let mut stream = self
            .store
            .scan_prefix_stream(Bytes::copy_from_slice(&prefix), 1024);
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result.map_err(IndexError::from)?;
            for (key_bytes, _) in batch {
                self.store
                    .remove(key_bytes)
                    .await
                    .map_err(IndexError::from)?;
            }
        }
        self.stats.reset();
        Ok(())
    }
}
