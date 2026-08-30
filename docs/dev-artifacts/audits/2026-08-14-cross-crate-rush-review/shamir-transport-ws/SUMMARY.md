# shamir-transport-ws — Consolidated 7-lens review (synthesis of the 2026-08-14 cross-crate sweep)

Crate: `crates/shamir-transport-ws/` — the WebSocket transport binding (tokio-tungstenite 0.24):
native WSS `/shamir/v1` + browser WSS `/shamir/v1/browser` with length-prefixed framing
(`[u32_be length][payload]` per BINARY message), Origin-policy-gated browser handshakes,
TLS-exporter extraction for SCRAM channel binding, and WS listener profiles. This is the
WSS front door for `shamir-server` (browser endpoint included).

Review basis — synthesis (not a fresh review) of the seven lens files produced by the
2026-08-14 23-crate × 7-lens sweep, all carried forward in full and deduped across lenses:

- `correctness-tdd.md` · `concurrency-lockfree.md` · `security-crypto.md` ·
  `performance-hotpath.md` · `api-wire-protocol.md` · `error-handling-lifecycle.md` ·
  `style-claude-md.md` (same directory)

Structure/tone/rigor calibrated against the two exemplar syntheses:
`../shamir-client-node/SUMMARY.md` and `../shamir-transport-ipc/SUMMARY.md` (the latter a
sibling transport crate). The workspace-wide `SUMMARY.md` rows for this crate (49
lens-tagged findings: 0c/3h/11m/21l/14n; verdict *"moderate — spec/interop gaps and an
untested security control"*) were used as a cross-check only — the independent recount of
the seven files below matches them exactly.

Method: read-only. No build/test/lint commands were run; no source file under `crates/` was
modified. Every load-bearing file:line citation was spot-checked against the current tree
(`server.rs`, `framing.rs`, `browser.rs`, `tls_exporter.rs`, `listener.rs`, `lib.rs`,
`Cargo.toml`, `tests/framing_round_trip.rs`); one source-file inaccuracy was found and is
corrected inline at 7.2. No new defects surfaced during spot-checking.

Finding numbering below preserves each lens file's own numbering (1.1 = correctness #1, …)
so every entry is traceable to its source. Where the same root-cause defect was flagged by
multiple lenses, the full write-up lives once under its primary lens and the other lenses
carry a short `*(dedup — primary: X.Y)*` stub.

## Executive summary

Structurally this is one of the cleanest crates in the workspace — lock-free by
construction, zero `unsafe`, `thiserror`-only errors, exemplary test/layout conformance —
but it is not shippable as documented: (1) **the endpoint paths are hardcoded string
literals while the server treats `path` as operator configuration** — a reconfigured
listener boots cleanly and then 404s every upgrade, total silent connectivity loss (5.2);
(2) **the spec §2.1 `Sec-WebSocket-Protocol: shamir-v1` negotiation is entirely
unimplemented** — a spec-conformant browser client fails the connection itself after the
101, and the spec's mismatch→400 downgrade defense never fires (5.1); (3) **the browser
endpoint's primary anti-CSWSH control (Origin enforcement) is wired by zero tests anywhere
in the workspace**, while the typed `WsAcceptError` taxonomy advertises Origin/path
classification it can never deliver (1.1, 6.1) — so the security control can be silently
regressed green. Fix those three, plus the one-line manifest fix that removes the phantom
`tungstenite 0.29` parser copy (3.1), before anything else ships from this crate.

---

## 1. correctness-tdd

### 1.1 [HIGH] `accept_browser_ws` — the spec §9 Origin enforcement path — has zero test coverage *(primary of the accept-wiring test group; also 3.2, 5.6, 6.2)*
- **File:line:** `crates/shamir-transport-ws/src/server.rs:108-146`; absent from
  `src/tests/server_tests.rs` (native-only) and `src/tests/browser_tests.rs` (pure
  `validate_origin` only); confirmed workspace-wide: only non-test reference is
  `shamir-server/src/server/server_launcher.rs:1266`.
- **Issue:** Only the pure function `validate_origin` is tested. Nothing tests the handshake
  wiring it is embedded in: the header extraction (`req.headers().get(ORIGIN)` +
  `to_str().ok()`, where a non-UTF-8 header silently maps to `None`→`Missing`), the 403
  `ErrorResponse` on rejection, the wrong-path 404, or that the policy actually gates the 101
  upgrade. `shamir-server/tests/mvp_ws_e2e.rs` exercises only `/shamir/v1` (native). The
  repo's own NEW-1 work sets the standard of live-wiring tests for accept-path security caps
  (`live_accept_rejects_message_over_cap`); the Origin control never got the equivalent.
- **Failure scenario:** A refactor of the callback (inverted check, dropped `validate_origin`
  call, dropped `move` capture falling back to an empty default policy, path-string typo,
  header name change, validation moved after the upgrade response) silently disables the
  primary cross-site WebSocket-hijacking defence — and the entire suite stays green.
- **Suggested fix:** Add live handshake tests in `src/tests/server_tests.rs` mirroring the
  existing `live_accept_*` duplex pattern: (a) allowed Origin → upgrade succeeds (101 + frame
  round-trip); (b) missing Origin → handshake fails with HTTP 403; (c) disallowed Origin →
  403, no 101; (d) wrong path on both endpoints → 404; (e) non-UTF-8 Origin value → rejected.

### 1.2 [MEDIUM] Framing's malformed-input error paths untested; `rejects_oversized_frame` is under-asserted *(primary of the framing-error-path test group; also 5.6, 6.2)*
- **File:line:** `tests/framing_round_trip.rs:57-71` (asserts only `is_err()` at :69);
  `crates/shamir-transport-ws/src/framing.rs:148-153` (short-message `LengthMismatch`),
  `:157-162` (prefix-tamper `LengthMismatch` — the module doc's headline "defence-in-depth"
  invariant, framing.rs:7-9), `:173` (`PeerClose` on Close frame), `:144` (`PeerClose` on
  stream end), `:177-182` (`NonBinaryMessage` for TEXT), `:176` (Ping/Pong skip loop) — none
  tested.
- **Issue:** The only negative framing test sends a valid 200-byte frame and receives with a
  tiny cap, then asserts merely `result.is_err()`. It would still pass if the error were an
  unrelated `Io`/`Capacity` error, i.e. it does not pin the `TooLarge` contract. Every tamper
  path the module doc advertises (mismatched inner prefix, sub-4-byte message, TEXT
  rejection) is dead code as far as the suite can prove. Note these are pure logic over an
  already-assembled `Message` — trivially testable without raw-frame crafting.
- **Failure scenario:** A reorder of the `declared != body.len()` vs `TooLarge` checks, a
  flipped comparison, an accidental `Ok(())` on the TEXT arm, or a `continue` added to the
  TEXT arm would not be caught by any test.
- **Suggested fix:** (a) Assert `matches!(result, Err(WsFrameError::TooLarge { .. }))` (and
  the declared/actual fields). (b) Add unit-style tests through `ws_recv_into_stream` over a
  custom `Stream` yielding crafted `Message`s: `Binary(vec![0,0,0,9, ...])` with wrong body,
  2-byte binary, `Text`, `Close`, then `None` — one per error variant.

### 1.3 [LOW] *(dedup — primary: 6.1)* `WsAcceptError::WrongPath` never constructed; `OriginRejected(#[from])` unreachable
Full write-up at 6.1 (the error-handling lens rated it medium; this lens low). The
correctness-angle detail: verified `WsAcceptError` appears only in `server.rs`/`lib.rs`;
`shamir-server` merely logs `?e` — no caller anywhere can match the dead variants today.

### 1.4 [LOW] *(dedup — primary: 5.3)* `ws_send_sink` has no send-side cap; `payload.len() as u32` silently truncates
Full write-up at 5.3. Receiver-side containment (the mismatch check at framing.rs:157
rejects the corrupt frame) was this lens's contribution: system damage is a confusing
remote `LengthMismatch` instead of a local error, not desync.

### 1.5 [LOW] Wildcard origin matcher is raw string logic — accepts literal `*` and userinfo forms; unvalidated patterns fail silently *(primary of the origin-matcher group; also 3.8, 5.10)*
- **File:line:** `crates/shamir-transport-ws/src/browser.rs:56-76` (`origin_matches`),
  `:37-41` (`allow` performs no pattern validation).
- **Issue:** (a) The `*` label itself matches literally: `Origin: https://*.example.com` is
  accepted by pattern `https://*.example.com` (after_scheme = `*.example.com`, first dot is
  at index 1, suffix compares equal). (b) Userinfo-bearing origins pass:
  `https://user@app.example.com` matches `https://*.example.com` because the "first dot"
  split ignores the `@`. (c) A pattern without a scheme (`*.example.com`) contains no
  `//*.`, falls through to exact comparison, and therefore matches *nothing* — a config
  typo silently narrows the allowlist with no error at construction. Browsers never
  serialize origins in forms (a)/(b), so the CSRF defence holds in practice (and failures
  are fail-closed), but the matcher accepts origins a strict host-label match would reject.
  The api lens adds: `origin_matches` is case-sensitive byte comparison (RFC 6454 makes
  scheme/host case-insensitive), and wildcard detection is an unanchored whole-pattern
  `find("//*.")` scan with no port/trailing-dot normalization.
- **Failure scenario:** An allowlisted user whose environment emits a mixed-case Origin is
  rejected; operators "fix" it by adding both casings to the allowlist, accumulating
  near-duplicate entries. Independently, a typo'd pattern boots cleanly and full-rejects —
  surfaced only via client-side 403s.
- **Suggested fix:** In `origin_matches`, reject `after_scheme.starts_with('*')`, validate
  the first component as a non-empty DNS label without `/`, `@`, `:`, or `?`, ASCII-lowercase
  both sides, and anchor the wildcard check to immediately after `scheme://`. In `allow`,
  reject (or `debug_assert`) patterns lacking `<scheme>://` — or add `try_allow -> Result` —
  so operator typos surface at boot.

### 1.6 [LOW] *(dedup — primary: 3.1)* Direct `tungstenite = "0.29"` dependency is unused and version-skewed vs the effective 0.24
Full write-up at 3.1 (five lenses flagged this one dependency line).

### 1.7 [NIT] `BrowserOriginPolicy::empty()` doc references a nonexistent `accept_no_origin` mode
- **File:line:** `crates/shamir-transport-ws/src/browser.rs:28-30`.
- **Issue:** The doc says the empty policy "rejects everything except the explicit
  `accept_no_origin = true` mode (which is for testing only)". No such mode or parameter
  exists anywhere in the crate — `validate_origin` (browser.rs:95-105) unconditionally
  rejects a missing origin, and `accept_browser_ws` exposes no bypass. Spot-checked against
  the current tree: confirmed stale.
- **Suggested fix:** Delete the stale sentence; if an escape hatch is ever added, document
  it then.

### 1.8 [NIT] *(dedup — primary: 3.5)* `BROWSER_CHANNEL_BINDING` is dead; the zeros invariant is encoded twice
Full write-up at 3.5 (covers the dead const, the re-hardcoded `[0u8; 32]` literals, and the
`unwrap_or`-invitation API shape as one root defect).

### 1.9 [NIT] *(dedup — primary: 4.4)* Error-semantics / doc-accuracy warts
Full write-up at 4.4 (the pre-auth-cap misattribution group; includes this finding's
`LengthMismatch::actual` field-doc half — framing.rs:148-153 reports `bytes.len()` for
sub-4-byte messages while framing.rs:42-43 documents "body length minus 4").

## 2. concurrency-lockfree

**General verdict: clean.** Reading all six source files plus a crate-wide grep found zero
`std::sync::Mutex`/`RwLock`/`parking_lot`, zero atomics, and no `scc`/`dashmap`/hash-map
surface at all — so there are no locks held across `.await`, no `scc::*::len()` sites, and
no Fx-hash default to violate, by definition. All potentially shared state is avoided
through ownership (`&mut` free-fn parameters, `split()` halves); every I/O op is
`async fn` — exactly the pillar-1/2 shape. Coverage of the concurrency claims (split-half
duplex under real tokio tasks, live accept-path wiring) is present and adequate. The
theme's two findings are O(x→0) items:

### 2.1 [LOW] *(dedup — primary: 4.1)* Send framing ships only the allocating variant — production send path allocates + memcpys per message while recv is zero-alloc
Full write-up at 4.1. This lens's framing: the asymmetry is sharpest *within* the crate —
`ws_recv_into`/`ws_recv_into_stream` write into caller scratch (zero-alloc steady state,
framing.rs:92/132), so a fully pooled per-connection duplex loop is impossible on WS; and
the doc's parity claim with the TCP sibling (framing.rs:62) overstates — TCP ships three
variants (`write_frame` / `write_frame_into` / `write_frame_prereserved`,
shamir-transport-tcp/src/framing.rs:149/:187/:233), WS ships one.

### 2.2 [LOW] `accept_browser_ws` deep-clones the entire origin allowlist per accepted connection *(primary of the policy-clone group; also 5.9, 4.5)*
- **File:line:** `crates/shamir-transport-ws/src/server.rs:118` (`let policy =
  policy.clone();`; type at `src/browser.rs:23-25`; caller
  `shamir-server/src/server/server_launcher.rs:1266`).
- **Issue:** Deep-copies `BrowserOriginPolicy` (a `Vec<String>`: 1 + P heap allocations,
  P = number of configured origins) on **every** accepted browser connection solely to move
  owned state into the `move` handshake callback — per-op state duplication for a value
  that is immutable for the handshake's lifetime. The pillars ask for read-shared config to
  move by refcount, not by deep copy (pillar 3 / pillar 5's single-writer-many-reader →
  `Arc` guidance; no lock is involved). The api lens adds: tungstenite's `Callback` trait
  carries no `'static` bound, so the closure could equally borrow `&'a BrowserOriginPolicy`
  with an explicit lifetime on the async fn — the per-connection clone in `server_launcher`
  (which clones at :1240 to move into the spawned task) would then be the only copy.
- **Failure scenario:** None for correctness; wasted allocation + linear copy on the
  per-connection accept path — amortizes to nothing at low churn, visible at high connection
  churn and/or large operator allowlists, and invites the same clone to propagate outward
  as "precedent."
- **Suggested fix:** Take `policy: &Arc<BrowserOriginPolicy>` (or add an
  `accept_browser_ws_arc` overload, or a borrowed-`'a` signature) and clone the `Arc` into
  the closure — one atomic refcount bump instead of 1 + P allocations. The sole production
  caller already builds the policy once at boot (`browser_origin_policy_from`,
  server_launcher.rs:924-930) and holds it per-listener, so switching that field to
  `Arc<BrowserOriginPolicy>` is mechanical.

No further findings for this theme.

## 3. security-crypto

**Boundary verdict: largely sound.** Zero `unsafe` (grep-verified); Origin is validated
inside tungstenite's handshake callback **before** the 101 response; endpoint paths
exact-match; framing enforces `declared == actual` as defense-in-depth; the NEW-1 pre-auth
buffering cap is pinned to 16 MiB and live-tested; the TLS-exporter boundary fails closed
on the client side. No timing-sensitive comparisons exist here (Origin matching guards no
secret; SCRAM/HMAC and the binding_mode anti-downgrade matrix live in shamir-connect, as
the crate docs correctly state). Checked and clean this theme: TEXT rejection reflects no
payload content (`TEXT len=N` only); non-ASCII Origin classifies as `Missing` (fail-closed);
TLS 1.3-only is delegated to `shamir-transport-tcp`'s rustls config rather than duplicated.

### 3.1 [MEDIUM] Phantom `tungstenite = "0.29"` dependency — two WS parsers compiled, the live one is the older *(primary of the phantom-dependency group; also 5.4, 4.3, 1.6, 7.4)*
- **File:line:** `crates/shamir-transport-ws/Cargo.toml:20` (evidence: `Cargo.lock:4233-4246`,
  `4455-4488`, `3769`).
- **Issue:** Every code path imports WS types via the `tokio_tungstenite::tungstenite::*`
  re-export — i.e. tungstenite **0.24.0**, the version tokio-tungstenite 0.24.0 pins. The
  direct `tungstenite = "0.29"` entry is never imported anywhere under `src/` (verified by
  grep). Cargo.lock consequently carries **both** tungstenite 0.24.0 and 0.29.0, and
  `shamir-transport-ws` is the sole depender of the 0.29.0 copy. The public API leaks 0.24
  types: `WsFrameError::Io(#[from] tokio_tungstenite::tungstenite::Error)` (framing.rs:33)
  and all `Message` handling — written against 0.24 semantics (`Message::Binary(Vec<u8>)`,
  vs `Bytes` in newer tungstenite). Five of the seven lenses independently flagged this
  line (the security reviewer's CVE-false-assurance angle is the sharpest statement of it).
- **Failure scenario:** A CVE fix or hardening release on the tungstenite 0.29 line gives
  false assurance — `cargo audit` triage and auditors see 0.29.0 present while the code
  that actually parses untrusted frames from unauthenticated peers runs 0.24.0 and receives
  nothing. A maintainer "responding to the advisory" by bumping the manifest line fixes
  nothing. A second full parser copy (plus its `rand 0.9` / `thiserror 2` closure) is also
  compiled into the binary for no benefit; and a future `use tungstenite::Message` would
  compile against 0.29 types that do not unify with the `WebSocketStream`'s 0.24 types.
- **Suggested fix:** Delete the `tungstenite = "0.29"` line; if tungstenite types are ever
  needed directly, use the `tokio_tungstenite::tungstenite` re-export (as the code already
  does) so exactly one version exists. Treat "move to a tokio-tungstenite release on the
  0.29 line" as its own deliberate upgrade task. While there, the direct `rustls` /
  `tokio-rustls` entries (Cargo.toml:23-24) are likewise unused by this crate's code (types
  arrive via `shamir_transport_tcp::tls::ConnectionExporter`); dropping them keeps the
  manifest honest about the crypto boundary.

### 3.2 [MEDIUM] *(dedup — primary: 1.1)* `accept_browser_ws` Origin enforcement has no live-wiring test coverage
Full write-up at 1.1. The security-angle framing: this is the only consumer path that turns
`validate_origin` into a security control, and it is exercised by **no test anywhere in the
workspace** (`accept_browser_ws` appears only in `server.rs`, `lib.rs`, and the production
caller `server_launcher.rs:1266`).

### 3.3 [LOW] Attacker-controlled `Origin` echoed into the HTTP 403 response body
- **File:line:** `crates/shamir-transport-ws/src/server.rs:136` (source: `browser.rs:87`,
  `browser.rs:103`).
- **Issue:** `OriginRejected::NotAllowed(origin.to_string())` embeds the raw header value,
  and the accept path interpolates it into the `ErrorResponse` body sent on the wire
  ("origin rejected: {rej}"). Exploitation as XSS is effectively blocked in practice
  (rendering the body requires a top-level navigation, which does not carry an
  attacker-chosen `Origin`; fetch/WS callers cannot read the cross-origin body), but it is
  needless reflection of untrusted input in a boundary response, and the same unsanitized
  string also flows into operator debug logs (`ws browser upgrade failed`,
  server_launcher.rs:1270).
- **Failure scenario:** Log/wire injection surfaces crafted origin text in operator
  consoles and HTTP responses; no code-execution path identified today.
- **Suggested fix:** Send a static body ("origin rejected") and keep the offending value in
  structured `tracing` fields only; `OriginRejected` already preserves it for library
  callers.

### 3.4 [LOW] Unbounded control-frame loop in `ws_recv_into_stream` (ping-flood liveness); auto-pong write queue is uncapped *(primary of the control-frame-flood group; also 4.2)*
- **File:line:** `crates/shamir-transport-ws/src/framing.rs:176`, `:183` (Ping/Pong/Frame
  `continue`); `src/server.rs:42-51` (`server_ws_config` deliberately leaves
  `max_send_queue` at default, per the deprecation comment at :43-45).
- **Issue:** `ws_recv_into_stream` loops over PING/PONG/Frame messages with no
  per-connection budget. Tungstenite auto-queues a Pong for every PING read; in the
  split-half layout shamir-server actually deploys (`WsFrameReader`/`WsFrameWriter` over
  `StreamExt::split`), those pongs sit in the shared write queue that only the writer task
  drains. A hostile peer that streams PINGs without reading keeps the read loop spinning
  indefinitely *and* drives the connection's outgoing buffer to grow without a crate-level
  cap (`max_send_queue` intentionally at tungstenite's default; the 16 MiB message caps in
  `server_ws_config` do not cover this). Production callers currently wrap reads in
  `auth_init_timeout`-style bounds, but the crate API itself offers no progress guarantee —
  any future `ws_recv*` caller without an outer timeout can be wedged one task per
  connection, forever.
- **Failure scenario:** An unauth'd (or auth'd) peer pins server memory per connection via
  ping-flood + no-read (slow-reader); multiplied by many peers this is a memory-pressure
  DoS vector the NEW-1 hardening does not cover, plus 1:1 forced outbound pong traffic.
