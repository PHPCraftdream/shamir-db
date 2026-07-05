//! FTS backend with BM25 ranking (Phase 3).
//!
//! Extends the MVP FtsBackend: posting values carry `{ tf, doc_len }`,
//! global FtsStats track doc_count/sum_doc_len atomically. Lookup
//! returns `IndexResult::Ranked` with BM25 scores sorted descending.

use crate::backend::{FtsMode, IndexBackend, IndexError, IndexQuery, IndexResult};
use crate::bm25::{self, Bm25Params, FtsPostingValue, FtsStats};
use crate::descriptor::IndexDescriptor;
use crate::posting_layout::{build_posting_key, type_tag, PostingKeyRef};
use crate::tokenizer::{self, token_hash, TokenizerEnum, WhitespaceTokenizer};
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
use std::collections::BTreeSet;
use std::sync::Arc;

pub struct FtsRankedBackend {
    descriptor: IndexDescriptor,
    field_path: Vec<u64>,
    tokenizer: TokenizerEnum,
    store: Arc<dyn Store>,
    pub(crate) stats: Arc<FtsStats>,
    params: Bm25Params,
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
        }
    }

    /// Resolve `self.field_path` to its interned-key form (see
    /// `FtsBackend::ipath`).
    fn ipath(&self) -> SmallVec<[InternerKey; 4]> {
        self.field_path
            .iter()
            .map(|&id| InternerKey::new(id))
            .collect()
    }

    fn tokenize_with_freq(&self, rec: &dyn RecordRef) -> (TFxMap<u64, u32>, u32) {
        let ipath = self.ipath();
        match rec.str_at(&ipath) {
            Some(text) => {
                let tokens = self.tokenizer.tokenize(text);
                let doc_len = tokens.len() as u32;
                let mut freq: TFxMap<u64, u32> = TFxMap::default();
                for t in tokens {
                    *freq.entry(token_hash(&t)).or_insert(0) += 1;
                }
                (freq, doc_len)
            }
            None => (TFxMap::default(), 0),
        }
    }

    fn tokenize_set(&self, rec: &dyn RecordRef) -> TFxSet<u64> {
        let (freq, _) = self.tokenize_with_freq(rec);
        freq.keys().copied().collect()
    }

    fn posting_key_for_token(&self, th: u64, rid: &RecordId) -> Vec<u8> {
        build_posting_key(self.descriptor.id, type_tag::FTS, &th.to_le_bytes(), rid)
    }

    fn prefix_for_token(&self, th: u64) -> Vec<u8> {
        let mut prefix = Vec::with_capacity(4 + 1 + 8);
        prefix.extend_from_slice(&self.descriptor.id.to_le_bytes());
        prefix.push(type_tag::FTS);
        prefix.extend_from_slice(&th.to_le_bytes());
        prefix
    }

    async fn scan_token_with_values(
        &self,
        th: u64,
    ) -> Result<Vec<(RecordId, FtsPostingValue)>, IndexError> {
        let prefix = self.prefix_for_token(th);
        let mut stream = self.store.scan_prefix_stream(Bytes::from(prefix), 1024);
        let mut results = Vec::with_capacity(16);
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result.map_err(|e| IndexError::Storage(e.to_string()))?;
            for (key_bytes, val_bytes) in batch {
                if let Some(pk) = PostingKeyRef::decode(&key_bytes) {
                    if pk.index_id == self.descriptor.id && pk.type_tag == type_tag::FTS {
                        let pv: FtsPostingValue = if val_bytes.is_empty() {
                            FtsPostingValue { tf: 1, doc_len: 1 }
                        } else {
                            bincode::deserialize(&val_bytes)
                                .unwrap_or(FtsPostingValue { tf: 1, doc_len: 1 })
                        };
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

    fn tokenize_query(&self, query: &str) -> Vec<u64> {
        self.tokenizer
            .tokenize(query)
            .iter()
            .map(|t| token_hash(t))
            .collect()
    }

    async fn plan_insert(
        &self,
        rid: RecordId,
        rec: &(dyn RecordRef + Sync + '_),
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        let (freq, doc_len) = self.tokenize_with_freq(rec);
        if doc_len == 0 {
            return Ok(Vec::new());
        }
        let mut ops = Vec::with_capacity(freq.len() + 1);
        for (th, tf) in &freq {
            let key = self.posting_key_for_token(*th, &rid);
            let pv = FtsPostingValue { tf: *tf, doc_len };
            let val = bincode::serialize(&pv).map_err(|e| IndexError::Backend(e.to_string()))?;
            ops.push(IndexWriteOp::SetPosting {
                key: Bytes::from(key),
                value: Bytes::from(val),
            });
        }
        ops.push(IndexWriteOp::BumpFtsStats { doc_len, sign: 1 });
        Ok(ops)
    }

    async fn plan_update(
        &self,
        rid: RecordId,
        old: &(dyn RecordRef + Sync + '_),
        new: &(dyn RecordRef + Sync + '_),
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        let old_set = self.tokenize_set(old);
        let (new_freq, new_doc_len) = self.tokenize_with_freq(new);
        let (_, old_doc_len) = self.tokenize_with_freq(old);
        let new_set: TFxSet<u64> = new_freq.keys().copied().collect();

        let mut ops = Vec::new();
        // Remove disappeared tokens.
        for &th in old_set.difference(&new_set) {
            let key = self.posting_key_for_token(th, &rid);
            ops.push(IndexWriteOp::RemovePosting {
                key: Bytes::from(key),
            });
        }
        // Add/update all tokens in new (tf or doc_len may have changed).
        for (th, tf) in &new_freq {
            let key = self.posting_key_for_token(*th, &rid);
            let pv = FtsPostingValue {
                tf: *tf,
                doc_len: new_doc_len,
            };
            let val = bincode::serialize(&pv).map_err(|e| IndexError::Backend(e.to_string()))?;
            ops.push(IndexWriteOp::SetPosting {
                key: Bytes::from(key),
                value: Bytes::from(val),
            });
        }
        ops.push(IndexWriteOp::BumpFtsStats {
            doc_len: old_doc_len,
            sign: -1,
        });
        ops.push(IndexWriteOp::BumpFtsStats {
            doc_len: new_doc_len,
            sign: 1,
        });
        Ok(ops)
    }

    async fn plan_delete(
        &self,
        rid: RecordId,
        rec: &(dyn RecordRef + Sync + '_),
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        let (freq, doc_len) = self.tokenize_with_freq(rec);
        let mut ops = Vec::with_capacity(freq.len() + 1);
        for th in freq.keys() {
            let key = self.posting_key_for_token(*th, &rid);
            ops.push(IndexWriteOp::RemovePosting {
                key: Bytes::from(key),
            });
        }
        if doc_len > 0 {
            ops.push(IndexWriteOp::BumpFtsStats { doc_len, sign: -1 });
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
            if let IndexWriteOp::BumpFtsStats { doc_len, sign } = op {
                if *sign > 0 {
                    self.stats.on_insert(*doc_len);
                } else {
                    self.stats.on_delete(*doc_len);
                }
            }
        }
        Ok(())
    }

    async fn lookup(&self, query: IndexQuery) -> Result<IndexResult, IndexError> {
        match query {
            IndexQuery::Fts { tokens, mode } => {
                if tokens.is_empty() {
                    return Ok(IndexResult::Ranked(Vec::new()));
                }
                let total_docs = self
                    .stats
                    .doc_count
                    .load(std::sync::atomic::Ordering::Relaxed);
                let avg_dl = self.stats.avg_doc_len();

                // Collect per-token posting lists with tf/doc_len.
                let mut per_token: Vec<Vec<(RecordId, FtsPostingValue)>> =
                    Vec::with_capacity(tokens.len());
                for th in &tokens {
                    per_token.push(self.scan_token_with_values(*th).await?);
                }

                match mode {
                    FtsMode::AndAll => {
                        // Intersect record sets, accumulate BM25.
                        let mut rid_sets: Vec<BTreeSet<RecordId>> = per_token
                            .iter()
                            .map(|entries| entries.iter().map(|(r, _)| *r).collect())
                            .collect();
                        let intersection = {
                            let mut iter = rid_sets.drain(..);
                            let first = iter.next().unwrap();
                            iter.fold(first, |acc, s| &acc & &s)
                        };
                        let mut scores: TFxMap<RecordId, f64> = TFxMap::default();
                        for entries in per_token.iter() {
                            let df = entries.len() as u64;
                            let idf_val = bm25::idf(total_docs, df);
                            for (rid, pv) in entries {
                                if intersection.contains(rid) {
                                    *scores.entry(*rid).or_insert(0.0) += bm25::term_score(
                                        &self.params,
                                        pv.tf,
                                        pv.doc_len,
                                        avg_dl,
                                        idf_val,
                                    );
                                }
                            }
                        }
                        let mut ranked: Vec<(RecordId, f32)> =
                            scores.into_iter().map(|(r, s)| (r, s as f32)).collect();
                        ranked.sort_by(|a, b| {
                            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                        });
                        Ok(IndexResult::Ranked(ranked))
                    }
                    FtsMode::OrAny => {
                        // Union, accumulate BM25 from each matching term.
                        let mut scores: TFxMap<RecordId, f64> = TFxMap::default();
                        for entries in &per_token {
                            let df = entries.len() as u64;
                            let idf_val = bm25::idf(total_docs, df);
                            for (rid, pv) in entries {
                                *scores.entry(*rid).or_insert(0.0) += bm25::term_score(
                                    &self.params,
                                    pv.tf,
                                    pv.doc_len,
                                    avg_dl,
                                    idf_val,
                                );
                            }
                        }
                        let mut ranked: Vec<(RecordId, f32)> =
                            scores.into_iter().map(|(r, s)| (r, s as f32)).collect();
                        ranked.sort_by(|a, b| {
                            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                        });
                        Ok(IndexResult::Ranked(ranked))
                    }
                }
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
            let batch = batch_res.map_err(|e| IndexError::Storage(e.to_string()))?;
            for (_key_bytes, val_bytes) in batch {
                let rec = match InnerValue::from_bytes(&val_bytes) {
                    Ok(r) => r,
                    Err(_) => continue,
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
            let batch = batch_result.map_err(|e| IndexError::Storage(e.to_string()))?;
            for (key_bytes, _) in batch {
                let _ = self.store.remove(key_bytes).await;
            }
        }
        Ok(())
    }
}
