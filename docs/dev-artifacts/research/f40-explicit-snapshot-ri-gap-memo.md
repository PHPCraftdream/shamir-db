# F-40 Concern 2 — explicit-Snapshot FK RI gap decision memo (#848)

**Status:** investigation complete. **Recommendation: option 2 (isolation-
independent "RI barrier" commit-time re-check) is the tractable direction;
option 1 (mid-flight isolation upgrade) is rejected as unsound. Neither is
implemented in this task — both need a dedicated follow-up task with its own
design spike (mirroring the F-28 Step 3 → Step 5 split).** This memo scopes
that follow-up.

This is concern 2 of the F-40 brief. Concern 1 (the fail-closed fix for
`require_footprint_if_fk_child` / `implicit_tx_isolation_for_fk_parent`) is
landed as tested code in the same task; this file is the investigation the
brief asked for on the SEPARATE, larger question.

---

## 1. The gap, stated precisely

F-28 Step 5 (S3-C) closed the cross-transaction FK TOCTOU race for the
**implicit (autocommit) DELETE/UPDATE** path only. The mechanism is
two-sided:

- **Parent side** — `query_runner.rs::implicit_tx_isolation_for_fk_parent`
  upgrades an *implicit* delete/update against an FK-parent table (one with
  a non-`NoAction` `on_delete`/`on_update` action) from `Snapshot` to
  `Serializable` at begin time, so the RESTRICT/CASCADE/SET NULL child
  scans record a real `PredicateDep::TableScan` SSI predicate
  (`table_manager_streaming.rs:244-248` records it unconditionally at scan
  entry when `tx.isolation == Serializable`), and Phase 2-bis
  (`pre_commit.rs:460-467`) aborts at commit if a concurrent committer's
  footprint touched the scanned child table.

- **Child side** — `query_runner.rs::require_footprint_if_fk_child`
  publishes this tx's commit footprint for an FK-child table via
  `tx.require_footprint_for(token)` regardless of this writer's own
  isolation, so a concurrent Serializable parent-delete's Phase 2-bis check
  has something to conflict against even when the writer is plain Snapshot.

**The gap is on the parent side, and only for EXPLICIT transactions.**
`implicit_tx_isolation_for_fk_parent` is called from exactly TWO sites
(`query_runner.rs:1418` for the implicit UPDATE arm, `:1583` for the
implicit DELETE arm) — both inside the `None =>` (autocommit) sub-arm of
the `match self.tx.as_deref_mut()` in the `BatchOp::Update` / `BatchOp::Delete`
arms. The `Some(tx) =>` (explicit-tx) sub-arms do NOT call it: the
isolation was fixed by the CLIENT at `open_interactive_tx(repo, iso)`
(`interactive_tx.rs:38-43`, which calls `repo.begin_tx(iso).await`) long
before `query_runner.rs` sees the individual op inside `execute_in_open_tx`.

So an explicit transaction opened at `Snapshot` and then performing a
parent-side DELETE/UPDATE against an FK-actionable table:

1. Runs its RESTRICT/CASCADE child scans under `Snapshot`. Because
   `record_predicate_shared` (`tx_context.rs:543-547`) gates on
   `self.isolation == Serializable`, **no `PredicateDep::TableScan` is ever
   recorded** — the predicate_set stays empty for this tx's whole lifetime.
   Symmetrically, `record_read_shared` (`tx_context.rs:514-525`) also gates
   on Serializable, so the read_set stays empty too.

2. At commit, Phase 2-bis (`pre_commit.rs:460-467`) requires BOTH
   `tx.isolation == Serializable` AND `!tx.predicate_set.is_empty()` before
   it calls `gate.predicate_conflicts_batch`. A Snapshot tx fails the FIRST
   condition; the check is skipped even if the predicate_set were somehow
   non-empty (which it cannot be per §1 above).

