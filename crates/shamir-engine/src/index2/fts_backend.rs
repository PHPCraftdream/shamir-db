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
