# P0 stage A5b — p0-index-compaction-cutover

## Goal

Card `RV-INDEX-9F62C8DF` (compaction double-write errors silently discarded; an incomplete graph is swapped in as live) in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` (read it whole). Code: `crates/shamir-index/src/vector/vector_backend/{backend.rs,maintenance.rs}`, `hnsw_adapter/`. This stage was split out of A5 after five failed executor rounds; the DESIGN IS DECIDED and BINDING (two rush reviewers + @oh): implement exactly it.

Hard-won lessons from the failed attempts (violating any of them failed review): (1) test hooks must be FIRED BY THE REAL WRITER/COORDINATOR PATH — a test that awaits a hook the production code never fires deadlocks; every park hook must keep its release sender alive in the test harness; (2) a coordinator future must be polled/spawned before the test awaits an event it produces — never await an unpolled future; (3) the snapshot after compaction must keep the maintenance ownership alive for its whole lifetime; (4) a target that failed must never be published; the failure test must drive the real coordinator, not a path that discards the failed target before the check; (5) historical-delete reconciliation must not remove successful re-insertions; (6) no sleeps, no yield loops as synchronization.

## Binding design

@oh concrete design (agreed by both reviewers):
- AdapterSlot { adapter, target: Option<Arc<CompactionTarget>>, gate: WriteGate } replaces the separate compaction_target field (one RCU load per write). WriteGate { writers: AtomicUsize, sealed: AtomicBool, drained: Notify }; CompactionTarget { adapter, failed: AtomicBool, reason: OnceLock<String> }; VectorBackend gets `published: Notify`. Put the new types in new files under vector_backend/ (backend.rs is at 1000 lines; tests go in tests/lifecycle/compaction_tests/).
- Writer: enter_write() loads the slot and takes a lease (fetch_add SeqCst then check sealed SeqCst; if sealed, exit and await `published`); the lease is held across BOTH the primary write and the target mirror. Primary write is authoritative and returns its error; the mirror NEVER fails the user's write (promote_vectors runs after the WAL is durable) - on target error it sets reason + failed (Release). Lease drop: fetch_sub; if last and sealed, notify drained. No nested enter_write while holding a lease (apply_staged_vector_deletes takes one lease for its loop).
- Coordinator: seal_and_drain(slot) = sealed.store(SeqCst) then await writers==0 via drained notify; publish(old,new) = compare_and_swap on adapter_swap + published.notify_waiters(). Arm: seal_and_drain(g0), publish g1 = {g0.adapter, Some(target), fresh gate}. Capture live set S0 AFTER that publish. Backfill error => disarm (seal, drain, publish slot without target) and Err; reconcile error => target.fail. Cutover: seal_and_drain(g1); if target.failed => publish slot {g1.adapter, None} and return Err (compaction retried later); else publish {new, None}. Snapshot after. Between seal and publish only the drain and a CAS - nothing fallible.
- Reviewers' extra rule: overlapping mutations to the SAME RID inside one generation window mark the target failed (ordering ambiguity) unless ordering is proven.
- Remove: let _ = on any target/reconcile result, the post-hoc latch, epoch revalidation, in-flight target-write counters, teardown deadlines.
- Tests (tests/lifecycle/compaction_tests/cutover_tests.rs): #[cfg(test)] hook points as oneshot pairs (writer after primary before mirror; writer waiting at gate; after arm publish; after seal) + fail_next_mirror injection; no sleeps/yields. T1 writer parked mid-mutation while coordinator seals: live slot unchanged until release, new primary contains the write. T2 injected mirror failure: coordinator Err, live stays old with the write, no snapshot. T3 writer in g0 during arm drain ends in new primary. T4 writer arriving while sealed completes on new. T5 failure at arm then retry succeeds. T6 target failure during plan_insert/apply_staged_vectors returns Ok to the caller. T1/T2 must be red against HEAD. Adapt basic.rs constructors.
- Keep your already-accepted fixes for RV-INDEX-2F3CBCA3, -4651AA00, -86978429 but make sure every last-review finding about their tests is addressed.

## Verdict — approve **(a) with mandatory fail-closed policy (b)**; reject a latch-only fix and defer (c)

Use a **generation-scoped mutation lease covering the entire primary-plus-target operation**, with atomic admission/cutover and **abort-on-any-uncertainty**. Publication must linearize against writer registration—not against a separately checked error flag.

Additionally, **abort the generation on overlapping mutations to the same RID**, unless the implementation supplies a proved version-ordering protocol. Draining successful double-writes alone does not guarantee that primary and target applied concurrent updates in the same order.

This is a design recommendation, **not runtime verification**. The references below are observations of source; interleavings and guarantees are static deductions.

### 1. What the current code actually establishes

**OBSERVED:**

- All six target-write branches discard adapter errors:
  - Nontransactional insert: `crates/shamir-index/src/vector/vector_backend/backend.rs:295`.
  - Update/upsert: `crates/shamir-index/src/vector/vector_backend/backend.rs:316`.
  - Update removing the vector: `crates/shamir-index/src/vector/vector_backend/backend.rs:353`.
  - Delete: `crates/shamir-index/src/vector/vector_backend/backend.rs:382`.
  - Committed vector batch: `crates/shamir-index/src/vector/vector_backend/backend.rs:511`.
  - Committed deletes: `crates/shamir-index/src/vector/vector_backend/backend.rs:547`.
- Reconciliation also discards delete errors at `crates/shamir-index/src/vector/vector_backend/maintenance.rs:247`.
- Target registration, live-set collection, publication and deregistration are separate operations at `maintenance.rs:223`, `maintenance.rs:228`, `maintenance.rs:252`, and `maintenance.rs:257`.
- **There is no existing backend swap guard in this implementation.** Step 5 is an unconditional `ArcSwap::store`, not a writer-exclusion boundary: `maintenance.rs:251`.
- An early backfill error returns before clearing `compaction_target`: `maintenance.rs:234–237`. The existing flight guard resets only the single-flight boolean: `maintenance.rs:121–127`.
- The requested card is still `confirmed-open`: `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md:1279–1295`.

**INFERRED counterexamples that the design must exclude:**

1. A writer captures the old primary, pauses, then resumes after publication and target clearing. Its successful primary mutation can affect only the retired graph.
2. Two writers apply A then B to the primary, but B then A to the target. Every call succeeds and the counter drains, yet the published target contains A rather than B.
3. Delete followed by reinsertion can be undone by blanket reconciliation of historical deleted RIDs. The set records “deleted sometime,” not “currently absent”: recording occurs at `backend.rs:379`; reconciliation deletes every recorded RID at `maintenance.rs:240–247`.

Thus **“no target errors” is necessary but insufficient**.

### 2. Chosen invariant

**PROPOSED invariant:**

At the atomic publication point for generation G:

1. No mutation lease admitted against G’s old primary remains outstanding.
2. No new lease can subsequently select that old primary.
3. Every mutation completed during G’s build interval either:
   - completed successfully on both graphs in compatible per-RID order; or
   - permanently invalidated G, making publication impossible.
4. Backfill and all graph-building/quantization work required for publication have completed successfully.
5. Historical tombstones cannot override a later successful reinsertion.

A lease starts **before selecting or mutating the primary** and ends **after target completion or generation invalidation**. Registering only the target call is too late.

A failed target mutation does **not** turn an already successful primary mutation into a purported transaction failure. It invalidates the maintenance generation; the old primary remains authoritative.

### 3. Concrete protocol and pseudocode

The following is conceptual pseudocode, not a proposed public Rust API.

#### A. Replace independent routing decisions with one atomic admission/publication domain

Apply this at the routing fields in `backend.rs:71` and `backend.rs:119–121`, and the writer entry points beginning at `backend.rs:281`, `backend.rs:502`, and `backend.rs:529`.

```text
Routing state:
    generation
    phase = Idle | Building | Invalid | Published
    active_mutations
    immutable descriptor owning primary and optional target

