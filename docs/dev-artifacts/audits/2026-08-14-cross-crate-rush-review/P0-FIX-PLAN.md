# P0 fix plan (from the 2026-10-24 revalidation)

Source: [P0.md](P0.md), revalidated at master `b0e1ddf7`. Open P0 cards: **114**
(`confirmed-open` 108, `partially-fixed` 4, `unverified` 2), by scoped risk: high 41, medium 46,
low 25, nit 2. P is the plan priority, not severity, so the order below is by risk and blast radius.

## How every stage is executed

- One stage = one PR from its own worktree (`worktrees/<slug>`), brief committed first under
  `docs/dev-artifacts/prompts/remediation/`, executor + reviewer pass, orchestrator verifies the diff.
- TDD: each card gets a failing regression test that names the card ID, pushed together with the fix
  (red/green observed in the PR's CI, never locally). A test that only shows the new code runs is
  not acceptance; it must fail against `b0e1ddf7`.
- Local gates only: `cargo check` / `cargo clippy -p <crate> --all-targets -- -D warnings` /
  `cargo fmt -p <crate>` / `python -I scripts/ci/check_layout.py`. Merge after the full matrix is
  green (`gh pr ready`), squash.
- Up to five stages run in parallel when their crate sets are disjoint. After a stage merges, its
  cards are re-decided (one reviewer pass) and P0.md is updated in place.

## Wave A — data integrity and durability (high)

| Stage | Crate(s) | Cards | Acceptance |
|---|---|---|---|
| A1 cache coherence | shamir-storage | RV-STORAGE-98F6650D, RV-STORAGE-E70A0DDC, RV-STORAGE-897437C3 | A concurrent write is never masked by a cache fill; every cache mutation is ordered with the inner store; `flush` returns an error instead of hanging when the write worker died. |
| A2 MVCC / WAL outcome | shamir-tx, shamir-wal, shamir-engine | RV-TX-2CB455A7, RV-WAL-3B29232E (remainder), RV-WAL-A025886E | A failed `history.transact` rolls the RecordCell back; `WalRequiresReopen` in explicit transactional batches is reported as an unknown outcome, not `aborted`; a failed sidecar removal can no longer resurrect stale metadata. |
| A3 interner cursor | shamir-engine, shamir-types | RV-TYPES-2A1AB215 | The durable interner ID cursor comes from the persisted high-water mark, not from the hydrated tuple count. |
| A4 catalogue / DDL durability | shamir-db | RV-DB-093EAA95, RV-DB-6540DC7B, RV-DB-42E3530E, RV-DB-E301E1EF | Renames write the new durable record before removing the old one; catalogue persistence errors propagate; cascade drops report per-table errors; removing a member of a missing group fails without creating it. |
| A5 index durability | shamir-index | RV-INDEX-2F3CBCA3, RV-INDEX-4651AA00, RV-INDEX-9F62C8DF, RV-INDEX-86978429 | Restart never reuses a delta index below the absorption watermark; replay and compaction failures fail closed (no snapshot or swap of an incomplete graph); `SortedIndexManager::load` propagates store errors. |
| A6 index correctness | shamir-index | RV-INDEX-56A505F0, RV-INDEX-A570106D | Dec/Big/Bin values hash distinctly in the functional backend; tokenizers case-fold non-ASCII words. |
| A7 NUMA replicas | shamir-numa | RV-NUMA-6E1761A2, RV-NUMA-C54E6A13 | `rcu`/`store` cannot publish an older value over a newer replica; the multi-threaded test asserts every node. |

## Wave B — security and resource boundaries (high)

| Stage | Crate(s) | Cards | Acceptance |
|---|---|---|---|
| B1 server auth path | shamir-server | RV-SERVER-17329FC9, RV-TRANSPORT-TCP-B5244FA2, RV-SERVER-C73466A7 | Bootstrap-token TTL is checked at proof time; an unauthenticated connection is dropped after a bounded handshake deadline; TxBegin/TxExecute/TxCommit honour `NodeMode::ReadOnly`. |
| B2 session layer | shamir-connect | RV-CONNECT-8A65768F, RV-CONNECT-06AB39CF, RV-CONNECT-8AD46452, RV-CONNECT-8AD8B1C8, RV-CONNECT-0FC58D93, RV-CONNECT-D8C2825A, RV-CONNECT-ECE3E157, RV-CONNECT-FED6910E | Audit chain memory is bounded; the refill watermark uses `fetch_max`; the public dispatch applies the post-auth limit; resume respects `MAX_SESSIONS_PER_USER`; the TOFU pin fires only after identity verification; no lock is held across an all-sessions scan; persistence failure is distinguished from replay; the `client` feature builds alone. |
| B3 wasm sandbox | shamir-wasm-host, shamir-funclib | RV-WASM-HOST-5E65E165, RV-WASM-HOST-2E9CF487, RV-WASM-HOST-6F709AC2, RV-WASM-HOST-A6416C6A, RV-WASM-HOST-8AFCE04E, RV-WASM-HOST-D2732518 | The forbidden-macro scan works on tokens, not text; fuel values above `i64::MAX` are clamped; the aggregate budget is reserved before a nested call and debited on cancellation; the Argon2 semaphore never loses a wakeup; the aggregate-fuel test fails against per-Store fuel. |
| B4 host gateway / compile | shamir-db | RV-DB-C9444758, RV-DB-B40771A1 | Header names/values and method with CR/LF or curl-config syntax are rejected; Rust source compilation runs on the blocking pool. |
| B5 parser and allocation limits | shamir-funclib, shamir-types, shamir-query-types | RV-FUNCLIB-84DD4DA9, RV-FUNCLIB-CDACB2E5, RV-TYPES-BB78489C, RV-QUERY-TYPES-8257ACB4, RV-FUNCLIB-64320F3C, RV-FUNCLIB-F1354271 | `is_json` has a depth cap; `repeat`/`pad_*`/`random_bytes` enforce a byte cap; msgpack preallocation is capped by the remaining input; planner walks are iterative or depth-bounded; regexes are cached without a global mutex. |
| B6 transports | shamir-transport-ipc, shamir-transport-ws | RV-TRANSPORT-IPC-EFF55124, RV-TRANSPORT-IPC-3C21F8FA, RV-TRANSPORT-WS-F54426DC, RV-TRANSPORT-WS-F5B1F757, RV-TRANSPORT-WS-4FC6BCDD | A cancelled Windows accept leaves the listener usable; a failed `set_permissions` removes the socket; native WS acceptance enforces the Origin policy; endpoint paths come from shared constants; `accept_browser_ws` has integration tests. |

## Wave C — clients and guest SDK (high/medium)

| Stage | Crate(s) | Cards | Acceptance |
|---|---|---|---|
| C1 Rust client lifecycle | shamir-client | RV-CLIENT-93C26EBF, RV-CLIENT-B42B8576, RV-CLIENT-47644B74, RV-CLIENT-E38BE183 | Reader exit closes every subscription channel and fails every pending request, including one registered during the drain. |
| C2 Rust client refs and resume | shamir-client | RV-CLIENT-5212ED16, RV-CLIENT-236DAD0A, RV-CLIENT-DE75ADDD | Reference detection covers `when` guards, `ForEach` and `ValueCompare` operands; resume verifies the pinned server identity before sending the ticket. |
| C3 Node binding | shamir-client-node | RV-CLIENT-NODE-1CFB3796, RV-CLIENT-NODE-8FB7D9BF | JS calls accept a timeout / abort signal; `host` accepts hostnames. |
| C4 query builder | shamir-query-builder, shamir-query-types | RV-QUERY-BUILDER-63B52B5C, RV-QUERY-BUILDER-2FD16C76, RV-QUERY-BUILDER-F3ED5A33, RV-QUERY-BUILDER-224EF7B1, RV-QUERY-BUILDER-20D9732A, RV-QUERY-BUILDER-7B79D840, RV-QUERY-BUILDER-E8D88CD7 (remainder) | Duplicate aliases and unknown handles are errors; nested batch dependencies are accepted; ref collection matches the planner; `Doc::set` is fallible and does not round-trip per field; SELECT/HAVING refs reach the production dependency path. |
| C5 query macros | shamir-query-builder-macros | RV-QUERY-BUILDER-MACROS-36328515, -A63844FD, -2C459E8E, -30DA1AC8, -E24D5F62, -E50875AB, -FB738525, -7E797992 | Expansion uses only the documented paths; a `trybuild` compile-fail suite covers every diagnostic; the listed silent-drop / duplicate-key / misleading-error cases fail with precise messages. |
| C6 guest SDK ABI | shamir-sdk, shamir-sdk-macros | RV-SDK-8190492D, RV-SDK-A68C70DE, RV-SDK-MACROS-4DB0D871, RV-SDK-MACROS-853FC350, RV-SDK-MACROS-D44EF500, RV-SDK-MACROS-FEC0E5E4, RV-SDK-MACROS-D3AD2579, RV-SDK-MACROS-68D9A5D0, RV-WASM-HOST-4C69CFE0, RV-WASM-HOST-B1A78B5B | Host-import buffers are freed on both sides; decode/encode failures surface as errors; validator params never coerce to Null; a Pending guest future is an explicit error; duplicate HTTP headers survive the codec; colon-prefixed keys round-trip. |

## Wave D — design decisions first, then fixes

These need an owner ruling before a brief can state the acceptance:

| Card(s) | Decision needed |
|---|---|
| RV-FUNCLIB-5FA455F7 | One equality semantics for `Value` (`==` variant-strict vs `compare` value-strict): which one filters, indexes and `DISTINCT` use. |
| RV-TYPES-F28FAFBF | Wire representation for Dec / Big / Set (protocol change and version gate) or documented degradation. |
| RV-TRANSPORT-WS-C0C43A18 | Implement the spec's WebSocket subprotocol negotiation or change the spec. |
| RV-CONNECT-0A42F42C | Capped login: implement the specified all-active rejection or change the spec to eviction. |
| RV-CLIENT-D4D604CA | The card's own title is a non-finding (no `unsafe`, sound SAFETY argument); reclassify to `not-applicable` unless a concrete obligation is named. |
| RV-TRANSPORT-TCP-0A23323D, RV-SDK-MACROS-68D9A5D0 | `unverified`: name the missing premise (provider auto-install, E0207 reachability) before fixing. |

## Wave E — performance, tests, docs (medium/low)

Batched per crate, after waves A–C: RV-TYPES-93D11F4F, RV-TYPES-1C6C4F54, RV-TYPES-6866D8B8,
RV-TX-088DE4B5, RV-TX-D54D3EE5 (remainder), RV-COLLECTIONS-91305482, RV-COLLECTIONS-D5FF5F41,
RV-COLLECTIONS-E214C501, RV-FUNCLIB-18A4B82E, RV-FUNCLIB-4936C6ED, RV-FUNCLIB-C8BC39BC,
RV-CONNECT-2FADEBC4, RV-CONNECT-CA7F732D, RV-STORAGE-CCF2090E (remainder), RV-TUNABLES-A3D3695D,
RV-TRANSPORT-IPC-07D10AFD, RV-TRANSPORT-IPC-466D0307, RV-TRANSPORT-IPC-4E8BE4FD,
RV-TRANSPORT-TCP-3A576732, RV-TRANSPORT-TCP-7DFC88FF, RV-BENCH-UTILS-E1F3D036,
RV-BENCH-UTILS-E6177532, RV-BENCH-UTILS-126E5E6F, RV-BENCH-UTILS-325F3AC6, RV-BENCH-UTILS-A7605FF1.

## Order and parallelism

1. Wave A and B in parallel lanes by disjoint crates: A1 (storage), A2 (tx/wal/engine), A4 (db),
   A5+A6 (index), B2 (connect) first; then A3, A7, B1, B3, B4, B5, B6 as lanes free up (A4 and B4
   share shamir-db, run them one after another).
2. Wave C after the crates it touches are free; C1 → C2 sequential (same crate).
3. Wave D rulings are collected while A–C run; the resulting stages join the queue.
4. Wave E last, one PR per crate.
