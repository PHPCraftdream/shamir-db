# Brief for F-50 Step 3a (#871, P0, spike) — persisted Building/Ready index2 state + crash/restart continuation design

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

F-50 Step 1 (#869, commit `07076dde`) and Step 2 (#870, commit `630c9bc0`)
closed the "guaranteed miss" family of bugs (a tx staging before a new
index2 backend/sorted-index is registered would permanently miss ops for
it). **Read `docs/dev-artifacts/research/f50-index-lifecycle-spike.md`
first** (Step 1's memo — its §5.3 sketches the original Step 3 scope; this
brief narrows and grounds it against real investigation done this session,
described below — do not re-derive these findings, build on them).

This is a **timeboxed design spike**, mirroring this session's established
precedent (F-40b's Step 1→Step 2 split, F-48→F-48b, F-50's own Step
1→Step 2 split). The goal is settling a design and proving the trickiest
mechanism works via a minimal prototype — NOT a full production
implementation.

## What was already investigated this session (do not re-derive)

**A. `IndexDescriptor` persistence shape and a real forward-compat risk:**

`IndexDescriptor` (`crates/shamir-index/src/descriptor.rs:8-29`) is
bincode-serialized inside `MetaEnvelope<T>`
(`crates/shamir-index/src/meta_envelope.rs:50-56`, magic="SDB2", version=1).
It already has one `#[serde(default)]` field (`options: Vec<u8>`).

**Critical open question this spike must settle first:** Step 1's memo
(quoting `VectorConfig`'s doc comment, `crates/shamir-index/src/kind.rs:172-186`)
already flagged that "bincode 1.3.3 does NOT honour `#[serde(default)]` for
skipped fields on read (it tries to read the field bytes and fails with
`UnexpectedEof`)" — that finding was about `#[serde(skip)]` specifically,
but it is NOT yet confirmed whether the DIFFERENT `#[serde(default)]`
annotation (used on `options`, not `skip`) actually round-trips correctly
against genuinely OLD on-disk bytes (encoded before that field existed) —
or whether `options`'s forward-compat has simply never been exercised
against real pre-field data. **Do not assume `#[serde(default)]` works for
this crate's bincode setup — prove it** with an actual test: hand-encode an
`IndexDescriptor`-shaped byte sequence that predates the new field (e.g. by
temporarily constructing an older-shaped version of the struct in a test,
or by bincode-serializing a struct literal that omits the new field via a
local test-only shadow type) and confirm `bincode::deserialize` into the
NEW (with the added field) struct either (a) succeeds with the default, or
(b) fails — and if it fails, that is the answer: `#[serde(default)]` does
NOT work here, and you need a different mechanism (e.g. bump
`MetaEnvelope`'s version and branch on it at load time, or wrap the new
field in an adapter that tries the new shape and falls back to the old
shape). **This is the single most important thing to settle correctly** —
getting it wrong would silently corrupt every existing on-disk index
descriptor the first time this workspace adds the new field.

**B. `create_index_v2`'s exact sequence and crash-window analysis**
(`crates/shamir-engine/src/table/table_manager_index_mgmt.rs:78-328`):

1. `:78` — acquire `unique_write_lock` (held for the whole sequence).
2. `:82` — set `index2_create_barrier` (RAII guard).
3. `:100` — `allocate_id()` (in-memory only, no durability yet).
4. `:118-119` — **FIRST** `save_index2_metadata` call — persists the
   `next_id` watermark (this is "#534 finding 2", already closes
   id-reuse-after-crash; do not re-touch this).
5. `:311` — `backfill_index2_backend()` (the streaming backfill).
6. `:322-325` — `index2_registry.insert(backend)` — the backend becomes
   LIVE and queryable in the running process's registry (F-50 Step 1/2's
   generation counter bumps here too).
7. `:327-328` — **FINAL** `save_index2_metadata` call — persists the new
   descriptor to disk.

**Crash-window analysis (already done — the worst case is the one this
spike's persisted state is meant to fix):**
- Crash before step 4: no risk (`allocate_id` just replays on restart).
- Crash between step 4 and step 6: id reserved, no descriptor persisted,
  backend absent from the live registry — orphan postings (if any were
  written, which they weren't yet at this point) are unreachable garbage,
  not a correctness bug, just wasted space.
- **Crash between step 6 and step 7 — THE gap this spike closes:** the
  backend IS live and queryable in the crashed process's registry (any
  concurrent tx that raced past the barrier could have gotten a correct
  posting via Step 1/2's re-derivation), but a restart's
  `load_index2_metadata` (`table_manager.rs:296-310`) only reconstructs
  from what step 7 actually wrote to disk — since step 7 never ran, the
  backend silently vanishes on restart. A query that worked before the
  crash silently stops working after, with no error and no detectable
  orphan state anywhere.

**C. No existing repair/doctor support for index2:**
`crates/shamir-engine/src/table/doctor.rs`'s `verify()`/`repair()`
(`:80-189`, `:199-299`) only know about the LEGACY regular/unique/sorted
indexes — index2 backends are entirely invisible to the doctor today.
There is no existing hook to extend; whatever this spike designs is new.

**D. No index-specific crash/WAL-replay logic beyond the one-time boot
load:** `table_manager.rs:296-310`'s `load_index2_metadata` runs once at
table open; crash recovery otherwise is generic repo-level WAL replay of
posting `KvOp`s (`RepoInstance::recover_v2_inflight`), which has no
awareness of index lifecycle state at all.

**E. DDL cancellation is explicitly OUT of scope for this spike.** A
separate investigation this session found index2 has **no `DROP INDEX`
support whatsoever** (filed as its own task, #872 — `handle_drop_index`,
`crates/shamir-db/src/shamir_db/execute/admin_table_index.rs:422-497`,
never touches `index2_registry`; only `DROP TABLE` cascade does, via
`remove_by_id`, and even that doesn't clean up posting entries). Since a
user cannot currently issue any kind of cancel/drop against an in-progress
or completed index2 build, "DDL cancellation mid-build" is **currently
unreachable** — do not design for it here. It will be revisited once #872
lands and a real cancel path exists to make unsafe.

## What to settle

### 1. The persisted-state forward-compat mechanism (see §A above — settle this FIRST, it gates everything else)

Prove which mechanism actually works for adding a new `state: IndexState`
enum field (at least `Building` / `Ready`) to `IndexDescriptor` without
corrupting existing on-disk descriptors. Write a real test proving your
chosen mechanism round-trips both an old-shaped (pre-field) encoding and a
new-shaped encoding correctly.

### 2. Crash-restart continuation decision for a `Building` index found on restart

Three options, pick one (or a documented combination) with reasoning:
- **Resume the backfill** — needs a persisted cursor/checkpoint of backfill
  progress (currently none exists — investigate `backfill_index2_backend`'s
  streaming shape to judge how hard adding one would be).
- **Restart from scratch** — drop whatever partial backend/postings exist
  under the reserved id and redo `create_index_v2` fully. Simplest; may
  duplicate backfill work but is trivially correct.
- **Leave as a permanently-`Building` orphan** for a NEW doctor extension
  to detect and report/clean (design the minimal `doctor.rs` addition this
  would need — a new check in `verify()`/`repair()` for index2 backends
  stuck in `Building`).

State your recommendation. There is no mandated answer, but justify it
against the actual backfill mechanism's shape (is it naturally
resumable/idempotent, or does restart-from-scratch avoid real complexity
for negligible cost given index builds are rare/operator-driven events?).

### 3. Where the `Building` state gets persisted in the existing 2-persist sequence

Does `Building` piggyback on the EXISTING first `save_index2_metadata`
call (`:118-119`, already there for the id-reuse fix), or does it need its
own additional persist point? Settle this against your answer to #1 (the
forward-compat mechanism) and #2 (the crash-continuation choice).

## What to prototype

At minimum: the round-trip test from §1 proving the forward-compat
mechanism. If time permits within the timebox, also prototype the
`IndexState` field itself (added to `IndexDescriptor`, defaulted to
`Ready` for a freshly-constructed descriptor) WITHOUT wiring the full
`Building`-at-start / `Ready`-at-finish state machine into
`create_index_v2` yet (that's Step 3b's job) — just prove the type and its
serialization round-trip. Do NOT implement the doctor extension, the
resume/restart logic, or wire the state machine into `create_index_v2` in
this spike — those are Step 3b, once the forward-compat mechanism and the
crash-continuation choice are settled here.

## Constraints

- Timebox this — if the forward-compat investigation surfaces genuine
  bincode/serde complexity beyond a straightforward round-trip test, stop,
  document precisely what you found and why it's hard, and let Step 3b
  handle the harder mechanics with the question settled from your
  investigation (even a negative result — "X doesn't work, here's proof" —
  is a valid, valuable spike outcome).
- Do NOT implement DDL cancellation (deferred to after #872).
- Do NOT implement the full doctor repair logic or wire a state machine
  into `create_index_v2` — prototype the persistence mechanism only.
- Do NOT touch F-50 Step 1/2's already-landed generation-gate mechanism
  (`IndexRegistry::generation`, `rederive_index2_ops_post_stage`,
  `SortedIndexManager::generation`) — unrelated to this spike.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-index -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` must be clean if
  any prototype code is committed.
- Clean up any scratch/debug log files you create in the repo root before
  finishing — they must not be committed.

## Deliverable

A decision memo at
`docs/dev-artifacts/research/f50-step3-crash-restart-spike.md` (mirroring
`f50-index-lifecycle-spike.md`'s structure): the settled forward-compat
mechanism with your round-trip test's actual output as proof, the
crash-restart continuation decision with reasoning, where `Building` gets
persisted, the minimal doctor-extension shape (designed, not
implemented), and a precise Step 3b implementation plan with exact touch
points.

## Verification the orchestrator will run

```
cargo fmt -p shamir-index -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index --full
```

When done, give your final summary as plain text: the forward-compat
mechanism you proved (or disproved) with actual test output, the
crash-restart decision and reasoning, the persist-point decision, the
doctor-extension design, the Step 3b implementation plan, and confirmation
fmt/clippy are clean if you committed prototype code.
