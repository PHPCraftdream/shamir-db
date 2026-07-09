//! Cache operations on [`Client`] — the async roundtrip glue between the
//! in-memory [`FieldMap`](crate::interner_cache::FieldMap) cache and the
//! server's `interner_dump` / `interner_touch` admin ops.
//!
//! These methods build their requests through the typed builder
//! (`shamir_query_builder::ddl::interner_*`), issue them via the existing
//! [`Client::execute`](crate::Client::execute) path, and merge the server's
//! `(name, id)` answer into the per-`(db, repo)` cache. Parsing the admin
//! payload out of `QueryResult.records` uses the QueryValue-native accessors
//! (`as_value()` / `QueryValue::get` / `as_u64` / `as_str` / `as_array`);
//! no deserialisation is involved on the read path — only QueryValue accessors.
//!
//! §9.4 discipline (single source of truth): ids enter the cache ONLY via
//! [`FieldMap::insert_entry`](crate::interner_cache::FieldMap::insert_entry),
//! which is called exclusively from the server-response parsers below. The
//! resolve fn [`Client::resolve_field`] never parses a numeric-looking name
//! into an id — `"42"` is the STRING "42".

use std::borrow::Cow;
use std::sync::Arc;

use serde_bytes::ByteBuf;
use shamir_collections::TFxMap;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_types::batch::{BatchOp, BatchRequest, BatchResponse, ResultEncoding};
use shamir_query_types::filter::{Filter, FilterValue};
use shamir_query_types::read::{QueryRecord, QueryResult};
use shamir_query_types::write::{DeleteOp, InsertOp, SetOp, UpdateOp};
use shamir_types::codecs::interned::{query_value_to_storage_bytes, record_view_deintern_with};
use shamir_types::core::interner::InternerKey;
use shamir_types::record_view::RecordView;
use shamir_types::types::value::{QueryValue, Value};

use crate::error::ClientError;
use crate::Client;

/// Parsed `interner_dump` admin payload.
///
/// Wire shape (server handler `admin_interner.rs`):
/// ```text
/// { "interner_dump": "<repo>", "epoch": <u64>, "entries": [[id, name], ...] }
/// ```
/// (`entries` is id-first.)
#[derive(Debug)]
struct DumpPayload {
    epoch: u64,
    entries: Vec<(u64, String)>,
}

/// Parsed `interner_touch` admin payload.
///
/// Wire shape (server handler `admin_interner.rs`):
/// ```text
/// { "interner_touch": "<repo>", "epoch": <u64>, "mappings": [[name, id], ...] }
/// ```
/// (`mappings` is name-first — note the asymmetry with `entries`.)
#[derive(Debug)]
struct TouchPayload {
    epoch: u64,
    mappings: Vec<(String, u64)>,
}

/// Extract the first record of a `QueryResult` as a `QueryValue`, or return
/// a protocol error if the admin payload is absent.
///
/// Returns `Cow::Borrowed` for the `Direct` variant (the common post-Stage-A
/// path for admin ops); `Cow::Owned` for the `Encoded` / `Inserted` variants.
fn first_record_value<'a>(
    resp: &'a BatchResponse,
    alias: &str,
) -> Result<Cow<'a, QueryValue>, ClientError> {
    let result = resp.results.get(alias).ok_or_else(|| {
        ClientError::Protocol(format!(
            "interner admin op: missing result for alias '{alias}'"
        ))
    })?;
    result.records.first().map(|r| r.as_value()).ok_or_else(|| {
        ClientError::Protocol(format!(
            "interner admin op: empty records for alias '{alias}'"
        ))
    })
}

fn parse_dump_payload(v: &QueryValue, alias: &str) -> Result<DumpPayload, ClientError> {
    let epoch = v
        .get("epoch")
        .and_then(QueryValue::as_u64)
        .ok_or_else(|| ClientError::Protocol(format!("interner_dump: missing epoch ({alias})")))?;
    let entries_v = v.get("entries").ok_or_else(|| {
        ClientError::Protocol(format!("interner_dump: missing entries ({alias})"))
    })?;
    let entries = parse_id_first_pairs(entries_v, "interner_dump.entries", alias)?;
    Ok(DumpPayload { epoch, entries })
}

