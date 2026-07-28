//! `MirroredStore` — an in-memory-primary `Store` that conditionally
//! mirrors a durable-config subset of its writes to a backing `Store`.
//!
//! Step 1 of the F-33 hybrid-repo campaign (task #835 / #826): table
//! DATA stays fully ephemeral (in-memory) while table CONFIGURATION
//! (index definitions, validator bindings, buffer config, the interner)
//! survives a process restart. `MirroredStore` composes an
//! [`InMemoryStore`] (the primary — every read goes here, nothing else
//! is ever consulted on the read path) with an `Arc<dyn Store>` mirror
//! (the durable backend), writing to the mirror ONLY for keys an
//! allowlist classifier ([`is_durable_table_config`]) accepts.
//!
//! On construction, the primary is hydrated by streaming every entry
//! already in the mirror and RE-RUNNING the classifier against each key
//! (F-41) — see [`MirroredStore::new`] for why the classifier is
//! re-applied at hydration (rather than trusting "was classified true
//! when written" as an eternal invariant) and what happens to entries
//! that no longer match the current classifier.
//!
//! ## `transact` atomicity (F-39)
//!
//! [`MirroredStore`] overrides [`Store::transact`] to give the
//! DURABLE-classified subset of a batch real cross-op atomicity on the
//! mirror — the ephemeral half remains per-op (matching
//! [`InMemoryStore`]'s inherited trait default, which has no multi-key
//! atomicity primitive). See the override's own doc comment for the
//! precise failure/compensation story and the residual concurrent-reader
//! window.

use crate::error::DbResult;
use crate::storage_in_memory::InMemoryStore;
use crate::storage_membuffer::MemBufferConfig;
use crate::types::{KvOp, RecordKey, RecordStream, Store};
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use std::sync::Arc;

/// System-record prefix — the first 4 bytes of every `RecordId::system`
/// key (see `shamir_types::types::record_id::RecordId::system` /
/// `SYSTEM_RECORD_PREFIX`). Kept as a local literal since the constant
/// itself is private to `shamir-types`; `RecordId::is_system` performs
/// the equivalent check for the `RecordId` type directly.
const SYSTEM_RECORD_PREFIX: [u8; 4] = [0, 0, 0, 0];

/// Total byte length of every `RecordId::system(..)`-derived key
/// (`4`-byte zero prefix + up to 12 name bytes, zero-padded).
const SYSTEM_KEY_LEN: usize = 16;

