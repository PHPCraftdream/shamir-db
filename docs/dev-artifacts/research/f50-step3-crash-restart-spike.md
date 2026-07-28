# F-50 Step 3a — crash/restart continuation design spike (#871, P0)

**Status:** spike complete. Three design questions settled, the trickiest
mechanism (forward-compat serialization of a new `state` field on
`IndexDescriptor`) PROVEN by a real round-trip test with actual output, and a
production-ready prototype of the type + its safe load path committed alongside
this memo for Step 3b to build on.

- **Forward-compat mechanism — DECIDED:** `#[serde(default)]` does **NOT** work
  for a new trailing field under this crate's bincode 1.3.3 setup (proven — see
  §1). Forward-compat is provided by a **try-current-shape-then-fallback-to-a
  pre-`state` shadow shape** in `persistence::load_index2_metadata`, lifting
  every legacy descriptor to `state = Ready`.
- **Crash-restart continuation — DECIDED:** **restart-from-scratch** (drop the
  partial backend/postings under the reserved id, redo the backfill). Resume is
  over-engineering for a rare, operator-driven DDL path with a checkpoint-less
  backfill; restart is trivially correct (§2).
- **Persist point — DECIDED:** `Building` **piggybacks on the existing first
  `save_index2_metadata` call** (`table_manager_index_mgmt.rs:118-119`, already
  there for the #534 id-reuse fix). It does NOT need a third persist point, but
  it DOES need Step 3b to make the in-flight descriptor visible to that persist
  (the descriptor is not in `all_descriptors()` yet today — §3).
- **Doctor extension — DESIGNED (not implemented):** a new `index2_backends`
  health section in `VerifyReport` that reports each backend's `state` and
  flags any stuck in `Building`. `repair()` is NOT the primary recovery path
  (the open path is) but can optionally re-trigger it (§4).
- **Step 3b plan:** exact touch points in §5.

---

## 1. Forward-compat serialization mechanism (settled FIRST — it gates the rest)

### 1.1 The question

`IndexDescriptor` (`crates/shamir-index/src/descriptor.rs`) is bincode-serialized
inside `MetaEnvelope<T>` (`meta_envelope.rs`, magic `SDB2`, **envelope
`version = 1`**). Adding a new `state: IndexState` field to `IndexDescriptor`
must not corrupt every existing on-disk descriptor the first time the field is
written. The brief flagged that `VectorConfig::quantization`'s doc comment
(`kind.rs:172-186`) already warned "bincode 1.3.3 does NOT honour
`#[serde(default)]` for skipped fields on read" — but that finding was about
`#[serde(skip)]` specifically, and it was **not** confirmed for the *different*
`#[serde(default)]` annotation (the one already used on `options`) against
genuinely OLD bytes.

### 1.2 The proof (actual test output)

A round-trip test (`crates/shamir-index/src/tests/index_state_compat_tests.rs`)
hand-encodes the OLD shape (a byte-faithful shadow struct matching the
pre-`state` `IndexDescriptor` field-for-field) and attempts to read it back as
the NEW shape (old + a trailing `#[serde(default)] state: SpikeState`).

**Result (bincode 1.3.3, the workspace's pinned version):**

```
serde_default_does_not_rescue_trailing_field ... ok
```

asserting:

```rust
let err = bincode::deserialize::<NewDescriptor>(&old_bytes).unwrap_err();
assert!(err.to_string().contains("unexpected end of file"), ...);
```

The raw probe (before it was rewritten as a clean assertion) printed the
decisive diagnostic:

```
SERDE_DEFAULT_FAILS: #[serde(default)] did NOT rescue the trailing `state` field.
bincode error: Io(Kind(UnexpectedEof)) (display: io error: unexpected end of file).
(bytes len = 92)
```

**Why:** bincode is a non-self-describing, positional format. Its struct
deserializer consumes a value for EVERY declared field in declaration order;
there are no field tags and no trailing terminator. `#[serde(default)]` only
fires when serde's derived visitor is told a field is *missing* — which a
self-describing format (JSON) signals explicitly but bincode never does. So a
trailing field absent from old bytes is read past EOF → `ErrorKind::Io(
UnexpectedEof)`. This is the **same** failure `VectorConfig::quantization`
flagged for `#[serde(skip)]`; this spike confirms it equally dooms
`#[serde(default)]`. The `options` field's existing `#[serde(default)]` has
simply never been exercised against real pre-`options` bytes — it is
forward-compat theatre, not a working mechanism.

### 1.3 The mechanism — try-current-then-fallback-legacy (DECIDED)

Since `#[serde(default)]` is a no-op here, forward-compat is delivered by the
LOAD path. `persistence::load_index2_metadata` (via the new helper
`decode_persisted_indexes`) tries the current (with-`state`) shape first; on a
`MetaError::Decode` it retries with a byte-faithful pre-`state` shadow shape
(`PersistedIndexesNoState` / `IndexDescriptorNoState`) and lifts each legacy
descriptor to `state = Ready`:

```rust
match MetaEnvelope::<PersistedIndexes>::open(bytes) {
    Ok(p) => Ok(p),
    Err(MetaError::Decode(new_err)) => {
        match MetaEnvelope::<forward_compat::PersistedIndexesNoState>::open(bytes) {
            Ok(legacy) => {
                log::warn!("index2 metadata: decoded with pre-`state` legacy fallback ...");
                Ok(PersistedIndexes::from(legacy))   // each desc → state = Ready
            }
            Err(legacy_err) => Err( /* neither shape — genuine corruption */ ),
        }
    }
    Err(e) => Err(e.into()),  // bad magic / unsupported version
}
```

**Why not bump `MetaEnvelope`'s `version`?** That is the mechanism the envelope
was *designed* for (`meta_envelope.rs:4-5`: "so future migrations can dispatch
on `version`"), but `ENVELOPE_VERSION` is a **shared** constant and
`MetaEnvelope::open` **strictly rejects** `version != ENVELOPE_VERSION`. A bump
would force ALL ~6 other `MetaEnvelope` consumers (recovery marker `u64`,
`PersistedValidators`, vector `SnapshotSidecar`/`SnapshotManifest`/`Vec<DeltaOp>`,
engine `MetaEnvelope` re-export, WAL) to grow a v1→v2 branch even though their
payload shapes did not change — large blast radius for a one-field index change.
The per-payload try/fallback is localized to `load_index2_metadata` and touches
nothing else.

**Soundness of the fallback ordering.** `try-new-first` means a NEW blob never
reaches the legacy path (its `state` is preserved exactly — proven by
`decode_preserves_explicit_building_state`). The legacy path is only reached
for a blob that fails the new shape; the pre-`state` shadow is a strict *prefix*
of the new shape, so a genuine pre-`state` blob decodes cleanly as legacy. A
blob that decodes as *neither* (genuine corruption) surfaces an error combining
both diagnostics — proven by `decode_corrupt_blob_errors`. The one theoretical
false-positive (a corrupt NEW blob that happens to be valid-as-legacy and gets
silently lifted to `Ready`) is strictly better than hard-failing on old blobs
and is vanishingly unlikely in practice; it is accepted and documented.

### 1.4 Test output (final, clean assertions)

```
./scripts/test.sh -p shamir-index -- index_state_compat
        PASS  decode_preserves_explicit_building_state
        PASS  new_shape_round_trips_with_itself
        PASS  decode_corrupt_blob_errors
        PASS  decode_loads_pre_state_blob_as_ready
        PASS  serde_default_does_not_rescue_trailing_field
     Summary  5 tests run: 5 passed, 506 skipped
exit=0
```

---

## 2. Crash-restart continuation — RESTART-FROM-SCRATCH (DECIDED)

### 2.1 The backfill's actual shape

`backfill_index2_backend` (`table_manager_index_mgmt.rs:419-439`) is a
**checkpoint-less full scan**:

```rust
let stream = self.list_stream(1000);        // batched full-table scan
while let Some(batch) = stream.next().await {
    for (rid, cow) in batch {
        let ops = backend.plan_insert(rid, &val).await?;
        apply_index_ops(&ops, &self.info_store, backend).await?;
    }
}
```

No cursor is persisted. No "last processed RecordId" bookmark exists. The
`plan_insert` + `apply_index_ops` per-row writes are not guaranteed idempotent
across backends (FTS `BumpFtsStats` is a counter; HNSW insertion is not a
pure set-union), so a naive *resume* would need per-backend idempotency
guarantees in addition to a checkpoint.

### 2.2 The decision + reasoning

**Restart-from-scratch.** On finding a `Building` descriptor at open time:
keep the reserved id (its `next_id` watermark is already durable from the first
`save_index2_metadata` — #534 finding 2), **drop the partial backend/postings**
under that id via the existing `IndexBackend::drop_all()` primitive, then redo
`create_index_v2`'s build sequence (construct backend, `backfill_index2_backend`,
flip to `Ready`, persist).

Rejected alternatives:

- **Resume the backfill** — needs a persisted cursor (new durable state, written
  per-batch or per-row to be crash-safe) PLUS a `list_stream` variant that
  starts from a given `RecordId` (range-resume) PLUS per-backend idempotency for
  the rows straddling the crash point. Real complexity for a path that runs
  once per `CREATE INDEX`. Index builds are rare, operator-driven DDL, not a hot
  path — the engineering cost is unjustified.
- **Leave as a permanently-`Building` orphan for the doctor** — leaves the index
  silently unusable on every restart until an operator notices and runs
  `repair`. Acceptable as a *diagnostic* layer (the doctor reports it — §4) but
  not as the primary recovery, because the symptom ("my index quietly stopped
  working after the crash") is exactly the silent-failure class this whole
  campaign exists to eliminate.

Restart-from-scratch is **trivially correct**: the `Building` backend is not
registered with the planner (Step 3b's Ready-gate, §5.6), so no reader can
depend on its partial postings; dropping them is safe. The orphan-posting
problem (postings a concurrent tx wrote via F-50 Step 1/2's Phase 2.7
re-derivation under the reserved id, which then vanish on restart) is handled
by the same drop — those postings are cleaned along with the backfill's
partial output, and the re-backfill reconstructs a complete, consistent index.

The cost — re-doing one O(N) scan — is negligible for a rare DDL event and is
strictly less work than the scan that was already in progress when the crash
hit.

### 2.3 Where the restart runs

In the **table-open path** (`table_manager.rs:296-310`'s
`load_index2_metadata` consumer), right after descriptors are loaded and
backends are reconstructed but BEFORE `restore_on_open`. A `Building`
descriptor is detected, its backend built, `drop_all()` called, the backfill
re-run, the state flipped to `Ready`, and the result re-persisted. This makes
a crash-restart **self-healing** — no operator action needed. The doctor (§4)
is the *observability + manual* layer on top.

---

## 3. Persist-point — piggyback on the FIRST save (DECIDED)

### 3.1 Why the existing first save is the right point

`create_index_v2`'s sequence today:

| step | line | action |
|---|---|---|
| 1 | `:78` | acquire `unique_write_lock` (whole sequence) |
| 2 | `:82` | set `index2_create_barrier` (RAII) |
| 3 | `:100` | `allocate_id()` (in-memory only) |
| 4 | `:118-119` | **FIRST** `save_index2_metadata` — persists the `next_id` watermark (#534 finding 2) |
| 5 | `:130-297` | build descriptor + backend (inside the `index_type` match) |
| 6 | `:311` | `backfill_index2_backend` |
| 7 | `:322-325` | `index2_registry.insert` — backend goes LIVE |
| 8 | `:327-328` | **FINAL** `save_index2_metadata` — persists the new descriptor |

The crash-window this spike closes is **between step 6 and step 8**: the
backend is live in the crashed process's registry (concurrent tx's got correct
postings via F-50 Step 1/2 re-derivation), but step 8 never ran, so a restart's
`load_index2_metadata` sees nothing for this index → it silently vanishes.

For a restart to *detect* the interrupted build, the `Building` descriptor must
be **on disk before step 6**. The existing first save (step 4) is the natural
persist point — it already runs pre-backfill, for the id-reuse fix. **`Building`
piggybacks on it; no third persist point is needed.**

### 3.2 The catch (Step 3b must resolve it)

The first save persists `registry.all_descriptors()` — but at step 4 the new
backend is **not yet in the registry** (`insert` is step 7), so
`all_descriptors()` returns only the PRE-existing set. The new `Building`
descriptor is invisible to that persist today. Step 3b has two ways to make it
visible (§5.4), both acceptable:

- **(A) Surgical — manual inclusion:** give `save_index2_metadata` (or a
  `_with_pending` variant) an `Option<IndexDescriptor>` for the in-flight
  `Building` descriptor; the first save writes `[all_descriptors() ∪ {Building
  desc}]`. Preserves the current backfill-before-register invariant; the
  backend stays out of the registry (and thus out of the live write-hook's
  routing) until step 7.
- **(B) Architectural — early registration in `Building` state:** insert the
  backend into the registry at step 4 in `Building` state, so
  `all_descriptors()` naturally carries it; the planner Ready-gate (§5.6) keeps
  it invisible to reads. Concurrent writes during backfill then route directly
  to the backend via the live hook (which is *correct* — they should be
  indexed). Larger semantic change; interacts with F-50 Step 1/2's
  generation-gate (the brief forbids touching that mechanism, so this option
  must be validated not to make it redundant).

**Recommendation: (A).** It is the smaller change, preserves the invariant the
current architecture depends on (the live `index2_on_insert` hook cannot route
to an unregistered backend — `backfill_index2_backend`'s own doc comment relies
on this), and avoids any interaction with the landed generation-gate.

### 3.3 The Building→Ready flip

The descriptor is owned immutably by its backend (`IndexBackend::descriptor()`
returns `&IndexDescriptor`; `all_descriptors()` clones it). Flipping
`Building→Ready` between step 6 and step 8 therefore needs a mechanism Step 3b
must pick (§5.5). The leading option, matching the registry's existing
generation-tag pattern, is to track the authoritative `state` in the
`IndexRegistry::by_id` tuple `(Arc<dyn IndexBackend>, u64 gen, IndexState)`,
with `all_descriptors()` merging the tuple's state into the cloned descriptor
at persist time and a `registry.set_state(id, state)` for `create_index_v2` to
call. This keeps `IndexDescriptor.state` a pure serialization carrier and
centralizes state in the lock-free registry (no per-backend interior
mutability).

---

## 4. Doctor extension — DESIGNED (not implemented)

The doctor (`crates/shamir-engine/src/table/doctor.rs`) only knows legacy
regular/unique/sorted indexes today. The minimal addition for index2:

### 4.1 `verify()` — detection + reporting

Add an `index2_backends: Vec<Index2Health>` field to `VerifyReport`, where:

```rust
pub struct Index2Health {
    pub id: u32,
    pub name: String,
    pub state: IndexState,       // Ready | Building
    pub healthy: bool,           // false iff state == Building
}
```

`verify()` gains a loop over `self.index2_registry.all_backends()` (the doctor
needs a handle to `index2_registry`, which `TableManager` already holds) that
records each backend's `state` and marks `Building` ones unhealthy with a clear
message ("index2 backend '{name}' (id={id}) is in Building state — build was
interrupted; reopen the table or run repair"). `is_healthy()` ANDs this into the
overall report. This is the *observability* layer — it makes a stuck-`Building`
index visible to an operator audit without requiring a restart.

### 4.2 `repair()` — NOT the primary recovery path

The primary recovery is the self-healing open path (§2.3). `repair()` is
OPTIONALLY extended to re-trigger the same restart-from-scratch logic for a
`Building` backend (drop + re-backfill + flip `Ready` + re-persist), for
operators who want to fix a stuck index without a full table reopen. This is a
thin wrapper over the open-path restart routine, not new logic. It is
out-of-scope for this spike and listed in the Step 3b plan (§5.7) as optional.

### 4.3 Why not make the doctor the *only* recovery path

Because the symptom of an unrepaired `Building` index is "queries silently miss
every row" — exactly the silent-failure class F-50 exists to eliminate. Forcing
an operator to notice + run `repair` reintroduces that silent failure. The
open-path self-heal (§2.3) is the correct default; the doctor is the
belt-and-suspenders visibility layer.

---

## 5. Step 3b implementation plan (exact touch points)

Items 1–3 are **already landed by this spike** (the prototype); items 4–8 are
Step 3b.

### Already landed (this spike)

1. **`crates/shamir-index/src/state.rs` (NEW)** — `IndexState` enum
   (`Ready` default, `Building`), serde, re-exported from `lib.rs`.
2. **`crates/shamir-index/src/descriptor.rs`** — `pub state: IndexState` field
   added (with `#[serde(default)]` for JSON/msgpack friendliness and
   consistency with `options`; **documented** that bincode forward-compat comes
   from the load path, NOT serde default). `IndexDescriptor::new` sets
   `state: IndexState::default()` (Ready).
3. **`crates/shamir-index/src/persistence.rs`** — `forward_compat` module
   (`PersistedIndexesNoState` / `IndexDescriptorNoState` shadow shapes),
   `From<PersistedIndexesNoState> for PersistedIndexes` (lifts to Ready), and
   `decode_persisted_indexes` (the try-current-then-fallback-legacy load path)
   wired into `load_index2_metadata`.

### Step 3b

4. **`table_manager_index_mgmt.rs::create_index_v2`** — construct the descriptor
   with `state = Building`; persist it at the first `save_index2_metadata`
   (`:118-119`) via mechanism (A) from §3.2 (`save_index2_metadata_with_pending`
   or an `Option<IndexDescriptor>` arg). After backfill + register, flip to
   `Ready` (mechanism §3.3/§5.5) before the final save (`:327-328`).
5. **`registry.rs`** — track authoritative `state` in the `by_id` tuple
   `(Arc<dyn IndexBackend>, u64 gen, IndexState)`; add `set_state(id, state)`;
   `all_descriptors()` merges the tuple state into the cloned descriptor.
   (Alternative: per-backend interior mutability — rejected as more invasive.)
6. **`read_planner.rs::try_plan_index2`** — add a `state == Ready` gate so a
   `Building` backend is invisible to the planner (the planner Ready-gate
   scaffold from Step 1 memo §5.1.E, now backed by the persisted field).
7. **`table_manager.rs` open path (`:296-310`)** — after loading descriptors,
   for each `Building` one: build the backend, `drop_all()`, re-run
   `backfill_index2_backend`, flip `Ready`, re-persist. (The self-healing
   restart-from-scratch of §2.3.)
8. **`doctor.rs`** — `Index2Health` + `index2_backends` in `VerifyReport`;
   `verify()` loop over `index2_registry.all_backends()`. Optional `repair()`
   re-trigger of the open-path restart.
9. **Tests** — crash/restart simulation: persist a `Building` descriptor,
   reopen the table, assert the backend is re-backfilled and reaches `Ready`
   and is queryable; planner Ready-gate test (a `Building` backend is invisible
   to reads); doctor `Building`-detection test.
10. **`KNOWN_LIMITATIONS.md`** — document the closed crash-restart gap and the
    restart-from-scratch choice (re-does O(N) backfill work on crash-resume).

### Explicitly OUT of scope for Step 3b (per the brief)

- **DDL cancellation** — deferred until #872 (DROP INDEX for index2) lands a
  real cancel path. Today no user can issue a cancel/drop against an in-progress
  or completed index2 build, so "cancel mid-build" is unreachable.
- **Touching F-50 Step 1/2's generation-gate** (`IndexRegistry::generation`,
  `rederive_index2_ops_post_stage`, `SortedIndexManager::generation`) — unrelated.

---

## 6. What was prototyped (committed alongside this memo)

Files changed:

- **`crates/shamir-index/src/state.rs` (NEW)** — `IndexState` enum (Ready/Building).
- **`crates/shamir-index/src/descriptor.rs`** — added the `state` field +
  forward-compat rationale comment; `::new` defaults it to `Ready`.
- **`crates/shamir-index/src/persistence.rs`** — `forward_compat` shadow-shape
  module, `From` lift, and `decode_persisted_indexes` (try-current-then-fallback)
  wired into `load_index2_metadata` with a `log::warn!` on legacy fallback.
- **`crates/shamir-index/src/lib.rs`** — `pub mod state;` + `pub use state::IndexState;`.
- **`crates/shamir-index/src/tests/mod.rs`** — register
  `index_state_compat_tests`.
- **`crates/shamir-index/src/tests/index_state_compat_tests.rs` (NEW)** — 5
  tests: the `#[serde(default)]` disproof, the new-shape self round-trip
  control, the legacy-blob→Ready integration test through the REAL
  `decode_persisted_indexes`, the explicit-`Building` preservation test, and the
  corrupt-blob-errors test.
- **`crates/shamir-engine/src/table/tests/has_any_index_tests.rs`** — the one
  literal `IndexDescriptor { ... }` construction updated to set
  `state: IndexState::default()`.

The prototype does NOT wire the `Building`-at-start / `Ready`-at-finish state
machine into `create_index_v2`, does NOT implement the doctor extension, and
does NOT implement the restart-on-open logic — those are Step 3b. It proves
the type, its serialization round-trip, and the safe load path.

---

## 7. Exact commands + verification (run 2026-07-30)

```
cargo fmt -p shamir-index -- --check                                    # exit=0
cargo clippy --workspace --all-targets -- -D warnings                   # exit=0
./scripts/test.sh -p shamir-index -- index_state_compat                 # 5/5 pass
./scripts/test.sh -p shamir-index --full                                # 511/511 pass
./scripts/test.sh -p shamir-engine -- has_any_index                     # 7/7 pass
./scripts/test.sh -p shamir-engine -- index2_create_barrier             # 7/7 pass
```

`--full` was used for shamir-index (the brief's verification scope). The
shamir-engine scopes confirm the descriptor-field addition causes no regression
in the two test files most sensitive to it (the literal-construction site and
the index2 barrier suite that exercises `create_index_v2` end-to-end).

---

## 8. Decision summary

| Question | Decision | Rationale |
|---|---|---|
| Forward-compat for a new `state` field | **try-current-then-fallback-legacy** in `load_index2_metadata`; `#[serde(default)]` is a no-op under bincode 1.3.3 (PROVEN) | bincode reads fields positionally with no terminator; a trailing field absent from old bytes hits `UnexpectedEof`. `#[serde(default)]` never fires. Bumping the shared `MetaEnvelope` version would force v1→v2 branches on ~6 unrelated payloads. |
| Crash-restart continuation | **restart-from-scratch** (drop + re-backfill) on table open | Backfill is checkpoint-less; resume needs a persisted cursor + range-resume stream + per-backend idempotency — unjustified for rare DDL. Restart is trivially correct (Building backend is planner-invisible, so its partial postings are safely droppable). |
| Persist point | **piggyback on the existing first `save_index2_metadata`** (`:118-119`); no third persist point | It already runs pre-backfill. Catch: the in-flight descriptor is not in `all_descriptors()` yet — Step 3b makes it visible via a `pending: Option<IndexDescriptor>` arg (surgical), preserving the backfill-before-register invariant. |
| Doctor extension | **DESIGNED only** — `index2_backends` health section in `VerifyReport` flags `Building` backends; `repair()` optionally re-triggers the open-path restart | The open path is the primary (self-healing) recovery; the doctor is the visibility + manual layer. Making the doctor the *only* path reintroduces the silent-miss symptom F-50 exists to kill. |