fn parse_touch_payload(v: &QueryValue, alias: &str) -> Result<TouchPayload, ClientError> {
    let epoch = v
        .get("epoch")
        .and_then(QueryValue::as_u64)
        .ok_or_else(|| ClientError::Protocol(format!("interner_touch: missing epoch ({alias})")))?;
    let mappings_v = v.get("mappings").ok_or_else(|| {
        ClientError::Protocol(format!("interner_touch: missing mappings ({alias})"))
    })?;
    let mappings = parse_name_first_pairs(mappings_v, "interner_touch.mappings", alias)?;
    Ok(TouchPayload { epoch, mappings })
}

/// Parse `[[id, name], ...]` pairs (dump's `entries` — id-first).
fn parse_id_first_pairs(
    v: &QueryValue,
    ctx: &str,
    alias: &str,
) -> Result<Vec<(u64, String)>, ClientError> {
    let arr = v
        .as_array()
        .ok_or_else(|| ClientError::Protocol(format!("{ctx} not an array ({alias})")))?;
    let mut out = Vec::with_capacity(arr.len());
    for pair in arr {
        let p = pair.as_array().ok_or_else(|| {
            ClientError::Protocol(format!("{ctx} element not an array ({alias})"))
        })?;
        if p.len() != 2 {
            return Err(ClientError::Protocol(format!(
                "{ctx} element not a 2-tuple ({alias})"
            )));
        }
        let id = p[0]
            .as_u64()
            .ok_or_else(|| ClientError::Protocol(format!("{ctx} id not u64 ({alias})")))?;
        let name = p[1]
            .as_str()
            .ok_or_else(|| ClientError::Protocol(format!("{ctx} name not string ({alias})")))?;
        out.push((id, name.to_string()));
    }
    Ok(out)
}

/// Parse `[[name, id], ...]` pairs (touch's `mappings` — name-first).
fn parse_name_first_pairs(
    v: &QueryValue,
    ctx: &str,
    alias: &str,
) -> Result<Vec<(String, u64)>, ClientError> {
    let arr = v
        .as_array()
        .ok_or_else(|| ClientError::Protocol(format!("{ctx} not an array ({alias})")))?;
    let mut out = Vec::with_capacity(arr.len());
    for pair in arr {
        let p = pair.as_array().ok_or_else(|| {
            ClientError::Protocol(format!("{ctx} element not an array ({alias})"))
        })?;
        if p.len() != 2 {
            return Err(ClientError::Protocol(format!(
                "{ctx} element not a 2-tuple ({alias})"
            )));
        }
        let name = p[0]
            .as_str()
            .ok_or_else(|| ClientError::Protocol(format!("{ctx} name not string ({alias})")))?;
        let id = p[1]
            .as_u64()
            .ok_or_else(|| ClientError::Protocol(format!("{ctx} id not u64 ({alias})")))?;
        out.push((name.to_string(), id));
    }
    Ok(out)
}

impl Client {
    /// Reference the interner cache registry (e.g. for builder planning).
    pub fn interner_cache(&self) -> &Arc<crate::interner_cache::InternerCacheRegistry> {
        &self.interner_cache
    }

