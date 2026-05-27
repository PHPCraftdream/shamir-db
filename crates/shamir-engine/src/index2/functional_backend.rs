//! `IndexBackend` for functional (computed) indexes.
//!
//! On insert / update / delete, evaluates `IndexExpr` against the
//! record, hashes the result, and stores/removes a type-tagged
//! posting in the backing `Store`.

use crate::index2::backend::{IndexBackend, IndexError, IndexQuery, IndexResult};
use crate::index2::descriptor::IndexDescriptor;
use crate::index2::expr::{ExprError, IndexExpr};
use crate::index2::posting_layout::{build_posting_key, type_tag, PostingKeyRef};
use crate::index2::write_ops::{apply_index_ops, IndexWriteOp};
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use fxhash::FxHasher;
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::collections::BTreeSet;
use std::hash::Hasher;
use std::sync::Arc;

pub struct FunctionalBackend {
    descriptor: IndexDescriptor,
    expr: IndexExpr,
    store: Arc<dyn Store>,
}

impl FunctionalBackend {
    pub fn new(descriptor: IndexDescriptor, expr: IndexExpr, store: Arc<dyn Store>) -> Self {
        Self {
            descriptor,
            expr,
            store,
        }
    }

    pub fn hash_value(val: &InnerValue) -> [u8; 16] {
        let mut h1 = FxHasher::default();
        let mut h2 = FxHasher::default();
        hash_inner(val, &mut h1);
        h2.write_u64(h1.finish());
        h2.write_u8(0xFF);
        hash_inner(val, &mut h2);
        let a = h1.finish().to_le_bytes();
        let b = h2.finish().to_le_bytes();
        let mut out = [0u8; 16];
        out[..8].copy_from_slice(&a);
        out[8..].copy_from_slice(&b);
        out
    }

    fn posting_key(&self, val: &InnerValue, rid: &RecordId) -> Vec<u8> {
        let h = Self::hash_value(val);
        build_posting_key(self.descriptor.id, type_tag::FUNCTIONAL, &h, rid)
    }

    fn eval_or_null(&self, rec: &InnerValue) -> InnerValue {
        match self.expr.eval(rec) {
            Ok(v) => v,
            Err(ExprError::FieldNotFound) => InnerValue::Null,
            Err(ExprError::TypeMismatch { .. }) => InnerValue::Null,
            Err(ExprError::DivisionByZero) => InnerValue::Null,
        }
    }

    fn prefix_for_value_hash(&self, value_hash: &[u8]) -> Vec<u8> {
        let mut prefix = Vec::with_capacity(4 + 1 + value_hash.len());
        prefix.extend_from_slice(&self.descriptor.id.to_le_bytes());
        prefix.push(type_tag::FUNCTIONAL);
        prefix.extend_from_slice(value_hash);
        prefix
    }

    async fn scan_postings_by_prefix(
        &self,
        prefix: &[u8],
    ) -> Result<Vec<(Bytes, Bytes)>, IndexError> {
        let mut stream = self
            .store
            .scan_prefix_stream(Bytes::copy_from_slice(prefix), 1024);
        let mut all = Vec::new();
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result.map_err(|e| IndexError::Storage(e.to_string()))?;
            all.extend(batch);
        }
        Ok(all)
    }
}

fn hash_inner(val: &InnerValue, h: &mut FxHasher) {
    match val {
        InnerValue::Null => h.write_u8(0),
        InnerValue::Bool(b) => {
            h.write_u8(1);
            h.write_u8(*b as u8);
        }
        InnerValue::Int(n) => {
            h.write_u8(2);
            h.write_i64(*n);
        }
        InnerValue::F64(f) => {
            h.write_u8(3);
            h.write_u64(f.to_bits());
        }
        InnerValue::Str(s) => {
            h.write_u8(4);
            h.write(s.as_bytes());
        }
        InnerValue::List(items) => {
            h.write_u8(5);
            h.write_usize(items.len());
            for item in items {
                hash_inner(item, h);
            }
        }
        InnerValue::Map(m) => {
            h.write_u8(6);
            h.write_usize(m.len());
            for (k, v) in m.iter() {
                h.write(k.as_bytes());
                hash_inner(v, h);
            }
        }
        _ => h.write_u8(255),
    }
}

#[async_trait]
impl IndexBackend for FunctionalBackend {
    fn descriptor(&self) -> &IndexDescriptor {
        &self.descriptor
    }

    async fn plan_insert(
        &self,
        rid: RecordId,
        rec: &InnerValue,
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        let val = self.eval_or_null(rec);
        let key = self.posting_key(&val, &rid);
        Ok(vec![IndexWriteOp::SetPosting {
            key: Bytes::from(key),
            value: Bytes::new(),
        }])
    }

