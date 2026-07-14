//! `IndexBackend` for functional (computed) indexes.
//!
//! On insert / update / delete, evaluates `IndexExpr` against the
//! record, hashes the result, and stores/removes a type-tagged
//! posting in the backing `Store`.

use crate::backend::{IndexBackend, IndexError, IndexQuery, IndexResult};
use crate::descriptor::IndexDescriptor;
use crate::expr::{ExprError, IndexExpr};
use crate::posting_layout::{build_posting_key, type_tag, PostingKeyRef};
use crate::write_ops::IndexWriteOp;
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use rustc_hash::FxHasher;
use shamir_storage::types::RecordKey;
use shamir_storage::types::Store;
use shamir_types::record_view::RecordRef;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::collections::BTreeSet;
use std::hash::Hasher;
use std::sync::Arc;

pub struct FunctionalBackend {
    descriptor: IndexDescriptor,
    expr: IndexExpr,
    store: Arc<dyn Store>,
    /// Optional scalar resolver for `IndexExpr::Scalar` variants.
    /// `None` for indexes that only use built-in `IndexExpr` variants
    /// (Lower, Upper, Trim, Length). Set when the index expression
    /// contains a user-registered scalar function.
    ///
    /// Uses `ArcSwapOption` so the resolver can be updated after
    /// construction (e.g. when a re-registered user scalar becomes
    /// available after DB reopen) without locking on the hot eval path.
    resolver: arc_swap::ArcSwapOption<shamir_funclib::scalar_resolver::ScalarResolver>,
}

impl FunctionalBackend {
    /// Build a FunctionalBackend with no scalar resolver (for indexes
    /// that only use built-in IndexExpr variants).
    pub fn new(descriptor: IndexDescriptor, expr: IndexExpr, store: Arc<dyn Store>) -> Self {
        Self {
            descriptor,
            expr,
            store,
            resolver: arc_swap::ArcSwapOption::empty(),
        }
    }

    /// Build a FunctionalBackend with a scalar resolver (for indexes
    /// backed by user-registered `.trusted_pure()` scalars).
    pub fn with_resolver(
        descriptor: IndexDescriptor,
        expr: IndexExpr,
        store: Arc<dyn Store>,
        resolver: shamir_funclib::scalar_resolver::ScalarResolver,
    ) -> Self {
        Self {
            descriptor,
            expr,
            store,
            resolver: arc_swap::ArcSwapOption::new(Some(std::sync::Arc::new(resolver))),
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

    /// Evaluate the index expression. `FieldNotFound`, `TypeMismatch`,
    /// and `DivisionByZero` collapse to `InnerValue::Null` (a missing or
    /// wrong-type field yields a stable Null posting key — that is correct
    /// behaviour). `ScalarError` PROPAGATES — an unresolved user scalar
    /// (e.g. not re-registered after reopen) must NOT be silently turned
    /// into a Null index key, because every record would hash to the same
    /// key, silently corrupting the index.
    fn eval_or_null(&self, rec: &dyn RecordRef) -> Result<InnerValue, ExprError> {
        let resolver_guard = self.resolver.load();
        let resolver: Option<&_> = resolver_guard.as_deref();
        match self.expr.eval_with_scalars(rec, resolver) {
            Ok(v) => Ok(v),
            Err(ExprError::FieldNotFound) => Ok(InnerValue::Null),
            Err(ExprError::TypeMismatch { .. }) => Ok(InnerValue::Null),
            Err(ExprError::DivisionByZero) => Ok(InnerValue::Null),
            Err(scalar_err) => Err(scalar_err),
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
    ) -> Result<Vec<(RecordKey, Bytes)>, IndexError> {
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

/// Hash a computed expression value into the FUNCTIONAL posting key using
/// EXPLICIT u8 type tags (0..=6, 255), NOT `std::mem::discriminant`.
///
/// §5b/byte-identity floor: this is the persisted posting hash format —
/// the tag scheme is part of on-disk index compatibility and must stay
/// stable. The `InnerValue` here is the owned computed expression result
/// (see `IndexExpr::eval`), not a record materialization.
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
                let (buf, len) = k.as_bytes_buf();
                h.write(&buf[..len]);
                hash_inner(v, h);
            }
        }
        _ => h.write_u8(255),
    }
}

#[async_trait]
impl IndexBackend for FunctionalBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn descriptor(&self) -> &IndexDescriptor {
        &self.descriptor
    }

    async fn plan_insert(
        &self,
        rid: RecordId,
        rec: &(dyn RecordRef + Sync + '_),
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        let val = self.eval_or_null(rec).map_err(|e| {
            IndexError::Backend(format!(
                "functional index '{}': scalar eval failed: {}",
                self.descriptor.name, e
            ))
        })?;
        let key = self.posting_key(&val, &rid);
        Ok(vec![IndexWriteOp::SetPosting {
            key: Bytes::from(key),
            value: Bytes::new(),
        }])
    }

    async fn plan_update(
        &self,
        rid: RecordId,
        old: &(dyn RecordRef + Sync + '_),
        new: &(dyn RecordRef + Sync + '_),
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        let eval_err = |e: ExprError| {
            IndexError::Backend(format!(
                "functional index '{}': scalar eval failed: {}",
                self.descriptor.name, e
            ))
        };
        let old_val = self.eval_or_null(old).map_err(&eval_err)?;
        let new_val = self.eval_or_null(new).map_err(&eval_err)?;
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
        rec: &(dyn RecordRef + Sync + '_),
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        let val = self.eval_or_null(rec).map_err(|e| {
            IndexError::Backend(format!(
                "functional index '{}': scalar eval failed: {}",
                self.descriptor.name, e
            ))
        })?;
        let key = self.posting_key(&val, &rid);
        Ok(vec![IndexWriteOp::RemovePosting {
            key: Bytes::from(key),
        }])
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

    fn update_scalar_resolver(&self, resolver: &shamir_funclib::scalar_resolver::ScalarResolver) {
        // Clone the ScalarResolver into an Arc and store it — the old
        // resolver (if any) is replaced atomically. Subsequent eval_or_null
        // calls will pick up the new resolver via the ArcSwapOption load.
        self.resolver
            .store(Some(std::sync::Arc::new(resolver.clone())));
    }
}
