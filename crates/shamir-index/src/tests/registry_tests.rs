use crate::backend::{IndexBackend, IndexError, IndexQuery, IndexResult};
use crate::build_index2_backend;
use crate::descriptor::IndexDescriptor;
use crate::expr::IndexExpr;
use crate::kind::{
    FunctionalConfig, IndexKind, TokenizerKind, VectorBackendRef, VectorConfig, VectorMetric,
};
use crate::persistence::{load_index2_metadata, save_index2_metadata};
use crate::registry::IndexRegistry;
use async_trait::async_trait;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use smallvec::SmallVec;
use std::collections::BTreeSet;
use std::sync::Arc;

struct DummyBackend(IndexDescriptor);

#[async_trait]
impl IndexBackend for DummyBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn descriptor(&self) -> &IndexDescriptor {
        &self.0
    }
    async fn lookup(&self, _: IndexQuery) -> Result<IndexResult, IndexError> {
        Ok(IndexResult::Set(BTreeSet::new()))
    }
    async fn rebuild(&self, _: Arc<dyn Store>) -> Result<(), IndexError> {
        Ok(())
    }
    async fn drop_all(&self) -> Result<(), IndexError> {
        Ok(())
    }
}

fn make(id: u32, name_interned: u64) -> Arc<dyn IndexBackend> {
    Arc::new(DummyBackend(IndexDescriptor::new(
        id,
        format!("idx_{id}"),
        name_interned,
        SmallVec::new(),
        IndexKind::Btree { unique: false },
    )))
}

#[tokio::test]
async fn insert_and_lookup_by_id() {
    let reg = IndexRegistry::new();
    let b = make(10, 100);
    reg.insert(b).await.unwrap();
    let got = reg.get_by_id(10).await.unwrap();
    assert_eq!(got.descriptor().id, 10);
}

#[tokio::test]
async fn insert_and_lookup_by_name() {
    let reg = IndexRegistry::new();
    reg.insert(make(11, 200)).await.unwrap();
    let got = reg.get_by_name(200).await.unwrap();
    assert_eq!(got.descriptor().id, 11);
}

#[tokio::test]
async fn allocate_id_monotonic() {
    let reg = IndexRegistry::new();
    let a = reg.allocate_id();
    let b = reg.allocate_id();
    let c = reg.allocate_id();
    assert!(a < b && b < c);
}

#[tokio::test]
async fn duplicate_id_rejected() {
    let reg = IndexRegistry::new();
    reg.insert(make(42, 1)).await.unwrap();
    let err = reg.insert(make(42, 2)).await.unwrap_err();
    assert!(matches!(err, IndexError::Backend(_)));
}

#[tokio::test]
async fn remove_drops_both_maps() {
    let reg = IndexRegistry::new();
    reg.insert(make(7, 300)).await.unwrap();
    assert!(reg.get_by_name(300).await.is_some());
    let removed = reg.remove_by_id(7).await.unwrap();
    assert_eq!(removed.descriptor().id, 7);
    assert!(reg.get_by_id(7).await.is_none());
    assert!(reg.get_by_name(300).await.is_none());
}

// ============================================================================
// P0-5a (#961) — RENAME INDEX survives restart for index2 backends
// ============================================================================
//
// Before the fix, `rename_entry` updated ONLY the `by_name` reverse index,
// leaving the backend's own (immutable) descriptor carrying the stale original
// name. `all_descriptors()` — the persistence path — only overrode `.state`
// from the tuple, so `save_index2_metadata` wrote the OLD name to disk. After a
// restart the registry reloaded the persisted blob with the old name: the
// rename was silently reverted. These tests prove the fix for all three
// index2 kinds (FTS / functional / vector), since each backend's construction
// path is exercised by the reopen rebuild.

