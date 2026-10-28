# P1 stage G7 — p1-transport-ws

## Goal

WS: codec ceilings, close codes, heartbeat, send cap, ping flood. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row G7): Codec ceilings admit the full documented payload range; rejections use the specified close codes; heartbeat/Pong deadline implemented or the spec changed; send has a frame cap without length truncation; control-frame loops are bounded; dead variants/APIs removed; send path avoids the extra copy.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-TRANSPORT-WS-C038F1F3 — Accepted WS codec ceilings exclude the final four bytes of the documented logical payload range

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-transport-ws/src/server.rs:85; crates/shamir-transport-ws/src/framing.rs:154; changed by: none
- R0 note (includes the fix site): OBSERVED: both codec ceilings remain 16 MiB including the four-byte prefix, while logical validation permits a 16-MiB payload; payload sizes M−3 through M therefore exceed the assembled-message ceiling. Fix site: crates/shamir-transport-ws/src/server.rs must account for prefix overhead in codec limits.

### RV-TRANSPORT-WS-B0F1151C — Transport rejection discards the specified WebSocket close-code semantics

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-server/src/sync/framer.rs:430; crates/shamir-server/src/sync/framer.rs:420; changed by: none
- R0 note (includes the fix site): OBSERVED: Text and malformed-length errors still lose WS close semantics in generic framing errors, codec errors become strings, and shutdown sends Close(None); the intervening diff only adds TCP EOF handling. Fix site: crates/shamir-server/src/sync/framer.rs must preserve and propagate explicit rejection close codes.

### RV-TRANSPORT-WS-A3B8161A — Specified server heartbeat and Pong deadline have no production implementation

- R0 verdict: `confirmed-open`, risk `medium`; evidence: docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md:77; crates/shamir-server/src/sync/connection/request_loop.rs:178; changed by: none
- R0 note (includes the fix site): OBSERVED: the specification still promises server Ping every 30 seconds and a 10-second Pong deadline; the writer only receives application messages and framing discards controls without heartbeat tracking. Fix site: crates/shamir-server/src/sync/connection/request_loop.rs needs heartbeat scheduling and correlated Pong handling, with the reserved-1006 specification corrected first.

### RV-TRANSPORT-WS-624F81B1 — Send path has no frame-size cap and truncates the length prefix at `u32::MAX` — diverges from the TCP sibling

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-transport-ws/src/framing.rs:118; crates/shamir-server/src/sync/framer.rs:360; changed by: none
- R0 note (includes the fix site): OBSERVED: WS send still narrows usize to u32 and allocates/copies before any outbound cap, and the production writer delegates without a guard. INFERRED: over-cap sends remain accepted locally and lengths wrap starting at 2^32 bytes, subject to sink/allocation failure. Fix site: crates/shamir-transport-ws/src/framing.rs before narrowing or allocation.

### RV-TRANSPORT-WS-B8ED1366 — Unbounded control-frame loop in `ws_recv_into_stream` (ping-flood liveness)