- **Suggested fix:** Cap consecutive non-BINARY messages (4-8, or a 64-class budget) and
  return a dedicated `WsFrameError::ControlFrameFlood`, matching the crate's fail-closed
  style; and/or document/enforce a write-buffer cap for the pinned tokio-tungstenite 0.24
  (verify the 0.24 default; 0.26+ replaced `max_send_queue` with bounded
  `write_buffer_size` options — a version bump also fixes it structurally). Pair with an
  idle/dead-peer timeout at the session layer.

### 3.5 [LOW] `Option`-returning exporter API + public all-zeros constant invites silent zero-substitution on the native path *(primary of the exporter-placeholder group; also 6.4, 5.8, 1.8, 7.3)*
- **File:line:** `crates/shamir-transport-ws/src/tls_exporter.rs:20-25`; consumption sites
  `shamir-server/src/server/server_launcher.rs:1073`, `:1159` (`unwrap_or([0u8; 32])` on the
  **native** path), `:1263` (`let exporter = [0u8; 32];`); the crate's own doc prose
  (`lib.rs:11-12`, `server.rs:11`).
- **Issue:** `extract_tls_exporter_from_stream -> Option<[u8; 32]>` plus the exported
  `BROWSER_CHANNEL_BINDING = [0u8; 32]` makes `unwrap_or(<zeros>)` the path of least
  resistance — which is exactly what the production callers do today, including on the
  **native** path where binding_mode = 0x01/TlsExporter. Today this fails closed only by
  accident of the client also failing closed on `None` (`shamir-client/src/client.rs:391-392`,
  `:603-604`): zero-vs-real binding bytes break the SCRAM proof. But it masks a broken TLS
  state as a generic auth failure, and the design is one client-side `unwrap_or` away from
  a native session genuinely bound with the browser placeholder. Compounding it, the
  exported `BROWSER_CHANNEL_BINDING` const — which exists precisely to single-source the
  "browser exporter = zeros per spec §6.4" invariant — has **zero references in the entire
  workspace**: it is not re-exported in `lib.rs`, not used by `accept_browser_ws`, not
  tested, while `server_launcher` re-hardcodes the literal twice (two encodings of a
  protocol-mandated value = drift hazard). The error-handling lens adds: the `Option`
  shape swallows the `rustls::Error` cause — on the native endpoint the exporter always
  exists post-handshake (TLS 1.3 always supports it, per
  `shamir-transport-tcp/src/tls.rs:74-76`), so `None` is an error, not an absent value.