    /// Pull the FULL current `(name, id)` dictionary for `(db, repo)` into the
    /// cache. Guards the first call with the per-`FieldMap` `OnceCell` so that
    /// concurrent first-callers share one dump roundtrip (stampede guard).
    /// Subsequent calls re-dump unconditionally (use [`refresh_repo`] for the
    /// delta path).
    pub async fn dump_repo(&self, db: &str, repo: &str) -> Result<(), ClientError> {
        let fm = self.interner_cache().get_or_create(db, repo);

        // The OnceCell closure may borrow `self` (Client) and `fm` (Arc) by
        // reference — `get_or_init` holds `&FieldMap` and the closure holds
        // `&Client`; these do not alias. The closure runs at most once per
        // FieldMap; concurrent first-callers await the same future (stampede
        // guard). Per tokio's OnceCell contract: if this future is cancelled
        // or the closure returns early (it cannot — it swallows errors and
        // resolves `()`), the cell stays uninitialized and a later call
        // retries.
        fm.ensure_populated(|| {
            let fm = Arc::clone(&fm);
            let db = db.to_string();
            let repo = repo.to_string();
            async move {
                let alias = "_ic_dump";
                let mut batch = Batch::new();
                batch.op(alias, ddl::interner_dump().repo(&repo));
                // Errors are logged + swallowed so the OnceCell still resolves
                // with `()`; the maps stay empty until a successful retry.
                match self.execute(&db, batch.build()).await {
                    Ok(resp) => match first_record_value(&resp, alias)
                        .and_then(|v| parse_dump_payload(&v, alias))
                    {
                        Ok(payload) => {
                            for (id, name) in &payload.entries {
                                fm.insert_entry(name, *id);
                            }
                            fm.set_epoch(payload.epoch);
                        }
                        Err(e) => {
                            tracing::warn!("interner dump_repo parse failed: {e}");
                        }
                    },
                    Err(e) => {
                        tracing::warn!("interner dump_repo roundtrip failed: {e}");
                    }
                }
            }
        })
        .await;
        Ok(())
    }

    /// Delta refresh: pull only entries with id > the cache's current epoch and
    /// merge them. CAS-maxes the epoch from the response.
    pub async fn refresh_repo(&self, db: &str, repo: &str) -> Result<(), ClientError> {
        let fm = self.interner_cache().get_or_create(db, repo);
        let epoch = fm.epoch();
        let alias = "_ic_refresh";
        let mut batch = Batch::new();
        batch.op(alias, ddl::interner_dump().repo(repo).since(epoch));
        let resp = self.execute(db, batch.build()).await?;
        let payload_value = first_record_value(&resp, alias)?;
        let payload = parse_dump_payload(&payload_value, alias)?;
        for (id, name) in &payload.entries {
            fm.insert_entry(name, *id);
        }
        fm.set_epoch(payload.epoch.max(epoch));
        Ok(())
    }

    /// Register `names` against the server's interner for `(db, repo)` and
    /// merge the returned `(name, id)` mappings into the cache. Returns the
    /// resolved mappings in input order.
    ///
    /// Idempotent: the server returns the existing id for a name already
    /// interned, so re-touching is a cache-confirming no-op.
    pub async fn touch_fields(
        &self,
        db: &str,
        repo: &str,
        names: &[&str],
    ) -> Result<Vec<(String, u64)>, ClientError> {
        let fm = self.interner_cache().get_or_create(db, repo);
        // Filter to unknown names — skip the roundtrip entirely if everything
        // is already cached.
        let unknown: Vec<String> = fm.missing_names(names.iter().copied());
        if unknown.is_empty() {
            // Everything already cached; return the resolved mappings.
            return Ok(names
                .iter()
                .filter_map(|n| fm.id_of(n).map(|id| (n.to_string(), id)))
                .collect());
        }
        let alias = "_ic_touch";
        let mut batch = Batch::new();
        batch.op(
            alias,
            ddl::interner_touch(unknown.iter().cloned()).repo(repo),
        );
        let resp = self.execute(db, batch.build()).await?;
        let payload_value = first_record_value(&resp, alias)?;
        let payload = parse_touch_payload(&payload_value, alias)?;
        for (name, id) in &payload.mappings {
            fm.insert_entry(name, *id);
        }
        fm.set_epoch(payload.epoch);
        // Return the full input set's resolved mappings (touches + already-known).
        Ok(names
            .iter()
            .filter_map(|n| fm.id_of(n).map(|id| (n.to_string(), id)))
            .collect())
    }