- R0 verdict: `partially-fixed`, risk `medium`; evidence: crates/shamir-server/src/sync/connection/handshake.rs:256; crates/shamir-transport-ws/src/framing.rs:140; changed by: ef893fbc (#282)
- R0 note (includes the fix site): OBSERVED: an absolute effective-KDF handshake budget now encloses challenge write and proof read, and terminal handshake shutdown has a two-second grace, closing the indefinite proof-stage lifetime witness; the adapter still loops over controls without a count or byte budget. Fix site: crates/shamir-transport-ws/src/framing.rs remains the site for bounding control-only work rather than relying solely on caller timeouts.

### RV-TRANSPORT-WS-6C58DE5C — `WsAcceptError::OriginRejected` and `WrongPath` are dead variants — all accept rejections surface as `Handshake`

- R0 verdict: `partially-fixed`, risk `low`; evidence: crates/shamir-transport-ws/src/server.rs:167; crates/shamir-transport-ws/src/server.rs:246; changed by: 493d30d3 (#285)
- R0 note (includes the fix site): OBSERVED: both acceptors now construct WrongPath from HTTP 404 and preserve the actual path; OriginRejected remains unconstructed, with origin rejection falling through to Handshake. Fix site: crates/shamir-transport-ws/src/server.rs must preserve and return the callback's typed origin rejection.

### RV-TRANSPORT-WS-B7D06174 — WSS send hot path: fresh heap alloc + full-payload copy per frame; TCP's prereserved zero-copy path is silently defeated

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-server/src/sync/framer.rs:123; crates/shamir-transport-ws/src/framing.rs:118; changed by: none
- R0 note (includes the fix site): OBSERVED: the production writer uses the default prereserved adapter, strips the prefix, then ws_send_sink allocates another buffer and copies the complete payload; no WS override removes this work. Fix site: crates/shamir-server/src/sync/framer.rs needs an ownership-compatible WS prereserved send path; runtime impact remains unmeasured.

### RV-TRANSPORT-WS-A8502651 — `Option`-returning exporter API + public all-zeros constant invites silent zero-substitution on the native path

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-transport-ws/src/tls_exporter.rs:20; crates/shamir-server/src/engine/server/server_launcher/accept_loops.rs:341; changed by: none
- R0 note (includes the fix site): OBSERVED: exporter errors are still erased into Option and the native listener substitutes zeros on None; normal completed-TLS extraction failure remains unestablished, so this is fail-closed API debt rather than demonstrated authentication bypass. Fix site: crates/shamir-server/src/engine/server/server_launcher/accept_loops.rs must reject native extraction failure instead of substituting zeros.

### RV-TRANSPORT-WS-0B4767DE — Zero-length frame means "graceful close" on TCP but is a legal empty frame on WS — undocumented divergence in a claimed-identical wire format

- R0 verdict: `confirmed-open`, risk `nit`; evidence: crates/shamir-transport-ws/src/framing.rs:154; crates/shamir-transport-tcp/src/framing.rs:80; changed by: none
- R0 note (includes the fix site): OBSERVED: a four-zero-byte WS Binary message still returns an empty payload, whereas TCP returns PeerClose; TCP's new writer restriction does not document this WS distinction. Shared layout alone does not promise identical close semantics. Fix site: crates/shamir-transport-ws/src/framing.rs for an explicit transport-semantics clarification.

### RV-TRANSPORT-WS-82800EF6 — Exporter-extraction ordering: this crate's doc contradicts its only production caller

- R0 verdict: `confirmed-open`, risk `nit`; evidence: crates/shamir-transport-ws/src/server.rs:5; crates/shamir-server/src/engine/server/server_launcher/accept_loops.rs:338; changed by: none
- R0 note (includes the fix site): OBSERVED: transport docs still require extraction AFTER upgrade while the launcher says BEFORE because TLS becomes inaccessible; the current e2e source accesses TLS through ws.get_ref() after upgrade. Fix site: crates/shamir-server/src/engine/server/server_launcher/accept_loops.rs must clarify that pre-upgrade extraction is a choice, not an access restriction.

### RV-TRANSPORT-WS-82B13B1B — `BROWSER_CHANNEL_BINDING` is dead public API; the zero placeholder exists only in prose elsewhere

- R0 verdict: `confirmed-open`, risk `nit`; evidence: crates/shamir-transport-ws/src/tls_exporter.rs:25; crates/shamir-server/src/engine/server/server_launcher/accept_loops.rs:448; changed by: none
- R0 note (includes the fix site): OBSERVED: repository search finds only the constant's declaration, while production supplies a real zero literal; only the unused-API facet holds, not the prose-only allegation. Fix site: crates/shamir-server/src/engine/server/server_launcher/accept_loops.rs can use the public browser-binding constant.

## Binding rulings

These decisions are final; do not re-litigate them.

### Binding ruling RU2 — WebSocket heartbeat / Pong deadline (RV-TRANSPORT-WS-A3B8161A)

Decided by 2x rush reviewer + @oh: **option A, implement the specified heartbeat in production** (3/3). The only disagreement was the close code on a missed Pong: the reviewers proposed 1008, @oh a private 4003. Final: **4003 `heartbeat_timeout`** (the spec already uses private 4xxx codes at TRANSPORT_WS.md ~lines 71-73; 1008 means policy violation, and 1006 is reserved and must never be transmitted).

Facts: TRANSPORT_WS.md section 7 promises a 30 s server Ping and a 10 s Pong deadline; nothing implements it (the receive loop in `crates/shamir-transport-ws/src/framing.rs` just skips Ping/Pong; the writer in `crates/shamir-server/src/sync/connection/request_loop.rs` only waits on `rx.recv()`). Nothing in `crates/` sets TCP keepalive, so the only liveness check is the 600 s idle sleep. Browsers and the Node `ws` package answer Pings automatically; the native Rust client has no WebSocket transport.

Acceptance:
1. Spec (same PR as the code): TRANSPORT_WS.md section 7 says: server Ping every 30 s with an 8-byte opaque payload; a Pong counts only if its payload equals the latest Ping; the 10 s deadline starts when the Ping has been written; a miss causes Close 4003 `heartbeat_timeout`, then the socket is dropped; 1006 is never transmitted; Pongs do not reset the idle timer. Section 6 gains a 4003 row.
2. Two constants in `shamir_tunables::instance_defaults`: `WS_PING_INTERVAL = 30 s`, `WS_PONG_TIMEOUT = 10 s`, passed to the loop through `ConnectionContext` the way `idle_timeout` is. No new config keys.
3. The heartbeat belongs to the transport: `FrameWriter` gets a heartbeat capability (WS sends a Ping; TCP reports unsupported); `request_loop` arms it only when the capability exists. A test asserts the TCP framer sends no extra bytes and closes as before.
4. Per-connection state: one `Arc` with atomic `last_sent`, `last_acked`, `reader_parked`; one pinned `Sleep` reused via `reset()` inside the writer's `select!` next to `rx.recv()`; no extra task, no lock, no per-tick allocation besides the 8-byte payload. The timer lives in the WRITER task (the reader stops polling the socket while it waits for a permit).
5. The WS reader records a matched Pong (payload == `last_sent`) into `last_acked` before the code that skips control frames; a wrong, stale or unsolicited Pong is ignored for the deadline.
6. On deadline: if `last_acked < last_sent` and the reader is reading the socket, the writer sends Close(4003, "heartbeat_timeout") with a 2 s limit and the loop tears down; if the reader is parked waiting for a permit (server back-pressure), re-arm instead of firing.
7. The final writer shutdown after the loop also gets the 2 s bound. Add ONE coded-Close method to `FrameWriter` and use it for both this card and the close-code card RV-TRANSPORT-WS-B0F1151C (G7 does both; no duplicate API).
8. Tests with virtual time (`tokio::time::pause` + `advance`) driving the real `request_loop` with `WsFramer::split()` over `tokio::io::duplex` and a tungstenite client: (a) after 30 s the client receives an 8-byte-payload Ping; (b) an auto-ponging client stays open through at least 5 intervals; (c) a client that never reads gets Close 4003 at ~40 s and `request_loop` returns; (d) a Pong with a mismatched payload does not prevent (c); (e) a client that only answers Pings still hits the 600 s idle close; (f) a reader blocked on permits (`max_in_flight = 1`, one dispatch held) is not killed; (g) no Close frame ever carries 1006.
9. Ping-flood bound (RV-TRANSPORT-WS-B8ED1366): matched Pongs (at most one per interval) do not count toward the control-frame budget; a test runs Pings at the budget limit while the heartbeat runs and checks neither trips the other.

Compat: no wire format or disk change; a WS peer sees one Ping every 30 s and, if it fails to Pong, Close 4003 instead of hanging until the 600 s idle close; TS clients ignore close codes. Non-goal: a writer stuck on a dead peer with a full TCP buffer belongs to the send-path card RV-TRANSPORT-WS-624F81B1.


## Requirements

1. Before editing, confirm each mechanism in current code (read callers and guards). If a card is already fixed or wrong at HEAD, do not change code for it: say so with file:line evidence in the final message.
2. Fix at the narrowest layer that owns the invariant. No unrelated refactors, renames or comment edits.
3. TDD for runtime cards: add a regression test whose name contains the card's hex suffix in lower case (for example `rv_storage_98f6650d_...`). The test must fail on the unfixed code and pass on the fix — reason about that from the code, you cannot run it. Tests live in the module's `tests/` folder (`tests/mod.rs` is a manifest only; no inline `#[cfg(test)] mod tests {}`) and are registered there. Prefer deterministic constructions (injected failures, barriers, paused clocks) over sleeps and timing. Docs-only, layout-only and comment cards need no test: say so in the final message.
4. House rules: imports at the top of the file; `mod.rs` re-exports only; one primary export per file; at most 7 entries per folder and 1000 lines per source file for new/changed code (check_layout is the gate); Fx hashers; no `std::sync::Mutex`/`RwLock` on hot paths; `Result` instead of panics; no private machine paths in committed files; terse comments; queries only through the query builder.
5. Do not bump crate or dependency versions, do not add dependencies, do not touch CI workflows (except where a card says so).
6. Run only the allowed local gates from the policy (clippy, fmt, layout) on the touched crates until clean. If a changed public signature affects dependent crates, run clippy on those too.
7. Other agents work concurrently on other crates; stay inside the files your cards name plus their tests.

## Final message (mandatory)

1. Per card: fixed / already-fixed / refuted / docs-only, with the changed file:line and the regression test name.
2. Why each test fails on the old code (one sentence each).
3. Real exit codes of clippy, fmt --check and check_layout. Anything you could not verify.