- **Failure scenario:** A native WSS caller maps `None` to the zero placeholder: SCRAM
  channel binding silently weakens with no error surfaced, and the protocol's
  anti-downgrade matrix will not flag it because 0x02 is a legal mode. Independently, if
  the placeholder semantics ever change, the literal sites drift from the unused const.
- **Suggested fix:** Expose one fail-closed helper, e.g. `channel_binding_for(stream,
  BindingMode) -> Result<[u8; 32], ChannelBindingError>`, which errors when extraction
  fails for `TlsExporter` and returns the placeholder only for `TlsNoExport` (carrying the
  rustls cause, per 6.4); use `BROWSER_CHANNEL_BINDING` at both server_launcher sites (it
  is already exported at `shamir_transport_ws::tls_exporter::`); make the native path
  return an error (or at least a loud warn + explicit downgrade marker) on extraction
  failure rather than quietly reusing the browser placeholder; and gate/rename the const
  (`TLS_NO_EXPORT_PLACEHOLDER`) with a doc warning against `unwrap_or` on native paths.

### 3.6 [NIT] *(dedup — primary: 4.4)* Doc misattributes the 4 KiB pre-auth cap to this crate's framing layer
Full write-up at 4.4.

### 3.7 [NIT] *(dedup — primary: 5.3)* `ws_send_sink` truncates the length prefix for payloads >= 4 GiB
Full write-up at 5.3 (equality is enforced on both sides, so no desync or memory-safety
issue — only a confusing remote protocol error; `u32::try_from` costs nothing).