register_mutation():
    atomically select current generation/primary and increment its count
    return owning lease

publish_if_quiescent(G):
    atomically:
        require current generation == G
        require phase == Building
        require active_mutations == 0
        transition routing to target-as-primary
```

**The selection, registration and publication predicate must share one linearizable CAS protocol.** They must not be independently sampled `ArcSwap` pointers plus a counter.

A packed atomic control word with stable generation descriptors is one possible representation. Descriptor reclamation must retain ownership for outstanding leases. An auxiliary `adapter` pointer may remain for readers, but **writers must not select their primary through an unsynchronized mirror**.

This atomic transition is the proposed **swap guard**. It is not a global mutex held over graph work.

#### B. Cover idle writers and establish the start boundary

Replace Step 2 at `maintenance.rs:222`.

```text
create target privately

try atomically transition:
    Idle(old_primary, active = 0)
      -> Building(G, old_primary, target, active = 0)

if transition cannot be established within maintenance budget:
    discard private target; retry on a later trigger

capture S0 from old_primary
backfill target
```

An idle writer admitted before arming either completes before arming or prevents that arming attempt. It cannot escape both S0 and double-write registration.

Do not install a target while previously admitted, untracked old-primary mutations remain in flight. S0 is not a substitute: `collect_live_vectors` scans several maps and omits entries whose vector has not yet appeared, at `crates/shamir-index/src/vector/hnsw_adapter/fit.rs:36–47`.

#### C. One mutation helper, used by every branch

```text
mutate(operation):
    lease = register_mutation()       // before primary load or side effect
    G = lease.generation

    if G is Building:
        claim each affected RID in G's nonblocking active-RID set
        if any RID is already claimed:
            invalidate G permanently
            continue on primary without waiting for that RID

    result = apply operation to lease.primary

    if result failed or execution became uncertain:
        invalidate G if building
        return primary error

    if G is still eligible:
        apply operation to lease.target
        if target failed or execution became uncertain:
            invalidate G permanently

    finish lease                     // terminal outcome before count release
    return primary success
