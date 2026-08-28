# shamir-transport-ws -- Correctness & TDD-coverage

## Summary

The crate's core happy paths are genuinely tested (framing round-trips, split-half duplex, live
16 MiB-cap accept), and test organization conforms to CLAUDE.md's layout (per-crate `tests/` dir,
manifest-only `mod.rs`, `#[cfg(test)] mod tests;` wiring, no inline test mods). The two dominant
gaps are coverage-shaped: the browser endpoint's spec-§9 Origin enforcement (`accept_browser_ws`)
has **zero** test coverage at the handshake level anywhere in the workspace, and the framing layer's
entire malformed-input surface (`LengthMismatch`, `NonBinaryMessage`, `PeerClose`, and even the
`TooLarge` variant assertion) is untested or under-asserted. Separately, `WsAcceptError::WrongPath`
and the `OriginRejected(#[from])` conversion are unreachable — the typed error API advertises
discrimination it can never deliver.

## Findings

### 1. `accept_browser_ws` — the spec §9 Origin enforcement path — has zero test coverage
- **File:line:** `crates/shamir-transport-ws/src/server.rs:108-146`; absent from
  `src/tests/server_tests.rs` (native-only) and `src/tests/browser_tests.rs` (pure
  `validate_origin` only); confirmed workspace-wide: only non-test reference is
  `shamir-server/src/server/server_launcher.rs:1266`.
- **Severity:** high (for this theme)
- **Issue:** Only the pure function `validate_origin` is tested. Nothing tests the handshake
  wiring it is embedded in: the header extraction (`req.headers().get(ORIGIN)` +
  `to_str().ok()`, where a non-UTF-8 header silently maps to `None`→`Missing`), the 403
  `ErrorResponse` on rejection, the wrong-path 404, or that the policy actually gates the 101
  upgrade. `shamir-server/tests/mvp_ws_e2e.rs` exercises only `/shamir/v1` (native).
- **Failure scenario:** A refactor of the callback (inverted check, dropped `validate_origin`
  call, path-string typo, header name change) silently disables the primary cross-site
  WebSocket-hijacking defence — and the entire suite stays green. This is exactly the regression
  class the Red step of the TDD protocol exists to pin.
- **Suggested fix:** Add live handshake tests in `src/tests/server_tests.rs` mirroring the
  existing `live_accept_*` duplex pattern: (a) allowed Origin → upgrade succeeds; (b) missing
  Origin → handshake fails with HTTP 403; (c) disallowed Origin → 403; (d) wrong path on both
  endpoints → 404; (e) non-UTF-8 Origin value → rejected.

### 2. Framing's malformed-input error paths untested; `rejects_oversized_frame` is under-asserted
- **File:line:** `tests/framing_round_trip.rs:57-71` (asserts only `is_err()`);
  `crates/shamir-transport-ws/src/framing.rs:148-153` (short-message `LengthMismatch`),
  `:157-162` (prefix-tamper `LengthMismatch` — the module doc's headline "defence-in-depth"
  invariant, framing.rs:7-9), `:173` (`PeerClose` on Close frame), `:144` (`PeerClose` on stream
  end), `:177-182` (`NonBinaryMessage` for TEXT), `:176` (Ping/Pong skip loop) — none tested.
- **Severity:** medium
- **Issue:** The only negative framing test sends a valid 200-byte frame and receives with a tiny
  cap, then asserts merely `result.is_err()`. It would still pass if the error were an unrelated
  `Io`/`Capacity` error, i.e. it does not pin the `TooLarge` contract. Every tamper path the
  module doc advertises (mismatched inner prefix, sub-4-byte message, TEXT rejection) is dead
  code as far as the suite can prove. Note these are pure logic over an already-assembled
  `Message` — trivially testable without raw-frame crafting.
- **Failure scenario:** A reorder of the `declared != body.len()` vs `TooLarge` checks, a flipped
  comparison, or an accidental `Ok(())` on the TEXT arm would not be caught by any test.