    /// Resolve a field name to its interner id from the cache. `None` if the
    /// name is not yet cached (call [`dump_repo`] / [`touch_fields`] first).
    ///
    /// §9.4 guard: `name` is an opaque STRING. `"42"` resolves to the field
    /// whose name is "42"; it is NEVER parsed to the integer 42. Ids come only
    /// from server responses inserted via [`FieldMap::insert_entry`].
    pub fn resolve_field(&self, db: &str, repo: &str, name: &str) -> Option<u64> {
        self.interner_cache().get_or_create(db, repo).id_of(name)
    }

    /// Reverse lookup: interner id → field name, from the cache.
    pub fn field_name(&self, db: &str, repo: &str, id: u64) -> Option<String> {
        self.interner_cache().get_or_create(db, repo).name_of(id)
    }

    /// Merge a response's `interner_delta` into the per-`(db, repo)` cache.
    ///
    /// Ambient sync path (Stage 5-wire Part A): called from [`Client::execute`]
    /// after every batch response. For each `(repo, delta)`: get-or-create the
    /// FieldMap, `insert_entry(name, id)` for each entry (idempotent), then
    /// `set_epoch(delta.epoch)` (CAS-max). §9.4-safe — ids come only from the
    /// server response.
    pub(crate) fn merge_interner_delta(&self, db: &str, response: &BatchResponse) {
        for (repo, delta) in &response.interner_delta {
            let fm = self.interner_cache().get_or_create(db, repo);
            for (id, name) in &delta.entries {
                fm.insert_entry(name, *id);
            }
            fm.set_epoch(delta.epoch);
        }
    }

