# P2 fix sprint - final report (2026-10-30)

Scope: the 339 active P2 cards of the 2026-08-14 cross-crate review, re-verified against master `7f04e096` on 2026-10-29 (`P2-REVALIDATION-2026-10-29.md`) and grouped into 19 crate-based executor stages plus P2-U01 (`P2-FIX-TASKS.md`).

## Outcome

- 19 of 19 executor stages were implemented by rush executors in isolated worktrees, independently reviewed by a read-only rush reviewer, accepted by the orchestrator and merged into the integration branch `integration/p2-w1`. P2-U01 (8 statically undecidable cards) was closed by a design-authority decision without code.
- The integration branch was verified in the fork `rmyva/shamir-db` (PR #5, draft first: fast CI tier = fmt, clippy and the lib tests of every crate, 8,600+ tests). Fast runs: first 12 failed tests, second 5, third 4, the fourth and the sixth (complete tree `8e89bf23`) green. Then the single full-matrix run (`gh pr ready`) on the final SHA; its result and the upstream landing are recorded in the landing PR description, not here, so that the verified SHA and the landed SHA stay identical.
- Nothing was compiled or tested on the developer machine beyond `cargo check`, `cargo clippy`, `cargo fmt` and the layout checker, scoped to the touched crates.

## How a stage was accepted

executor (brief + decisions, no builds or tests) -> orchestrator diff read, commit and local gates -> independent review (`PASS`, `PASS_WITH_NOTES` or `FAIL`) -> follow-up rounds until accepted -> merge into the integration branch -> fork CI. Typical first-pass `FAIL` causes: wrong test oracles, fail-open siblings of a fixed authorization defect, a feature-gated test compiled without its feature, unmounted files left by test relocations.

## Cards not completed or not as originally recommended

Everything not listed here was implemented and reviewed (behavioural results come from CI only).

| Stage | Card | State | Note |
|---|---|---|---|
| P2-01 db | RV-DB-C2DCAC96 | DEFERRED | facade builder migration (owner decision) |
| P2-01 db | RV-DB-7C59E01B | ACCEPTED UNCHANGED | retained admin/schema lock keys |
| P2-01 db | RV-DB-39178D2D, 3EB43AC7 | PARTIAL | implemented, no cost-discriminating regression |
| P2-02 tx | RV-TX-7BB0D16C | DEFERRED | A10 barrier vs GC starvation: no proven safe epoch handoff, barrier unchanged |
| P2-02 tx | RV-TX-4A1FDAAE | SKIPPED | owner decision |
| P2-04 engine | RV-ENGINE-3C71057D | NOT DONE | batch-scoped FK parent reuse |
| P2-04 engine | RV-ENGINE-62C6F2B1 | PARTIAL | borrowed field-path lookup only, no plan-local condition slots |
| P2-05 index | RV-INDEX-4DC29EFB | DEFERRED | streaming snapshot (owner decision) |
| P2-05 index | RV-INDEX-40BCFF22 | PARTIAL | v1 layouts decode only with a supported hnsw_rs graph version; a 0.3.4 graph is rejected and the index rebuilds from source |
| P2-06 index | RV-INDEX-53CEF405, 9E472725, FBB72544 | DEFERRED | persisted hash identity redesign ("index identity v2"), only golden vectors + KNOWN_LIMITATIONS |
| P2-06 index | RV-INDEX-605EEC5D, F39434C2 | PARTIAL | golden vectors only; logging without a capture test |
| P2-09 client | RV-CLIENT-66E9A5FC | DEFERRED | allocation oracles and benchmarks need CI measurement |
| P2-10 connect | RV-CONNECT-387EAA05 | PARTIAL | remaining `UnixNanos::now`, Argon2 semaphore, session touch and Ed25519 generation panic paths |
| P2-10 connect | RV-CONNECT-E4631CD9, 7D41FB60 | SKIPPED | owner decision / aggregate |
| P2-10 connect | RV-CONNECT-74705EAB | FALSE FINDING | "SHAMIR-AUTH-v1" is 14 bytes, the message 149; the stage briefly "fixed" the oracles to 13/148, CI caught it, the original assertions were restored |
| P2-12 types | RV-TYPES-A7B1FCEC, 71AFA12F, E82C6BE1, F070E432, FE40CB54, 313627EB | PARTIAL | implemented; no allocation-count oracles; 32-bit-only oracle for F070E432 |
| P2-13 query-types | RV-QUERY-TYPES-0A0D9092, 4F767AA5, 3D922433, D687DBA3 | PARTIAL | nested metadata reuse, caller migration off the lossy conversion adapter (`Doc::set`), allocation-free Big comparison, FieldPath file placement |
| P2-13 query-types | RV-QUERY-TYPES-182C703E, CC015A3C | SKIPPED | by decision |
| P2-14 query-builder | RV-QUERY-BUILDER-MACROS-13A0D25A | PARTIAL | span assertions, no compiler diagnostic-underlining fixtures |
| P2-14 query-builder | RV-QUERY-BUILDER-7A0F3500 | PARTIAL | DDL owner-file placement still under `config` |
| P2-16 wasm-host | RV-WASM-HOST-FF7C2D6E | SKIPPED | superseded |
| P2-18 net | RV-TRANSPORT-TCP-31C5E8A6 | PARTIAL | measurement recipe only |
| P2-18 net | RV-TRANSPORT-TCP-79910BA5 | NOT DONE | needs an edit of the shamir-client hot path |
| P2-18 net | RV-SERVER-8999F6B7 | PARTIAL | fixture does not use the real client DTO |
| P2-18 net | RV-TRANSPORT-IPC-FA481516 | SKIPPED | accepted risk, documented |
| P2-U01 | 8 undecidable cards | CLOSED | 5 not a defect (F53F775F, D5F412E2, C5E06B98, DD871E71, 84C0E18F), 3 accepted risk (CEE352CA, 1220BF81, 9BCA14B1) |

Test oracles that also pass on the old code (reviewers' notes, not blockers): P2-04 (BE59E793, 931318A1, 62C6F2B1), P2-07 (scalar expression tests, timestamp helper), P2-09 (DDL status, id encoding), P2-12 (borrowed metadata wrapper, secret wipe), P2-13 (scalar accessors, pagination compatibility), P2-17 (range tests use the helper, not `from_raw`; consumer fixtures are native-only).

## Disclosures for the owner

- New optional config key `[replication].upstream_pins` (SHA-256 public-key pins per upstream address, Ktav values unquoted) with in-memory TOFU continuity and a new terminal subscription state `upstream_key_mismatch`; unpinned subscriptions keep working and re-learn the key after every restart.
- New fixed FTS n-gram limits per field value: 1,048,576 grams and 16 MiB; exceeding them is a typed `TokenBudgetExceeded`. A Building index that hits it during table-open recovery is marked Failed (planner-invisible, repair or DROP INDEX) instead of failing table open.
- Source-level API breaks (authorised): `TicketWire::to_bytes` and `start_change_password_challenge` are fallible; interner hydration returns `Result`; `ResponseBytes::from_raw` returns `Result` and is wasm32-only; `Doc`/builder additions (`execute_owned`, consuming requests); `SecureResumeOptions` / `Client::resume_secure`; `FieldPathCache` key type.
- WAL: encoded frames above 64 MiB fail recovery in every mode (no truncation); WMT3 sidecar format (WMT2 still readable).
- Index identity: no persisted index format change; hash-identity risks accepted (strongest: RV-INDEX-9E472725, false `DuplicateKey` on composite numeric unique indexes) with a follow-up "index identity v2".
- No third-party dependency was added; direct dependencies removed: argon2, rustls, tokio-rustls, rcgen (shamir-db), serde (shamir-wasm-host), four dev-dependencies of shamir-transport-ws, one of shamir-connect; `thiserror` (already a workspace dependency) added to shamir-query-builder; the procedure example no longer needs the workaround dependency.
- `shamir-numa`: `detect_cached()` (process-wide) used by index constructors; generator revision bumped to `clustered_vectors/2` (the `Lcg::next_f32` mapping changed).
- Two real races were found by the first CI run and fixed: drainer startup seeding after foreground finalisation (now under the replay mutex and filtered by the durable watermark) and WAL retained-bytes measurement racing truncation (now holds the truncation gate). Further drain tests assumed no background drainer and were isolated.

## Follow-ups (not scheduled)

- "Index identity v2" (53CEF405 / 9E472725 / FBB72544); facade builder migration (C2DCAC96); streaming snapshot (4DC29EFB); client hot-path framing reuse (79910BA5); allocation-count oracles and benchmarks in CI (66E9A5FC, A7B1FCEC, E82C6BE1, 313627EB); remaining panic paths of 387EAA05; compiler-diagnostic fixtures (13A0D25A); binding-level credential tests for client-node.
- New finding from P2-U01: `WireReplSource` is shared by all pull targets of one subscription and long polls hold its client lock up to `poll_wait_ms` (default 5000), so N targets can wait on each other; the comments in `wire_source.rs` and `prod_factory.rs` calling it "single-tasked/uncontended" are stale.
- Process: draft-PR CI cannot see the wasm32 target or the full matrix; the feature-gated and Windows-only failures surface only in the single full run.