- **Suggested fix:** (a) Assert `matches!(result, Err(WsFrameError::TooLarge { .. }))` (and the
  declared/actual fields). (b) Add unit-style tests through `ws_recv_into_stream` over a custom
  `Stream` yielding crafted `Message`s: `Binary(vec![0,0,0,9, ...])` with wrong body, 2-byte
  binary, `Text`, `Close`, then `None` — one per error variant.

### 3. `WsAcceptError::WrongPath` never constructed; `OriginRejected(#[from])` unreachable
- **File:line:** `crates/shamir-transport-ws/src/server.rs:60-61` (`#[from] OriginRejected`),
  `:64-70` (`WrongPath`), vs. `:91-93` / `:125-127` / `:135-139` where both conditions are
  converted to `ErrorResponse` inside the callback and surface only via `?` as `Handshake`.
- **Severity:** low
- **Issue:** Both accept fns translate wrong-path and origin rejections into a
  `tungstenite::Error::Http(ErrorResponse)` before `?` maps them, so the only variant ever
  produced is `Handshake`. The typed variants are dead public API (re-exported from `lib.rs:34`):
  no caller can ever match `WrongPath` or `OriginRejected`. Verified: `WsAcceptError` appears
  only in `server.rs`/`lib.rs`; `shamir-server` merely logs `?e`.
- **Failure scenario:** A caller writing `match err { WsAcceptError::WrongPath {..} => ... }`
  gets an arm that can never fire and misclassifies every rejection as a generic handshake
  failure; the 404-vs-403 distinction is recoverable only by digging into
  `Handshake(Error::Http(resp))`.
- **Suggested fix:** Either inspect the `Error::Http` status inside the accept fns and re-wrap
  into the typed variants (404 → `WrongPath`, 403 → `OriginRejected`), or delete the two
  variants so the API stops promising discrimination it doesn't deliver.

### 4. `ws_send_sink` has no send-side cap; `payload.len() as u32` silently truncates
- **File:line:** `crates/shamir-transport-ws/src/framing.rs:118` (`as u32`), `:114-124`.
- **Severity:** low
- **Issue:** The recv side enforces `max_frame_size`, but the send side accepts any payload and
  casts its length to `u32`. For a payload ≥ 4 GiB the prefix wraps and the frame goes on the
  wire with a corrupt declared length.
- **Failure scenario:** System-wide the damage is contained — the receiver's mismatch check
  (framing.rs:157) rejects the frame — but a caller feeding an unbounded buffer gets a
  confusing `LengthMismatch` on the far side instead of a local error, and tungstenite's own
  caps only cover configurations that set them.
- **Suggested fix:** Return `WsFrameError::TooLarge { actual: payload.len(), max: MAX_WS_FRAME_SIZE }`
  (or a caller-supplied cap) when `payload.len() > MAX_WS_FRAME_SIZE`, mirroring the recv-side
  asymmetry check.

### 5. Wildcard origin matcher is raw string logic — accepts literal `*` and userinfo forms; unvalidated patterns fail silently
- **File:line:** `crates/shamir-transport-ws/src/browser.rs:56-76` (`origin_matches`),
  `:37-41` (`allow` performs no pattern validation).
- **Severity:** low
- **Issue:** (a) The `*` label itself matches literally: `Origin: https://*.example.com` is
  accepted by pattern `https://*.example.com` (after_scheme = `*.example.com`, first dot is at
  index 1, suffix compares equal). (b) Userinfo-bearing origins pass: `https://user@app.example.com`
  matches `https://*.example.com` because the "first dot" split ignores the `@`. (c) A pattern
  without a scheme (`*.example.com`) contains no `//*.`, falls through to exact comparison, and
  therefore matches *nothing* — a config typo silently narrows the allowlist with no error at
  construction. Browsers never serialize origins in forms (a)/(b), so the CSRF defence holds in
  practice (and failures are fail-closed), but the matcher accepts origins a strict
  host-label match would reject.