3. The net effect: a concurrent OTHER transaction inserting a child row
   that references the parent, racing the explicit-Snapshot parent
   operation's "RESTRICT scan found no child → commit" window, is **not
   detected**. The parent delete commits; the child insert commits; a
   dangling reference exists. This is exactly the cross-transaction TOCTOU
   F-28 Step 5 closed for the implicit path, still open for the explicit
   Snapshot path.

Note the **asymmetry** that makes this subtler than "just upgrade
everything": the child-side hook (`require_footprint_if_fk_child`) IS
called from the explicit-tx `Some(tx)` sub-arms of Insert (`:1214`),
Update (`:1352`), and Set (`:1688`) — so a Snapshot writer into a CHILD
table still publishes its footprint, and a *Serializable* parent-delete
(confirmed: the same machinery, just caller-chosen isolation) still
conflicts against it. The gap is exclusively the REVERSE direction: a
*Snapshot* parent-delete has no predicate to conflict against a *footprint-
publishing* child insert. The protection is one-directional under explicit
Snapshot; bilateral only when the parent side is (implicitly or explicitly)
Serializable.

---

## 2. This is an already-documented, accepted residual

`docs/guide-docs/KNOWN_LIMITATIONS.md:139-145` (the "Residual scope"
sub-bullet under the FK TOCTOU entry) states verbatim:

> This closes the race for the IMPLICIT (autocommit) delete/update path
> only. An EXPLICIT transaction that the caller opens as `Snapshot` for its
> own reasons does not get an automatic Serializable upgrade — a caller
> wanting the same protection for an explicit transaction opens it
> `Serializable` itself; the same footprint/predicate machinery already
> protects that case once wired that way.

So this is not a silent lie: the workaround today is "the client opens the
explicit tx Serializable if it wants FK-race protection." The review's ask
(F-40 concern 2) is to scope what an *automatic* fix would look like, not
to paper over an undocumented hole.

---

## 3. Option 1 — auto-upgrade the explicit tx's isolation mid-flight. REJECTED (unsound).

**The idea:** when `query_runner.rs`'s explicit-tx DELETE/UPDATE arm is
about to run an FK-relevant parent mutation, flip `tx.isolation` from
`Snapshot` to `Serializable` in-place before running the RESTRICT/CASCADE
scans, so the scans record their predicates and Phase 2-bis fires at
commit.

**Why it does not work — the retroactive-read-tracking problem.**
`tx.isolation` is a single field set once at `TxContext::new`
(`tx_context.rs:102`, `pub`, no setter, never mutated post-construction in
this codebase), and ALL the SSI bookkeeping — `predicate_set`
(`record_predicate_shared`, `tx_context.rs:543-547`) and `read_set`
(`record_read_shared`, `tx_context.rs:514-525`) — is populated as a
side-effect of reads/scans that check `self.isolation == Serializable` at
the moment they run. A Snapshot tx that performed ANY reads before the
DELETE/UPDATE op recorded NOTHING: its predicate_set and read_set are empty
for everything it observed up to that point.

Flipping `tx.isolation` to Serializable mid-flight and then running the
RESTRICT/CASCADE scan would record the scan's OWN predicate (good), but the
resulting tx would be a **false Serializable**: its commit-time Phase 2
validation (`pre_commit.rs:438` read-set check, `:460-467` phantom check)
would run against a PARTIAL view that misses every read the tx did BEFORE
the upgrade. A concurrent committer that wrote a key this tx read under
Snapshot (before the upgrade) would NOT be caught by the read-set check,
because that read recorded no entry. The upgrade would thus WEAKEN the tx's
correctness story compared to a tx that was Serializable from the start —
it would claim Serializable protection while actually having a Snapshot-
shaped blind spot for its earlier reads. That is a silent correctness
regression, not a fix.

There is no "retroactively re-validate the earlier reads" path either:
Snapshot reads did not capture the versions they observed (the read_set is
empty), so there is nothing to re-validate against. Re-reading every key
the tx ever touched, to capture versions retroactively, is not even
expressible — the tx does not remember what it read.