/// Allowlist of system-record tags (the trimmed `key[4..16]`, NUL bytes
/// stripped) that are durable TABLE CONFIGURATION and therefore belong
/// in the mirror. Cross-checked against every
/// `shamir_engine::meta::namespace::MetaKey` variant (see module docs
/// below for the per-variant rationale) — this is deliberately an
/// ALLOWLIST: an unrecognized tag defaults to ephemeral (ok bug: a
/// future config key stops surviving restart) rather than a denylist
/// (dangerous bug: a future derived-state key would silently
/// resurrect stale data on reopen).
///
/// **12-byte truncation gotcha.** `RecordId::system(name)` silently
/// truncates `name` to its first 12 bytes (see that constructor's
/// doc). Three `MetaKey::tag()` strings exceed 12 bytes, so the
/// ACTUAL bytes landing in a key are the truncated form, not the tag
/// literal in `namespace.rs`:
/// - `MetaKey::LegacyIndexesUnique` tag `"indexes_unique"` (14 bytes)
///   → truncated to `"indexes_uniq"`.
/// - `MetaKey::SortedIndexes` tag `"sorted_indexes"` (14 bytes) →
///   truncated to `"sorted_index"`.
/// - `MetaKey::BufferConfig` tag `"buffer_config"` (13 bytes) →
///   truncated to `"buffer_confi"`.
///
/// The entries below use the TRUNCATED (actual on-the-wire) forms —
/// matching against the untruncated tag string would silently never
/// fire, since no key `RecordId::system` produces ever contains those
/// extra bytes. Verified by this module's exhaustiveness test, which
/// builds keys via `RecordId::system(tag)` (the real constructor,
/// truncation included) rather than comparing strings directly.
const ALLOWLIST: &[&str] = &[
    // MetaKey::LegacyIndexes / LegacyIndexesUnique / SortedIndexes /
    // Validators / Internals / BufferConfig / Tables / Wal / Migrations
    // / Indexes — static, hand-authored or admin-driven table
    // configuration. None of these are derived from row data, so they
    // are safe (and desirable) to survive a restart of a hybrid table.
    "indexes",
    "indexes_uniq", // MetaKey::LegacyIndexesUnique, truncated from "indexes_unique"
    "sorted_index", // MetaKey::SortedIndexes, truncated from "sorted_indexes"
    "_m.idx",
    "_m.val",
    "buffer_confi", // MetaKey::BufferConfig, truncated from "buffer_config"
    "internals",
    "_m.tbl",
    "_m.wal",
    "_m.mig",
    // `_m.idx.lfv` — legacy posting-format version stamp
    // (`shamir_index::persistence::save_legacy_index_version` /
    // `legacy_indexes_need_rebuild`, `crates/shamir-index/src/persistence.rs`
    // ~line 44/94-123). NOT a `MetaKey` variant — it's an ad-hoc
    // `RecordId::system("_m.idx.lfv")` key constructed directly in
    // `shamir-index`, so the `MetaKey`-exhaustiveness cross-check alone
    // does not surface it; found by a separate workspace-wide grep for
    // every `RecordId::system(...)` call site outside `namespace.rs`/tests.
    // Static config (marks "the on-disk posting format is up to date"),
    // not derived from row data — if this stamp were missing after a
    // hybrid reopen, `legacy_indexes_need_rebuild` would return `true` on
    // every reopen and trigger a wasted (harmless, since it runs over the
    // correctly-empty ephemeral history, but pointless) `repair()` pass.
    "_m.idx.lfv",
];

/// Tag prefix for interner delta chunks (see
/// `shamir_engine::table::interner_manager::CHUNK_TAG_PREFIX`). Every
/// tag of the shape `"i.d" + 9-digit zero-padded index` is a durable
/// interner chunk record and belongs in the mirror.
const INTERNER_CHUNK_PREFIX: &str = "i.d";

