use std::borrow::Cow;
use std::cell::RefCell;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use rustc_hash::FxHashMap;

use crate::function::builtin_scalars;
use crate::query::write::{InsertOp, InsertedRecord, WriteResult};
use shamir_storage::error::DbResult;
use shamir_types::codecs::interned::{
    query_value_to_storage_bytes_into, record_view_to_query_value, validate_keys_resolve_interner,
};
use shamir_types::core::interner::InternerKey;
use shamir_types::record_view::RecordView;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use crate::validator::WriteOp;
use shamir_types::access::Actor;

use crate::table::table_manager::TableManager;
use crate::table::write::set::execute::{build_insert_result_records, project_query_value};
use crate::table::write::write_helpers::{
    apply_defaults, apply_transforms, make_layered_interner, resolve_computed_record,
    validator_failure_to_db_error,
};

impl TableManager {
    /// FG-2/FG-7: optimistic-concurrency (CAS) version check for
    /// `UpdateOp`/`DeleteOp::expected_version`.
    ///
    /// Three-part check (brief §"Validation timing"):
    /// 1. **Immediate check**: for each matched row, call
    ///    `MvccStore::version_of(key)` and compare against `expected`. A
    ///    mismatch on ANY row aborts the WHOLE operation with
    ///    `DbError::VersionConflict` — no partial application.
    /// 2. **SSI registration**: call `record_read_shared(table_token, key,
    ///    expected)` for every matched row, feeding the EXISTING SSI
    ///    `validate_read_set` commit-time re-check to close the race window
    ///    between step 1 and this tx's actual commit. Serializable-only —
    ///    a documented no-op under Snapshot.
    /// 3. **FG-7 CAS registration**: call `record_cas(table_token, key,
    ///    expected)` for every matched row, UNCONDITIONAL of isolation
    ///    level. Validated at commit by the new Phase CAS
    ///    (`pre_commit_locked_validate` / `pre_commit_locked`) regardless of
    ///    Snapshot/Serializable — this is what makes "exactly one wins"
    ///    hold on every entry path (implicit batch, explicit Snapshot,
    ///    explicit Serializable), not just under an explicit
    ///    `.isolation(Serializable)` opt-in.
    ///
    /// When the table has no MvccStore, the immediate check is skipped
    /// (there is no version authority) but steps 2/3 still run — a
    /// concurrent committer on a different path could still trigger an
    /// abort. Non-MVCC tables simply don't support meaningful CAS; the read
    /// side returns `versions: None` for them, so a client never gets a
    /// version to pass in.
    pub(in crate::table::write) fn check_expected_version(
        &self,
        matched_ids: &[RecordId],
        expected: u64,
        tx: &shamir_tx::TxContext,
    ) -> DbResult<()> {
        let table_token = self.table_token();
        if let Some(mvcc) = self.mvcc_store_ref() {
            for id in matched_ids {
                let actual = mvcc.version_of(id.as_bytes());
                if actual != expected {
                    return Err(shamir_storage::error::DbError::VersionConflict(format!(
                        "table '{}': key {:?} expected version {} but found {}",
                        self.name(),
                        id.as_bytes(),
                        expected,
                        actual
                    )));
                }
            }
        }
        // Register each matched key as a tx read at `expected` so the
        // existing SSI validate_read_set re-check at commit catches a
        // concurrent write between this check and commit (Serializable
        // only — no-op otherwise).
        //
        // FG-7: ALSO record an independent CAS check, unconditional of
        // isolation level — validated at commit by `pre_commit_locked*`'s
        // new Phase CAS regardless of Snapshot/Serializable, closing the
        // race window on every entry path (implicit batch, explicit
        // Snapshot, explicit Serializable).
        for id in matched_ids {
            tx.record_read_shared(table_token, id.to_bytes(), expected);
            tx.record_cas(table_token, id.to_bytes(), expected);
        }
        Ok(())
    }