**Second, narrower blocker — the commit-lock invariant.** The lock-free
commit path decides whether to take `gate.commit_lock()` based on
`tx.isolation == Serializable || !tx.cas_set.is_empty()` at commit ENTRY
(`commit.rs:742`). The lock-free fast path (no commit_lock) is correct for
Snapshot because Snapshot publishes a footprint and never validates a
predicate/read-set; the slow path (commit_lock held) serializes
Serializable committers so the predicate_conflicts_batch scan sees a stable
commit window. Mid-flight escalation could in principle flip this decision
consistently if the upgrade happened before commit entry — but combined
with the retroactive-read problem above, the lock would be acquired for a
tx whose read-set/predicate-set are incomplete, so the lock would not
deliver the invariant it exists to protect. The lock acquisition is a
symptom, not the root cause; the root cause is the missing earlier-read
tracking.

**Verdict on option 1:** unsound. Do not pursue. There is no safe way to
escalate an in-flight Snapshot tx to Serializable that delivers real
Serializable guarantees; any apparent fix would be a false Serializable
with a Snapshot-shaped blind spot for pre-upgrade reads. (A tx that
genuinely needs Serializable protection must open Serializable at `begin`
time — which is exactly the documented workaround in
`KNOWN_LIMITATIONS.md`.)

---

## 4. Option 2 — an isolation-independent "RI barrier" commit-time re-check. TRACTABLE, but not small.

**The idea:** leave `tx.isolation` untouched. Instead, record the
FK-relevant child-table scans the tx performs (regardless of isolation
level) into a SEPARATE set, and at commit time run a TARGETED phantom check
against that set — reusing the EXISTING `predicate_conflicts_batch` /
`record_conflicts` machinery that Phase 2-bis already uses, just gated on
the new set rather than on `predicate_set + Serializable`.

This is the S3-C footprint_tokens pattern applied to the OTHER direction:
`footprint_tokens` widened the COMMIT-WRITE-LOG side (what a Snapshot
writer PUBLISHES); the RI barrier widens the VALIDATION side (what a
Snapshot parent-delete CHECKS at commit).

### 4.1 What already exists and is directly reusable

- `gate.predicate_conflicts_batch(&deps, tx.snapshot_version)`
  (`repo_tx_gate.rs`) walks the commit window
  `(tx.snapshot_version, last_committed]` in the commit_write_log and
  tests each `CommitWriteRecord` against each dep via `record_conflicts`
  (`TableScan { table_token }` conflicts iff
  `per_table[token].touched`). This is the EXACT check an RI barrier needs;
  it takes a slice of `PredicateDep` and a snapshot version, with no
  dependency on the tx's isolation level.

- `shamir_tx::predicate_set::PredicateDep::TableScan { table_token }` is a
  plain enum variant — no isolation coupling in the type itself. A Vec/Set
  of these is all the barrier needs to pass into `predicate_conflicts_batch`.

- The commit window is already bounded and scanned for every Serializable
  tx at commit; the barrier just extends the SAME scan to qualifying
  Snapshot txs. No new commit-window machinery, no new lock.

- `table_manager_streaming.rs:244-248` already has the "record a TableScan
  dep at scan entry" shape — the barrier's recording site would lift the
  `if t.isolation == Serializable` gate from that call (or add a parallel
  recording into the new barrier set that fires regardless of isolation).

### 4.2 The minimal mechanism (design sketch, NOT implemented)

1. **New `TxContext` field** — `ri_barrier_tokens: TFxSet<u64>` (or a small
   `Vec<PredicateDep>`), mirroring `footprint_tokens` exactly. Empty for
   the overwhelming majority of txs (any tx not doing an FK-parent
   delete/update); `is_empty()` is the single zero-overhead gate.