/// Returns `true` iff `key` is a durable-table-config system record —
/// i.e. it should ALSO be written to [`MirroredStore`]'s durable
/// mirror, on top of always landing in the ephemeral in-memory
/// primary.
///
/// **Allowlist semantics — read this before adding a new
/// `MetaKey`.** A key defaults to EPHEMERAL unless its tag is
/// explicitly recognized below. This bias is deliberate: forgetting to
/// ADD a future config tag here just means "a setting doesn't survive
/// restart" (a bug report); forgetting to EXCLUDE a derived-state tag
/// would mean "stale state silently corrupts a reopened hybrid table"
/// (much worse — the store would lie about what happened since the
/// last restart).
///
/// **Explicitly excluded system tags** (all cross-checked against
/// `shamir_engine::meta::namespace::MetaKey` — see that enum for the
/// canonical tag strings):
///
/// - `MetaKey::Count` (tag `"count"`) — the record counter's
///   persisted value, DERIVED FROM DATA. Row data is fully ephemeral
///   in a hybrid table, so persisting the last-seen count would make a
///   reopened table's row count lie about how many rows actually exist
///   post-restart.
/// - `MetaKey::LastCommittedVersion` (tag `"_t.lcv"`) and
///   `MetaKey::NextTxId` (tag `"_t.nti"`) — MVCC/WAL recovery markers.
///   Both exist purely to let recovery seed its counters against
///   already-committed transactional history (see
///   `shamir_engine::meta::recovery_marker`'s module doc). That history
///   lives in the ephemeral primary's row data; persisting these
///   markers without the data they describe would let a reopened
///   hybrid table seed a `RepoTxGate` counter from a version space that
///   no longer has any corresponding committed rows — the same
///   "derived-from-data, not configuration" hazard as `Count`, just
///   for the transaction log instead of the row counter.
/// - `MetaKey::ReplicationBookmark` (tag `"_t.rbm"`) — the highest
///   leader `commit_version` a follower has durably applied (see
///   `shamir_engine::meta::repl_bookmark`'s module doc). It gates
///   idempotent re-delivery against locally-applied DATA. If a hybrid
///   table's data does not survive restart, a persisted bookmark would
///   make the follower silently skip re-applying leader events whose
///   effects were actually lost — an even sharper case of "stale
///   derived state corrupts a reopened table" than `Count`.
///
/// These three (`LastCommittedVersion`, `NextTxId`,
/// `ReplicationBookmark`) are NOT listed in the original design sketch
/// for this classifier — found by cross-checking every `MetaKey`
/// variant in `namespace.rs` per this task's brief. They are grouped
/// with `Count` as data-derived, not configuration.
///
/// Non-system keys (postings, sorted-index entries, vector snapshot /
/// delta chunks) never reach the tag comparison at all: they fail the
/// `len() == 16` / `[0,0,0,0]` prefix check first (posting keys are
/// `4+1+value_len+16` bytes with a `u32` index-id prefix that is only
/// all-zero for `index_id == 0`, and even then byte 4 is a `type_tag`
/// discriminant rather than the start of an ASCII ALLOWLIST tag; sorted
/// index / vector snapshot keys are ASCII strings like
/// `"<keyspace>.g0.data.000000"`, never 16 bytes).
pub fn is_durable_table_config(key: &RecordKey) -> bool {
    let bytes = key.as_slice();
    if bytes.len() != SYSTEM_KEY_LEN {
        return false;
    }
    if bytes[0..4] != SYSTEM_RECORD_PREFIX {
        return false;
    }

    let tag_bytes = &bytes[4..16];
    // Trim trailing NUL padding — `RecordId::system` zero-pads the
    // 12-byte name field, so a short tag like "internals" is stored
    // as "internals\0\0\0".
    let trimmed_len = tag_bytes
        .iter()
        .rposition(|&b| b != 0)
        .map(|last_nonzero| last_nonzero + 1)
        .unwrap_or(0);
    let tag_bytes = &tag_bytes[..trimmed_len];

    let Ok(tag) = std::str::from_utf8(tag_bytes) else {
        return false;
    };

    ALLOWLIST.contains(&tag) || tag.starts_with(INTERNER_CHUNK_PREFIX)
}

/// In-memory-primary, conditionally-durable-mirror `Store`.
///
/// - **Reads** (`get`, `get_many`, `iter_stream`, `scan_prefix_stream`,
///   `iter_range_stream`, `iter_range_stream_reverse`) go to `primary`
///   ONLY — `mirror` is never consulted after construction, keeping
///   this store zero-cost on the read path.
/// - **Writes** (`insert`, `set`, `remove`) always land in `primary`
///   unconditionally (matching `InMemoryStore`'s exact semantics), and
///   additionally reach `mirror` iff `classify(&key)` returns `true`.
/// - **`flush`** flushes `mirror` only (`primary` is pure in-memory).
/// - **`apply_buffer_config`** forwards to `mirror` only.
/// - **`raw_backend`** returns `None` — see its trait-doc contract:
///   this store is not a pure cache-wrapper around a single backend,
///   and unwrapping to `mirror` would let a caller (e.g.
///   `fully_unwrap_store`) bypass the classifier and write everything
///   unconditionally to disk.
pub struct MirroredStore {
    primary: InMemoryStore,
    mirror: Arc<dyn Store>,
    classify: fn(&RecordKey) -> bool,
}

