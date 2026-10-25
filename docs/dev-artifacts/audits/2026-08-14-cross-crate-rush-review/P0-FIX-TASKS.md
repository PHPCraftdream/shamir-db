# P0 fix tasks — execution board

Companion of [P0-FIX-PLAN.md](P0-FIX-PLAN.md) (acceptance per stage) and [P0.md](P0.md) (the cards).
Every open P0 card (114) belongs to exactly one task below. Status: `[ ]` todo, `[~]` in flight, `[x]` merged.

## Rules for every task

- Own worktree and branch (`worktrees/<slug>`, branch `fix/<slug>`), brief committed first as
  `docs/dev-artifacts/prompts/remediation/<NN>-p0-<slug>.md`.
- One failing regression test per card, named after the card ID, in the same PR as the fix.
- Local gates only (`cargo check` / `clippy -p <crate> --all-targets -- -D warnings` / `fmt -p` /
  `check_layout.py`); CI is the test environment; merge when the full matrix is green.
- After merge: re-decide the task's cards (one reviewer pass), update P0.md in place, tick the box.
- A task starts only when every task in its `after` column is merged (shared crates).

## Lanes

Five lanes run in parallel; inside a lane the tasks run top to bottom because they share crates.

| Lane | Crates | Order |
|---|---|---|
| L1 storage / tx | shamir-storage, shamir-tx, shamir-wal, shamir-engine | A1 → A2 → A3 → E-tx → E-storage |
| L2 db / clients | shamir-db, shamir-client, shamir-client-node, shamir-query-builder(-macros) | A4 → B4 → C1 → C2 → C3 → C4 → C5 |
| L3 index / numa | shamir-index, shamir-numa | A5 → A6 → A7 → E-bench |
| L4 network | shamir-connect, shamir-server, shamir-transport-* | B2 → B1 → B6 → E-connect → E-ipc → E-tcp |
| L5 wasm / limits | shamir-wasm-host, shamir-funclib, shamir-types, shamir-query-types, shamir-sdk(-macros) | B3 → B5 → C6 → E-funclib → E-types → E-collections |

Cross-lane `after` edges: A3 → B5 (shamir-types), B5 → C4 (shamir-query-types), A2 → B1 (engine
read-only gate touches the tx path only through shamir-server; kept sequential to avoid the
engine/server rebase churn).

## Tasks

### Wave A — data integrity and durability

| ID | Slug | Lane | After | Risk | Cards |
|---|---|---|---|---|---|
| [ ] A1 | p0-storage-cache | L1 | — | high | RV-STORAGE-98F6650D, RV-STORAGE-E70A0DDC, RV-STORAGE-897437C3 |
| [ ] A2 | p0-mvcc-wal-outcome | L1 | A1 | high | RV-TX-2CB455A7, RV-WAL-3B29232E, RV-WAL-A025886E |
| [ ] A3 | p0-interner-cursor | L1 | A2 | high | RV-TYPES-2A1AB215 |
| [ ] A4 | p0-db-catalogue | L2 | — | high | RV-DB-093EAA95, RV-DB-6540DC7B, RV-DB-42E3530E, RV-DB-E301E1EF |
| [ ] A5 | p0-index-durability | L3 | — | high | RV-INDEX-2F3CBCA3, RV-INDEX-4651AA00, RV-INDEX-9F62C8DF, RV-INDEX-86978429 |
| [ ] A6 | p0-index-correctness | L3 | A5 | high | RV-INDEX-56A505F0, RV-INDEX-A570106D |
| [ ] A7 | p0-numa-replicas | L3 | A6 | high | RV-NUMA-6E1761A2, RV-NUMA-C54E6A13 |

### Wave B — security and resource boundaries

| ID | Slug | Lane | After | Risk | Cards |
|---|---|---|---|---|---|
| [ ] B1 | p0-server-auth | L4 | B2, A2 | high | RV-SERVER-17329FC9, RV-TRANSPORT-TCP-B5244FA2, RV-SERVER-C73466A7 |
| [ ] B2 | p0-connect-session | L4 | — | high | RV-CONNECT-8A65768F, RV-CONNECT-06AB39CF, RV-CONNECT-8AD46452, RV-CONNECT-8AD8B1C8, RV-CONNECT-0FC58D93, RV-CONNECT-D8C2825A, RV-CONNECT-ECE3E157, RV-CONNECT-FED6910E |
| [ ] B3 | p0-wasm-sandbox | L5 | — | high | RV-WASM-HOST-5E65E165, RV-WASM-HOST-2E9CF487, RV-WASM-HOST-6F709AC2, RV-WASM-HOST-A6416C6A, RV-WASM-HOST-8AFCE04E, RV-WASM-HOST-D2732518 |
| [ ] B4 | p0-db-gateway-compile | L2 | A4 | high | RV-DB-C9444758, RV-DB-B40771A1 |
| [ ] B5 | p0-parser-limits | L5 | B3, A3 | high | RV-FUNCLIB-84DD4DA9, RV-FUNCLIB-CDACB2E5, RV-TYPES-BB78489C, RV-QUERY-TYPES-8257ACB4, RV-FUNCLIB-64320F3C, RV-FUNCLIB-F1354271 |
| [ ] B6 | p0-transports | L4 | B1 | high | RV-TRANSPORT-IPC-EFF55124, RV-TRANSPORT-IPC-3C21F8FA, RV-TRANSPORT-WS-F54426DC, RV-TRANSPORT-WS-F5B1F757, RV-TRANSPORT-WS-4FC6BCDD |