2. **Recording site** — the RESTRICT/CASCADE/SET-NULL child-scan entry
   points (`fk_restrict.rs::child_has_reference`'s `list_stream_tx`,
   `fk_actions.rs`'s cascade scans, `fk_on_update.rs`'s on-update scans)
   record the child `table_token` into `tx.ri_barrier_tokens` regardless
   of isolation. This is the one place the recording logic diverges from
   the Serializable-only predicate_set: the barrier records for ALL
   isolations, because its commit-time check is isolation-independent.

3. **Commit-time check** — three sites widen their guard from
   `tx.isolation == Serializable && !tx.predicate_set.is_empty()` to
   `(... && !tx.predicate_set.is_empty()) || !tx.ri_barrier_tokens.is_empty()`:
   - `pre_commit.rs:460-467` (the main lock-free commit phantom check),
   - `pre_commit.rs:614` (the legacy AsyncIndex commit path's identical
     check — opt-in, but a Snapshot FK-parent delete can opt into it),
   - `group_commit.rs:184-187` (the inter-batch phantom check for grouped
     commits).
   Each then calls `predicate_conflicts_batch` with the UNION of
   `predicate_set.snapshot_deps()` and the barrier tokens' `TableScan`
   deps. For a Snapshot tx the predicate_set half is empty (always), so
   only the barrier tokens drive the check — exactly the targeted scope.

4. **Commit-lock acquisition** — `commit.rs:742`'s guard widens
   symmetrically: a tx with non-empty `ri_barrier_tokens` must take
   `commit_lock` (same as Serializable), because the barrier check needs a
   stable commit window just as much as the SSI phantom check does.

### 4.3 False-abort / performance profile

The barrier's `TableScan { table_token }` dep conflicts with ANY committer
in the window whose footprint touched the child table — the SAME
conflict semantics as the existing Serializable phantom check. So the
false-abort profile is **identical to the already-shipped implicit-
Serializable path**: a concurrent tx that touched the child table (insert,
update, OR delete — even one not referencing this parent) triggers a
conflict, aborting the parent. This is the known, accepted cost of
predicate-based SSI on a table scan, already borne by every implicit
FK-parent delete today. The barrier does not make it worse; it makes it
apply uniformly.

The commit-time cost for a qualifying Snapshot tx is one
`predicate_conflicts_batch` scan of the commit window (a `scc::TreeIndex`
walk bounded by the number of committers in `(snapshot, last_committed]`),
plus the commit_lock acquisition. This is strictly less than what a real
Serializable tx pays (Serializable also validates the full read_set), and
zero for any Snapshot tx that does not touch an FK-parent table
(`ri_barrier_tokens.is_empty()` short-circuits before any work).

### 4.4 Why it is still NOT a "small, safe, ship-it-now" change

Three coordinated edit sites across the commit pipeline (`pre_commit.rs`
x2, `group_commit.rs`, `commit.rs:742`), plus a new `TxContext` field
with its own serialization/recovery story (does it need to survive
crash? does it interact with the WAL?), plus recording-site changes in
`fk_restrict.rs` / `fk_actions.rs` / `fk_on_update.rs` that must fire for
ALL isolations without perturbing the existing Serializable path. That is
materially more surface than F-28 Step 5's `footprint_tokens` widening was
(one new field + one `build_footprint_from_tx` gate change, no commit-
pipeline edits). It is exactly the kind of commit-pipeline change that
benefits from its own design spike — the same way F-28 Step 3 (the S3-C
spike) preceded F-28 Step 5 (the implementation). Rushing it into the same
task as a P1 fail-closed fix would compound review surface and risk.

There are also two open design questions the spike must settle:
- **Recursion / multi-level cascade**: a CASCADE that fans out across
  multiple child/grandchild tables needs a barrier token per table scanned.
  Is a flat `TFxSet<u64>` of table_tokens sufficient, or does the barrier
  need the richer `PredicateDep` shape (e.g. `IndexRange` for an indexed
  FK-field scan, to avoid a full-table-scan conflict dep when a tighter
  one is available)?