```

Required details:

- The RID claim spans **both** primary and target work. A collision aborts compaction, not the mutation; it adds no per-RID waiting.
- For batches, claim distinct RIDs before effects, and keep claims until the batch terminates. Batch failures may be partial; treat them as generation-invalidating.
- Failure/invalidation must happen-before the final lease-count release; publication uses the corresponding acquire synchronization.
- An unfinished lease dropped on cancellation or unwind invalidates the generation **before** unregistering.
- An invalidated generation may stop receiving target writes immediately. It can never become eligible again.
- A target-operation budget may cancel the target future and invalidate G. Detached blocking work must retain the discarded target privately; it cannot regain publication eligibility.

This helper replaces—not supplements—the six duplicated branches listed above. Rebuild/startup methods need explicit lifecycle exclusion or routing participation; `backend.rs:591–596` must not become an untracked online mutation escape hatch.

The raw mutable adapter returned by `backend.rs:153–155` also needs an explicit boundary: mutations through that handle cannot be covered by backend leases. Return a generation-aware mutation facade or prohibit such mutation while attached to a live backend. Existing tests do mutate it directly, for example `crates/shamir-engine/src/table/tests/read/vector/snapshot_vector_tests.rs:265`.

#### D. Finish backfill; remove unconditional historical-delete reconciliation

At `maintenance.rs:230–249`:

```text
await all backfill work
on any failure: invalidate G; abort

do not delete every historically deleted RID
```

The backfill protocol already checks deleted-RID markers before and under the RID entry claim: `hnsw_adapter/fit.rs:79–83` and `fit.rs:108–118`.

**INFERRED:** With successful ordered double-writes, completed backfill and drained mutation leases, those markers can suppress stale S0 insertion without a final blanket delete. A later reinsertion must survive. If reconciliation is retained, it must use versioned latest state—not the current historical set.

Any adapter fit path that can leave partial searchable state must propagate failure or otherwise invalidate G. The backend cannot detect an adapter failure converted to `Ok`: best-effort fit calls exist at `hnsw_adapter/vector_impl.rs:199–203` and `hnsw_adapter/fit.rs:273–277`. Their claimed safe fallback must be established before relying on it.

#### E. Drain without blocking writers, then publish atomically

Replace `maintenance.rs:251–257`.

```text
after backfill completes:
    wait asynchronously for a quiescent opportunity

    attempt atomic publish_if_quiescent(G)
    if admitted writer wins the race:
        retry publication within maintenance budget
    if invalid, cancelled, or maintenance budget exhausted:
        invalidate G; detach target; retain old primary; return Aborted

    on successful publication:
        retire G's compaction metadata
        schedule snapshot of the published primary