    /// Explicit pre-touch write entry (Stage 5 mode a / S-client).
    ///
    /// Walks `batch`'s insert / set(upsert) / update records, collects the
    /// field names appearing as map keys (grouped by the repo named on each
    /// op's table-ref), calls [`touch_fields`] on the unknown ones (warming
    /// the cache so the subsequent write finds them already interned on the
    /// server), then:
    ///
    /// * **v2 server** (`server_query_version >= 2`): encodes each fully-literal
    ///   (no `$fn`) INSERT record via [`query_value_to_storage_bytes`] into
    ///   `InsertOp.records_idmsgpack`; the original string-keyed values are
    ///   REMOVED from `values` (so they are not double-sent). Records containing
    ///   a `$fn` marker stay on `values` unchanged. Sets `result_encoding = Id`
    ///   so the server returns id-keyed rows; the client de-interns them
    ///   transparently before returning (public API stays name-keyed). §9.4: key
    ///   interning goes through `FieldMap::id_of(name)` — `"42"` resolves to the
    ///   id the server assigned to the FIELD named "42", never the integer 42.
    ///
    /// * **v1 server** (`server_query_version < 2`) or `$fn` records: the records
    ///   stay on `values` UNCHANGED (today's behaviour). No id-keyed encoding.
    ///
    /// # result_encoding choice
    /// `execute_with_touch` always requests `result_encoding = Id` on v2. This
    /// is the "smart-write path" that already pre-touches all fields, so the
    /// FieldMap is guaranteed warm and de-interning always succeeds without an
    /// extra roundtrip. The lower-level `execute` method does NOT set this —
    /// admin/interner callers that call `execute` directly get name-keyed rows, which
    /// is backward-compatible. This design keeps the smart path fully transparent
    /// to callers (all rows come back as name-keyed `QueryValue`) while not
    /// touching unrelated code.
    pub async fn execute_with_touch(
        &self,
        db: &str,
        mut batch: BatchRequest,
    ) -> Result<BatchResponse, ClientError> {
        // Collect field names per repo (write ops only — INSERT/SET/UPDATE).
        // Each repo gets ONE aggregated touch. The repo is read straight off
        // each op's table-ref. TFxMap = HashMap with the workspace THasher
        // (`HashMap::new` is banned — SipHash).
        let mut per_repo: TFxMap<String, Vec<String>> = TFxMap::default();
        for entry in batch.queries.values() {
            collect_field_names(&entry.op, &mut per_repo);
        }

        // Collect the FULL set of repos referenced by ANY data op (read OR
        // write). This wider set is what deintern_response consults: a
        // read-only batch has no write ops (per_repo empty), but its SELECT
        // results may come back as IdBytes rows that need a FieldMap to
        // de-intern. Using table_ref() covers Read, Insert, Update, Set, Delete.
        let mut all_repos: TFxMap<String, ()> = TFxMap::default();
        for entry in batch.queries.values() {
            if let Some(tr) = entry.op.table_ref() {
                all_repos.insert(tr.repo.clone(), ());
            }
        }
        let repos: Vec<String> = all_repos.into_keys().collect();

        // Touch each repo's unknown fields. The touch fn short-circuits when
        // every name is already cached, so a warm cache adds no roundtrip.
        for (repo, mut names) in per_repo {
            names.sort_unstable();
            names.dedup();
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            if !refs.is_empty() {
                self.touch_fields(db, &repo, &refs).await?;
            }
        }

        // v2 id-keyed write path: encode fully-literal INSERT records into
        // records_idmsgpack and request Id result encoding for transparent
        // server-side pass-through. v1 path: send batch unchanged.
        if self.server_query_version() >= 2 {
            for entry in batch.queries.values_mut() {
                if let BatchOp::Insert(ref mut op) = entry.op {
                    let fm = self
                        .interner_cache()
                        .get_or_create(db, &op.insert_into.repo);
                    // Drain values, separating literal from $fn records.
                    // Literal records → records_idmsgpack (id-keyed storage bytes).
                    // $fn records → remain on values (server-side eval required).
                    let mut remaining: Vec<QueryValue> = Vec::with_capacity(op.values.len());
                    for qv in op.values.drain(..) {
                        if qv_has_fn_marker(&qv) {
                            remaining.push(qv);
                        } else {
                            // §9.4: fm.id_of(name) — name is an opaque STRING,
                            // never parsed as a number.
                            let bytes = encode_record_idmsgpack(&qv, &fm)?;
                            op.records_idmsgpack.push(ByteBuf::from(bytes));
                        }
                    }
                    op.values = remaining;
                }
            }
            // Request id-keyed result rows ONLY when the batch has no
            // cross-query references. Finding 1.4: a batch carrying a `$query`
            // / `$param` ref or a sub-batch relies on the server's INTERMEDIATE
            // results staying name-keyed — under `ResultEncoding::Id` those
            // intermediates become opaque `QueryRecord::IdBytes` rows whose
            // `as_value()` is `Null`, so ref path-resolution (`@dep[i].field`)
            // silently breaks (proven by
            // `shamir-engine … query_ref_does_not_resolve_under_id_encoding`).
            // This mirrors the TS client's `batchHasRefs` guard — keeping the
            // two clients aligned on the smart-write path.
            if !batch_has_refs(&batch) {
                batch.result_encoding = ResultEncoding::Id;
            }
        }

        let response = self.execute(db, batch).await?;

        // De-intern any IdBytes rows back to name-keyed QueryValues. The repos
        // slice tells deintern_response which FieldMaps to consult. Only called
        // on v2 (result_encoding = Id was set above), but safe to call on v1
        // responses too since deintern_response is a no-op when no IdBytes rows
        // are present.
        deintern_response(self, db, response, &repos).await
    }
}

/// Walk a single `BatchOp`, collecting field names that appear as top-level
/// map keys of insert/upsert/update record values, grouped by repo.
///
/// Recurses into nested `Value::Map`s so that a record like
/// `{ "profile": { "age": 30 } }` registers both "profile" and "age". The
/// repo is read from the op's table-ref.
fn collect_field_names(op: &BatchOp, out: &mut TFxMap<String, Vec<String>>) {
    let (repo, values): (&str, Vec<&QueryValue>) = match op {
        BatchOp::Insert(InsertOp {
            insert_into,
            values,
            ..
        }) => (insert_into.repo.as_str(), values.iter().collect()),
        BatchOp::Set(SetOp { set, key, value }) => {
            // Upsert: both the key map and the value map contribute field names.
            (set.repo.as_str(), vec![key, value])
        }
        BatchOp::Update(UpdateOp { update, set, .. }) => (update.repo.as_str(), vec![set]),
        _ => return,
    };
    if values.is_empty() {
        return;
    }
    let bucket = out.entry(repo.to_string()).or_default();
    for v in values {
        collect_map_keys(v, bucket);
    }
}

