//! `IndexBackend` for functional (computed) indexes.
//!
//! On insert / update / delete, evaluates `IndexExpr` and stores canonical
//! IDV2 identities in the functional posting namespace.
//!
//! # Persisted format
//!
//! A posting key is `[index_id u32 LE][0x12][I][record_id 16]`
//! (`type_tag::FUNCTIONAL_V2`). `I` is the canonical encoding `C` of the
//! evaluated value (the frozen codec in `crate::model::identity`) when
//! `|C| <= 256`, otherwise `0xFF || BLAKE3-256(C)`; the posting VALUE is
//! empty for the inline form and `C` itself for the digest form, so a lookup
//! compares canonical bytes and never trusts a digest alone. Equal values of
//! every numeric class share one identity, and a Map is encoded sorted by
//! key id, so the identity does not depend on insertion order. A NaN result
//! has no identity and writes no posting.
//!
//! History: v1 keys used `type_tag::FUNCTIONAL` (`0x02`) with a 64-bit
//! FxHash of the value, which was collision-prone and insertion-order
//! dependent for maps. The open-time migration sweeps those postings and
//! rebuilds under identity v2 (envelope version 2, see
//! `crate::persistence`). An index over a user-registered scalar that cannot
//! be resolved at open stays `Failed` with a persisted reason until the
//! scalar is registered and `TableManager::repair()` runs.

use crate::backend::{IndexBackend, IndexError, IndexQuery, IndexResult};
use crate::descriptor::IndexDescriptor;
use crate::expr::{ExprError, IndexExpr};
use crate::posting_layout::{build_posting_key, type_tag, PostingKeyRef};
use crate::write_ops::IndexWriteOp;
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::types::{RecordKey, Store};
use shamir_types::record_view::RecordRef;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::collections::BTreeSet;
use std::sync::Arc;

/// `0xFE` is neither a canonical leaf tag nor the digest marker (`0xFF`), so no
/// stored identity starts with it.
const NO_IDENTITY_PROBE: [u8; 1] = [0xFE];

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
            resolver: arc_swap::ArcSwapOption::new(Some(Arc::new(resolver))),
        }
    }

    /// Probe key of `val`: its canonical encoding. A value with no identity
    /// (a top-level NaN) gets [`NO_IDENTITY_PROBE`], which no posting can
    /// carry, so the lookup matches nothing.
    pub fn hash_value(val: &InnerValue) -> Vec<u8> {
        crate::model::identity::canonical::encode_inner_value(val)
            .unwrap_or_else(|_| NO_IDENTITY_PROBE.to_vec())
    }

    fn identity(val: &InnerValue) -> Result<(Vec<u8>, Vec<u8>), IndexError> {
        let canonical = crate::model::identity::canonical::encode_inner_value(val)
            .map_err(|e| IndexError::Backend(e.to_string()))?;
        let identity = crate::model::identity::identity_bytes::identity_bytes(&canonical);
        let tail = if canonical.len() > crate::model::identity::identity_bytes::INLINE_MAX {
            canonical
        } else {
            Vec::new()
        };
        Ok((identity, tail))
    }

    fn posting_key(
        &self,
        val: &InnerValue,
        rid: &RecordId,
    ) -> Result<(Vec<u8>, Bytes), IndexError> {
        let (identity, tail) = Self::identity(val)?;
        Ok((
            build_posting_key(self.descriptor.id, type_tag::FUNCTIONAL_V2, &identity, rid),
            Bytes::from(tail),
        ))
    }

    fn prefix_for_value(&self, identity: &[u8]) -> Vec<u8> {
        let mut prefix = Vec::with_capacity(5 + identity.len());
        prefix.extend_from_slice(&self.descriptor.id.to_le_bytes());
        prefix.push(type_tag::FUNCTIONAL_V2);
        prefix.extend_from_slice(identity);
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
        while let Some(batch) = stream.next().await {
            all.extend(batch.map_err(IndexError::from)?);
        }
        Ok(all)
    }

    /// A missing field, type mismatch, or division by zero means the expression
    /// does not produce an indexable value, so these cases are represented as
    /// Null. Other errors (including unavailable user scalars) must propagate:
    /// silently turning them into Null would corrupt the index.
    fn eval_or_null(&self, rec: &dyn RecordRef) -> Result<InnerValue, ExprError> {
        let resolver_guard = self.resolver.load();
        match self.expr.eval_with_scalars(rec, resolver_guard.as_deref()) {
            Ok(v) => Ok(v),
            Err(
                ExprError::FieldNotFound
                | ExprError::TypeMismatch { .. }
                | ExprError::DivisionByZero,
            ) => Ok(InnerValue::Null),
            Err(error) => Err(error),
        }
    }
}