- **Suggested fix:** In `origin_matches`, reject `after_scheme.starts_with('*')` and validate the
  first component as a non-empty DNS label without `/`, `@`, `:`, or `?`. In `allow`, reject (or
  `debug_assert`) patterns lacking `<scheme>://` so operator typos surface at boot.

### 6. Direct `tungstenite = "0.29"` dependency is unused and version-skewed vs the effective 0.24
- **File:line:** `crates/shamir-transport-ws/Cargo.toml:20` (declared), vs. every import going
  through `tokio_tungstenite::tungstenite` (framing.rs:22-23, server.rs:17-19) where
  tokio-tungstenite 0.24 re-exports tungstenite **0.24**.
- **Severity:** low
- **Issue:** No file in the crate (or workspace, per grep) imports `tungstenite::` directly, so
  the 0.29 dependency resolves but is never used — while pulling a second, incompatible
  major version into the lockfile. A future `use tungstenite::Message` would compile against
  0.29 types that do not unify with the `WebSocketStream`'s 0.24 types, producing confusing
  type errors (or, worse for enums like `Message`, silent no-unification traps).
- **Suggested fix:** Remove the direct dependency, or pin it to `"0.24"` to match
  tokio-tungstenite's re-export if a direct path is intentionally kept.

### 7. `BrowserOriginPolicy::empty()` doc references a nonexistent `accept_no_origin` mode
- **File:line:** `crates/shamir-transport-ws/src/browser.rs:28-30`.
- **Severity:** nit
- **Issue:** The doc says the empty policy "rejects everything except the explicit
  `accept_no_origin = true` mode (which is for testing only)". No such mode or parameter exists
  anywhere in the crate — `validate_origin` (browser.rs:95-105) unconditionally rejects a
  missing origin, and `accept_browser_ws` exposes no bypass.
- **Suggested fix:** Delete the stale sentence; if an escape hatch is ever added, document it
  then.

### 8. `BROWSER_CHANNEL_BINDING` is dead; the zeros invariant is encoded twice
- **File:line:** `crates/shamir-transport-ws/src/tls_exporter.rs:25` (unused const);
  duplicate literal at `shamir-server/src/server/server_launcher.rs:1263` (`let exporter = [0u8; 32];`).
- **Severity:** nit
- **Issue:** The exported const exists precisely to single-source the "browser exporter = zeros
  per spec §6.4" invariant, but the production browser accept loop hardcodes its own `[0u8; 32]`.
  Two encodings of a protocol-mandated value is a drift hazard.
- **Suggested fix:** Use `shamir_transport_ws::tls_exporter::BROWSER_CHANNEL_BINDING` in
  `accept_loop_ws_browser` (or delete the const if the cross-crate dependency is unwanted).

### 9. Error-semantics / doc-accuracy warts
- **File:line:** `crates/shamir-transport-ws/src/framing.rs:148-153` vs `:42-43` (field doc);
  `crates/shamir-transport-ws/src/server.rs:27` (HIGH-1 claim).
- **Severity:** nit
- **Issue:** (a) For a <4-byte message the error reports `actual: bytes.len()` while the
  `LengthMismatch::actual` field doc says "WS message body length minus 4" — the reported number
  doesn't follow the documented semantic. (b) `server_ws_config`'s doc states the 4 KiB pre-auth
  logical check is "enforced in `crate::framing::ws_recv_into`" — framing enforces whatever cap
  the *caller* passes (the actual enforcement point is the shamir-server framer
  `framer.rs:341/399` passing the phase-dependent `max`); as written, the doc attributes an
  invariant to this crate that lives at its call sites.
- **Suggested fix:** Either special-case the short-message error (new variant or corrected
  `actual`) or amend the field doc; reword the server.rs comment to "enforced by callers of
  `ws_recv_into` via the `max_frame_size` parameter".