/// Recursively collect map keys from a `Value::Map` (and its nested maps).
fn collect_map_keys(v: &QueryValue, out: &mut Vec<String>) {
    if let Value::Map(m) = v {
        for (k, child) in m.iter() {
            out.push(k.clone());
            collect_map_keys(child, out);
        }
    } else if let Value::List(items) = v {
        // A list of records (e.g. insert values carried as a list of maps).
        for item in items {
            collect_map_keys(item, out);
        }
    }
}

// ─── v2 id-keyed helpers ──────────────────────────────────────────────────────

/// Returns `true` if `v` contains a map with the key `"$fn"` anywhere in the
/// value tree. Records containing `$fn` rely on server-side evaluation and
/// MUST NOT be encoded as id-keyed storage bytes.
///
/// The check is recursive: a `$fn` nested anywhere (including inside a list of
/// maps or a nested map value) marks the whole record as non-literal.
fn qv_has_fn_marker(v: &QueryValue) -> bool {
    match v {
        Value::Map(m) => {
            if m.contains_key("$fn") {
                return true;
            }
            m.values().any(qv_has_fn_marker)
        }
        Value::List(items) => items.iter().any(qv_has_fn_marker),
        Value::Set(items) => items.iter().any(qv_has_fn_marker),
        // Scalars: no $fn possible.
        _ => false,
    }
}

/// Finding 1.4: returns `true` if the batch carries any cross-query reference
/// that requires the server's INTERMEDIATE results to stay name-keyed — namely
/// a `$query` / `$param` [`FilterValue`] anywhere in a Read/Update/Delete
/// filter, or a sub-batch op (whose `bind` map and inner queries can carry
/// refs). Such batches must NOT request `ResultEncoding::Id`, because id-keyed
/// (`QueryRecord::IdBytes`) intermediates are opaque to path resolution.
///
/// Mirrors the TS client's `batchHasRefs`.
pub(crate) fn batch_has_refs(batch: &BatchRequest) -> bool {
    batch.queries.values().any(|entry| op_has_refs(&entry.op))
}

/// `true` if a single [`BatchOp`] carries a `$query`/`$param` ref or a
/// sub-batch. Recurses into nested sub-batches.
fn op_has_refs(op: &BatchOp) -> bool {
    match op {
        BatchOp::Read(q) => q.r#where.as_ref().is_some_and(filter_has_refs),
        BatchOp::Update(UpdateOp { where_clause, .. }) => {
            where_clause.as_ref().is_some_and(filter_has_refs)
        }
        BatchOp::Delete(DeleteOp { where_clause, .. }) => filter_has_refs(where_clause),
        // A sub-batch always depends on server-side name-keyed intermediates
        // (its `bind` map + inner queries reference the outer scope), so treat
        // any sub-batch as ref-bearing unconditionally.
        BatchOp::Batch(_) => true,
        _ => false,
    }
}