### 3.8 [NIT] *(dedup — primary: 1.5)* `BrowserOriginPolicy::allow` accepts malformed patterns that silently never match
Full write-up at 1.5. Fail-closed, but a configuration trap surfaced only via client-side
403s; validate at construction (`try_allow -> Result`) or `debug_assert!` the shape
(must contain `://`; at most one `*`, only in the `//\*.` slot).

## 4. performance-hotpath

Scope note: judged against pillar 3 (O(x→0)) and pillar 1 (no hot-path locks), with
consumer context `shamir-server/src/framer.rs` + `connection/request_loop.rs`, which drive
`ws_send_sink` / `ws_recv_into_stream` once per request/response frame on the WSS path.
**The receive path is genuinely O(x→0)-clean:** `ws_recv_into_stream` recycles the caller's
scratch buffer (`clear` + `extend_from_slice`), capacity reuse is proven by
`tests/framing_round_trip.rs::round_trip_into_buffer_reuses_capacity`, validation is all
constant-time checks, `BrowserOriginPolicy::allows` is a linear scan over an
operator-config allowlist (constant w.r.t. traffic), and error-path `format!`/`to_string()`
allocations fire only on rejection paths. Not findings (checked clean): recv-path
allocation, hidden O(N) scans, pillars 1/3/5. The send path is the weak side:

### 4.1 [MEDIUM] WSS send hot path: fresh heap alloc + full-payload copy per frame; TCP's prereserved zero-copy path is silently defeated *(primary of the send-alloc group; also 2.1)*
- **File:line:** `crates/shamir-transport-ws/src/framing.rs:114-124` (`ws_send_sink`; parity
  claim in doc at `:62`); consumer context `shamir-server/src/framer.rs:110-127, 348-365,
  405-415`, `shamir-server/src/connection/request_loop.rs:198-213`.
- **Issue:** `ws_send_sink` takes `&[u8]` and, on every call, allocates
  `Vec::with_capacity(4 + payload.len())` and memcpys the entire payload into it
  (framing.rs:119-121) before handing it to tungstenite (which copies it again into its
  internal write buffer). The server's request loop builds every response as an already
  length-prefixed buffer precisely to avoid a memcpy — `write_frame_prereserved` — and TCP
  overrides it with `tcp_write_frame_prereserved` (zero copy). The WS `FrameWriter` does
  not override it, so the trait's default strips the 4-byte prefix and routes through
  `ws_send_sink`: the caller's prereserved copy is wasted and a second full-payload copy
  into a brand-new heap `Vec` is paid on **every WSS response frame**. The
  `Framer::write_frame_into` `scratch` parameter, designed for exactly this zero-alloc
  reuse, is explicitly ignored by the WS impl (`_scratch`, framer.rs:355/408 — "WS already
  builds its own send buffer (one allocation per message)"), i.e. one malloc+free per frame
  on the hottest WSS loop, plus a redundant O(payload) memcpy; at the 16 MiB frame ceiling
  that is two extra 16 MiB traversals per frame.
- **Failure scenario:** Throughput/latency tax on all WSS traffic (browser endpoint
  included) proportional to payload size; allocator churn under high RPS. Not asymptotically
  worse than the transport itself, so medium rather than high.
- **Suggested fix:** Mirror the TCP trio where it maps onto tungstenite's owned
  `Message::Binary(Vec<u8>)` model: (a) add an ownership-taking variant, e.g.
  `ws_send_sink_vec(sink, Vec<u8>)` — a caller that yields its prereserved buffer moves it
  with **zero** copy, since tungstenite copies into its internal write buffer at flush
  regardless; this is the variant that actually removes the extra copy — then override
  `write_frame_prereserved` in shamir-server's `WsFrameWriter` to use it. (b) Optionally a
  scratch-reusing variant matching the `Framer` scratch contract, and/or an owned-`Vec`
  variant that reserves and prepends the header in place (`Vec::splice(0..0, …)`) to drop
  the per-send allocation. Do **not** copy `write_frame_into`'s scratch-buffer signature
  blindly — `Message::Binary` takes ownership, so a pooled scratch would need a `clone()`
  (same memcpy, zero gain). Document the copy semantics on `ws_send_sink`.

### 4.2 [LOW] *(dedup — primary: 3.4)* Receive loop consumes unbounded consecutive control frames; auto-pong write queue is uncapped
Full write-up at 3.4 (this lens contributed the outbound write-queue growth analysis and
the tokio-tungstenite 0.26+ `write_buffer_size` note).

### 4.3 [LOW] *(dedup — primary: 3.1)* Dead direct dependency `tungstenite = "0.29"` compiles a second, unused copy of tungstenite
Full write-up at 3.1. This lens's angle: none at runtime — wasted CI/dev build time,
binary bloat, and a latent type-mismatch hazard (`Cargo.lock:3769` shows this crate as the
sole depender of tungstenite 0.29.0).

### 4.4 [LOW] Doc drift: `server_ws_config` claims a "4 KiB pre-auth logical check … enforced in `crate::framing::ws_recv_into`" — no such enforcement exists in this crate *(primary of the pre-auth-doc group; also 3.6, 6.6, 1.9)*
- **File:line:** `crates/shamir-transport-ws/src/server.rs:26-28` (and mirrored in
  `src/tests/server_tests.rs:11-13`); the doc's own Residual paragraph at :33-41 says the
  opposite, contradicting the earlier sentence.
- **Issue:** framing.rs has no pre-auth constant; the ceiling is entirely the
  caller-supplied `max_frame_size` parameter, and the actual 4 KiB pre-auth enforcement
  lives in shamir-server (`connection/handshake.rs` passing `MAX_PRE_AUTH_FRAME`, with the
  constant itself in shamir-connect; enforcement points framer.rs:341/399). The doc's
  cross-reference implies the transport crate self-enforces the 4 KiB pre-auth budget; it
  does not — tungstenite will have buffered up to the full 16 MiB before any logical check
  runs. Misdocuments where the guard lives for future perf/security work. Two companion
  doc warts ride in the same group: (a) correctness 1.9 — for a <4-byte message the error
  reports `actual: bytes.len()` while `LengthMismatch::actual`'s field doc (framing.rs:42-43)
  says "WS message body length minus 4"; (b) error 6.6 — an error-enforcement claim
  pointing at the wrong layer invites future verification in the wrong file.
- **Failure scenario:** A future refactor trusts the comment, drops/changes the
  caller-supplied pre-auth cap, and silently re-widens unauthenticated buffering to
  16 MiB/peer — reinstating exactly what NEW-1 fixed.
- **Suggested fix:** Reword to "enforced by the caller (shamir-server passes
  `MAX_PRE_AUTH_FRAME`); this crate only bounds tungstenite's buffering via
  `server_ws_config`" (also at server_tests.rs:11-13); either special-case the
  short-message error or amend the `LengthMismatch::actual` field doc.

