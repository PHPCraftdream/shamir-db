# Brief 75 — #1068 (P1): DDL op-log retention — the cap/eviction is dead code, make it real

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## The defect

`crates/shamir-index/src/base_index/ddl_op_log.rs`:
- `const DDL_OP_LOG_CAP: usize = 10000;` (line ~26) — declared, `#[allow(dead_code)]`,
  never read anywhere.
- `pub async fn maybe_evict_terminal_records(_info_store: &Arc<dyn Store>) -> Result<(), DbError>`
  (line ~116) — the parameter is literally underscore-prefixed; the body is
  `Ok(())`. Zero call sites anywhere in the workspace (verify yourself:
  `grep -rn "maybe_evict_terminal_records" crates/`).

Every successful, tracked DDL operation (`write_op_status`) permanently adds
a record to `info_store` and NOTHING ever removes it. On a long-lived
installation with active DDL churn, this is unbounded growth with no
feedback — worse than not having a cap declared at all, because the
constant reads as "there is a limit" when there isn't one.

## The key insight that resolves this task's stated design concern

The task's own description raises a real-sounding blocker: *"ключ статуса
после #1051 — `b"ddl_op:"` + сырые 16 байт `op_id`, то есть по ключу НЕЛЬЗЯ
отсортировать по времени... нужен либо отдельный временной индекс, либо
смена схемы ключа."*

**This is not actually true — verify it yourself, then build on it, don't
re-derive a separate index.** `RecordId` (`crates/shamir-types/src/types/record_id.rs`,
`from_ts`, ~line 41) writes its FIRST 8 bytes as
`relative_micros.to_be_bytes()` — a big-endian, monotonically-increasing
timestamp — before the trailing random/collision bytes. `op_status_key`
(`ddl_op_log.rs`, `op_status_key`) is `"ddl_op:"` (7 fixed bytes) + the raw
16 bytes of `op_id` (`op_id.as_bytes()`). Since ALL keys under this prefix
share the same 7-byte prefix, and the next 8 bytes of every key are a
big-endian timestamp, **lexicographic byte order over this keyspace IS
chronological order** — no separate time index needed.

`Store::scan_prefix_stream` (`crates/shamir-storage/src/types.rs`, ~line 293)
and `Store::iter_range_stream` (~line 338) both carry an explicit, hard
contract: *"Keys within and across batches are yielded in ascending
lexicographic byte order — the SAME guarantee documented on
`Store::scan_prefix_stream`, every implementor MUST uphold it."* This is
relied on elsewhere in the codebase for correctness (cited: `storage_membuffer.rs`'s
merge-overlay scans, task #530) — it is a load-bearing guarantee, not an
implementation detail you're discovering for the first time.

**Consequence: `scan_prefix_stream(b"ddl_op:", batch_size)` yields every
DDL op-status record in oldest-first chronological order, for free, on
every `Store` backend.** FIFO eviction is "remove from the front of this
stream until under cap" — no schema change, no migration, no new index.

**One caveat to note in your implementation's doc comment (not a blocker,
just be honest about it):** a client-supplied `request_id` (the
`DropIndexOp`/`RenameIndexOp::request_id` field, threaded through since an
earlier round of the DDL-status work) is technically any valid 16-byte
`RecordId`-shaped value — a non-conforming caller COULD supply a
non-timestamp-prefixed id and slightly perturb strict chronological
ordering for that one record. This does not break the eviction policy
(worst case, one record sorts slightly out of true insertion order among
thousands) and is not worth defending against for an alpha-grade FIFO cap.

## The fix

### 1. Implement real eviction in `maybe_evict_terminal_records`

- Scan `scan_prefix_stream(b"ddl_op:".to_vec().into(), <a reasonable batch size>)`.
- For each record: strip the version byte, decode the envelope (reuse the
  SAME version-check logic `read_op_status` already has — factor it into a
  shared helper if that's cleaner, don't duplicate the version-byte check
  verbatim).