### Wave C — clients and guest SDK

| ID | Slug | Lane | After | Risk | Cards |
|---|---|---|---|---|---|
| [ ] C1 | p0-client-lifecycle | L2 | B4 | high | RV-CLIENT-93C26EBF, RV-CLIENT-B42B8576, RV-CLIENT-47644B74, RV-CLIENT-E38BE183 |
| [ ] C2 | p0-client-refs-resume | L2 | C1 | high | RV-CLIENT-5212ED16, RV-CLIENT-236DAD0A, RV-CLIENT-DE75ADDD |
| [ ] C3 | p0-client-node | L2 | C2 | high | RV-CLIENT-NODE-1CFB3796, RV-CLIENT-NODE-8FB7D9BF |
| [ ] C4 | p0-query-builder | L2 | C3, B5 | medium | RV-QUERY-BUILDER-63B52B5C, RV-QUERY-BUILDER-2FD16C76, RV-QUERY-BUILDER-F3ED5A33, RV-QUERY-BUILDER-224EF7B1, RV-QUERY-BUILDER-20D9732A, RV-QUERY-BUILDER-7B79D840, RV-QUERY-BUILDER-E8D88CD7 |
| [ ] C5 | p0-query-macros | L2 | C4 | medium | RV-QUERY-BUILDER-MACROS-36328515, RV-QUERY-BUILDER-MACROS-A63844FD, RV-QUERY-BUILDER-MACROS-2C459E8E, RV-QUERY-BUILDER-MACROS-30DA1AC8, RV-QUERY-BUILDER-MACROS-E24D5F62, RV-QUERY-BUILDER-MACROS-E50875AB, RV-QUERY-BUILDER-MACROS-FB738525, RV-QUERY-BUILDER-MACROS-7E797992 |
| [ ] C6 | p0-guest-sdk-abi | L5 | B5 | medium | RV-SDK-8190492D, RV-SDK-A68C70DE, RV-SDK-MACROS-4DB0D871, RV-SDK-MACROS-853FC350, RV-SDK-MACROS-D44EF500, RV-SDK-MACROS-FEC0E5E4, RV-SDK-MACROS-D3AD2579, RV-SDK-MACROS-68D9A5D0, RV-WASM-HOST-4C69CFE0, RV-WASM-HOST-B1A78B5B |

### Wave D — owner rulings (no code until decided)

| ID | Topic | Cards |
|---|---|---|
| [ ] D1 | One equality semantics for `Value` | RV-FUNCLIB-5FA455F7 |
| [ ] D2 | Wire representation of Dec / Big / Set | RV-TYPES-F28FAFBF |
| [ ] D3 | WebSocket subprotocol: implement or change the spec | RV-TRANSPORT-WS-C0C43A18 |
| [ ] D4 | Capped login: reject (spec) or evict (code) | RV-CONNECT-0A42F42C |
| [ ] D5 | Reclassify the non-finding | RV-CLIENT-D4D604CA |
| [ ] D6 | Name the missing premise of the `unverified` TLS provider card | RV-TRANSPORT-TCP-0A23323D |

Each ruling becomes a stage appended to the lane of its crate. `RV-SDK-MACROS-68D9A5D0`
(`unverified`) is settled inside C6 (a compile-fail test either reproduces E0207 or refutes it).

### Wave E — performance, tests, docs (one PR per crate)

| ID | Slug | Lane | After | Cards |
|---|---|---|---|---|
| [ ] E-tx | p0-tx-perf-tests | L1 | A3 | RV-TX-088DE4B5, RV-TX-D54D3EE5 |
| [ ] E-storage | p0-storage-config-tunables | L1 | E-tx | RV-STORAGE-CCF2090E, RV-TUNABLES-A3D3695D |
| [ ] E-bench | p0-bench-utils | L3 | A7 | RV-BENCH-UTILS-E1F3D036, RV-BENCH-UTILS-E6177532, RV-BENCH-UTILS-126E5E6F, RV-BENCH-UTILS-325F3AC6, RV-BENCH-UTILS-A7605FF1 |
| [ ] E-connect | p0-connect-tests | L4 | B6 | RV-CONNECT-2FADEBC4, RV-CONNECT-CA7F732D |
| [ ] E-ipc | p0-ipc-tests | L4 | E-connect | RV-TRANSPORT-IPC-07D10AFD, RV-TRANSPORT-IPC-466D0307, RV-TRANSPORT-IPC-4E8BE4FD |
| [ ] E-tcp | p0-tcp-framing | L4 | E-ipc | RV-TRANSPORT-TCP-3A576732, RV-TRANSPORT-TCP-7DFC88FF |
| [ ] E-funclib | p0-funclib-resolver | L5 | C6 | RV-FUNCLIB-18A4B82E, RV-FUNCLIB-4936C6ED, RV-FUNCLIB-C8BC39BC |
| [ ] E-types | p0-types-perf | L5 | E-funclib | RV-TYPES-93D11F4F, RV-TYPES-1C6C4F54, RV-TYPES-6866D8B8 |
| [ ] E-collections | p0-collections-contract | L5 | E-types | RV-COLLECTIONS-91305482, RV-COLLECTIONS-D5FF5F41, RV-COLLECTIONS-E214C501 |

## Start set

A1, A4, A5, B2, B3 — one per lane, disjoint crates.
