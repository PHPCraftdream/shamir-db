# Windows integration test runtime (WIN-TEST-RUNTIME)

Investigation record: why the full-tier Windows integration step takes ~6.5x longer than
on the Ubuntu runner (baseline: 632 s vs 96 s, 1062 tests, same binaries), the measured
per-test constant cost behind it, what changed, and what the next CI run must show.
Brief: `docs/dev-artifacts/prompts/ci/19-win-test-runtime.md`.

## Baseline (per-binary sums of per-test nextest times, seconds)

| binary | Windows | Linux | ratio | tests |
|---|---|---|---|---|
| shamir-server::e2e | 622 | 102 | 6.1x | 86 |
| shamir-server::admin | 434 | 91 | 4.8x | 89 |
| shamir-db::schema_ddl | 364 | 20 | 18.2x | 97 |
| shamir-e2e::matrix | 339 | 30 | 11.3x | 40 |
| shamir-e2e::batch | 141 | 8 | 17.6x | 15 |
| shamir-e2e::interner | 90 | 6 | 15.0x | 10 |
| shamir-e2e::cursor | 68 | 11 | 6.2x | 7 |
| shamir-e2e::internals | 65 | 5 | 13.0x | 7 |

## Measured: the disk-backed per-test cost is a CONSTANT, not contention

The first fix attempt serialised the 57 disk-backed schema_ddl tests into the
`fsync-heavy` group (max-threads = 1). CI measured the same 57 tests on Windows:

| regime | median | max | sum |
|---|---|---|---|
| parallel (before) | 6.36 s | 7.5 s | 362 s |
| serial in fsync-heavy (after) | 7.37 s | 11.6 s | 417 s |
| Linux, same 57 tests | 0.32 s | 0.5 s | — |

The 40 in-memory tests of the same binary: 0.05 s Windows / 0.03 s Linux median.

Reading: each disk-backed test costs a nearly CONSTANT ~6.4 s on Windows against
0.32 s on Linux — a ~20x per-test fixed cost with a tight spread, unchanged by
parallelism. Serialisation made the run strictly worse (a 417 s serial chain), so
the regroup was REVERTED and the earlier contention-dominant model is retired for
this binary: the cost is fixed per-open-cycle storage work, not a queue.

## Mechanism (per open cycle, from the code)

Every disk-backed test constructs a FRESH on-disk system store
(`ShamirDb::init(SystemStoreConfig::Fjall(sys_path))` in a fresh temp directory):

1. `FjallRepo::new` opens a fresh fjall `Database::builder(path).open()`
   (`shamir-storage/src/stores/storage_fjall.rs:411-416`).
2. The system repo is configured with 13 catalogue tables
   (`shamir-db/src/shamir_db/system_store/store.rs:171-184`); the boot catalogue
   load reads them (`core.rs` `load_tables`), and each table materialises as its
   own fjall keyspace via `db.keyspace(name, KeyspaceCreateOptions::default)`
   (`storage_fjall.rs:426-431`) — 13 fresh keyspaces (journal partitions +
   manifest writes) per open.
3. The store is MemBuffer-wrapped; its background flusher persists the backend
   with `db.persist(PersistMode::SyncAll)` — a full-database journal fsync — on a
   500 ms idle cadence (`storage_membuffer/core.rs:59`; the single SyncAll site is
   `storage_fjall.rs:40-43, 493-497`). A ~1-2 s test accumulates ~2-4 persists.
4. Drop: fjall shutdown releases directory locks lazily (the reopen tests poll
   up to 100 x 50 ms for it), then the temp-directory tree is deleted.

At 100-300 ms per sync point on the runner, ~15-20 sync points plus file churn
reproduce the measured constant ~6.4 s; Linux does the identical work in 0.32 s.
This is the storage open/close lifecycle, not a production-side sleep or timer —
no production change is warranted or made.

## What changed (after the measurement)

1. The `fsync-heavy` regroup of schema_ddl was REVERTED (`.config/nextest.toml`,
   both profiles) — serialising constant-cost tests only lengthens the chain.
2. 50 of the 57 disk-backed schema_ddl tests switched to the in-memory fixture
   (`ShamirDb::init_memory()`), the same fixture the binary's other 40 tests use.
   Selection rule: every test that never reopens from disk and never inspects the
   directory (verified: no `std::fs`/`read_dir`/`exists` use in the whole
   schema_ddl tree). Seeding and assertions are byte-identical; only the catalogue
   backing store changed. The `create_repo`-over-in-memory pattern is already
   proven in-tree (`tests/rename_repo_e2e/durability_override.rs` `setup_memory`).
3. 7 tests KEEP on-disk stores — their subject is durability across a real
   reopen: `ddl_schema_survives_reopen`, `ddl_phase_b_through_reopen`,
   `default_stamp_survives_durable_reopen`,
   `combined_constraints_survive_durable_reopen`, `fk_survives_durable_reopen`,
   `auto_now_timestamps_not_restamped_on_durable_reopen`,
   `unique_survives_durable_reopen`.
4. NOT touched, with reason: `shamir-e2e::matrix/batch/interner/cursor/internals`
   and the shamir-server binaries boot a REAL `ServerLauncher` per test
   (`ServerLauncher` present in every one of those test files) — their subject is
   the real wire/TLS/SCRAM round-trip, so no in-memory equivalent exists; their
   constant per-test cost is the server boot lifecycle (seven on-disk stores +
   clean-shutdown flush + temp-dir delete). Fixing those would require production
   boot-path changes, out of scope.
5. The FlakeGuard boot-stage marks in the three shared server fixtures
   (shamir-server http_rest, shamir-e2e matrix + token matrix) stay in place;
   they print only when a fixture fails.

## What the next CI run must show

- Test count unchanged (1321 in the measured run); zero new failures.
- `shamir-db::schema_ddl` (Windows): the 50 switched tests drop to in-memory
  cost (~0.05-0.3 s each); the 7 kept disk tests remain ~6.4 s (the two
  double-open tests up to ~13 s). Expected binary sum: ~45-95 s, down from
  362-417 s. Linux sum: ~5-8 s, down from ~20 s.
- All other binaries: unchanged within noise (nothing about their scheduling or
  fixtures changed).
- If the schema_ddl numbers land as predicted, the remaining Windows-vs-Linux gap
  is the server-boot binaries' storage lifecycle — addressable only at the
  production boot path or by fewer per-test boots, both out of scope here.