- Classify each record's `DdlOpState`:
  - `InProgress` → **never evict**, regardless of age or count. This is the
    hard invariant the required tests must prove.
  - `Succeeded { .. }` / `Failed { .. }` / `SucceededViaCrashRecovery { .. }`
    → terminal, eligible for eviction.
- Count terminal records. If the total is over `DDL_OP_LOG_CAP`, delete the
  OLDEST excess terminal records (the stream is already oldest-first, so
  this is "delete from the front of the terminal subsequence until
  `terminal_count <= DDL_OP_LOG_CAP`") via `Store::remove`/`remove_many`
  (check which is available and idiomatic here — other maintenance sweeps
  in this codebase, e.g. `sweep_index2_postings_by_id` in
  `crates/shamir-index/src/persistence.rs`, use `remove_many` with a
  batched `Vec<RecordKey>` — mirror that pattern rather than one `remove`
  call per key).
- Remove the `#[allow(dead_code)]` on `DDL_OP_LOG_CAP` once it's actually
  read.
- This eviction is naturally idempotent and crash-safe WITHOUT any
  tombstone/recovery machinery: it just re-derives "which records are over
  cap" from the current on-disk state every time it runs. An interrupted
  eviction (crash mid-batch) simply leaves slightly more than `DDL_OP_LOG_CAP`
  terminal records — the NEXT invocation re-scans and finishes the job. Say
  this explicitly in the function's doc comment so a future reader doesn't
  assume it needs the tombstone-based crash-recovery pattern the DROP/RENAME
  paths use (those need it because they have irreversible partial states;
  this doesn't).

### 2. Actually call it — throttled, not on every write

Calling a full prefix scan on every single `write_op_status` would be
O(N) per DDL op (→ O(N²) over the log's lifetime) — DDL ops are low-frequency
but this is still the wrong shape per this codebase's O(x→0) discipline
(see CLAUDE.md's "🔒 Code ideology" section). Instead:

- Add a lightweight, in-memory `AtomicU64` write counter (mirrors this
  codebase's established pattern for cheap O(1) cardinality tracking — see
  CLAUDE.md's note on `scc::*::len()` being banned and the
  `AtomicUsize` mirror convention used elsewhere, e.g.
  `Drainer::window_depth`/`VersionedOverlay::count`). Increment it on every
  `write_op_status` call for a TERMINAL state (skip `InProgress` writes —
  they don't grow the eviction-eligible population). Every Nth write
  (pick a throttle constant — 100 is a reasonable starting point, name it
  clearly, e.g. `DDL_OP_LOG_EVICTION_CHECK_INTERVAL`), trigger
  `maybe_evict_terminal_records` — fire-and-forget is NOT acceptable here
  (silent failures would defeat the whole point); await it inline and
  `log::error!` (not swallow) on failure, mirroring this task family's
  established "do not swallow status-write errors" convention.
- Where does the counter live? `write_op_status` is a free function taking
  `&Arc<dyn Store>` — it has no natural place to keep a persistent atomic.
  Check whether `IndexManager`/`TableManager` (whichever owns the
  `info_store` these calls thread through) already has a sensible home for
  a small piece of per-table maintenance state, or whether a
  process-wide (not per-table) counter is more appropriate given this log
  is genuinely a single shared keyspace per `info_store`. Make a clear,
  documented choice — don't leave it ambiguous.
- ALSO call `maybe_evict_terminal_records` once at `TableManager::create`
  (table open), same reasoning as this codebase's other periodic
  maintenance sweeps at open time (mirrors `recover_index2_drops`/
  `recover_hash_renames` running at open, though this isn't a recovery
  path — just a good moment to catch up if the throttle counter reset
  across a restart, since the in-memory counter obviously doesn't survive
  a crash).

### 3. Make it observable

Per this task's own description: *"он должен РЕАЛЬНО выполняться и быть
наблюдаемым (metrics или `doctor`)"*. Minimum bar: `log::info!` when
eviction actually removes records, stating how many were removed and the
resulting terminal-record count (e.g. `"#1068: DDL op-log eviction removed
{n} terminal record(s), {remaining} remain (cap: {DDL_OP_LOG_CAP})"`).
Check whether `crates/shamir-engine/src/table/doctor.rs` (the `verify`/
`repair` surface referenced throughout this task family's error messages)
has an existing hook for this kind of "storage housekeeping" fact you
could surface through `TableManager::verify()`'s output — if there's a
natural, LOW-EFFORT slot for a `ddl_op_log_terminal_record_count` fact,
add it; if wiring it into `doctor` would require a disproportionate
refactor for this task's scope, the `log::info!` above is an acceptable
minimum — use your judgement and say which you did and why.

## Tests — required, per this task's own description

New file under `crates/shamir-index/src/base_index/tests/` (this module's
existing test-organisation convention), covering:

1. **Cap exceeded → oldest terminal records evicted, newest survive.**
   Write more than `DDL_OP_LOG_CAP` terminal records (you'll want a MUCH
   smaller test-only cap to keep this fast — check whether `DDL_OP_LOG_CAP`
   should be `pub(crate)` + overridable via a test seam, or whether a
   private test-only constant/parameter makes more sense; don't write
   10,000 real records in a test if you can avoid it. A clean option: make
   `maybe_evict_terminal_records` take the cap as an explicit parameter,
   with a wrapper using `DDL_OP_LOG_CAP` for the real call sites — simpler
   than a global test-seam toggle and lets the test use a cap of e.g. 5).
   Assert: after eviction, exactly `cap` terminal records remain, and they
   are the NEWEST ones (poll for the oldest ones and confirm `read_op_status`
   now returns `None`/absent for them, confirm the newest ones are still
   present).
2. **Eviction never touches `InProgress` records**, even when they're the
   OLDEST records in the log and the terminal population alone exceeds the
   cap. Seed a mix (some `InProgress`, many terminal, total exceeding cap)
   and assert every `InProgress` record survives regardless of age/position,
   while terminal records are correctly trimmed to cap.
3. **Idempotent / crash-safe re-run.** Run eviction, then run it again
   immediately with no new writes in between — assert it's a clean no-op
   the second time (no error, no further deletions, no double-counting).
   This is your proof for the "crash mid-GC doesn't corrupt the log" claim
   in the brief above — a real crash-injection test isn't necessary here
   since the design is inherently idempotent by re-derivation, but a
   run-twice test is the cheap, direct way to demonstrate it.
4. **Throttled trigger actually fires.** Write `DDL_OP_LOG_EVICTION_CHECK_INTERVAL`
   (or however many) terminal-status writes through the real
   `write_op_status`-calling path (not calling `maybe_evict_terminal_records`
   directly) and confirm eviction ran automatically without an explicit
   caller-side trigger — this is the test that proves item 2 ("actually
   call it") isn't dead wiring.

**Every test must FAIL on code lacking the mechanism it proves** — this
codebase's established convention (verify at least test 2, the
`InProgress`-survives invariant, with a revert-and-check: temporarily let
eviction consider `InProgress` records eligible, confirm the test goes red,
restore the fix).

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
./scripts/test.sh -p shamir-db
./scripts/test.sh -p shamir-server
```

Paste the actual final summary line from each `./scripts/test.sh`
invocation (pass/fail counts) — literal output, not a paraphrase. List
every test you wrote by name with individual pass/fail status, and the
outcome of the mandatory revert-and-check self-verification. If anything
fails, fix it before reporting done. This codebase's #1065 task went
through 4 rounds because earlier attempts self-reported success that
direct verification disproved — the standard here is that everything you
report is something you personally watched pass, with the command's
actual output as evidence.