    async fn plan_update(
        &self,
        rid: RecordId,
        old: &InnerValue,
        new: &InnerValue,
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        let old_val = self.eval_or_null(old);
        let new_val = self.eval_or_null(new);
        let old_hash = Self::hash_value(&old_val);
        let new_hash = Self::hash_value(&new_val);
        if old_hash != new_hash {
            let old_key = self.posting_key(&old_val, &rid);
            let new_key = self.posting_key(&new_val, &rid);
            Ok(vec![
                IndexWriteOp::RemovePosting {
                    key: Bytes::from(old_key),
                },
                IndexWriteOp::SetPosting {
                    key: Bytes::from(new_key),
                    value: Bytes::new(),
                },
            ])
        } else {
            Ok(vec![])
        }
    }

    async fn plan_delete(
        &self,
        rid: RecordId,
        rec: &InnerValue,
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        let val = self.eval_or_null(rec);
        let key = self.posting_key(&val, &rid);
        Ok(vec![IndexWriteOp::RemovePosting {
            key: Bytes::from(key),
        }])
    }

    async fn on_insert(&self, rid: RecordId, rec: &InnerValue) -> Result<(), IndexError> {
        let ops = self.plan_insert(rid, rec).await?;
        apply_index_ops(&ops, &self.store, self).await
    }

    async fn on_update(
        &self,
        rid: RecordId,
        old: &InnerValue,
        new: &InnerValue,
    ) -> Result<(), IndexError> {
        let ops = self.plan_update(rid, old, new).await?;
        apply_index_ops(&ops, &self.store, self).await
    }

    async fn on_delete(&self, rid: RecordId, rec: &InnerValue) -> Result<(), IndexError> {
        let ops = self.plan_delete(rid, rec).await?;
        apply_index_ops(&ops, &self.store, self).await
    }

    async fn on_batch_insert(&self, items: &[(RecordId, &InnerValue)]) -> Result<(), IndexError> {
        for (rid, rec) in items {
            self.on_insert(*rid, rec).await?;
        }
        Ok(())
    }

    async fn lookup(&self, query: IndexQuery) -> Result<IndexResult, IndexError> {
        match query {
            IndexQuery::Point { keys } => {
                let mut result = BTreeSet::new();
                for value_hash in &keys {
                    let prefix = self.prefix_for_value_hash(value_hash);
                    let entries = self.scan_postings_by_prefix(&prefix).await?;
                    for (key_bytes, _) in entries {
                        if let Some(pk) = PostingKeyRef::decode(&key_bytes) {
                            if pk.index_id == self.descriptor.id
                                && pk.type_tag == type_tag::FUNCTIONAL
                                && pk.value_bytes == value_hash.as_slice()
                            {
                                result.insert(pk.record_id_owned());
                            }
                        }
                    }
                }
                Ok(IndexResult::Set(result))
            }
            _ => Err(IndexError::Backend(
                "FunctionalBackend only supports Point queries".into(),
            )),
        }
    }

    async fn rebuild(&self, _source: Arc<dyn Store>) -> Result<(), IndexError> {
        // FunctionalBackend has no in-memory state — postings live entirely
        // in the info_store. Nothing to rebuild.
        Ok(())
    }