### 4.5 [NIT] *(dedup — primary: 2.2)* `accept_browser_ws` deep-clones the origin policy per connection accept
Full write-up at 2.2. This lens's severity call: O(allowlist) per *connection* (constant
w.r.t. traffic), so minor — but trivially avoidable.

## 5. api-wire-protocol

The framing, origin-policy, and listener-profile APIs are clean and well-documented, and
this crate constructs no queries or raw JSON anywhere, so the builder-only rule is
satisfied by construction. The wire-protocol problems are at the handshake and
cross-transport seams:

### 5.1 [HIGH] Spec-mandated WebSocket subprotocol negotiation is unimplemented
- **File:line:** `crates/shamir-transport-ws/src/server.rs:85-99` and `:120-143` (both
  accept callbacks); spec: `docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md:18`
  (§2.1).
- **Issue:** Spec TRANSPORT_WS §2.1 is normative for this crate (every module cites the
  spec by section): the client sends `Sec-WebSocket-Protocol: shamir-v1`, the server
  "confirm[s] same. Mismatch → 400." Neither `accept_native_ws` nor `accept_browser_ws`
  reads or echoes the `Sec-WebSocket-Protocol` header — the handshake callbacks check only
  the URI path and (browser) `Origin`. There is no transport-layer protocol-version gate at
  all.
- **Failure scenario:** A spec-conformant browser client that requests `shamir-v1` gets a
  101 response with no echoed `Sec-WebSocket-Protocol`; per RFC 6455 the browser then
  *fails the connection itself* ("Incorrect 'Sec-WebSocket-Protocol' header") — a
  conformant client cannot connect at all. Conversely, a client offering a different/future
  subprotocol (`shamir-v2`) is silently accepted, so the spec's mismatch→400 downgrade
  defense never fires. First-party clients dodge both today only because they also ignore
  the spec (`shamir-client-ts/src/platform/browser.ts:118` calls `new WebSocket(url)` with
  no subprotocol) — implementation and spec have drifted in opposite directions.
- **Suggested fix:** In both handshake callbacks, read `Sec-WebSocket-Protocol`: reject with
  400 unless it contains exactly `shamir-v1`, and echo `shamir-v1` back via the callback's
  `Response` headers (`resp.headers().append(...)`). Update the TS client to request it, or
  amend the spec to make the subprotocol optional and record why.

### 5.2 [HIGH] Endpoint paths hardcoded as string literals; no shared constants; incompatible with the server's configurable `path`
- **File:line:** `crates/shamir-transport-ws/src/server.rs:90` (`!= "/shamir/v1"`), `:124`
  (`!= "/shamir/v1/browser"`).
- **Issue:** `/shamir/v1` and `/shamir/v1/browser` are the protocol's version markers on
  the wire, but they exist only as `&'static str` literals inside the two accept functions.
  There is no `pub const` in this crate, and the accept API offers no path parameter.
  Meanwhile the sole production consumer treats the path as operator configuration:
  `shamir-server/src/config.rs:767-777` accepts *any* `path` starting with `/`, then
  `server_launcher.rs:1162/:1266` calls these hardcoded acceptors.
- **Failure scenario:** Operator sets `path: /db-ws` for a ws listener → config boots
  cleanly → every WS upgrade is answered 404 by the hardcoded check → total, silent
  connectivity loss on that listener with no boot-time error. Independently, the literals
  are now duplicated in ≥5 uncoordinated places (shamir-server config/tests,
  `shamir-client-ts` client.ts, deploy `*.ktav` files, docs), so the version string can
  drift between client and server without any compile-time check.
- **Suggested fix:** Export `pub const NATIVE_WS_PATH: &str = "/shamir/v1";` and
  `pub const BROWSER_WS_PATH: &str = "/shamir/v1/browser";` from this crate; either (a)
  make the accept fns take the expected path (or validate server config against the
  constants at boot, refusing unknown paths), or (b) document that the paths are
  protocol-fixed and have the server config validator reject any other value up front
  instead of accepting it.

### 5.3 [MEDIUM] Send path has no frame-size cap and truncates the length prefix at `u32::MAX` — diverges from the TCP sibling *(primary of the send-cap group; also 6.3, 1.4, 3.7)*
- **File:line:** `crates/shamir-transport-ws/src/framing.rs:118` (`let len = payload.len()
  as u32;`), `:114-124`.
- **Issue:** `ws_send` / `ws_send_sink` accept any payload length: no `MAX_WS_FRAME_SIZE`
  check, and `payload.len() as u32` silently wraps for payloads > 4 GiB. The TCP transport
  this crate mirrors does enforce it on send: `shamir-transport-tcp/src/framing.rs:153-156`
  and `:192-195` return `FrameError::TooLarge` for `payload.len() >
  MAX_FRAME_SIZE_DEFAULT`. The asymmetry means the WS send API's contract differs from the
  wire format it claims to share (framing.rs:1-9) — the sender-side error contract is
  strictly weaker than the TCP transport's for the same wire format.