impl MirroredStore {
    /// Construct a `MirroredStore` over `mirror`, hydrating `primary`
    /// with every entry currently in `mirror` that STILL passes the
    /// current classifier.
    ///
    /// **F-41 — classifier re-filter at hydration.** Each streamed
    /// entry's key is re-run through `classify(&key)` rather than
    /// trusting "everything in the mirror was classified `true` when it
    /// was written" as an eternal invariant. A key can be present in
    /// `mirror` yet fail the CURRENT classifier via either:
    /// - a classifier change across a version upgrade (a tag durable
    ///   under an OLD allowlist — e.g. a once-accepted
    ///   `Count` / `ReplicationBookmark`-style derived-state tag — may
    ///   no longer be durable under the new one), or
    /// - on-disk corruption / manual tampering that planted an
    ///   arbitrary key in the backing store.
    ///
    /// For a key that does NOT pass the current classifier, hydration
    /// SKIPS it (does NOT load it into `primary`) and emits a
    /// `log::warn!` naming the key and the reason — surfacing the drift
    /// for operator visibility. **Skipping is the safer choice for THIS
    /// classifier:** every tag it deliberately excludes (`Count`,
    /// `LastCommittedVersion`, `NextTxId`, `ReplicationBookmark`) is
    /// DERIVED FROM DATA, excluded precisely because surfacing it after
    /// a restart makes a reopened hybrid table LIE about its state (a
    /// stale row count; stale MVCC counters; a replication bookmark
    /// that skips re-applying leader events whose effects were lost).
    /// Loading such a key from the mirror back into `primary` would
    /// re-activate exactly that hazard — and since the key would keep
    /// failing `classify` on every subsequent reopen, the stale value
    /// would be re-loaded on every restart until manually purged.
    /// Skipping leaves the stale entry inert in the mirror (it is NOT
    /// deleted, just not surfaced to the live process) while the
    /// `warn!` surfaces the drift. For the orthogonal case — a
    /// LEGITIMATE config key accidentally dropped from the allowlist by
    /// a future classifier regression — skipping reproduces the
    /// already-documented "a setting doesn't survive restart" failure
    /// mode (see [`is_durable_table_config`]'s doc), and the `warn!`
    /// makes that regression immediately visible rather than silent.
    ///
    /// Config state is small (a handful of keys plus interner chunks),
    /// so a full streamed hydration at construction time is cheap;
    /// this is intentionally not lazy.
    pub async fn new(mirror: Arc<dyn Store>, classify: fn(&RecordKey) -> bool) -> DbResult<Self> {
        let primary = InMemoryStore::new();

        let mut stream = mirror.iter_stream(shamir_tunables::store_defaults::FULL_SCAN_BATCH);
        while let Some(batch) = stream.next().await {
            let batch = batch?;
            for (key, value) in batch {
                // F-41: re-run the classifier against each streamed
                // entry — see this constructor's doc for why "was
                // classified true when written" is not a safe eternal
                // invariant, and why a mismatching key is SKIPPED.
                if !(classify)(&key) {
                    log::warn!(
                        "MirroredStore hydration: skipping mirror entry that \
                         no longer matches the durable-config classifier \
                         (classifier drift or mirror tampering) — key = {:?}",
                        key
                    );
                    continue;
                }
                // `set_no_flag` — hydration doesn't need the
                // created/updated flag, and every key is fresh in a
                // brand-new `InMemoryStore`.
                primary.set_no_flag(key, value).await?;
            }
        }

        Ok(Self {
            primary,
            mirror,
            classify,
        })
    }
}