/// Top-level NaN leaf (RV-FUNCLIB-5FA455F7): NaN claims no posting,
/// same as the scan filter, which never matches NaN.
fn inner_is_top_level_nan(value: &InnerValue) -> bool {
    matches!(value, InnerValue::F64(f) if f.is_nan())
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
        let value = self.eval_or_null(rec).map_err(|e| {
            IndexError::Backend(format!(
                "functional index '{}': scalar eval failed: {e}",
                self.descriptor.name
            ))
        })?;
        if inner_is_top_level_nan(&value) {
            return Ok(vec![]);
        }
        let (key, tail) = self.posting_key(&value, &rid)?;
        Ok(vec![IndexWriteOp::SetPosting {
            key: Bytes::from(key),
            value: tail,
            provenance: crate::write_ops::index2_provenance(&self.descriptor),
        }])
    }

    async fn plan_update(
        &self,
        rid: RecordId,
        old: &(dyn RecordRef + Sync + '_),
        new: &(dyn RecordRef + Sync + '_),
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        let eval = |rec: &(dyn RecordRef + Sync + '_)| {
            self.eval_or_null(rec).map_err(|e| {
                IndexError::Backend(format!(
                    "functional index '{}': scalar eval failed: {e}",
                    self.descriptor.name
                ))
            })
        };
        let old_value = eval(old)?;
        let new_value = eval(new)?;
        let provenance = crate::write_ops::index2_provenance(&self.descriptor);
        // NaN claims no posting: remove the old one, never write a NaN
        // one (RV-FUNCLIB-5FA455F7).
        match (
            inner_is_top_level_nan(&old_value),
            inner_is_top_level_nan(&new_value),
        ) {
            (true, true) => return Ok(vec![]),
            (true, false) => {
                let (key, value) = self.posting_key(&new_value, &rid)?;
                return Ok(vec![IndexWriteOp::SetPosting {
                    key: Bytes::from(key),
                    value,
                    provenance,
                }]);
            }
            (false, true) => {
                let (key, _) = self.posting_key(&old_value, &rid)?;
                return Ok(vec![IndexWriteOp::RemovePosting {
                    key: Bytes::from(key),
                    provenance,
                    owner: None,
                }]);
            }
            _ => {}
        }
        let (old_identity, old_tail) = Self::identity(&old_value)?;
        let (new_identity, new_tail) = Self::identity(&new_value)?;
        if old_identity != new_identity || old_tail != new_tail {
            let (old_key, _) = self.posting_key(&old_value, &rid)?;
            let (new_key, new_value) = self.posting_key(&new_value, &rid)?;
            Ok(vec![
                IndexWriteOp::RemovePosting {
                    key: Bytes::from(old_key),
                    provenance,
                    owner: None,
                },
                IndexWriteOp::SetPosting {
                    key: Bytes::from(new_key),
                    value: new_value,
                    provenance,
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
        let value = self.eval_or_null(rec).map_err(|e| {
            IndexError::Backend(format!(
                "functional index '{}': scalar eval failed: {e}",
                self.descriptor.name
            ))
        })?;
        if inner_is_top_level_nan(&value) {
            return Ok(vec![]);
        }
        let (key, _) = self.posting_key(&value, &rid)?;
        Ok(vec![IndexWriteOp::RemovePosting {
            key: Bytes::from(key),
            provenance: crate::write_ops::index2_provenance(&self.descriptor),
            owner: None,
        }])
    }

    async fn lookup(&self, query: IndexQuery) -> Result<IndexResult, IndexError> {
        match query {
            IndexQuery::Point { keys } => {
                let mut result = BTreeSet::new();
                for canonical in keys {
                    let identity =
                        crate::model::identity::identity_bytes::identity_bytes(&canonical);
                    for (key_bytes, value_bytes) in self
                        .scan_postings_by_prefix(&self.prefix_for_value(&identity))
                        .await?
                    {
                        if let Some(pk) = PostingKeyRef::decode(&key_bytes) {
                            if pk.index_id == self.descriptor.id
                                && pk.type_tag == type_tag::FUNCTIONAL_V2
                                && pk.value_bytes == identity.as_slice()
                                && if canonical.len()
                                    > crate::model::identity::identity_bytes::INLINE_MAX
                                {
                                    value_bytes.as_ref() == canonical.as_slice()
                                } else {
                                    value_bytes.is_empty()
                                }
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
        let entries = self
            .scan_postings_by_prefix(&self.descriptor.id.to_le_bytes())
            .await?;
        for (key, _) in entries {
            self.store.remove(key).await.map_err(IndexError::from)?;
        }
        Ok(())
    }

    fn update_scalar_resolver(&self, resolver: &shamir_funclib::scalar_resolver::ScalarResolver) {
        // Clone the ScalarResolver into an Arc and store it — the old
        // resolver (if any) is replaced atomically. Subsequent eval_or_null
        // calls will pick up the new resolver via the ArcSwapOption load.
        self.resolver.store(Some(Arc::new(resolver.clone())));
    }
}