- **Failure scenario:** A caller serializing a large SELECT result (>16 MiB, or merely
  violating the spec §8 cap) gets `Ok(())` from `ws_send_sink`; the frame goes out, and the
  failure surfaces later and remotely — the receiving peer's framing layer returns
  `TooLarge` mid-connection (or buffers it if the peer's `WebSocketConfig` is loose). For a
  >4 GiB payload the prefix wraps, the declared length is corrupt, and the receiver reports
  a baffling `LengthMismatch` on a frame the sender believed valid.
- **Suggested fix:** Mirror the TCP writer: check `payload.len() > MAX_WS_FRAME_SIZE` (or a
  caller-supplied cap symmetric with `ws_recv`'s `max_frame_size` param) and return
  `WsFrameError::TooLarge { actual, max }` before building the buffer —
  `u32::try_from(payload.len()).map_err(|_| WsFrameError::TooLarge { .. })` also removes the
  silent `as u32` truncation path.

### 5.4 [MEDIUM] *(dedup — primary: 3.1)* Unused, version-mismatched direct dependency `tungstenite = "0.29"` while the public API is pinned to tungstenite 0.24
Full write-up at 3.1. This lens's angle: the API-coupling hazard — every `Message` /
`bytes[4..]` site is written against 0.24 semantics with no manifest hint of the coupling,
so a future tokio-tungstenite upgrade that changes `Message::Binary`'s payload type breaks
them all at once.

### 5.5 [MEDIUM] Zero-length frame means "graceful close" on TCP but is a legal empty frame on WS — undocumented divergence in a claimed-identical wire format
- **File:line:** `crates/shamir-transport-ws/src/framing.rs:1-9` (doc claim), `:146-171`
  (recv path); contrast `shamir-transport-tcp/src/framing.rs:8,31` (`length == 0` →
  `FrameError::PeerClose`).
- **Issue:** framing.rs documents "Same wire format as `shamir-transport-tcp::framing`" —
  true for the byte layout (`[u32_be length][payload]`, length excludes prefix, 16 MiB
  cap), but the zero-length semantic differs: TCP defines declared length 0 as a
  graceful-close indicator and surfaces `FrameError::PeerClose`; over WS a 4-byte
  `[0,0,0,0]` message passes the mismatch and cap checks and returns `Ok(())` with an empty
  buffer (close is signaled only by the WS Close frame, framing.rs:173). Nothing in the
  module doc notes the divergence.
- **Failure scenario:** Cross-transport code reuse — a client port that sends the TCP-style
  zero-frame "close" while on WSS gets its message decoded as an empty payload, which then
  fails msgpack deserialization downstream as an opaque protocol error instead of a clean
  close; or a server handler written against TCP semantics treats the empty frame as EOF
  and skips cleanup that the WS path requires.
- **Suggested fix:** Either document the divergence explicitly in the framing.rs header
  ("length 0 is NOT a close indicator here; close is the WS Close frame"), or reject
  declared==0 frames as a protocol error so the two transports stay behaviorally aligned.

### 5.6 [MEDIUM] *(dedup — primary: 1.1 + 1.2)* `accept_browser_ws` — the Origin-enforcing handshake path — has zero integration tests; the framing length-mismatch invariant is also untested
Splits across the two primary entries: the accept-wiring half at 1.1, the framing
error-path half at 1.2. This lens's framing: the subprotocol fix (5.1) edits exactly the
closure whose behavior is untested — landing 5.1 without 1.1's tests re-creates the
silent-regression risk by construction.

### 5.7 [LOW] Exporter-extraction ordering: this crate's doc contradicts its only production caller
- **File:line:** `crates/shamir-transport-ws/src/server.rs:5-7` ("extracts the exporter ...
  AFTER the WS handshake completes") and `:76-77` ("Caller then extracts the TLS
  exporter"); contrast `shamir-server/src/server/server_launcher.rs:1156-1159` ("CRITICAL:
  extract exporter BEFORE the WS upgrade consumes `tls`. After upgrade the TLS state is
  owned by the WebSocketStream and not directly accessible.").
- **Issue:** The public contract in server.rs tells callers to extract the TLS exporter
  *after* `accept_native_ws` returns; the production caller insists it must happen *before*
  the WS upgrade and calls the after-path impossible. Both cannot be right (technically
  `WebSocketStream::get_ref()` may make the after-path workable, but the two crates'
  documentation actively disagree about the safe order for a security-critical step).
- **Failure scenario:** A new integrator (Go/Python client work, second server embedding)
  follows this crate's doc, extracts after the upgrade, and either fails to get the
  exporter or — worse, if it silently returns `None` — falls into a zeros-placeholder
  channel binding without realizing the ordering is contested (see 3.5).
- **Suggested fix:** Settle the ordering once (server_launcher's "before the upgrade" is
  the conservative choice and is what production does), and rewrite server.rs's doc for
  `accept_native_ws` to state it, ideally with the `extract_tls_exporter_from_stream` call
  shown in the doc example.

### 5.8 [LOW] *(dedup — primary: 3.5)* `BROWSER_CHANNEL_BINDING` constant exported but consumers re-hardcode `[0u8; 32]`
Full write-up at 3.5. This lens flagged the native-endpoint fallback (server_launcher.rs:
1159) as the worst instance — the spec-conformant native value is the real exporter, not
the placeholder.

### 5.9 [LOW] *(dedup — primary: 2.2)* `accept_browser_ws` clones the origin policy on every connection
Full write-up at 2.2 (this lens contributed the borrowed-closure alternative: no `'static`
bound on the `Callback` trait → `&'a BrowserOriginPolicy` capture works).

### 5.10 [NIT] *(dedup — primary: 1.5)* Origin matching is case-sensitive; wildcard detection is a whole-pattern substring search
Full write-up at 1.5.

### 5.11 [NIT] *(dedup — primary: 7.6)* `is_loopback` re-implements `IpAddr::is_loopback`
Full write-up at 7.6.

### 5.12 [NIT] *(dedup — primary: 7.5)* Unused dev-dependencies: `hex`, `serde`, `serde_bytes`, `rmp-serde` (plus the direct `tungstenite`, see 3.1)
Full write-up at 7.5. This lens's observation: the unused `rmp-serde` also means the
crate's tests never exercise a real msgpack payload despite the docs calling this
"length-prefix msgpack framing."

## 6. error-handling-lifecycle

Crate-level error discipline is largely faithful to CLAUDE.md: every fallible API returns
`Result` over a `thiserror` enum (`#[from]` where natural), src contains zero
`panic!`/`unwrap`/`anyhow`/`Box<dyn Error>`, and the bind path validates policy *before*
socket creation (nothing to close on the reject path); both accept fns take ownership of
the stream, so any `Err` drops the socket; the crate spawns no tasks, holds no locks, owns
no files — nothing else to leak on an error path. Test layout matches the documented
convention; the covered modules (`browser_tests.rs` 10 tests, `listener_tests.rs`
validate-before-bind) are solid. The real defects sit in the accept layer:

### 6.1 [MEDIUM] `WsAcceptError::OriginRejected` and `WrongPath` are dead variants — all accept rejections surface as `Handshake` *(primary of the dead-variants group; also 1.3)*
- **File:line:** `crates/shamir-transport-ws/src/server.rs:55-71` (enum; `#[from]
  OriginRejected` at :60-61, `WrongPath` at :64-70), `:99` and `:144` (the only `?`
  construction sites); re-exported from `lib.rs:34`.
- **Issue:** Both accept fns implement wrong-path rejection (server.rs:90-94, :124-128) and
  origin rejection (:133-139) by returning `ErrorResponse` from the handshake callback.
  Tungstenite converts that into `Error::Http`, and the single `?` maps it through
  `#[from]` into `WsAcceptError::Handshake`. A workspace-wide grep confirms nothing ever
  constructs `WsAcceptError::OriginRejected(...)` or `WsAcceptError::WrongPath { .. }`;
  the `#[from] OriginRejected` conversion is unreachable. (The `OriginRejected` *type* is
  real — `validate_origin` produces it — but its typed propagation into `WsAcceptError`
  never happens.)
- **Failure scenario:** The first consumer wiring `/shamir/v1/browser` (e.g. shamir-server)
  that matches `WsAcceptError::OriginRejected(_)` to count or escalate policy denials — or
  to map the rejection back to HTTP 403 — never hits that arm; a security-relevant origin
  rejection (the endpoint's primary anti-CSWSH defence) is indistinguishable from a TLS/IO
  handshake failure except by string-matching the `Http` response body (`"origin rejected:
  …"`). Any test asserting `Err(WsAcceptError::OriginRejected(_))` cannot pass by
  construction.
- **Suggested fix:** After the `.await`, inspect the `Error::Http` response (status
  404/403 + reason) and re-map into `WrongPath` / `OriginRejected` before returning, since
  the request is only visible inside the callback. Alternatively remove the dead variants
  and document how to classify from `Handshake(Error::Http)`. Either way, the exported
  error taxonomy should stop promising classification it does not deliver.

### 6.2 [MEDIUM] *(dedup — primary: 1.1 + 1.2)* Error paths of framing and accept have no variant-asserting tests
Splits across 1.1 (accept rejection paths — no test drives either accept fn to an error)
and 1.2 (framing error variants; the only framing error test asserts bare `is_err()`).
This lens's addition: a refactor that wraps peer-close into `Io` or treats `Message::Text`
as skippable passes the whole suite silently — regressions in the fail-closed guarantees
go unnoticed; suggested layout per CLAUDE.md is `src/framing/tests/framing_error_tests.rs`.

### 6.3 [LOW] *(dedup — primary: 5.3)* Send path lacks the TCP sibling's size guard; `payload.len() as u32` truncates silently
Full write-up at 5.3.

### 6.4 [LOW] *(dedup — primary: 3.5)* Exporter extraction failure is indistinguishable from unavailability (`Option` discards the cause)
Full write-up at 3.5. Mitigating note carried from this lens: the wrapper mirrors the
sibling `extract_tls_exporter`, which uses the same `Option` pattern, so the shape is at
least workspace-consistent — minimally, document in caps that `None` on the native
endpoint MUST abort the connection and must never fall back to the placeholder.

### 6.5 [NIT] Framing error leaves the caller's scratch buffer holding the previous frame
- **File:line:** `crates/shamir-transport-ws/src/framing.rs:169-170` (`buf.clear()` only on
  the success path).
- **Issue:** On `LengthMismatch` / `TooLarge` / `PeerClose`, the caller-supplied buffer
  still contains the *previous* frame's payload. (Fine for `ws_recv`'s fresh `Vec`; wrong
  only for reused scratch buffers.)
- **Failure scenario:** A caller that logs `buf` on the error path — or ignores the
  `Result` and proceeds — treats the previous request's bytes as the current frame's.
- **Suggested fix:** Either document that `buf` is unspecified on `Err`, or clear it at
  function entry so the error path is side-effect-free.

### 6.6 [NIT] *(dedup — primary: 4.4)* Doc attributes the 4 KiB pre-auth check to `ws_recv_into`, which enforces nothing by itself
Full write-up at 4.4.

### 6.7 [NIT] `Origin` header containing obs-text bytes is reported as `Missing` rather than rejected-as-present
- **File:line:** `crates/shamir-transport-ws/src/server.rs:129-132` (with `browser.rs:99`).
- **Issue:** `to_str().ok()` maps a present-but-non-ASCII `Origin` header to `None`, so the
  rejection reason becomes `OriginRejected::Missing` ("browser endpoint requires Origin
  header"). The upgrade is still rejected (fail-closed either way), but the error label
  misstates reality, which matters once rejections are logged or matched on.
- **Suggested fix:** Read the raw `HeaderValue`; if `to_str()` fails, return
  `OriginRejected::NotAllowed` (e.g. with a `<non-utf8>` placeholder) so present-but-invalid
  is distinguishable from absent.

## 7. style-claude-md

**Largely exemplary.** Verified against CLAUDE.md: `lib.rs` is module decls + re-exports +
`#[cfg(test)] mod tests;` only; `src/tests/mod.rs` is a manifest-only re-export file; tests
split by topic (`browser_tests.rs`, `listener_tests.rs`, `server_tests.rs`); no
implementation file carries an inline `#[cfg(test)]` block; all `src/` files hoist imports
to the header (grep: zero indented `use` statements crate-wide); inline
`#[allow(clippy::result_large_err)]` at server.rs:87/:122 each carry the required
one-line justification; all error enums derive `thiserror` (no `anyhow`, no leaked
`Box<dyn Error>`, no `panic!` outside tests); one-file-one-closely-coupled-group holds for
every module; the crate-root `tests/framing_round_trip.rs` is a genuine public-API
integration test mirroring the established sibling pattern
(`shamir-transport-tcp/tests/framing.rs`). Coverage claim: 10 origin-policy tests, 9
listener-profile tests, 3 accept-path tests (incl. live 16 MiB cap), 7 framing tests.

### 7.1 [MEDIUM] Mid-file `use` statement violates the "imports at the top" rule
- **File:line:** `crates/shamir-transport-ws/tests/framing_round_trip.rs:98` (spot-checked:
  present, after the section banner at :94-96).
- **Issue:** `use futures_util::{SinkExt, StreamExt};` sits at line 98, after four complete
  test functions and a section-banner comment, instead of in the file header alongside the
  other imports (lines 3-8). CLAUDE.md is explicit: "All `use` statements live in the
  **file header** (or the enclosing module's header), never inside a function or block
  body." This is module-level rather than function-body scope, but it is plainly not the
  file header — the exact drift the rule targets. `SinkExt` (line 131) and `StreamExt`
  (line 108) only resolve because of this buried import.
- **Failure scenario:** A reader or automated tool scanning the header concludes
  `futures_util` is unused; a later edit that reorders or deletes the "Split-half tests"
  banner section silently breaks the two split-half tests below it. Every other file in the
  crate (src and tests) follows the header convention, making this file the outlier.
- **Suggested fix:** Hoist the import into the header block next to lines 3-8 and delete
  line 98. The section banner comment can stay. (The workspace roadmap already schedules an
  imports-at-top sweep touching this crate — fold it in there.)

### 7.2 [LOW] `lib.rs` re-export set incomplete vs. module public APIs (and vs. sibling transport crate) — *partially corrected on spot-check*
- **File:line:** `crates/shamir-transport-ws/src/lib.rs:29-35`; genuine gap at
  `src/framing.rs:26` (`MAX_WS_FRAME_SIZE`).
- **Issue:** The crate's convention is "every public item re-exported at the root", and the
  workspace sibling `shamir-transport-tcp/src/lib.rs:12` re-exports its full framing
  surface including the size const (`read_frame, write_frame, FrameError,
  MAX_FRAME_SIZE_DEFAULT`). **Spot-check correction:** the source file claimed `ws_recv`
  is also missing — it is not; current `lib.rs:30-32` re-exports
  `ws_recv, ws_recv_into, ws_recv_into_stream, ws_send, ws_send_sink, WsFrameError` in
  full (the working tree is clean, so this is a source-file inaccuracy, not drift since
  review). The surviving gap is `MAX_WS_FRAME_SIZE` only.
- **Failure scenario:** Callers who use the root re-exports discover the size const only
  by browsing `framing::`; a hard-coded 16 MiB literal in a caller can silently diverge
  from the const.
- **Suggested fix:** Add `MAX_WS_FRAME_SIZE` to the framing re-export set (or, if the const
  is intentionally kept namespaced, document the intent).

### 7.3 [LOW] *(dedup — primary: 3.5)* `BROWSER_CHANNEL_BINDING` is dead public API; the zero placeholder exists only in prose elsewhere
Full write-up at 3.5. This lens's structural note: it makes `tls_exporter.rs` the one file
whose second export is unanchored to any consumer, and the prose in `lib.rs:11-12` /
`server.rs:11` can drift from the unused const independently.

### 7.4 [LOW] *(dedup — primary: 3.1)* Unused direct dependency `tungstenite = "0.29"` (version-mismatched with tokio-tungstenite)
Full write-up at 3.1 (five-lens group; style angle: lockfile noise + the confusing
trait-bound errors a future direct import would produce).

### 7.5 [LOW] Unused `[dev-dependencies]`: `hex`, `serde`, `serde_bytes`, `rmp-serde` *(primary of the unused-dev-deps group; also 5.12)*
- **File:line:** `crates/shamir-transport-ws/Cargo.toml:32-36`.
- **Issue:** None of `hex`, `serde`, `serde_bytes`, or `rmp-serde` appears in any `.rs`
  file in the crate (grep over `crates/shamir-transport-ws/**/*.rs`: zero matches). All
  test files (`src/tests/*`, `tests/framing_round_trip.rs`) depend only on `tokio`,
  `futures-util`, `tokio-tungstenite`, and the crate itself. Likely leftovers from a test
  that encoded msgpack payloads by hand.
- **Failure scenario:** None at runtime; cost is lockfile/build-graph noise and a false
  signal that framing tests do msgpack round-trips (they don't — payloads are opaque
  bytes).
- **Suggested fix:** Remove the whole `[dev-dependencies]` block (and the direct
  `tungstenite` per 3.1), or keep only entries a future test actually needs — if
  payload-level round-trips are wanted, a single rmp-serde test through `ws_send`/`ws_recv`
  would justify keeping it and simultaneously cover the real payload shape.

### 7.6 [NIT] Redundant `is_loopback` helper duplicates `IpAddr::is_loopback` *(primary of the is_loopback group; also 5.11)*
- **File:line:** `crates/shamir-transport-ws/src/listener.rs:47-52` (spot-checked: present).
- **Issue:** The private `fn is_loopback(ip: IpAddr) -> bool` hand-dispatches to
  `V4::is_loopback`/`V6::is_loopback`, but `IpAddr::is_loopback()` is a stable inherent
  method that already does exactly this — and the crate itself calls it directly at
  `src/tests/listener_tests.rs:62`. The helper adds an indirection layer (and an extra
  private item in a file otherwise free of them) for no behavioural difference.
- **Suggested fix:** Inline to `WsListenerProfile::PlainWsLoopback => addr.ip().is_loopback()`
  at listener.rs:42 and delete the helper.

---

## Finding counts

Raw lens-tagged total (each finding counted as severity-tagged in its own lens file):
**49** — matches the workspace SUMMARY's per-crate breakdown row for this crate exactly.

| Severity | Lens-tagged findings | Finding numbers (dedup groups in one row count once) |
|---|---|---|
| critical | 0 | — |
| high | 3 | 5.1 (subprotocol unimplemented) · 5.2 (hardcoded endpoint paths) · 1.1 + 3.2\* (Origin live-wiring tests — one defect, two lenses) |
| medium | 11 | 1.2 + 5.6 + 6.2 (framing/accept error-path tests — one defect, three lenses) · 3.1 + 5.4 + 4.3\* + 1.6\* + 7.4\* (phantom `tungstenite 0.29` — one defect, five lenses) · 5.3 + 6.3\* + 1.4\* + 3.7\* (send-side cap / `u32` truncation — one defect, four lenses) · 5.5 (zero-length-frame divergence) · 6.1 + 1.3\* (dead `WsAcceptError` variants — one defect, two lenses) · 4.1 + 2.1\* (send alloc / prereserved path defeated — one defect, two lenses) · 7.1 (mid-file `use`) |
| low | 21 | 1.5 + 3.8\* + 5.10\* (origin-matcher robustness — one defect, three lenses) · 3.4 + 4.2 (control-frame flood / uncapped pong queue — one defect, two lenses) · 3.5 + 6.4 + 5.8 + 1.8\* + 7.3\* (exporter placeholder story — one defect, five lenses) · 2.2 + 5.9 + 4.5\* (policy deep-clone per accept — one defect, three lenses) · 4.4 + 3.6\* + 6.6\* + 1.9\* (pre-auth-cap doc misattribution + companion doc warts — one defect, four lenses) · 5.7 (exporter ordering doc contradiction) · 3.3 (Origin echoed into 403 body) · 7.2 (lib.rs re-export set, corrected) · 7.5 + 5.12\* (unused dev-deps — one defect, two lenses) |
| nit | 14 | 1.7 (phantom `accept_no_origin` doc) · 6.5 (scratch buffer unspecified on `Err`) · 6.7 (obs-text Origin reported `Missing`) · 7.6 + 5.11 (redundant `is_loopback` helper — one defect, two lenses) |
| **total** | **49** | lens-tagged findings; **23 distinct defects** after dedup |

\* tagged lower in its own lens file; the dedup group is listed under its highest tag, and
every member is still counted in its own severity row above — columns therefore sum to 49.

Deduplicated defect census: **0 critical, 3 high, 7 medium, 9 low, 4 nit = 23 distinct
defects** (49 lens-tagged findings across the seven files; the phantom-dependency and
exporter-placeholder defects were each independently flagged by five of the seven lenses).

## Fix Plan

**P0 — before anything else ships from this crate**

1. **Resolve hardcoded paths vs configurable `path` (5.2).** Export
   `NATIVE_WS_PATH`/`BROWSER_WS_PATH` consts; either parameterize the accept fns or make
   the server config validator reject/refuse any other ws path **at boot** with a clear
   error. Closes: 5.2 (silent total connectivity loss on reconfigured listeners).
2. **Implement or formally drop the `shamir-v1` subprotocol (5.1).** Negotiate
   `Sec-WebSocket-Protocol` in both callbacks (reject 400 on mismatch, echo `shamir-v1`),
   and update `shamir-client-ts` to request it — or amend the spec and record why. Red
   test: conformant handshake must succeed. Closes: 5.1.
3. **Live handshake tests for the Origin control, Red first per CLAUDE.md TDD (1.1/3.2).**
   Allowed/missing/disallowed Origin → 101/403/403; wrong path → 404; non-UTF-8 Origin →
   rejected; plus the framing error-variant tests and the tightened
   `rejects_oversized_frame` assertion (1.2/5.6/6.2). Closes: 1.1, 1.2, 3.2, 5.6, 6.2 —
   the untested-security-control cluster.
4. **Delete the phantom `tungstenite = "0.29"` dependency (and the unused direct
   `rustls`/`tokio-rustls` entries while there) (3.1).** One-line manifest fix; removes the
   CVE-false-assurance trap and the second compiled parser. Closes: 3.1, 5.4, 4.3, 1.6,
   7.4.

**P1 — soon**

5. **Send-side frame-size guard (5.3):** reject `payload.len() > MAX_WS_FRAME_SIZE` (or a
   symmetric caller-supplied cap) via `u32::try_from` before the cast. Closes: 5.3, 6.3,
   1.4, 3.7.
6. **Make the accept error taxonomy real (6.1):** inspect `Error::Http` status after the
   handshake and re-map into `WrongPath` (404) / `OriginRejected` (403), or delete the dead
   variants and document classification from `Handshake`. Closes: 6.1, 1.3.
7. **Zero-alloc WS send variant (4.1):** `ws_send_sink_vec(sink, Vec<u8>)` ownership-taking
   variant; override `write_frame_prereserved` in shamir-server's `WsFrameWriter`; document
   copy semantics. Closes: 4.1, 2.1.
8. **Control-frame budget (3.4):** cap consecutive non-BINARY messages and return
   `WsFrameError::ControlFrameFlood`; verify/enforce a write-buffer cap for
   tokio-tungstenite 0.24 (or plan the 0.26+ `write_buffer_size` bump). Closes: 3.4, 4.2.
9. **Fail-closed exporter story (3.5):** `channel_binding_for(stream, BindingMode) ->
   Result<[u8; 32], ChannelBindingError>` (error on native-path extraction failure,
   placeholder only for TlsNoExport); wire `BROWSER_CHANNEL_BINDING` into both
   `server_launcher` sites or rename/gate it. Closes: 3.5, 6.4, 5.8, 1.8, 7.3.
10. **Settle the exporter-extraction ordering doc (5.7):** server.rs must agree with
    server_launcher's extract-before-upgrade contract. Closes: 5.7.
11. **Zero-length-frame alignment decision (5.5):** minimum — document the divergence in
    framing.rs's header; better — reject declared==0 so both transports behave identically.
    Closes: 5.5.

**P2 — backlog**

12. **Origin-matcher hardening (1.5):** `try_allow -> Result` (reject scheme-less/malformed
    patterns at boot), reject `*`-leading labels, ASCII-lowercase comparisons, anchor
    wildcard detection. Closes: 1.5, 3.8, 5.10.
13. **Stop deep-cloning the origin policy per accept (2.2):** `Arc<BrowserOriginPolicy>`
    (or borrowed-closure) signature; mechanical switch of the `server_launcher` field.
    Closes: 2.2, 5.9, 4.5.
14. **Static 403 body (3.3):** stop reflecting the attacker-controlled Origin into the
    response body; keep it in structured `tracing` fields. Closes: 3.3.
15. **Doc hygiene pass (4.4 + 1.7 + 6.5 + 6.7 + 7.2):** reword the 4-KiB attribution at
    server.rs:26-28 (and server_tests.rs:11-13) + `LengthMismatch::actual` field doc;
    delete the phantom `accept_no_origin` sentence; document `buf`-on-`Err` (or clear at
    entry); classify non-UTF-8 Origin as present-but-invalid; add `MAX_WS_FRAME_SIZE` to
    the root re-exports. Closes: 4.4, 3.6, 6.6, 1.9, 1.7, 6.5, 6.7, 7.2.
16. **Style/backlog sweep:** hoist the mid-file `use` (7.1 — fold into the workspace
    imports-at-top sweep); delete the four unused dev-deps (7.5, 5.12) or add one real
    msgpack round-trip test; inline `is_loopback` (7.6, 5.11).
