# TOKEN-AUTH1 item 1 design

## Design

Item 1 adds wire-independent HTTP-login scope encoding, transcript construction, proof verification and server-signature helpers in `shamir-connect`; it does not add HTTP routing or pending-handshake state. Scope encoding validates label controls and bounds, database syntax/order/count, and checked aggregate size before allocating. Transcript construction validates protocol metadata, canonical username byte equality, client KDF limits, nonce constraints, and scope structure before allocation. Lockout pair status is a synchronous read-only probe that reports only active deadlines while retaining the last-failure timestamp. The scheduler test implementation delegates to its initialized in-memory store. Token label and database caps are 128 and 255 bytes.

## Progress

- Item 1 source changes are in progress and have not been fully reviewed. No acceptance claim is made.
- The active test topic is `http_login/tests.rs`, explicitly wired by `http_login/mod.rs`; the nested `http_login/tests/mod.rs` artifact remains nonempty and unused.
- No commands, builds, tests, formatting, or layout checks were run, per instruction. Runtime verification is pending CI.

## Remaining review risks

The transcript-vector test currently uses `serde_json`, which is not listed in `shamir-connect/Cargo.toml`; resolve this without changing Cargo files. Review the test source and all touched files for compile-time and behavioral consistency before claiming completion. Work is limited to numbered item 1; later endpoint, admission, pending-state, and transport work has not begun.