/// Full rename → save → reload → reopen round-trip for one index2 kind.
///
/// Proves three things at once:
///   1. The persisted descriptor carries BOTH the new `name` and the new
///      `name_interned` (a lookup-only test could pass by accident if only
///      one field were fixed).
///   2. After a simulated restart (load blob + rebuild backends into a fresh
///      registry), a lookup by the NEW name succeeds and by the OLD name
///      fails.
///   3. The live (non-restart) in-memory rename still works.
async fn assert_rename_survives_reopen(kind: IndexKind, kind_label: &str) {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let reg = IndexRegistry::new();

    let old_ni = 1000_u64;
    let new_ni = 2000_u64;

    // Build + insert a real backend under the OLD name.
    let backend = build_index2_backend(
        IndexDescriptor::new(1, "orig_name", old_ni, SmallVec::new(), kind),
        &store,
    );
    reg.insert(backend).await.unwrap();

    // ── (3) Live in-memory rename still works ─────────────────────────────
    // The by_name reverse index flips to the new id; old name stops resolving.
    assert!(
        reg.rename_entry(old_ni, "renamed_name".to_string(), new_ni)
            .await,
        "rename_entry must succeed for a present {kind_label} backend"
    );
    assert!(
        reg.get_by_name(new_ni).await.is_some(),
        "live rename: new name must resolve in-memory ({kind_label})"
    );
    assert!(
        reg.get_by_name(old_ni).await.is_none(),
        "live rename: old name must NOT resolve in-memory ({kind_label})"
    );

    // ── Persist (this is the all_descriptors path — the original bug site) ──
    save_index2_metadata(&reg, &store).await.unwrap();

    // ── (2) Assert BOTH name fields in the persisted descriptor ───────────
    let loaded = load_index2_metadata(&store).await.unwrap().unwrap();
    assert_eq!(
        loaded.descriptors.len(),
        1,
        "exactly one {kind_label} descriptor must be persisted"
    );
    let d = &loaded.descriptors[0];
    assert_eq!(
        d.name, "renamed_name",
        "persisted {kind_label} descriptor must carry the NEW name (P0-5a)"
    );
    assert_eq!(
        d.name_interned, new_ni,
        "persisted {kind_label} descriptor must carry the NEW name_interned (P0-5a)"
    );

    // ── (1) Reopen into a fresh registry: lookup by new/old name ──────────
    let fresh = IndexRegistry::new();
    for d in &loaded.descriptors {
        let b = build_index2_backend(d.clone(), &store);
        fresh.insert(b).await.unwrap();
    }
    assert!(
        fresh.get_by_name(new_ni).await.is_some(),
        "after reopen, {kind_label} must be findable by the NEW name"
    );
    assert!(
        fresh.get_by_name(old_ni).await.is_none(),
        "after reopen, {kind_label} must NOT be findable by the OLD name"
    );
}

#[tokio::test]
async fn rename_persists_across_reopen_fts() {
    assert_rename_survives_reopen(
        IndexKind::Fts {
            tokenizer: TokenizerKind::Whitespace,
            language: None,
        },
        "fts",
    )
    .await;
}

#[tokio::test]
async fn rename_persists_across_reopen_functional() {
    assert_rename_survives_reopen(
        IndexKind::Functional(Box::new(FunctionalConfig {
            expr: IndexExpr::Lower(Box::new(IndexExpr::Field(vec![200]))),
        })),
        "functional",
    )
    .await;
}

#[tokio::test]
async fn rename_persists_across_reopen_vector() {
    assert_rename_survives_reopen(
        IndexKind::Vector(Box::new(VectorConfig {
            dim: 2,
            metric: VectorMetric::Cosine,
            backend: VectorBackendRef::InProcessHnsw {
                ef_construct: 200,
                m: 16,
            },
            quantization: None,
        })),
        "vector",
    )
    .await;
}

/// Regression: the EXISTING in-memory rename behavior (which already worked via
/// `by_name` before the fix) must be unaffected by the new `by_id` name-slot
/// update. A rename with a destination name that is ALREADY taken must still
/// be rejected and leave the old name intact.
#[tokio::test]
async fn rename_conflict_leaves_old_name_intact() {
    let reg = IndexRegistry::new();
    reg.insert(make(30, 3000)).await.unwrap();
    reg.insert(make(31, 3100)).await.unwrap();

    // Renaming 3000 → 3100 must fail (3100 is occupied) and roll back.
    let ok = reg.rename_entry(3000, "taken".to_string(), 3100).await;
    assert!(!ok, "rename onto an occupied name must be rejected");
    // Old name still resolves (rollback restored it).
    assert!(
        reg.get_by_name(3000).await.is_some(),
        "failed rename must leave the source name intact"
    );
    // The would-be victim is untouched.
    assert_eq!(
        reg.get_by_name(3100).await.unwrap().descriptor().id,
        31,
        "destination backend must be the original occupant, not the rename source"
    );
}

/// P0-5a (#961) secondary fix: after a rename, `remove_by_id` must unlink the
/// AUTHORITATIVE (new) interned name from `by_name`, not the backend's stale
/// construction-time `descriptor().name_interned`. Before the fix,
/// `remove_by_id` read `backend.descriptor().name_interned` (the OLD id) and
/// would leave a dangling `by_name` entry under the new id pointing at a
/// removed backend.
#[tokio::test]
async fn remove_after_rename_unlinks_new_name() {
    let reg = IndexRegistry::new();
    reg.insert(make(40, 4000)).await.unwrap();

    // Rename 4000 → 4100.
    assert!(reg.rename_entry(4000, "renamed_40".to_string(), 4100).await);
    // The new name resolves, the old does not.
    assert!(reg.get_by_name(4100).await.is_some());
    assert!(reg.get_by_name(4000).await.is_none());

    // Drop by id — must clean up the NEW name (4100), not the stale old (4000).
    let removed = reg.remove_by_id(40).await;
    assert!(removed.is_some(), "backend must be removed");

    // Neither name resolves now: no dangling entry under the new id.
    assert!(
        reg.get_by_name(4100).await.is_none(),
        "remove_by_id after rename must unlink the NEW name (no dangling by_name entry)"
    );
    assert!(reg.get_by_id(40).await.is_none());
}

