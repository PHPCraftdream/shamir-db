//! FTS (Full-Text Search) index backend — MVP without BM25.
//!
//! Tokenizes text fields, stores per-token postings via type-tagged
//! keys. Supports AND/OR queries. Update uses token-set diff to
//! minimize I/O.

use crate::index2::backend::{FtsMode, IndexBackend, IndexError, IndexQuery, IndexResult};
use crate::index2::descriptor::IndexDescriptor;
use crate::index2::posting_layout::{build_posting_key, type_tag, PostingKeyRef};
use crate::index2::tokenizer::{self, token_hash, Tokenizer, WhitespaceTokenizer};
use crate::index2::write_ops::IndexWriteOp;
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

pub struct FtsBackend {
    descriptor: IndexDescriptor,
    field_path: Vec<u64>,
    tokenizer: Arc<dyn Tokenizer>,
    store: Arc<dyn Store>,
}

impl FtsBackend {
    pub fn new(descriptor: IndexDescriptor, field_path: Vec<u64>, store: Arc<dyn Store>) -> Self {
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
        }
    }

    pub fn with_tokenizer(
        descriptor: IndexDescriptor,
        field_path: Vec<u64>,
        tokenizer: Arc<dyn Tokenizer>,
        store: Arc<dyn Store>,
    ) -> Self {
        Self {
            descriptor,
            field_path,
            tokenizer,
            store,
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

    fn tokenize_record(&self, rec: &InnerValue) -> HashSet<u64> {
        match self.extract_text(rec) {
            Some(text) => self
                .tokenizer
                .tokenize(text)
                .into_iter()
                .map(|t| token_hash(&t))
                .collect(),
            None => HashSet::new(),
        }
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

    async fn scan_token_postings(&self, th: u64) -> Result<BTreeSet<RecordId>, IndexError> {
        let prefix = self.prefix_for_token(th);
        let mut stream = self
            .store
            .scan_prefix_stream(Bytes::from(prefix.clone()), 1024);
        let mut rids = BTreeSet::new();
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result.map_err(|e| IndexError::Storage(e.to_string()))?;
            for (key_bytes, _) in batch {
                if let Some(pk) = PostingKeyRef::decode(&key_bytes) {
                    if pk.index_id == self.descriptor.id && pk.type_tag == type_tag::FTS {
                        rids.insert(pk.record_id_owned());
                    }
                }
            }
        }
        Ok(rids)
    }
}

#[async_trait]
impl IndexBackend for FtsBackend {
    fn descriptor(&self) -> &IndexDescriptor {
        &self.descriptor
    }

    async fn plan_insert(
        &self,
        rid: RecordId,
        rec: &InnerValue,
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        let tokens = self.tokenize_record(rec);
        let mut ops = Vec::with_capacity(tokens.len());
        for th in tokens {
            let key = self.posting_key_for_token(th, &rid);
            ops.push(IndexWriteOp::SetPosting {
                key: Bytes::from(key),
                value: Bytes::new(),
            });
        }
        Ok(ops)
    }

    async fn plan_update(
        &self,
        rid: RecordId,
        old: &InnerValue,
        new: &InnerValue,
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        let old_tokens = self.tokenize_record(old);
        let new_tokens = self.tokenize_record(new);
        let mut ops = Vec::new();
        // Remove tokens that disappeared.
        for &th in old_tokens.difference(&new_tokens) {
            let key = self.posting_key_for_token(th, &rid);
            ops.push(IndexWriteOp::RemovePosting {
                key: Bytes::from(key),
            });
        }
        // Add tokens that appeared.
        for &th in new_tokens.difference(&old_tokens) {
            let key = self.posting_key_for_token(th, &rid);
            ops.push(IndexWriteOp::SetPosting {
                key: Bytes::from(key),
                value: Bytes::new(),
            });
        }
        Ok(ops)
    }

    async fn plan_delete(
        &self,
        rid: RecordId,
        rec: &InnerValue,
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        let tokens = self.tokenize_record(rec);
        let mut ops = Vec::with_capacity(tokens.len());
        for th in tokens {
            let key = self.posting_key_for_token(th, &rid);
            ops.push(IndexWriteOp::RemovePosting {
                key: Bytes::from(key),
            });
        }
        Ok(ops)
    }

    async fn lookup(&self, query: IndexQuery) -> Result<IndexResult, IndexError> {
        match query {
            IndexQuery::Fts { tokens, mode } => {
                if tokens.is_empty() {
                    return Ok(IndexResult::Set(BTreeSet::new()));
                }
                let mut sets: Vec<BTreeSet<RecordId>> = Vec::with_capacity(tokens.len());
                for th in &tokens {
                    sets.push(self.scan_token_postings(*th).await?);
                }
                let result = match mode {
                    FtsMode::AndAll => {
                        let mut iter = sets.into_iter();
                        let first = iter.next().unwrap();
                        iter.fold(first, |acc, s| &acc & &s)
                    }
                    FtsMode::OrAny => {
                        let mut union = BTreeSet::new();
                        for s in sets {
                            union.extend(s);
                        }
                        union
                    }
                };
                Ok(IndexResult::Set(result))
            }
            _ => Err(IndexError::Backend(
                "FtsBackend only supports Fts queries".into(),
            )),
        }
    }

    async fn rebuild(&self, _source: Arc<dyn Store>) -> Result<(), IndexError> {
        // FtsBackend has no in-memory state — postings live entirely in
        // the info_store. Nothing to rebuild.
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

    async fn apply_insert(
        backend: &FtsBackend,
        store: &Arc<dyn Store>,
        rid: RecordId,
        rec: &InnerValue,
    ) {
        let ops = backend.plan_insert(rid, rec).await.unwrap();
        crate::index2::apply_index_ops(&ops, store, backend)
            .await
            .unwrap();
    }

    async fn apply_update(
        backend: &FtsBackend,
        store: &Arc<dyn Store>,
        rid: RecordId,
        old: &InnerValue,
        new: &InnerValue,
    ) {
        let ops = backend.plan_update(rid, old, new).await.unwrap();
        crate::index2::apply_index_ops(&ops, store, backend)
            .await
            .unwrap();
    }

    async fn apply_delete(
        backend: &FtsBackend,
        store: &Arc<dyn Store>,
        rid: RecordId,
        rec: &InnerValue,
    ) {
        let ops = backend.plan_delete(rid, rec).await.unwrap();
        crate::index2::apply_index_ops(&ops, store, backend)
            .await
            .unwrap();
    }

    fn make_fts(interner: &Interner, store: Arc<dyn Store>) -> FtsBackend {
        let desc = IndexDescriptor::new(
            10,
            "body_fts",
            intern(interner, "body_fts"),
            SmallVec::new(),
            IndexKind::Fts {
                tokenizer: TokenizerKind::Whitespace,
                language: None,
            },
        );
        FtsBackend::new(desc, vec![intern(interner, "body")], store)
    }

    #[tokio::test]
    async fn and_query() {
        let i = Interner::new();
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let fts = make_fts(&i, Arc::clone(&store));

        let r1 = RecordId::new();
        let r2 = RecordId::new();
        let r3 = RecordId::new();
        apply_insert(&fts, &store, r1, &make_rec(&i, "hello world foo")).await;
        apply_insert(&fts, &store, r2, &make_rec(&i, "hello bar")).await;
        apply_insert(&fts, &store, r3, &make_rec(&i, "world bar")).await;

        // AND("hello", "world") → only r1
        let result = fts
            .lookup(IndexQuery::Fts {
                tokens: vec![token_hash("hello"), token_hash("world")],
                mode: FtsMode::AndAll,
            })
            .await
            .unwrap();
        match result {
            IndexResult::Set(s) => {
                assert!(s.contains(&r1));
                assert!(!s.contains(&r2));
                assert!(!s.contains(&r3));
            }
            _ => panic!("expected Set"),
        }
    }

    #[tokio::test]
    async fn or_query() {
        let i = Interner::new();
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let fts = make_fts(&i, Arc::clone(&store));

        let r1 = RecordId::new();
        let r2 = RecordId::new();
        let r3 = RecordId::new();
        apply_insert(&fts, &store, r1, &make_rec(&i, "hello world")).await;
        apply_insert(&fts, &store, r2, &make_rec(&i, "hello bar")).await;
        apply_insert(&fts, &store, r3, &make_rec(&i, "baz qux")).await;

        // OR("hello", "baz") → r1, r2, r3
        let result = fts
            .lookup(IndexQuery::Fts {
                tokens: vec![token_hash("hello"), token_hash("baz")],
                mode: FtsMode::OrAny,
            })
            .await
            .unwrap();
        match result {
            IndexResult::Set(s) => {
                assert!(s.contains(&r1));
                assert!(s.contains(&r2));
                assert!(s.contains(&r3));
            }
            _ => panic!("expected Set"),
        }
    }

    #[tokio::test]
    async fn update_diff_tokens() {
        let i = Interner::new();
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let fts = make_fts(&i, Arc::clone(&store));

        let rid = RecordId::new();
        let old = make_rec(&i, "alpha beta gamma");
        apply_insert(&fts, &store, rid, &old).await;

        let new_rec = make_rec(&i, "alpha delta gamma");
        apply_update(&fts, &store, rid, &old, &new_rec).await;

        // "beta" gone
        let r = fts
            .lookup(IndexQuery::Fts {
                tokens: vec![token_hash("beta")],
                mode: FtsMode::AndAll,
            })
            .await
            .unwrap();
        match r {
            IndexResult::Set(s) => assert!(s.is_empty()),
            _ => panic!("expected Set"),
        }

        // "delta" present
        let r = fts
            .lookup(IndexQuery::Fts {
                tokens: vec![token_hash("delta")],
                mode: FtsMode::AndAll,
            })
            .await
            .unwrap();
        match r {
            IndexResult::Set(s) => assert!(s.contains(&rid)),
            _ => panic!("expected Set"),
        }

        // "alpha" still present (unchanged)
        let r = fts
            .lookup(IndexQuery::Fts {
                tokens: vec![token_hash("alpha")],
                mode: FtsMode::AndAll,
            })
            .await
            .unwrap();
        match r {
            IndexResult::Set(s) => assert!(s.contains(&rid)),
            _ => panic!("expected Set"),
        }
    }

    #[tokio::test]
    async fn delete_removes_all_tokens() {
        let i = Interner::new();
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let fts = make_fts(&i, Arc::clone(&store));

        let rid = RecordId::new();
        let rec = make_rec(&i, "hello world");
        apply_insert(&fts, &store, rid, &rec).await;
        apply_delete(&fts, &store, rid, &rec).await;

        let r = fts
            .lookup(IndexQuery::Fts {
                tokens: vec![token_hash("hello")],
                mode: FtsMode::AndAll,
            })
            .await
            .unwrap();
        match r {
            IndexResult::Set(s) => assert!(s.is_empty()),
            _ => panic!("expected Set"),
        }
    }

    #[tokio::test]
    async fn empty_query_returns_empty() {
        let i = Interner::new();
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let fts = make_fts(&i, store);

        let r = fts
            .lookup(IndexQuery::Fts {
                tokens: vec![],
                mode: FtsMode::AndAll,
            })
            .await
            .unwrap();
        match r {
            IndexResult::Set(s) => assert!(s.is_empty()),
            _ => panic!("expected Set"),
        }
    }

    #[tokio::test]
    async fn plan_insert_returns_set_postings() {
        let i = Interner::new();
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let backend = make_fts(&i, Arc::clone(&store));
        let rec = make_rec(&i, "hello world hello");
        let rid = RecordId::new();
        let ops = backend.plan_insert(rid, &rec).await.unwrap();
        // "hello" and "world" -> 2 unique tokens -> 2 SetPostings
        assert_eq!(ops.len(), 2);
        assert!(ops
            .iter()
            .all(|op| matches!(op, IndexWriteOp::SetPosting { .. })));
    }

    #[tokio::test]
    async fn plan_update_returns_diff_ops() {
        let i = Interner::new();
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let backend = make_fts(&i, Arc::clone(&store));
        let old_rec = make_rec(&i, "hello world");
        let new_rec = make_rec(&i, "hello rust");
        let rid = RecordId::new();
        let ops = backend.plan_update(rid, &old_rec, &new_rec).await.unwrap();
        // "world" removed, "rust" added, "hello" unchanged
        let removes: Vec<_> = ops
            .iter()
            .filter(|o| matches!(o, IndexWriteOp::RemovePosting { .. }))
            .collect();
        let sets: Vec<_> = ops
            .iter()
            .filter(|o| matches!(o, IndexWriteOp::SetPosting { .. }))
            .collect();
        assert_eq!(removes.len(), 1); // "world"
        assert_eq!(sets.len(), 1); // "rust"
    }

    #[tokio::test]
    async fn plan_delete_returns_remove_for_all_postings() {
        let i = Interner::new();
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let backend = make_fts(&i, Arc::clone(&store));
        let rec = make_rec(&i, "foo bar baz");
        let rid = RecordId::new();
        let ops = backend.plan_delete(rid, &rec).await.unwrap();
        assert_eq!(ops.len(), 3); // 3 unique tokens
        assert!(ops
            .iter()
            .all(|op| matches!(op, IndexWriteOp::RemovePosting { .. })));
    }

    #[tokio::test]
    async fn plan_apply_round_trip() {
        let i = Interner::new();
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let backend = make_fts(&i, Arc::clone(&store));
        let rec = make_rec(&i, "alpha beta gamma");
        let rid = RecordId::new();

        apply_insert(&backend, &store, rid, &rec).await;

        // Verify all 3 tokens are searchable
        for token in &["alpha", "beta", "gamma"] {
            let result = backend
                .lookup(IndexQuery::Fts {
                    tokens: vec![token_hash(token)],
                    mode: FtsMode::AndAll,
                })
                .await
                .unwrap();
            match result {
                IndexResult::Set(s) => assert!(s.contains(&rid), "token '{token}' should match"),
                _ => panic!("expected Set"),
            }
        }
    }

    #[tokio::test]
    async fn plan_insert_writes_postings_per_token() {
        let i = Interner::new();
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let backend = make_fts(&i, Arc::clone(&store));
        let rid = RecordId::new();
        let rec = make_rec(&i, "hello world hello");

        let ops = backend.plan_insert(rid, &rec).await.unwrap();
        let set_ops = ops
            .iter()
            .filter(|o| matches!(o, IndexWriteOp::SetPosting { .. }))
            .count();
        assert_eq!(set_ops, 2, "expected exactly 2 SetPostings (hello + world)");

        crate::index2::apply_index_ops(&ops, &store, &backend)
            .await
            .unwrap();

        let stream = store.iter_stream(64);
        futures::pin_mut!(stream);
        let mut count = 0usize;
        while let Some(batch) = stream.next().await {
            count += batch.unwrap().len();
        }
        assert_eq!(
            count, 2,
            "store should hold exactly two postings after apply"
        );
    }
}