    /// tx-aware variant of INSERT.
    ///
    /// Stages each insert through [`insert_tx`](Self::insert_tx) — no
    /// physical writes until `commit_tx` Phase 5. Returns the same
    /// `WriteResult` shape (records with `_id`, affected count).
    /// Interner / counter persistence is **skipped** — commit_tx
    /// handles that uniformly for all staged mutations.
    pub async fn execute_insert_tx(
        &self,
        op: &InsertOp,
        tx: &mut shamir_tx::TxContext,
        return_result: bool,
        resolver: Option<&dyn crate::query::TableResolver>,
        actor: &Actor,
    ) -> DbResult<WriteResult> {
        let start = Instant::now();
        let interner = self.interner().get().await?;

        // Resolve inline `$fn` computed fields first (fail-closed), then
        // intern field names.
        //
        // C5: on the IMPLICIT / auto-commit path (`run_implicit_batch_tx` →
        // `tx.implicit == true`) we intern field names **directly into base**
        // instead of into the tx overlay. The implicit tx always commits
        // (Snapshot, last-writer-wins, never rolls back), so there is no
        // rollback-isolation to preserve — and base-interning makes the
        // commit-time `commit_interner_overlay` remap EMPTY, which
        // short-circuits the deep `rewrite_set_bytes` walk over every staged
        // record (the redundant second full pass this cycle removes). The
        // newly-interned `(name, id)` pairs are still collected here and
        // threaded into `tx.interner_deltas` below so they reach the
        // `WalEntryV2.interner_delta` — recovery replays them via
        // `touch_with_id` exactly as for the overlay path (inv. #2). This
        // mirrors the original non-tx `execute_insert`, which also interns
        // straight into base via `touch_ind`.
        //
        // On the INTERACTIVE path (`tx.implicit == false`) we keep the tx
        // overlay: multi-statement txs must stay rollback-isolated, so new
        // field names live in the overlay until commit (inv. #1).
        //
        // Per-batch intern cache (C1): field names repeat across every row in
        // the batch (e.g. all 100 rows have "email", "city", "score"). Without
        // the cache, the per-field intern does a DashMap lookup per field per
        // row — O(N×k) sharded-map operations. With the cache we pay one
        // DashMap lookup per unique field name per batch and amortise the rest
        // to an FxHashMap lookup — 3-5× cheaper for typical short batches with
        // uniform schema.
        let intern_to_base = tx.implicit;
        let mut resolved_values: Vec<Cow<'_, QueryValue>> = Vec::with_capacity(op.values.len());
        // Phase ②.4c — literal-default rules for this table, fetched ONCE per
        // batch. Empty for the common case (no `default` declared) → the
        // `apply_defaults` call below is skipped entirely (fast-skip; the
        // hot path pays nothing). When non-empty, each ABSENT field is
        // stamped with its default AFTER `resolve_computed_record` and BEFORE
        // encode — so both the stored bytes and the validator see the
        // default value. Explicit values (including explicit `Null`) are
        // never overwritten (replay-safe invariant, DDL-EVOLUTION-PLAN §②.4a).
        let defaults = self.schema_defaults();
        // ③.2b — declarative transform rules for this table, fetched ONCE per
        // batch.  Empty until #281/#282 land the schema-surface fields
        // (`auto_now` / `auto_now_add` / expression-default) → wiring is
        // inert for now, which is the expected and correct state.
        let transforms = self.schema_transforms();
        // now_ns: admission-time wall-clock nanoseconds, computed ONCE per
        // batch so every record in the batch shares the same timestamp
        // (deterministic within a batch, not per-record).  Off-replay by
        // construction — transforms run at admission, not WAL-replay.
        let now_ns: u64 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        // Collected on the base-intern path only: `(field_name, base_id)` for
        // every name genuinely NEW to the base interner this insert. Drained
        // into `tx.interner_deltas` after the immutable-borrow block ends.
        let new_base_keys: RefCell<Vec<(String, u64)>> = RefCell::new(Vec::new());
        // #1205: every `InternerKey` id `intern_fn` returns, captured at the
        // SAME per-unique-key granularity as `cache` (a key repeated across
        // the batch is captured once, on its first/cache-miss occurrence —
        // the target is a SET of referenced ids, so later cache-hit
        // occurrences of the same key would add nothing new). Drained into
        // `tx.referenced_interner_ids` after the block ends, so
        // `pre_commit_prelock`'s A8 scan doesn't have to re-decode `staged`
        // to rediscover the same ids.
        let referenced_ids: RefCell<Vec<u64>> = RefCell::new(Vec::with_capacity(op.values.len()));
        // W2d-cutover: build storage Bytes directly via the byte-identical
        // encoder `query_value_to_storage_bytes` (no InnerValue tree).
        let staged: Vec<bytes::Bytes> = {
            let layered = make_layered_interner(interner, tx);
            let layered_failure = RefCell::new(None);
            let base_intern_fn = |key: &str| {
                layered
                    .try_touch_sync(key)
                    .map(shamir_types::core::interner::InternerKey::new)
                    .map_err(|error| {
                        *layered_failure.borrow_mut() = Some(error);
                        shamir_types::codecs::CodecError::Encode(
                            "layered interner failure".to_string(),
                        )
                    })
            };
            let direct_failure = RefCell::new(None);
            // RefCell lets the closure capture and mutate the cache while
            // satisfying the `Fn` (not `FnMut`) bound on
            // `query_value_to_storage_bytes`.
            let cache: RefCell<FxHashMap<String, InternerKey>> = RefCell::new(FxHashMap::default());
            let intern_fn = |key: &str| -> Result<InternerKey, shamir_types::codecs::CodecError> {
                {
                    let c = cache.borrow();
                    if let Some(ik) = c.get(key) {
                        return Ok(ik.clone());
                    }
                }
                let ik = if intern_to_base {
                    // Intern straight into base. `touch_ind` returns whether the
                    // key was newly created so we can record the delta for WAL /
                    // recovery — the overlay path's `commit_interner_overlay`
                    // builds the identical `(name, id)` delta from `is_new`.
                    // RU8: map through CodecError::Interner so the typed
                    // quota error (wire code interner_quota_exceeded)
                    // survives into DbError; Err here precedes any staging,
                    // so no WAL record or visible row is produced.
                    let ti = interner.touch_ind(key).map_err(|error| {
                        *direct_failure.borrow_mut() = Some(error);
                        shamir_types::codecs::CodecError::Encode(
                            "base interner failure".to_string(),
                        )
                    })?;
                    if ti.is_new() {
                        new_base_keys
                            .borrow_mut()
                            .push((key.to_string(), ti.key().id()));
                    }
                    ti.into_key()
                } else {
                    base_intern_fn(key)?
                };
                referenced_ids.borrow_mut().push(ik.id());
                cache.borrow_mut().insert(key.to_string(), ik.clone());
                Ok(ik)
            };
            let mut out = Vec::with_capacity(op.values.len());
            let mut scratch = Vec::new();
            for value in &op.values {
                let mut resolved = resolve_computed_record(value, interner)
                    .map_err(shamir_storage::error::DbError::Codec)?;
                // Phase ②.4c — stamp literal defaults onto absent fields
                // BEFORE encode so both stored bytes and validators see them.
                // Fast-skip when the table declares no defaults (common case).
                if !defaults.is_empty() {
                    apply_defaults(resolved.to_mut(), &defaults);
                }
                // ③.2b/③.2d — apply declarative transforms (computed-default +
                // server-side timestamps) AFTER literal defaults and BEFORE
                // encode so both stored bytes and CHECK-validators see the
                // transformed values.  Fast-skip when empty (hot path).
                // is_insert=true: AutoNowAdd/ComputedDefault run here;
                // AutoNow also runs (it always runs).
                if !transforms.is_empty() {
                    apply_transforms(
                        resolved.to_mut(),
                        &transforms,
                        builtin_scalars(),
                        now_ns,
                        true,
                    );
                }
                let bytes = query_value_to_storage_bytes_into(&resolved, &intern_fn, &mut scratch)
                    .map_err(|error| {
                        if let Some(typed) = direct_failure.borrow_mut().take() {
                            shamir_storage::error::DbError::from(
                                shamir_types::codecs::CodecError::Interner(typed),
                            )
                        } else if let Some(typed) = layered_failure.borrow_mut().take() {
                            shamir_storage::error::DbError::from(
                                shamir_types::codecs::CodecError::Interner(typed),
                            )
                        } else {
                            shamir_storage::error::DbError::from(error)
                        }
                    })?;
                out.push(bytes);
                resolved_values.push(resolved);
            }
            out
        };

        // C5: thread the base-interned delta into the tx so pre-commit emits it
        // in the `WalEntryV2.interner_delta`. The overlay stays empty on this
        // path, so `pre_commit` Phase 1 skips the overlay merge entirely and
        // the staging-bytes remap is a no-op (no overlay ids to rewrite).
        //
        // Stage I: the interner is per-REPO (one id-namespace across tables),
        // so the delta is a single flat `Vec` — no per-table key.
        let new_base_keys = new_base_keys.into_inner();
        if !new_base_keys.is_empty() {
            tx.interner_deltas.extend(new_base_keys);
        }
        // #1205: fold this batch's captured ids into the tx-wide set now —
        // `insert_tx_many_bytes` below marks the table's `StagingStore` as
        // captured, which only amortizes A8's scan if the ids are already
        // sitting in `tx.referenced_interner_ids` by the time it runs.
        for id in referenced_ids.into_inner() {
            tx.referenced_interner_ids.insert(id, ());
        }

        // S3: run validators on each record before staging.
        // tx path: resolve keys through the tx overlay so brand-new field
        // names (staged above into the layered interner, not yet in base)
        // resolve at validation time. The caller's `actor` is threaded so a
        // validator's cross-table reads run with the SAME privileges the
        // caller already used to reach this write (RI-7).
        //
        // W1: feed the resolved `QueryValue` directly (the resolved record
        // already carries the tx-overlay keys as plain strings, so no
        // de-intern is needed). Identity vs. the legacy
        // `run_validators_tx` path is proven by
        // `validator::tests::query_value_conv_tests`.
        // L4-RV-ENGINE-3C71057D: one batch-scoped FK membership cache shared
        // by every per-row validator invocation below — N children
        // referencing the same parent row perform ONE committed-state parent
        // probe instead of N. Transaction-staged parents stay visible (the
        // cache never bypasses the tx's staged-overlay probe).
        let fk_cache = crate::validator::BatchFkCache::new();
        for qv in &resolved_values {
            self.run_validators_qv(
                WriteOp::Insert,
                Some(qv),
                None,
                actor,
                Some(tx),
                crate::validator::ValidatorFkCtx {
                    resolver,
                    fk_cache: Some(&fk_cache),
                },
            )
            .await
            .map_err(validator_failure_to_db_error)?;
        }

        // W2d-cutover: lens-driven batched tx insert — the staged bytes
        // feed `insert_tx_many_bytes`, which builds `RecordView`s over the
        // bytes and drives every index/unique/vector planner through the
        // zero-copy lens. No InnerValue tree is built on the insert path.
        let values_ids: Vec<RecordId> = self.insert_tx_many_bytes(&staged, tx).await?;
        // #1205: `staged`'s referenced ids were captured above into
        // `tx.referenced_interner_ids` — tell A8 it can skip decoding this
        // table's staged bytes.
        if !staged.is_empty() {
            self.mark_staged_ids_captured(tx);
        }

        // S-write id-keyed branch: accept pre-encoded id-msgpack records
        // from `op.records_idmsgpack`. For each record:
        //   1. Structural validation — `RecordView::new` (must be a msgpack map).
        //   2. Security spine — `validate_keys_resolve_interner` confirms every
        //      interned key id resolves in the server interner. REJECT the whole
        //      op on first unresolved id — never write bytes with stale/forged keys.
        //   3. Per-write interning is SKIPPED (the client already interned all
        //      keys; `tx.interner_deltas` for this branch is empty).
        //   4. Validators — if any Insert validators are bound, decode each row
        //      via `record_view_to_query_value` and run `run_validators_qv`.
        //      If no validators are bound, skip the decode entirely (lens-only).
        //   5. Feed the validated bytes verbatim to `insert_tx_many_bytes` —
        //      same lens-driven path as the `values` branch above.
        //
        // When BOTH `values` and `records_idmsgpack` are non-empty, the
        // `values` records are inserted first and their IDs appear first in
        // the combined result.
        let idmsgpack_ids: Vec<RecordId> = if op.records_idmsgpack.is_empty() {
            Vec::new()
        } else {
            // Batch-scoped FK membership cache for the id-keyed rows
            // (L4-RV-ENGINE-3C71057D — see the `values` branch above).
            let idmsgpack_fk_cache = crate::validator::BatchFkCache::new();
            // Collect validated bytes — validate-before-collect so we never
            // stage any bytes whose keys don't resolve (security invariant).
            let mut idmsgpack_staged: Vec<Bytes> = Vec::with_capacity(op.records_idmsgpack.len());
            // Check if any Insert validators are bound; if not, skip the
            // per-row decode. One snapshot covers the whole batch.
            let has_validators = {
                let bindings = self.validator_bindings();
                bindings.iter().any(|b| b.ops.contains(&WriteOp::Insert))
            };
            // Phase ②.4c — DEFAULT stamp on the id-keyed branch.
            //
            // MVP choice: (ii) — the id-keyed (client-pre-interned) path
            // assumes a COMPLETE record and does NOT stamp literal defaults
            // here. Rationale: the bytes are pre-interned msgpack from the
            // client; stamping a default would require decode → stamp →
            // re-encode (re-interning brand-new field names through the tx
            // overlay), which breaks the lens-only verbatim fast path that
            // makes id-keyed worth using. The common case (no defaults on
            // the table) is a verbatim copy regardless — `has_defaults` is
            // computed purely for documentation/clarity and does not gate a
            // decode here. If a future use case needs defaults on id-keyed,
            // gate a decode-stamp-reencode behind `has_defaults` (variant (i)).
            let _has_defaults = !defaults.is_empty();
            for buf in &op.records_idmsgpack {
                // 1. Structural validation — reject non-map bytes.
                let view = RecordView::new(buf.as_ref()).map_err(|e| {
                    shamir_storage::error::DbError::Validation(format!(
                        "execute_insert_tx (id-keyed): malformed record bytes: {e}"
                    ))
                })?;
                // 2. Security spine — reject if any key id is unresolved.
                validate_keys_resolve_interner(&view, interner).map_err(|e| {
                    shamir_storage::error::DbError::Validation(format!(
                        "execute_insert_tx (id-keyed): unresolved interner key — {e}"
                    ))
                })?;
                // 2b (#1205): capture referenced ids for A8 off the SAME
                // zero-copy `RecordView` lens step 2 just validated with —
                // no extra decode, just also recording ids this time.
                shamir_types::codecs::interned::collect_referenced_ids_from_view(
                    &view,
                    &mut tx.referenced_interner_ids,
                );
                // 3. Validators — only decode when validators are actually bound.
                if has_validators {
                    let qv = record_view_to_query_value(&view, interner)?;
                    self.run_validators_qv(
                        WriteOp::Insert,
                        Some(&qv),
                        None,
                        actor,
                        Some(tx),
                        crate::validator::ValidatorFkCtx {
                            resolver,
                            fk_cache: Some(&idmsgpack_fk_cache),
                        },
                    )
                    .await
                    .map_err(validator_failure_to_db_error)?;
                }
                idmsgpack_staged.push(Bytes::copy_from_slice(buf.as_ref()));
            }
            let ids = self.insert_tx_many_bytes(&idmsgpack_staged, tx).await?;
            // #1205: ids captured above (step 2b) into
            // `tx.referenced_interner_ids` — safe to mark this table's
            // staging as A8-covered.
            if !idmsgpack_staged.is_empty() {
                self.mark_staged_ids_captured(tx);
            }
            ids
        };

        // Merge IDs: values first, then id-keyed (deterministic ordering).
        let mut all_ids = values_ids;
        all_ids.extend_from_slice(&idmsgpack_ids);

        // Skip result-map assembly for fire-and-forget inserts
        // (return_result=false) — avoids per-row QueryValue map build
        // and clone on the hot batch-insert path.
        let records = if return_result {
            let mut records =
                build_insert_result_records(&resolved_values, &all_ids[..resolved_values.len()]);
            // Build RETURNING rows for id-keyed records: decode each
            // record via `record_view_to_query_value` to get name-keyed
            // `QueryValue`, then wrap in the same `InsertedRecord::Direct`
            // the values branch produces.  Order: values-first, then
            // id-keyed — matching `all_ids`.
            if !op.records_idmsgpack.is_empty() {
                let id_offset = resolved_values.len();
                for (i, buf) in op.records_idmsgpack.iter().enumerate() {
                    let view = RecordView::new(buf.as_ref()).map_err(|e| {
                        shamir_storage::error::DbError::Validation(format!(
                            "execute_insert_tx (id-keyed return_result): malformed record bytes: {e}"
                        ))
                    })?;
                    let qv = record_view_to_query_value(&view, interner)?;
                    records.push(InsertedRecord {
                        id: Some(all_ids[id_offset + i]),
                        fields: qv,
                    });
                }
            }
            // Optional RETURNING projection (InsertSelect.fields). When set,
            // restrict every returned row to the named fields. This is a
            // post-build pass — the rows above were already materialised by
            // `build_insert_result_records` / the id-keyed branch, so the
            // projection just walks each `fields: QueryValue::Map` and drops
            // unrequested keys. Cheap (one map rebuild per row) and only
            // runs when the caller actually asks for it.
            if let Some(fields) = op.select.as_ref().and_then(|s| s.fields.as_deref()) {
                for rec in records.iter_mut() {
                    rec.fields = project_query_value(rec.fields.clone(), Some(fields));
                }
            }
            records
        } else {
            Vec::new()
        };

        let affected = all_ids.len() as u64;
        Ok(WriteResult {
            affected,
            records,
            execution_time_us: start.elapsed().as_micros() as u64,
        })
    }
}