```

**Do not seal admission and make writers wait for a drain.** Writers keep registering; the coordinator waits or abandons compaction. Publication and registration race through the same atomic control state, so exactly one wins.

This preserves bounded **compaction-induced** mutation overhead: registration, RID bookkeeping and at most the existing additional target operation. It does not claim a hard wall-clock bound for existing HNSW work or OS scheduling.

### 4. Why not the other designs?

| Option | Decision |
|---|---|
| **(a) Epoch + registration + drain** | **Choose**, but register entire mutations, cover the arming boundary, prevent same-RID order divergence, and publish through the admission CAS domain. |
| **(b) Abort on target failure** | **Mandatory policy within (a), insufficient alone.** A latch without admission/publication synchronization retains the check-then-publish race. |
| **(c) Mutation-log replay under swap guard** | **Defer.** Correct only with complete capture, primary-linearization ordering, cancellation handling and bounded final catch-up. An arbitrarily long replay under exclusion violates the latency requirement. |

The existing durable delta log is not such a complete ordered capture mechanism: it is appended separately at `backend.rs:754–776`, whereas nontransactional graph writes happen directly at `backend.rs:281–384`.

### 5. Post-swap snapshot obligations

**OBSERVED:** Step 7 resets accounting before snapshot success and launches another task at `maintenance.rs:259–289`. Snapshot and compaction admission use separate check/CAS sequences at `backend.rs:797–806` and `maintenance.rs:147–155`.

**RECOMMENDATION:**

- Use one maintenance-owner state for snapshot versus compaction; mutual boolean checks are not mutual exclusion.
- An aborted generation must never trigger its target snapshot, publish a manifest, prune deltas or reset accounting.
- Snapshot failure after a valid swap must not roll routing back to the old graph: later mutations may already exist only on the new graph.
- Capture/reset mutation accounting relative to a successful snapshot boundary, preserving concurrent increments—not unconditional `store(0)`.
- Durable absorption requires a completed contiguous delta boundary, not merely an allocated index. Allocation precedes persistence at `backend.rs:773–775`; snapshot captures that allocation cursor at `maintenance.rs:69`.

The epoch protocol proves the **in-memory cutover**, not independent crash-recovery correctness. Snapshot/reopen acceptance must test the latter explicitly.

### 6. Deterministic CI oracles

Exercise the **production backend and coordinator**, not a hand-written simulation. Existing stress coverage deliberately finishes workers before swapping: `crates/shamir-index/src/vector/tests/lifecycle/compaction_tests/basic.rs:391–401`.

Use instance-scoped barriers/oneshots and injected adapter failures; no sleeps or yield-based ordering.

| Forced schedule | Required oracle |
|---|---|
| Pause a writer after admission, before primary mutation; request cutover. | Publication attempt explicitly reports outstanding lease; after release, mutation survives publication or compaction aborts with old primary retained. |
| Pause after primary success, before target completion; inject target error. | Generation invalidates before lease release; target is never published. Primary contains the exact successful mutation. |
| Pause an idle-generation writer; attempt arming. | Arming cannot bypass it. After completion, a later build includes the mutation. |
| Force publication CAS versus writer-registration CAS in both orders. | Writer uses old generation and prevents publication, or uses the new primary. Never old-only success after publication. |
| Force two updates to one RID to overlap, reversing target timing. | Generation aborts on collision; no divergent target publication. |
| Delete, then reinsert the same RID before backfill finishes. | Final value is the reinsertion; no historical reconciliation deletes it. |
| Inject failure at each of the six backend target branches, including a partially applied batch. | Old primary identity retained; exact affected values/membership preserved; no target snapshot or prune. |
| Cancel/unwind writer or coordinator at registered, primary-applied and backfill stages. | Generation becomes unpublishable; leases and maintenance ownership are released; a later attempt can start. |
| Keep writes arriving while coordinator seeks quiescence. | Writers complete without waiting for compaction drain; coordinator eventually aborts within its maintenance budget. |
| Successful publication, concurrent later mutations, snapshot failure/success and reopen. | Exact expected membership/latest values survive; failed snapshot does not erase retry accounting or prune necessary deltas. |

Use exact RID-to-value comparisons for f32 and independent quantization-aware expectations for SQ8. Membership alone misses stale updates; approximate ANN recall alone is not an integrity oracle.

### Acceptance criteria

Approve execution only when:

1. One documented linearization protocol covers **arming, writer admission, invalidation and publication**.
2. Every online mutation path participates; raw adapter mutation cannot bypass it.
3. Every target/build uncertainty permanently invalidates its generation.
4. Same-RID reordering and delete–reinsert histories cannot produce an eligible divergent target.
5. Cleanup is generation-specific on success, error, cancellation and unwind.
6. No graph work, replay or drain wait occurs under a global writer lock.
7. Deterministic CI tests above fail when registration, invalidation or publication synchronization is removed.
8. Snapshot/reopen assertions substantiate any durability claim separately.

**Files changed:** none. **Builds/tests run:** none; read-only source review only.

## Requirements

1. Before editing, confirm each mechanism in current code (read callers and guards). If a card is already fixed or wrong at HEAD, do not change code for it: say so with file:line evidence in the final message.
2. Fix at the narrowest layer that owns the invariant. No unrelated refactors, renames or comment edits.
3. TDD: for every card add a regression test whose name contains the card's hex suffix in lower case (for example `rv_storage_98f6650d_...`). The test must fail on the unfixed code and pass on the fix — reason about that from the code, you cannot run it. Tests live in the module's `tests/` folder (`tests/mod.rs` is a manifest only; no inline `#[cfg(test)] mod tests {}`). Prefer deterministic constructions (injected failures, barriers, paused clocks) over sleeps and timing.
4. House rules: imports at the top of the file; `mod.rs` re-exports only; one primary export per file; at most 7 entries per folder and 1000 lines per source file for new/changed code; Fx hashers; no `std::sync::Mutex`/`RwLock` on hot paths; `Result` instead of panics; no private machine paths in committed files; terse comments.
5. Do not bump crate or dependency versions, do not add dependencies, do not touch CI workflows.
6. Run only the allowed local gates from the policy (clippy, fmt, layout) on the touched crates until clean.

## Final message (mandatory)

1. Per card: fixed / already-fixed / refuted, with the changed file:line and the regression test name.
2. Why each test fails on the old code (one sentence each).
3. Real exit codes of clippy, fmt --check and check_layout. Anything you could not verify.