- **Interaction with `retry_on_tx_conflict`**: the implicit path wraps its
  commit in a bounded retry so an already-resolved race does not surface as
  a client error. The explicit path has no such wrapper today (the client
  owns the retry decision). Should the barrier-triggered abort be a
  retryable `PhantomConflict` (surfaced as `tx_conflict`), and if so, does
  the explicit-tx client need a documented retry expectation? Or should the
  barrier use a distinct error code so clients can distinguish "RI barrier
  abort" from a generic SSI conflict?

---

## 5. Recommendation

**Adopt option 2 (RI barrier), as a dedicated follow-up task with its own
design spike first.** Do not implement option 1 (it is unsound per §3).
Do not implement option 2 in the F-40 task that landed concern 1 — the
commit-pipeline surface (3 guard-widening sites + commit_lock acquisition +
new TxContext field + 3 recording-site changes) deserves the same
spike-then-implement split F-28 used for S3-C, and the two open design
questions in §4.4 need to be settled before writing production code.

The F-40 task's contribution to concern 2 is this scoping memo: it
establishes that (a) option 1 is a dead end, (b) option 2 is the right
direction and reuses existing machinery cleanly, (c) the implementation
surface and open questions are bounded and enumerated. That is the same
deliverable shape F-28 Step 3's spike memo took.

### What the follow-up task must do