/// `true` if a filter tree contains a `$query`/`$param` [`FilterValue`].
fn filter_has_refs(f: &Filter) -> bool {
    match f {
        Filter::Eq { value, .. }
        | Filter::Ne { value, .. }
        | Filter::Gt { value, .. }
        | Filter::Gte { value, .. }
        | Filter::Lt { value, .. }
        | Filter::Lte { value, .. }
        | Filter::Contains { value, .. }
        | Filter::FieldEq { value, .. } => fv_has_refs(value),
        Filter::In { values, .. }
        | Filter::NotIn { values, .. }
        | Filter::ContainsAny { values, .. }
        | Filter::ContainsAll { values, .. } => values.iter().any(fv_has_refs),
        Filter::Between { from, to, .. } => fv_has_refs(from) || fv_has_refs(to),
        Filter::And { filters } | Filter::Or { filters } => filters.iter().any(filter_has_refs),
        Filter::Not { filter } => filter_has_refs(filter),
        Filter::Computed {
            expr_args, value, ..
        } => {
            fv_has_refs(value)
                || expr_args
                    .as_ref()
                    .is_some_and(|args| args.iter().any(fv_has_refs))
        }
        // Pattern / null / existence / index-accel operators carry no
        // FilterValue positions that can hold a $query / $param ref.
        _ => false,
    }
}

/// `true` if a [`FilterValue`] is (or recursively contains) a `$query` /
/// `$param` reference.
///
/// Recurses into `$fn`/`$expr`/`$cond` argument positions (found in @fl
/// review of task #497): `execute_with_touch` is a public API accepting an
/// arbitrary caller-built `BatchRequest` — `FilterValue::query_ref()` +
/// `FnCall::complex()`/`FilterExpr::new()`/`Cond::new()` are all public
/// builder constructors, so a caller can legally construct a
/// `$fn`/`$expr`/`$cond`-wrapped `$query`/`$param` ref. A flat (non-
/// recursing) check would silently reintroduce the finding-1.4 bug for that
/// shape. Mirrors the TS client's `hasQueryRef`, which recurses into every
/// nested object.
fn fv_has_refs(fv: &FilterValue) -> bool {
    match fv {
        FilterValue::QueryRef { .. } | FilterValue::Param { .. } => true,
        FilterValue::Array(items) => items.iter().any(fv_has_refs),
        FilterValue::FnCall { call } => call.args().iter().any(fv_has_refs),
        FilterValue::Expr { expr } => expr.args.iter().any(fv_has_refs),
        FilterValue::Cond { cond } => {
            filter_has_refs(&cond.condition)
                || fv_has_refs(&cond.then)
                || fv_has_refs(&cond.or_else)
        }
        _ => false,
    }
}

/// Encode a single fully-literal `QueryValue` map record to id-keyed storage
/// bytes via [`query_value_to_storage_bytes`].
///
/// The intern closure resolves each field NAME from the FieldMap.
/// §9.4: `FieldMap::id_of(name)` looks up the name as an opaque STRING;
/// a field literally named "42" maps to its server-assigned id, never 42.
fn encode_record_idmsgpack(
    qv: &QueryValue,
    fm: &crate::interner_cache::FieldMap,
) -> Result<Vec<u8>, ClientError> {
    let intern = |name: &str| {
        fm.id_of(name).map(InternerKey::new).ok_or_else(|| {
            shamir_types::codecs::CodecError::Encode(format!(
                "field '{}' not in FieldMap — touch_fields must be called first",
                name
            ))
        })
    };
    query_value_to_storage_bytes(qv, &intern)
        .map(|bytes| bytes.to_vec())
        .map_err(|e| ClientError::Protocol(format!("id-keyed record encode: {e}")))
}

/// De-intern all `QueryRecord::IdBytes` rows in `response` using the FieldMaps
/// for `repos` under `db`. Returns the response with all IdBytes rows replaced
/// by name-keyed `QueryRecord::Direct(QueryValue)`.
///
/// This function is only called from `execute_with_touch` on v2 responses.
/// It is a no-op if no IdBytes rows are present (v1 responses, or responses
/// from non-insert ops that return name-keyed rows).
///
/// **Repo selection:** the caller passes the repos that were targeted by the
/// batch — these are exactly the repos whose FieldMaps are pre-warmed by
/// `touch_fields`. For each IdBytes row we try each repo's FieldMap; the
/// correct repo's map fully resolves all ids. If no repo resolves the row,
/// we call `refresh_repo` ONCE per repo and retry.
async fn deintern_response(
    client: &Client,
    db: &str,
    mut response: BatchResponse,
    repos: &[String],
) -> Result<BatchResponse, ClientError> {
    // Track which repos have been refreshed in this call to avoid double-refresh.
    let mut refreshed: TFxMap<String, ()> = TFxMap::default();

    for result in response.results.values_mut() {
        deintern_query_result(client, db, result, repos, &mut refreshed).await?;
    }
    Ok(response)
}