    async fn drop_all(&self) -> Result<(), IndexError> {
        let prefix = self.descriptor.id.to_le_bytes();
        let entries = self.scan_postings_by_prefix(&prefix).await?;
        for (key_bytes, _) in entries {
            let _ = self.store.remove(key_bytes).await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index2::kind::IndexKind;
    use crate::index2::write_ops::IndexWriteOp;
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_types::core::interner::{Interner, InternerKey, TouchInd};
    use shamir_types::types::common::new_map_wc;
    use smallvec::SmallVec;

    fn intern(i: &Interner, s: &str) -> u64 {
        match i.touch_ind(s).unwrap() {
            TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
        }
    }

    fn make_rec(interner: &Interner, email: &str, age: i64) -> InnerValue {
        let mut m = new_map_wc(3);
        m.insert(
            InternerKey::new(intern(interner, "email")),
            InnerValue::Str(email.into()),
        );
        m.insert(
            InternerKey::new(intern(interner, "age")),
            InnerValue::Int(age),
        );
        InnerValue::Map(m)
    }

    fn make_backend(interner: &Interner, store: Arc<dyn Store>) -> FunctionalBackend {
        let expr = IndexExpr::Lower(Box::new(IndexExpr::Trim(Box::new(IndexExpr::Field(vec![
            intern(interner, "email"),
        ])))));
        let desc = IndexDescriptor::new(
            1,
            "email_lower",
            intern(interner, "email_lower"),
            SmallVec::new(),
            IndexKind::Functional(Box::new(crate::index2::kind::FunctionalConfig {
                expr: expr.clone(),
            })),
        );
        FunctionalBackend::new(desc, expr, store)
    }

    #[tokio::test]
    async fn insert_and_lookup() {
        let interner = Interner::new();
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let backend = make_backend(&interner, store);

        let rid = RecordId::new();
        let rec = make_rec(&interner, "  Alice@FOO.COM  ", 30);
        backend.on_insert(rid, &rec).await.unwrap();

        let lookup_val = InnerValue::Str("alice@foo.com".into());
        let hash = FunctionalBackend::hash_value(&lookup_val);
        let result = backend
            .lookup(IndexQuery::Point {
                keys: smallvec::smallvec![hash.to_vec()],
            })
            .await
            .unwrap();

        match result {
            IndexResult::Set(s) => assert!(s.contains(&rid)),
            _ => panic!("expected Set"),
        }
    }

    #[tokio::test]
    async fn update_changes_posting() {
        let interner = Interner::new();
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let backend = make_backend(&interner, store);

        let rid = RecordId::new();
        let old = make_rec(&interner, "alice@old.com", 25);
        backend.on_insert(rid, &old).await.unwrap();

        let new_rec = make_rec(&interner, "bob@new.com", 25);
        backend.on_update(rid, &old, &new_rec).await.unwrap();

        let old_hash = FunctionalBackend::hash_value(&InnerValue::Str("alice@old.com".into()));
        let r = backend
            .lookup(IndexQuery::Point {
                keys: smallvec::smallvec![old_hash.to_vec()],
            })
            .await
            .unwrap();
        match r {
            IndexResult::Set(s) => assert!(s.is_empty(), "old posting should be gone"),
            _ => panic!("expected Set"),
        }

        let new_hash = FunctionalBackend::hash_value(&InnerValue::Str("bob@new.com".into()));
        let r = backend
            .lookup(IndexQuery::Point {
                keys: smallvec::smallvec![new_hash.to_vec()],
            })
            .await
            .unwrap();
        match r {
            IndexResult::Set(s) => assert!(s.contains(&rid)),
            _ => panic!("expected Set"),
        }
    }

    #[tokio::test]
    async fn plan_insert_returns_one_set_posting() {
        let interner = Interner::new();
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let backend = make_backend(&interner, store);

        let rid = RecordId::new();
        let rec = make_rec(&interner, "  Alice@FOO.COM  ", 30);
        let ops = backend.plan_insert(rid, &rec).await.unwrap();

        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], IndexWriteOp::SetPosting { value, .. } if value.is_empty()));
    }

    #[tokio::test]
    async fn plan_update_returns_remove_old_set_new_if_hash_changes() {
        let interner = Interner::new();
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let backend = make_backend(&interner, store);

        let rid = RecordId::new();
        let old = make_rec(&interner, "alice@old.com", 25);
        let new_rec = make_rec(&interner, "bob@new.com", 25);
        let ops = backend.plan_update(rid, &old, &new_rec).await.unwrap();

        assert_eq!(ops.len(), 2);
        assert!(matches!(&ops[0], IndexWriteOp::RemovePosting { .. }));
        assert!(matches!(&ops[1], IndexWriteOp::SetPosting { .. }));
    }

    #[tokio::test]
    async fn plan_update_returns_empty_if_hash_same() {
        let interner = Interner::new();
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let backend = make_backend(&interner, store);

        let rid = RecordId::new();
        let rec1 = make_rec(&interner, "same@email.com", 20);
        let rec2 = make_rec(&interner, "same@email.com", 99);
        let ops = backend.plan_update(rid, &rec1, &rec2).await.unwrap();

        assert!(ops.is_empty());
    }

    #[tokio::test]
    async fn equivalence_plan_apply_vs_direct_on_insert() {
        use crate::index2::write_ops::apply_index_ops;

        let interner = Interner::new();
        let store_a: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let store_b: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let backend_a = make_backend(&interner, Arc::clone(&store_a));
        let backend_b = make_backend(&interner, Arc::clone(&store_b));

        let rid = RecordId::new();
        let rec = make_rec(&interner, "  Test@Example.COM  ", 42);

        // Direct on_insert
        backend_a.on_insert(rid, &rec).await.unwrap();

        // plan + apply
        let ops = backend_b.plan_insert(rid, &rec).await.unwrap();
        apply_index_ops(&ops, &store_b, &backend_b).await.unwrap();

        // Both should yield the same lookup result
        let lookup_val = InnerValue::Str("test@example.com".into());
        let hash = FunctionalBackend::hash_value(&lookup_val);
        let query_a = IndexQuery::Point {
            keys: smallvec::smallvec![hash.to_vec()],
        };
        let query_b = IndexQuery::Point {
            keys: smallvec::smallvec![hash.to_vec()],
        };

        let res_a = backend_a.lookup(query_a).await.unwrap();
        let res_b = backend_b.lookup(query_b).await.unwrap();
        assert_eq!(format!("{:?}", res_a), format!("{:?}", res_b));
    }

    #[tokio::test]
    async fn delete_removes_posting() {
        let interner = Interner::new();
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let backend = make_backend(&interner, store);

        let rid = RecordId::new();
        let rec = make_rec(&interner, "del@me.com", 40);
        backend.on_insert(rid, &rec).await.unwrap();
        backend.on_delete(rid, &rec).await.unwrap();

        let hash = FunctionalBackend::hash_value(&InnerValue::Str("del@me.com".into()));
        let r = backend
            .lookup(IndexQuery::Point {
                keys: smallvec::smallvec![hash.to_vec()],
            })
            .await
            .unwrap();
        match r {
            IndexResult::Set(s) => assert!(s.is_empty()),
            _ => panic!("expected Set"),
        }
    }
}