1. **Spike first** (mirror `f28-s3-mechanism-decision.md`'s shape):
   - Settle the two open design questions in §4.4 (flat token set vs.
     `PredicateDep` shape; retry/error-code contract for explicit txs).
   - Prototype the recording site in ONE of `fk_restrict.rs` /
     `fk_actions.rs` / `fk_on_update.rs` and the guard widening in
     `pre_commit.rs:460-467` only, behind the new `ri_barrier_tokens`
     field.
   - Write a deterministic race harness proving the barrier catches the
     exact race §1 describes (an explicit Snapshot parent-delete vs. a
     concurrent child insert), reusing the `RaceInjectingResolver` shape
     from `fk_race_closure_tests.rs` (the `resolve_repo`-ordinal injection
     seam is identical; only the outer tx is explicit-Snapshot instead of
     implicit).
   - Measure the quiescent-abort rate (no concurrent writer) over a trial
     run to confirm zero false aborts, mirroring the F-28 Step 3 spike's
     50-trial quiescent assertion.

2. **Then implement** (mirror F-28 Step 5's shape):
   - Land the `TxContext.ri_barrier_tokens` field + recording at all three
     scan entry points + guard widening at all four commit-pipeline sites
     (`pre_commit.rs` x2, `group_commit.rs`, `commit.rs:742`).
   - Add end-to-end race-closure tests in `fk_race_closure_tests.rs` (or a
     sibling file) for the EXPLICIT-Snapshot path, paralleling the existing
     implicit-path RESTRICT/CASCADE race tests.
   - Update `KNOWN_LIMITATIONS.md:139-145`'s "Residual scope" bullet from
     "open for explicit Snapshot" to "CLOSED for explicit Snapshot via the
     RI barrier", mirroring how the F-28 Step 5 entry already reads for the
     implicit path.

3. **Out of scope for the follow-up** (call out explicitly, as F-28 did
   for S3-A): any change to `FkReverseCache` internals (F-35/F-36 already
   landed), any change to the Serializable path's existing predicate
   semantics (the barrier is ADDITIVE — it never weakens what Serializable
   already does), and any client-facing API change (the barrier is
   engine-internal; the wire shape of `BatchResponse.transaction` is
   unchanged).

---

## 6. Why this memo does not ship code

The brief explicitly allowed implementation only if the investigation
"genuinely concludes one is small and safe." It does not:

- Option 1 is unsound (§3) — not small, not safe, not a fix.
- Option 2 is the right direction but is a 4-site commit-pipeline change
  with a new `TxContext` field, 3 recording-site edits in the FK scan
  paths, and two unsettled design questions (§4.4). That is a commit-
  pipeline change of the same order as F-28 Step 5 itself, which had its
  own dedicated task after its own dedicated spike. Bundling it into F-40
  (a task scoped as a P1 fail-closed fix + a memo) would be the kind of
  rushed implementation the brief warned against.

The honest conclusion is: **this needs its own dedicated task**, and this
memo is the input to that task. The F-40 task ships concern 1 (the fail-
closed fix, tested) and this scoping memo (concern 2); it does not ship an
RI-barrier implementation.

---

## 7. Exact citations (for the follow-up task's spike)

- `crates/shamir-engine/src/query/batch/query_runner.rs:1418,1583` — the
  ONLY two call sites of `implicit_tx_isolation_for_fk_parent` (both in
  the `None =>` implicit sub-arms of Update / Delete). The `Some(tx) =>`
  sub-arms (`:1345` Update, `:1519` Delete) do not call it — the gap.
- `crates/shamir-engine/src/query/batch/interactive_tx.rs:38-43` —
  `open_interactive_tx(repo, iso)` → `repo.begin_tx(iso).await`; the
  client-chosen `iso` is fixed here, before `query_runner.rs` sees any op.
- `crates/shamir-tx/src/tx_context.rs:102` — `pub isolation: IsolationLevel`,
  set at `:286-321` (`TxContext::new`), no setter, never mutated
  post-construction. (Option 1's target field; option 2 leaves it alone.)
- `crates/shamir-tx/src/tx_context.rs:514-525` — `record_read_shared`,
  gated on `Serializable`. Empty under Snapshot.
- `crates/shamir-tx/src/tx_context.rs:543-547` — `record_predicate_shared`,
  gated on `Serializable`. Empty under Snapshot.
- `crates/shamir-engine/src/table/table_manager_streaming.rs:244-248` —
  `TableScan` predicate recorded at `list_stream_tx` entry when
  `tx.isolation == Serializable`. (Option 2's recording site would add a
  parallel, isolation-independent record into `ri_barrier_tokens`.)
- `crates/shamir-engine/src/tx/pre_commit.rs:438` — Phase 2 read-set
  validation, gated on Serializable.
- `crates/shamir-engine/src/tx/pre_commit.rs:460-467` — Phase 2-bis
  phantom check, gated on `Serializable && !predicate_set.is_empty()`.
  (Option 2's primary guard-widening site.)
- `crates/shamir-engine/src/tx/pre_commit.rs:594,614` — legacy AsyncIndex
  path's identical two checks (opt-in durability, but reachable by a
  Snapshot FK-parent delete that opts in).
- `crates/shamir-engine/src/tx/group_commit.rs:184-202` — inter-batch
  phantom check, gated on `Serializable && !predicate_set.is_empty()`.
  (Option 2's third guard-widening site.)
- `crates/shamir-engine/src/tx/commit.rs:742` — commit_lock acquisition,
  gated on `Serializable || !cas_set.is_empty()`. (Option 2's fourth
  guard-widening site — a tx with non-empty `ri_barrier_tokens` must take
  the lock so the window scan is stable.)
- `crates/shamir-tx/src/repo_tx_gate.rs:959-972` — `build_footprint_from_tx`,
  already widened by F-28 Step 5 to publish a Snapshot tx's footprint when
  `footprint_tokens` is non-empty. UNCHANGED by option 2 (the barrier is
  on the CHECK side, not the PUBLISH side).
- `crates/shamir-tx/src/repo_tx_gate.rs` `predicate_conflicts_batch` /
  `record_conflicts` — the existing window-scan machinery option 2 reuses
  verbatim.
- `crates/shamir-engine/src/query/batch/tests/fk_race_closure_tests.rs` —
  the `RaceInjectingResolver` / `INJECT_AT_RESOLVE_REPO_CALL` harness the
  follow-up's explicit-Snapshot race test should adapt.
- `docs/guide-docs/KNOWN_LIMITATIONS.md:139-145` — the accepted-residual
  entry this whole concern is about; the follow-up task rewrites it to
  "CLOSED for explicit Snapshot via the RI barrier."
