//! FTS backend with BM25 ranking (Phase 3).
//!
//! Extends the MVP FtsBackend: posting values carry `{ tf, doc_len }`,
//! global FtsStats track doc_count/sum_doc_len atomically. Lookup
//! returns `IndexResult::Ranked` with BM25 scores sorted descending.

use crate::index2::backend::{FtsMode, IndexBackend, IndexError, IndexQuery, IndexResult};
use crate::index2::bm25::{self, Bm25Params, FtsPostingValue, FtsStats};
use crate::index2::descriptor::IndexDescriptor;
use crate::index2::posting_layout::{build_posting_key, type_tag, PostingKeyRef};
use crate::index2::tokenizer::{self, token_hash, Tokenizer, WhitespaceTokenizer};
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

pub struct FtsRankedBackend {
    descriptor: IndexDescriptor,
    field_path: Vec<u64>,
    tokenizer: Arc<dyn Tokenizer>,
    store: Arc<dyn Store>,
    stats: Arc<FtsStats>,
    params: Bm25Params,
}

impl FtsRankedBackend {
    pub fn new(
        descriptor: IndexDescriptor,
        field_path: Vec<u64>,
        store: Arc<dyn Store>,
    ) -> Self {
        let tokenizer: Arc<dyn Tokenizer> = match &descriptor.kind {
            crate::index2::kind::IndexKind::Fts { tokenizer: tk, .. } => {
                Arc::from(tokenizer::build_tokenizer(tk))
            }
            _ => Arc::new(WhitespaceTokenizer),
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

    fn extract_text<'a>(&self, rec: &'a InnerValue) -> Option<&'a str> {
        let mut current = rec;
        for &seg in &self.field_path {
            match current {
                InnerValue::Map(m) => {
                    let key = shamir_types::core::interner::InternerKey::new(seg);
                    current = m.get(&key)?;
                }
                _ => return None,
            }
        }
        match current {
            InnerValue::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    fn tokenize_with_freq(&self, rec: &InnerValue) -> (HashMap<u64, u32>, u32) {
        match self.extract_text(rec) {
            Some(text) => {
                let tokens = self.tokenizer.tokenize(text);
                let doc_len = tokens.len() as u32;
                let mut freq: HashMap<u64, u32> = HashMap::new();
                for t in tokens {
                    *freq.entry(token_hash(&t)).or_insert(0) += 1;
                }
                (freq, doc_len)
            }
            None => (HashMap::new(), 0),
        }
    }

    fn tokenize_set(&self, rec: &InnerValue) -> HashSet<u64> {
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
        let mut stream = self
            .store
            .scan_prefix_stream(Bytes::from(prefix), 1024);
        let mut results = Vec::new();
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
    fn descriptor(&self) -> &IndexDescriptor {
        &self.descriptor
    }

    async fn on_insert(&self, rid: RecordId, rec: &InnerValue) -> Result<(), IndexError> {
        let (freq, doc_len) = self.tokenize_with_freq(rec);
        if doc_len == 0 {
            return Ok(());
        }
        for (th, tf) in &freq {
            let key = self.posting_key_for_token(*th, &rid);
            let pv = FtsPostingValue { tf: *tf, doc_len };
            let val = bincode::serialize(&pv).map_err(|e| IndexError::Backend(e.to_string()))?;
            self.store
                .set(Bytes::from(key), Bytes::from(val))
                .await
                .map_err(|e| IndexError::Storage(e.to_string()))?;
        }
        self.stats.on_insert(doc_len);
        Ok(())
    }

    async fn on_update(
        &self,
        rid: RecordId,
        old: &InnerValue,
        new: &InnerValue,
    ) -> Result<(), IndexError> {
        let old_set = self.tokenize_set(old);
        let (new_freq, new_doc_len) = self.tokenize_with_freq(new);
        let (_, old_doc_len) = self.tokenize_with_freq(old);
        let new_set: HashSet<u64> = new_freq.keys().copied().collect();

        // Remove disappeared tokens.
        for &th in old_set.difference(&new_set) {
            let key = self.posting_key_for_token(th, &rid);
            let _ = self.store.remove(Bytes::from(key)).await;
        }
        // Add/update all tokens in new (tf or doc_len may have changed).
        for (th, tf) in &new_freq {
            let key = self.posting_key_for_token(*th, &rid);
            let pv = FtsPostingValue {
                tf: *tf,
                doc_len: new_doc_len,
            };
            let val = bincode::serialize(&pv).map_err(|e| IndexError::Backend(e.to_string()))?;
            self.store
                .set(Bytes::from(key), Bytes::from(val))
                .await
                .map_err(|e| IndexError::Storage(e.to_string()))?;
        }
        self.stats.on_delete(old_doc_len);
        self.stats.on_insert(new_doc_len);
        Ok(())
    }

    async fn on_delete(&self, rid: RecordId, rec: &InnerValue) -> Result<(), IndexError> {
        let (freq, doc_len) = self.tokenize_with_freq(rec);
        for th in freq.keys() {
            let key = self.posting_key_for_token(*th, &rid);
            let _ = self.store.remove(Bytes::from(key)).await;
        }
        if doc_len > 0 {
            self.stats.on_delete(doc_len);
        }
        Ok(())
    }

    async fn on_batch_insert(
        &self,
        items: &[(RecordId, &InnerValue)],
    ) -> Result<(), IndexError> {
        for (rid, rec) in items {
            self.on_insert(*rid, rec).await?;
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
                        let mut scores: HashMap<RecordId, f64> = HashMap::new();
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
                        let mut ranked: Vec<(RecordId, f32)> = scores
                            .into_iter()
                            .map(|(r, s)| (r, s as f32))
                            .collect();
                        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                        Ok(IndexResult::Ranked(ranked))
                    }
                    FtsMode::OrAny => {
                        // Union, accumulate BM25 from each matching term.
                        let mut scores: HashMap<RecordId, f64> = HashMap::new();
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
                        let mut ranked: Vec<(RecordId, f32)> = scores
                            .into_iter()
                            .map(|(r, s)| (r, s as f32))
                            .collect();
                        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index2::kind::{IndexKind, TokenizerKind};
    use crate::index2::tokenizer::token_hash;
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_types::core::interner::{Interner, InternerKey, TouchInd};
    use shamir_types::types::common::new_map_wc;
    use smallvec::SmallVec;

    fn intern(i: &Interner, s: &str) -> u64 {
        match i.touch_ind(s).unwrap() {
            TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
        }
    }

    fn make_rec(interner: &Interner, body: &str) -> InnerValue {
        let mut m = new_map_wc(1);
        m.insert(
            InternerKey::new(intern(interner, "body")),
            InnerValue::Str(body.into()),
        );
        InnerValue::Map(m)
    }

    fn make_backend(interner: &Interner, store: Arc<dyn Store>) -> FtsRankedBackend {
        let desc = IndexDescriptor::new(
            20,
            "body_fts_ranked",
            intern(interner, "body_fts_ranked"),
            SmallVec::new(),
            IndexKind::Fts {
                tokenizer: TokenizerKind::Whitespace,
                language: None,
            },
        );
        FtsRankedBackend::new(desc, vec![intern(interner, "body")], store)
    }

    #[tokio::test]
    async fn ranked_and_query_returns_scores() {
        let i = Interner::new();
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let fts = make_backend(&i, store);

        let r1 = RecordId::new();
        let r2 = RecordId::new();
        // r1 mentions "rust" 3 times → higher tf
        fts.on_insert(r1, &make_rec(&i, "rust rust rust is great"))
            .await
            .unwrap();
        fts.on_insert(r2, &make_rec(&i, "rust is ok"))
            .await
            .unwrap();

        let result = fts
            .lookup(IndexQuery::Fts {
                tokens: vec![token_hash("rust")],
                mode: FtsMode::AndAll,
            })
            .await
            .unwrap();

        match result {
            IndexResult::Ranked(ranked) => {
                assert_eq!(ranked.len(), 2);
                // r1 has higher tf → higher score → should be first
                assert_eq!(ranked[0].0, r1);
                assert!(ranked[0].1 > ranked[1].1);
            }
            _ => panic!("expected Ranked"),
        }
    }

    #[tokio::test]
    async fn ranked_or_query_union() {
        let i = Interner::new();
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let fts = make_backend(&i, store);

        let r1 = RecordId::new();
        let r2 = RecordId::new();
        let r3 = RecordId::new();
        fts.on_insert(r1, &make_rec(&i, "alpha beta")).await.unwrap();
        fts.on_insert(r2, &make_rec(&i, "gamma delta")).await.unwrap();
        fts.on_insert(r3, &make_rec(&i, "alpha gamma")).await.unwrap();

        let result = fts
            .lookup(IndexQuery::Fts {
                tokens: vec![token_hash("alpha"), token_hash("gamma")],
                mode: FtsMode::OrAny,
            })
            .await
            .unwrap();

        match result {
            IndexResult::Ranked(ranked) => {
                assert_eq!(ranked.len(), 3);
                // r3 matches both terms → highest
                assert_eq!(ranked[0].0, r3);
            }
            _ => panic!("expected Ranked"),
        }
    }

    #[tokio::test]
    async fn stats_track_across_insert_delete() {
        let i = Interner::new();
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let fts = make_backend(&i, store);

        let r1 = RecordId::new();
        let rec = make_rec(&i, "hello world foo bar");
        fts.on_insert(r1, &rec).await.unwrap();

        assert_eq!(
            fts.stats.doc_count.load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        assert!((fts.stats.avg_doc_len() - 4.0).abs() < 0.01);

        fts.on_delete(r1, &rec).await.unwrap();
        assert_eq!(
            fts.stats.doc_count.load(std::sync::atomic::Ordering::Relaxed),
            0
        );
    }

    #[tokio::test]
    async fn longer_doc_gets_lower_score() {
        let i = Interner::new();
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let fts = make_backend(&i, store);

        let r_short = RecordId::new();
        let r_long = RecordId::new();
        // Same tf=1 for "rust", but r_long has many more words.
        fts.on_insert(r_short, &make_rec(&i, "rust rocks"))
            .await
            .unwrap();
        fts.on_insert(
            r_long,
            &make_rec(&i, "rust is just one of many many many many many words here"),
        )
        .await
        .unwrap();

        let result = fts
            .lookup(IndexQuery::Fts {
                tokens: vec![token_hash("rust")],
                mode: FtsMode::AndAll,
            })
            .await
            .unwrap();

        match result {
            IndexResult::Ranked(ranked) => {
                assert_eq!(ranked.len(), 2);
                assert_eq!(ranked[0].0, r_short, "shorter doc should rank higher");
                assert!(ranked[0].1 > ranked[1].1);
            }
            _ => panic!("expected Ranked"),
        }
    }

    #[tokio::test]
    async fn rebuild_restores_stats_from_data_store() {
        let i = Interner::new();
        // Separate stores: data_store holds records, info_store holds postings.
        let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let info_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let fts = make_backend(&i, Arc::clone(&info_store));

        // Insert records via data_store directly (simulating persisted data).
        let recs = [
            make_rec(&i, "alpha beta gamma"),       // 3 tokens
            make_rec(&i, "hello world foo bar"),    // 4 tokens
            make_rec(&i, "short"),                   // 1 token
            make_rec(&i, "a b c d e f"),             // 6 tokens
            make_rec(&i, ""),                        // 0 tokens — skipped
        ];
        for rec in &recs {
            let rid = RecordId::new();
            data_store
                .set(rid.to_bytes(), Bytes::from(rec.to_bytes().unwrap()))
                .await
                .unwrap();
            // Also feed into FTS so postings exist (rebuild only updates stats).
            fts.on_insert(rid, rec).await.unwrap();
        }

        // Verify stats are correct after inserts.
        assert_eq!(
            fts.stats.doc_count.load(std::sync::atomic::Ordering::Relaxed),
            4
        ); // "" has doc_len=0 → not counted
        assert_eq!(
            fts.stats.sum_doc_len.load(std::sync::atomic::Ordering::Relaxed),
            14
        ); // 3+4+1+6

        // Reset stats to zero (simulates reopen where counters start at 0).
        fts.stats.doc_count.store(0, std::sync::atomic::Ordering::Relaxed);
        fts.stats.sum_doc_len.store(0, std::sync::atomic::Ordering::Relaxed);

        assert_eq!(
            fts.stats.doc_count.load(std::sync::atomic::Ordering::Relaxed),
            0
        );

        // Rebuild from data_store.
        fts.rebuild(Arc::clone(&data_store)).await.unwrap();

        // Stats must be restored.
        assert_eq!(
            fts.stats.doc_count.load(std::sync::atomic::Ordering::Relaxed),
            4
        );
        assert_eq!(
            fts.stats.sum_doc_len.load(std::sync::atomic::Ordering::Relaxed),
            14
        );
        assert!((fts.stats.avg_doc_len() - 3.5).abs() < 0.01); // 14/4
    }
}