/// De-intern all `QueryRecord::IdBytes` rows in `result` in place.
async fn deintern_query_result(
    client: &Client,
    db: &str,
    result: &mut QueryResult,
    repos: &[String],
    refreshed: &mut TFxMap<String, ()>,
) -> Result<(), ClientError> {
    for record in &mut result.records {
        if let QueryRecord::IdBytes(ref bytes) = *record {
            let bytes_snapshot = bytes.clone();
            let qv =
                deintern_id_bytes(client, db, bytes_snapshot.as_ref(), repos, refreshed).await?;
            *record = QueryRecord::Direct(qv);
        }
    }
    Ok(())
}

/// De-intern a single `IdBytes` payload using the FieldMaps for `repos`.
///
/// Attempt 1: try each repo's FieldMap. Return the first successful de-intern.
/// Attempt 2 (if all fail): refresh repos once, then retry.
/// If still failing after refresh, return a protocol error.
async fn deintern_id_bytes(
    client: &Client,
    db: &str,
    bytes: &[u8],
    repos: &[String],
    refreshed: &mut TFxMap<String, ()>,
) -> Result<QueryValue, ClientError> {
    // Attempt 1: try all known repos without any refresh.
    if let Some(qv) = try_deintern_repos(client, db, bytes, repos) {
        return Ok(qv);
    }

    // Attempt 2: refresh all not-yet-refreshed repos, then retry.
    for repo in repos {
        if !refreshed.contains_key(repo) {
            client.refresh_repo(db, repo).await?;
            refreshed.insert(repo.clone(), ());
        }
    }

    try_deintern_repos(client, db, bytes, repos).ok_or_else(|| {
        ClientError::Protocol(format!(
            "de-intern: id-keyed row could not be resolved after refresh_repo (db={db})"
        ))
    })
}

/// Try to de-intern `bytes` using each repo's FieldMap in turn. Returns the
/// first successful de-intern, or `None` if all repos fail.
///
/// A repo's de-intern succeeds when EVERY id in the row is present in that
/// repo's FieldMap. If any id is missing, `record_view_deintern_with` returns
/// an error and we try the next repo.
///
/// **Assumption:** field-id namespaces are per-repo; a given id maps to the
/// same field name within a single repo and is not shared across repos. The
/// caller must pass only the repos targeted by the batch — `touch_fields`
/// ensures their FieldMaps are pre-warmed. In the common single-repo case
/// there is exactly one candidate and this is unconditionally correct. In a
/// multi-repo batch this is a best-effort first-match: if two repos happen to
/// assign the same id to different field names the result may come from the
/// wrong repo. Avoid mixing repos that share id-space in one batch.
fn try_deintern_repos(
    client: &Client,
    db: &str,
    bytes: &[u8],
    repos: &[String],
) -> Option<QueryValue> {
    // Single-repo batches are the overwhelmingly common case and are always
    // correct. Multi-repo batches rely on the per-repo id-namespace assumption
    // above; assert that callers don't silently pass an unbounded repo set.
    debug_assert!(
        !repos.is_empty(),
        "try_deintern_repos: repos must not be empty"
    );

    let view = RecordView::new(bytes)
        .map_err(|e| tracing::warn!("de-intern: RecordView::new failed: {:?}", e))
        .ok()?;

    for repo in repos {
        let fm = client.interner_cache().get_or_create(db, repo);
        let resolver = |id: u64| fm.name_of(id);
        if let Ok(qv) = record_view_deintern_with(&view, &resolver) {
            return Some(qv);
        }
    }
    None
}