// ============================================================================
// P1 (#992) — IndexRegistry::insert generation tagging must be linearizable
// ============================================================================
//
// Before the fix, `insert()` computed its per-entry generation tag as
// `generation.load(Acquire) + 1` — a classic read-then-write race. Two
// concurrent inserts that both read `generation == G` both computed `G+1`:
// the second's `fetch_max(G+1)` was a no-op, leaving `generation()` flat
// even though the second backend was published. At commit,
// `pre_commit.rs:825`'s `generation() == stage_gen` shortcut then skipped
// re-derivation ENTIRELY (not just `backends_newer_than` — the filter was
// never even called), so the tx committed with zero ops for the second
// backend (the exact "guaranteed miss" class #958/#987 exist to prevent).
//
// The fix replaces the racy `load() + 1` with a DEDICATED ticket counter
// (`insert_ticket`), decoupled from `generation`. `fetch_add` is a true
// atomic fetch-and-add, so two concurrent inserts are guaranteed DISTINCT
// tickets regardless of interleaving.
//
// These tests are property-based: they assert the invariant the fix
// guarantees (`fetch_add` uniqueness), which holds deterministically under
// the new code regardless of scheduling. A `tokio::sync::Barrier`
// synchronizes all tasks to enter `insert()` at the same instant —
// maximizing the contention that would trigger the OLD race — without any
// production-code pause-hook seam. A multi-threaded runtime gives true
// parallelism so the race window is exercised, not just simulated.

/// #992: N concurrent `insert()` calls must receive DISTINCT generation
/// tags. Before the fix, two inserts that raced on `generation.load()`
/// both computed the same `my_gen`, so both published with that tag and the
/// second's `fetch_max` was a no-op.
///
/// This is a property-based test: it asserts the invariant the fix
/// guarantees (`fetch_add` uniqueness → distinct tags), which holds
/// deterministically under the new code regardless of scheduling. A
/// `tokio::sync::Barrier` synchronizes all tasks to enter `insert()`
/// simultaneously, maximizing the contention that would trigger the old
/// race — making the test a strong regression detector without needing a
/// production-code pause-hook seam.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_inserts_get_distinct_generation_tags() {
    let reg = Arc::new(IndexRegistry::new());
    const N: u32 = 32;

    // Synchronize all tasks so they enter `insert()` at the same instant —
    // maximizes the chance that multiple tasks compute `my_gen` before any
    // `fetch_max` lands (the exact interleaving the old code raced on).
    let barrier = Arc::new(tokio::sync::Barrier::new(N as usize));
    let mut handles = Vec::new();
    for id in 1..=N {
        let reg = reg.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            reg.insert(make(id, id as u64 * 10)).await.unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    // Read back each entry's gen tag via the test-only accessor and assert
    // all are distinct. `fetch_add` guarantees distinct return values, so
    // the set of gens is exactly {1, 2, ..., N}.
    let mut gens = Vec::new();
    for id in 1..=N {
        gens.push(
            reg.entry_gen(id)
                .await
                .expect("backend must be registered after concurrent insert"),
        );
    }
    let distinct: BTreeSet<u64> = gens.iter().copied().collect();
    assert_eq!(
        distinct.len(),
        N as usize,
        "#992: concurrent inserts must receive DISTINCT generation tags; got {gens:?}"
    );

    // Cross-check via the public `backends_newer_than` API: with gens
    // {1, ..., N}, `backends_newer_than(t)` returns exactly N-t backends
    // for every threshold t in 0..=N. If any two gens collided (old bug),
    // this sweep would return a wider-than-expected count for some t.
    for t in 0..=N as u64 {
        let got = reg.backends_newer_than(t).await.len() as u64;
        let expected = N as u64 - t;
        assert_eq!(
            got, expected,
            "#992: backends_newer_than({t}) returned {got} backends, expected {expected} \
             (gens must be the contiguous set {{1..={N}}})"
        );
    }
}

/// #992: N concurrent `insert()` calls must advance `generation()` by
/// exactly N. Before the fix, concurrent inserts that raced on
/// `generation.load()` computed the same `my_gen`, so the second's
/// `fetch_max` was a no-op — `generation()` could stay flat even though a
/// new backend was published. At commit, `pre_commit.rs:825`'s
/// `generation() == stage_gen` shortcut then skipped re-derivation
/// entirely. This test asserts the invariant directly.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_inserts_advance_generation_by_exactly_n() {
    let reg = Arc::new(IndexRegistry::new());
    const N: u32 = 32;
    let gen_before = reg.generation();

    let barrier = Arc::new(tokio::sync::Barrier::new(N as usize));
    let mut handles = Vec::new();
    for id in 1..=N {
        let reg = reg.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            reg.insert(make(id, id as u64 * 10)).await.unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    let gen_after = reg.generation();
    assert_eq!(
        gen_after - gen_before,
        N as u64,
        "#992: {N} concurrent inserts must advance generation() by exactly {N} \
         (got {gen_before} -> {gen_after}); a smaller delta means some inserts \
         raced on the old `generation.load() + 1` and computed the same tag"
    );
}
