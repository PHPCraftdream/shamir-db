# Brief: what does opening the storage cost at start-up? (BOOT-COST, investigation)

## Why

The Windows timing work (`docs/dev-artifacts/perf/ci-timing-baseline.md`, `docs/dev-artifacts/perf/win-test-runtime.md`)
found that one open of the system store (Fjall) costs about 6.4 s on the Windows runner against 0.32 s on Linux, and that
a server start opens 7 stores (the `server_meta` redb plus 6 Fjall). That is the cold-start time of the product on
Windows and the remainder of the long integration tests (`shamir-server` e2e, admin, `shamir-e2e` matrix). Candidate
causes already named: 13 catalogue tables = 13 keyspaces opened on every open
(`crates/engine/shamir-db/src/shamir_db/system_store/store.rs`, around lines 171-184, and
`crates/storage/shamir-storage/src/stores/storage_fjall/` where the keyspace is created), the background MemBuffer
flusher doing a full `persist(SyncAll)` every 500 ms (`storage_membuffer/core.rs`), and the lock release and directory
removal that follow.

## What (investigation, NO production change)

Write `docs/dev-artifacts/perf/boot-cost.md` that answers, from the code and from evidence:

1. How many keyspace opens and how many fsync / `persist(SyncAll)` calls does ONE open of the system store perform,
   and of the whole server start (all 7 stores)? Walk the real call path and give file and line for each step.
2. Why is each step expensive on Windows and cheap on Linux (what the OS call is, why it differs)? Separate what the
   code shows from what is inferred; rank hypotheses by strength of evidence.
3. Which of the 7 opens are sequential and could run concurrently without weakening any durability or ordering rule?
4. Is a `SyncAll` every 500 ms on an idle store needed when nothing is dirty? What does the flusher do when there is
   nothing to persist?
5. Options with their risk: lazy opening of catalogue tables, one keyspace with key prefixes, parallel open, skipping
   an idle persist. Durability must never be weakened; say for each option which invariant guards it.
6. A measurement plan the orchestrator can run in CI (nothing runs on this machine): which counters exist
   (`FJALL_PERSIST_SYNC_ALL_COUNT` and its reset seam), which timing marks to add, and the exact test or job to run on
   `windows-latest` and `ubuntu-latest`. You may add the measuring code as PLAIN TESTS only (a `tests/` file in the
   crate that owns the seam; no production code change, no `#[ignore]`, no new dependency, multi-line literals), printing
   the numbers to stdout so that a CI log shows them.

Production fixes are separate tasks after a decision; do not make them. Do not touch `.rust-guard/stages.toml` (no
campaign: nothing but docs and the optional test file changes).

## Observable acceptance

The document exists, every claim cites file and line, the measurement test (if added) compiles under
`cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt` is clean, layout is OK. Final report lists the
document sections, the test file if any, what must be observed in CI, and your proposal in five lines.