#[async_trait]
impl Store for MirroredStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        // `InMemoryStore::insert` mints its key from a fresh
        // `RecordId::new()` (timestamp-prefixed, never all-zero — see
        // `RecordId::from_ts`), so the resulting key can NEVER match
        // the system-record shape (`[0,0,0,0]` prefix) `classify`
        // requires. `insert` therefore never needs to write to
        // `mirror` at all; every durable-config key in this store's
        // domain is always written via `set`, never `insert`.
        self.primary.insert(value).await
    }

    /// **F-41 write-ordering — mirror-commit-before-primary-publish
    /// for durable keys.**
    ///
    /// For a CLASSIFIED (durable) key the mirror write happens FIRST;
    /// only if it succeeds is `primary` mutated. This closes the
    /// pre-F-41 error-atomicity hole, where a mirror-write failure left
    /// `primary` already mutated (so the live process behaved as if the
    /// write had succeeded) even though the caller received `Err` and a
    /// restart reverted the write (the mirror never got it) — three
    /// divergent observable states across one operation's lifetime. Now
    /// a mirror failure means "nothing happened", honestly: `primary`
    /// is never touched.
    ///
    /// For an UNCLASSIFIED (ephemeral) key there is no mirror
    /// involvement, so the write goes straight to `primary`, unchanged
    /// from before.
    ///
    /// # Residual — reverse-direction divergence (investigated, documented)
    ///
    /// Mirror-first has the symmetric residual: a mirror SUCCESS
    /// followed by a `primary` FAILURE would leave `mirror` ahead of
    /// `primary` (the durable side holds a value the live process can't
    /// yet see, which a subsequent restart's hydration would surface).
    /// **This window does not exist in practice.** `InMemoryStore::set`
    /// is structurally infallible at the `DbResult` level — its body is
    /// `Ok(match self.data.insert_sync { ... })` with no `?`
    /// propagation and no `Err` return path: the `insert_sync` "error"
    /// is merely the duplicate-key signal it handles internally via
    /// `remove_sync` + re-`insert_sync` (a genuine allocation failure
    /// inside `scc` would `panic!`/abort the process, not yield `Err`).
    /// `InMemoryStore::remove` is likewise
    /// `Ok(self.data.remove_sync(..))` — `remove_sync` returns a
    /// `bool`, never an error. So once a mirror write succeeds, the
    /// following primary write cannot fail, and the reverse divergence
    /// is unreachable. (This confirms F-39's own investigation of the
    /// same two methods, made concrete here by reading the actual
    /// `InMemoryStore` impls rather than reasoning about them
    /// abstractly.)
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        if (self.classify)(&key) {
            // Durable key: mirror commit FIRST. If this errors, primary
            // is NEVER touched — the caller's `Err` is now an honest
            // "nothing happened", not "half happened".
            self.mirror.set(key.clone(), value.clone()).await?;
        }
        // Return value is `primary`'s "was this a fresh insert vs
        // overwrite" flag, preserving `InMemoryStore::set`'s exact
        // semantics (the mirror's own created-flag is deliberately
        // discarded — it is not meaningful to the caller, and mixing
        // it with primary's would be).
        self.primary.set(key, value).await
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.primary.get(key).await
    }

    async fn get_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<Option<Bytes>>> {
        self.primary.get_many(keys).await
    }

    /// **F-41 write-ordering — same mirror-first discipline as
    /// [`Self::set`], applied to removal.** For a CLASSIFIED key the
    /// mirror removal happens FIRST; only if it succeeds is the key
    /// removed from `primary`. For an UNCLASSIFIED key, `primary` is
    /// mutated directly with no mirror involvement. The same residual
    /// analysis as `set` applies (the reverse divergence is unreachable
    /// because `InMemoryStore::remove` is infallible).
    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        if (self.classify)(&key) {
            // Durable key: mirror removal FIRST — see `set` for the
            // full rationale.
            self.mirror.remove(key.clone()).await?;
        }
        // Return value is `primary`'s "did the key exist" flag,
        // preserving `InMemoryStore::remove`'s exact semantics.
        self.primary.remove(key).await
    }

    async fn flush(&self) -> DbResult<()> {
        // `primary` is pure in-memory — nothing to flush there.
        self.mirror.flush().await
    }

    async fn apply_buffer_config(&self, config: &MemBufferConfig) -> DbResult<()> {
        // `primary`, being pure in-memory, has no buffer tuning of its
        // own to apply.
        self.mirror.apply_buffer_config(config).await
    }

    async fn raw_backend(&self) -> Option<Arc<dyn Store>> {
        // NOT a pure cache-wrapper around a single backend: returning
        // `Some(mirror)` here would let a caller like
        // `types::fully_unwrap_store` bypass this store's classifier
        // entirely and write everything unconditionally to durable
        // storage. `primary` is this store's actual write path, so the
        // default `None` (already-raw) is the correct answer.
        None
    }

    fn iter_stream(&self, batch_size: usize) -> RecordStream {
        self.primary.iter_stream(batch_size)
    }

    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> RecordStream {
        self.primary.scan_prefix_stream(prefix, batch_size)
    }

    fn iter_range_stream(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> RecordStream {
        self.primary
            .iter_range_stream(start_inclusive, end_inclusive, batch_size)
    }

    fn iter_range_stream_reverse(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> RecordStream {
        self.primary
            .iter_range_stream_reverse(start_inclusive, end_inclusive, batch_size)
    }

    // `insert_many` / `set_many` / `remove_many`: NOT overridden. The
    // trait's default implementations loop calling `self.insert` /
    // `self.set` / `self.remove` per item — since THOSE overrides
    // already do the classify-and-conditionally-mirror work internally,
    // the default loops produce correct behavior for free (each element
    // gets the same primary-then-conditional-mirror treatment as a
    // standalone call). `InMemoryStore` itself has no native
    // transactional batch API to delegate to for a genuine efficiency
    // win, so there is no motivation to hand-roll a custom override
    // here.
    //
    // `transact` IS overridden — see below.

    /// Partitioned atomic-ish batch — gives the DURABLE-classified subset
    /// real cross-op atomicity on the mirror while leaving the ephemeral
    /// subset per-op (matching the inherited trait default's semantics).
    ///
    /// # Why an override is needed
    ///
    /// The trait's default `transact` loops over `self.set` / `self.remove`
    /// per op — each of which individually mirrors durable keys correctly.
    /// But that loop is **per-op atomic only, NOT cross-op atomic on the
    /// mirror**: a mid-batch failure (e.g. the Nth durable op's mirror
    /// write errors) leaves the first N−1 ops durably committed and the
    /// rest absent. On a hybrid repo, callers like index-rename code call
    /// `transact` specifically for all-or-nothing semantics (write the new
    /// posting, delete the old one, in one batch); the non-atomic default
    /// silently drops that guarantee on the durable side.
    ///
    /// This override splits the batch by [`Self::classify`]: the durable
    /// subset is committed to the mirror as ONE atomic unit via the
    /// mirror backend's own [`Store::transact`] (e.g. `FjallStore`'s
    /// `OwnedWriteBatch` — all-or-nothing), while the ephemeral subset
    /// is applied to primary per-op (no atomicity — `InMemoryStore`
    /// has no multi-key primitive to delegate to, and adding one is out
    /// of scope; see the concurrent-reader residual below).
    ///
    /// # Ordering — ephemeral-first, then durable-mirror, then durable-primary
    ///
    /// 1. **Ephemeral ops → `primary`** (per-op loop). If this fails
    ///    partway, no durable write is attempted (fail fast). `primary`
    ///    may be left partially mutated — the same pre-existing, tolerable
    ///    characteristic a plain `InMemoryStore` already has (nothing
    ///    durable was touched, so nothing inconsistent survives a
    ///    restart; the live process's in-memory state is transiently
    ///    inconsistent until the caller's own retry/compensation, exactly
    ///    as today for a non-mirrored in-memory store).
    ///
    /// 2. **Durable ops → `mirror`** atomically via `self.mirror.transact`
    ///    (all-or-nothing via the mirror backend's own transact, e.g.
    ///    `FjallStore`'s `OwnedWriteBatch`). **F-49: this runs BEFORE
    ///    `primary` is touched for the durable subset** — the same
    ///    mirror-commit-before-primary-publish discipline F-41 already
    ///    established for single-key `set`/`remove` (see [`Self::set`]'s
    ///    doc). If this fails, `primary` is NEVER mutated for the durable
    ///    subset: the caller's `Err` is an honest "nothing happened" for
    ///    the durable ops, not "primary already mutated but you got
    ///    `Err`" (the pre-F-49 bug — exactly the class F-41 closed for
    ///    `set`/`remove`, reopened here for the transact batch, and
    ///    especially dangerous for index-rename/metadata transactions,
    ///    the exact use case `transact`'s cross-op atomicity was added
    ///    for). `KvOp` derives `Clone` (types.rs), so the durable ops are
    ///    cloned for the mirror commit and the owned vec is retained for
    ///    the subsequent primary application (phase 3).
    ///
    /// 3. **Durable ops → `primary`** (per-op loop, for immediate read
    ///    visibility — reads go to `primary` ONLY). This runs ONLY after
    ///    the mirror commit succeeded, so a mirror failure can never
    ///    leave `primary` ahead of `mirror`.
    ///
    /// # Residual — reverse-direction divergence (investigated, documented)
    ///
    /// Mirror-first has the symmetric residual: a mirror SUCCESS followed
    /// by a `primary` FAILURE would leave `mirror` ahead of `primary`
    /// (the durable side holds a value the live process can't yet see,
    /// which a subsequent restart's hydration would surface). **This
    /// window does not exist in practice** for the SAME reason
    /// [`Self::set`]'s doc comment already established (see its
    /// "# Residual — reverse-direction divergence" section):
    /// `InMemoryStore::set` is structurally infallible at the `DbResult`
    /// level (`Ok(match self.data.insert_sync { ... })` — no `?`
    /// propagation, no `Err` return path; a genuine allocation failure
    /// inside `scc` would `panic!`/abort, not yield `Err`), and
    /// `InMemoryStore::remove` is likewise `Ok(self.data.remove_sync(..))`
    /// (`remove_sync` returns a `bool`, never an error). So once a mirror
    /// write succeeds, the following primary writes cannot fail, and the
    /// reverse divergence is unreachable — this cites the existing
    /// `set`/`remove` investigation rather than re-deriving it.
    ///
    /// # Concurrent-reader residual (investigated, documented — not fixed)
    ///
    /// While the ephemeral loop (phase 1) is applying its ops one at a
    /// time, a CONCURRENT reader (`get` / `iter_stream` / etc., which
    /// read `primary` directly with no lock) can observe a
    /// partially-applied ephemeral batch. This is a pre-existing
    /// characteristic inherited from `InMemoryStore` having no
    /// cross-key atomicity primitive, NOT something this fix introduces
    /// or worsens for the ephemeral side (it only fixes the DURABLE
    /// side's atomicity, which is the part that matters for
    /// post-restart correctness).
    ///
    /// `scc::TreeIndex` (v3.8.4, the lock-free B+ tree backing
    /// `InMemoryStore`) was checked for any batched / atomic multi-key
    /// primitive: its public API exposes only single-key mutations
    /// (`insert_sync`, `remove_sync`, `upsert_sync`, `remove_if_sync`)
    /// and `remove_range_sync` (contiguous-range removal — cannot set
    /// values, so it cannot express a mixed set+remove batch). There is
    /// no `insert_batch`, `transact`, or multi-key CAS. Closing this
    /// window would require either a global write lock around the
    /// ephemeral loop (defeating the lock-free design) or a
    /// snapshot/isolation layer in `InMemoryStore` — both out of scope
    /// for this task (too large a blast radius for the foundational,
    /// widely-used in-memory primitive).
    async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
        if ops.is_empty() {
            return Ok(());
        }

        // Partition by classifier: ephemeral ops (classify = false) go to
        // the first vec, durable ops (classify = true) to the second.
        let (ephemeral_ops, durable_ops): (Vec<KvOp>, Vec<KvOp>) =
            ops.into_iter().partition(|op| match op {
                KvOp::Set(k, _) | KvOp::Remove(k) => !(self.classify)(k),
            });

        // Phase 1 — ephemeral subset → primary (per-op, no cross-op
        // atomicity; see concurrent-reader residual in the doc comment).
        for op in ephemeral_ops {
            match op {
                KvOp::Set(k, v) => {
                    let _ = self.primary.set(k, v).await?;
                }
                KvOp::Remove(k) => {
                    let _ = self.primary.remove(k).await?;
                }
            }
        }

        // Phase 2 — durable subset → mirror FIRST (atomically, via the
        // mirror backend's own transact — e.g. FjallStore's
        // `OwnedWriteBatch`, all-or-nothing). F-49: mirroring the
        // mirror-commit-before-primary-publish discipline F-41 already
        // gives `set`/`remove`. If this fails, primary is NEVER touched
        // for the durable subset — the caller's `Err` is an honest
        // "nothing happened", not "primary already mutated". `KvOp`
        // derives `Clone`; clone the durable ops for the mirror commit
        // and retain the owned vec for phase 3.
        self.mirror.transact(durable_ops.clone()).await?;

        // Phase 3 — durable subset → primary (per-op, for immediate read
        // visibility — reads go to primary ONLY). Runs ONLY after the
        // mirror commit succeeded. An empty durable subset (clone was
        // empty) is a no-op loop.
        for op in durable_ops {
            match op {
                KvOp::Set(k, v) => {
                    let _ = self.primary.set(k, v).await?;
                }
                KvOp::Remove(k) => {
                    let _ = self.primary.remove(k).await?;
                }
            }
        }

        Ok(())
    }
}
