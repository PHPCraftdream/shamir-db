<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-transport-ws — style-claude-md independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Source organization broadly follows repository conventions. Import placement and dependency hygiene are minor issues. The root-export allegation was pre-existingly false, not source-fixed, and an unused related constant does not violate one-file-one-group.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 6 | 5 | 0 | 0 | 1 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Mid-file `use` statement violates the "imports at the top" rule

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The futures_util import follows four tests despite the file-header rule. Rust's module import scope does not depend on declaration order; deleting a banner cannot itself break name resolution.

Evidence: [crates/shamir-transport-ws/tests/framing_round_trip.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/tests/framing_round_trip.rs#L98); [CLAUDE.md:610](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L610).

<a id="review-2"></a>

### Claim 2 — `lib.rs` re-export set incomplete vs. module public APIs (and vs. sibling transport crate)

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

ws_recv is exported now and in initial commit 3653540c. MAX_WS_FRAME_SIZE is publicly nameable through framing. Neither repository instructions nor crate documentation require all items at the root.

Evidence: [crates/shamir-transport-ws/src/lib.rs:24](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/lib.rs#L24); [crates/shamir-transport-ws/src/lib.rs:31](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/lib.rs#L31); [crates/shamir-transport-ws/src/framing.rs:26](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L26).

<a id="review-3"></a>

### Claim 3 — `BROWSER_CHANNEL_BINDING` is dead public API; the zero placeholder exists only in prose elsewhere

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Repository non-use is confirmed, but production has an actual zero literal. The helper and constant form a closely related group; neither deadness nor the prose-only allegation proves a runtime defect.

Evidence: [crates/shamir-transport-ws/src/tls_exporter.rs:25](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/tls_exporter.rs#L25); [crates/shamir-server/src/server/server_launcher.rs:1495](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1495).

<a id="review-4"></a>

### Claim 4 — Unused direct dependency `tungstenite = "0.29"` (version-mismatched with tokio-tungstenite)

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Direct 0.29.0 remains unused while all WS public types come from effective 0.24.0. The issue is manifest honesty and incompatible type identities.

Evidence: [crates/shamir-transport-ws/Cargo.toml:20](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/Cargo.toml#L20); [crates/shamir-transport-ws/src/framing.rs:22](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L22); [Cargo.lock:4256](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4256).

Grouping/duplicate: [security-crypto.md#1](security-crypto.md#review-1). This is not an additional independent defect.

<a id="review-5"></a>

### Claim 5 — Unused `[dev-dependencies]`: `hex`, `serde`, `serde_bytes`, `rmp-serde`

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

No current crate Rust test or implementation references the four entries. Removing them is scoped dependency hygiene; inventing a parser test solely to justify them is unnecessary.

Evidence: [crates/shamir-transport-ws/Cargo.toml:32](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/Cargo.toml#L32); [crates/shamir-transport-ws/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/tests/mod.rs#L1); [crates/shamir-transport-ws/tests/framing_round_trip.rs:3](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/tests/framing_round_trip.rs#L3).

<a id="review-6"></a>

### Claim 6 — Redundant `is_loopback` helper duplicates `IpAddr::is_loopback`

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The helper uses the same V4/V6 loopback predicates as the inherent operation used by tests. This is optional readability cleanup with no demonstrated behavioral defect.

Evidence: [crates/shamir-transport-ws/src/listener.rs:47](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/listener.rs#L47); [crates/shamir-transport-ws/src/tests/listener_tests.rs:62](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/tests/listener_tests.rs#L62).

## Evidence and recipe corrections

- The SUMMARY current Claim 7.2 still classifies this optional root-export suggestion as open; it should match this report's refuted decision.
- The 10 policy, 9 listener, 3 server and 7 integration test declarations are registered, but declaration counts establish neither execution nor semantic completeness.
- No all-public-items-at-root or constant-test requirement was found. Missing ws_recv was never a later fix.
- No unrelated style sweep or dependency change was performed.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-transport-ws -- Style & CLAUDE.md structural conformance

## Summary

The crate is largely exemplary against CLAUDE.md's structural rules: `lib.rs` is module decls + re-exports + `#[cfg(test)] mod tests;` only, `src/tests/mod.rs` is a manifest-only re-export file, tests are split by topic into `<topic>_tests.rs`, no implementation file carries an inline `#[cfg(test)] mod tests { ... }` block, every `src/` file hoists its imports to the header, all error enums use `thiserror` (no `anyhow` leakage), and each module file holds a single closely-coupled group rather than unrelated exports. The findings below are drift-level: one mid-file `use` in the integration test, an incomplete `lib.rs` re-export set relative to both the module APIs and the sibling `shamir-transport-tcp` crate, a dead public constant, and unused Cargo.toml dependencies.

## Findings

### 1. Mid-file `use` statement violates the "imports at the top" rule
- **File:line:** `crates/shamir-transport-ws/tests/framing_round_trip.rs:98`
- **Severity:** medium
- **Issue:** `use futures_util::{SinkExt, StreamExt};` sits at line 98, after four complete test functions and a section-banner comment (lines 94–96), instead of in the file header alongside the other imports (lines 3–8). CLAUDE.md is explicit: "All `use` statements live in the **file header** (or the enclosing module's header), never inside a function or block body." This is module-level rather than function-body scope, but it is plainly not the file header — the exact drift the rule targets. `SinkExt` (line 131) and `StreamExt` (line 108) only resolve because of this buried import.
- **Failure scenario:** a reader or automated tool scanning the header concludes `futures_util` is unused; a later edit that reorders or deletes the "Split-half tests" banner section silently breaks the two split-half tests below it. Every other file in the crate (src and tests) follows the header convention, making this file the outlier.
- **Suggested fix:** hoist the import into the header block, i.e. add `use futures_util::{SinkExt, StreamExt};` next to the imports at lines 3–8 and delete line 98. The section banner comment can stay.

### 2. `lib.rs` re-export set incomplete vs. module public APIs (and vs. sibling transport crate)
- **File:line:** `crates/shamir-transport-ws/src/lib.rs:29-35`; gaps at `src/framing.rs:76` (`ws_recv`) and `src/framing.rs:26` (`MAX_WS_FRAME_SIZE`)
- **Severity:** low
- **Issue:** `lib.rs` re-exports `ws_recv_into`, `ws_recv_into_stream`, `ws_send`, `ws_send_sink`, `WsFrameError` — but not `ws_recv` (the allocating convenience variant documented at `framing.rs:72-75`) and not `MAX_WS_FRAME_SIZE`. Nothing is broken (all modules are `pub mod`, and `tests/framing_round_trip.rs:3-5` reaches both via the full `shamir_transport_ws::framing::` path), but the crate's own convention is "every public item re-exported at the root", and the workspace sibling `shamir-transport-tcp/src/lib.rs:12` re-exports its full framing surface including the size const (`read_frame, write_frame, FrameError, MAX_FRAME_SIZE_DEFAULT`). `ws_recv` missing while its four sibling variants are all re-exported reads as an oversight rather than a decision.
- **Failure scenario:** callers who use the root re-exports (as `server.rs` and the doctest-free docs encourage for everything else) discover `ws_recv` only by browsing `framing::`; a future "remove the full-path aliasing" cleanup could then look like `ws_recv` was deliberately private.
- **Suggested fix:** extend the framing re-export to `{ ws_recv, ws_recv_into, ws_recv_into_stream, ws_send, ws_send_sink, WsFrameError, MAX_WS_FRAME_SIZE }` (or, if the const is intentionally kept namespaced, that's fine — but then document the intent and add `ws_recv`, which has no such rationale).

### 3. `BROWSER_CHANNEL_BINDING` is dead public API; the zero placeholder exists only in prose elsewhere
- **File:line:** `crates/shamir-transport-ws/src/tls_exporter.rs:25`
- **Severity:** low
- **Issue:** `pub const BROWSER_CHANNEL_BINDING: [u8; 32]` has zero references in the entire workspace (grep: definition only). It is not re-exported in `lib.rs` (unlike its sibling `extract_tls_exporter_from_stream`), not used by `accept_browser_ws` (which validates Origin but never touches the binding value), and not covered by any test. Meanwhile the `[0u8; 32]` placeholder is described in prose in `lib.rs:11-12` and `server.rs:11` ("`tls_exporter_or_zeros = [0u8; 32]`"), so the constant and the documentation can drift independently. Structurally it also makes `tls_exporter.rs` the one file whose second export is unanchored to any consumer.
- **Failure scenario:** if the placeholder policy ever changes (e.g. a hashed fallback per the Noise-NK v2 roadmap mentioned in `listener.rs:7-8`), someone updates the docs and `accept_browser_ws` but misses the unused const — or vice versa — leaving a stale, exported "single source of truth" that isn't one.
- **Suggested fix:** either (a) wire it in: have the browser accept path / caller-facing docs reference `BROWSER_CHANNEL_BINDING` and re-export it from `lib.rs`, or (b) delete the const until a consumer exists.

### 4. Unused direct dependency `tungstenite = "0.29"` (version-mismatched with tokio-tungstenite)
- **File:line:** `crates/shamir-transport-ws/Cargo.toml:20`
- **Severity:** low
- **Issue:** No file in the crate ever names `tungstenite` directly — every tungstenite item (`Message`, `WebSocketConfig`, `Error`, handshake types) is reached through the `tokio_tungstenite::tungstenite::` re-export path (`framing.rs:22-23`, `server.rs:17-19`). The declared direct dep is also `"0.29"` while `tokio-tungstenite = "0.24"` bundles tungstenite `0.24`, so if someone did start using it directly they would get types incompatible with everything this crate passes around.
- **Failure scenario:** a contributor adds `use tungstenite::Message;` for convenience and gets a *different* `Message` type (0.29 vs 0.24) than `ws_send` accepts, producing confusing trait-bound errors; meanwhile the unused dep bloats the lockfile in the meantime.
- **Suggested fix:** delete the `tungstenite = "0.29"` line; continue accessing tungstenite items exclusively via the `tokio_tungstenite::tungstenite` path, as the code already does.

### 5. Unused `[dev-dependencies]`: `hex`, `serde`, `serde_bytes`, `rmp-serde`
- **File:line:** `crates/shamir-transport-ws/Cargo.toml:32-36`
- **Severity:** low
- **Issue:** None of `hex`, `serde`, `serde_bytes`, or `rmp-serde` appears in any `.rs` file in the crate (grep over `crates/shamir-transport-ws/**/*.rs`: zero matches). All test files (`src/tests/*`, `tests/framing_round_trip.rs`) depend only on `tokio`, `futures-util`, `tokio-tungstenite`, and the crate itself.
- **Failure scenario:** none at runtime; cost is lockfile/build-graph noise and a false signal that framing tests do msgpack round-trips (they don't — payloads are opaque bytes).
- **Suggested fix:** remove the whole `[dev-dependencies]` block, or keep only entries a future test actually needs.

### 6. Redundant `is_loopback` helper duplicates `IpAddr::is_loopback`
- **File:line:** `crates/shamir-transport-ws/src/listener.rs:47-52`
- **Severity:** nit
- **Issue:** The private `fn is_loopback(ip: IpAddr) -> bool` hand-dispatches to `V4::is_loopback`/`V6::is_loopback`, but `IpAddr::is_loopback()` is a stable inherent method that already does exactly this — and the crate itself calls it directly at `src/tests/listener_tests.rs:62` (`l.local_addr().unwrap().ip().is_loopback()`). The helper adds an indirection layer (and an extra private item in a file otherwise free of them) for no behavioural difference.
- **Failure scenario:** none; purely a readability/maintenance nit.
- **Suggested fix:** inline to `WsListenerProfile::PlainWsLoopback => addr.ip().is_loopback()` at `listener.rs:42` and delete the helper.

## Conformance notes (checked, no finding)

- **mod.rs re-export-only:** `src/tests/mod.rs` is exactly the documented manifest (`pub mod` lines, no code); `lib.rs` carries only doc comments, module decls, re-exports, and `#[cfg(test)] mod tests;`.
- **One-file-one-export:** every module file is a single closely-coupled group (framing variants + their shared error/const; origin policy + rejection enum + validator; accept paths + accept error; listener profile + bind error + bind/validate fns), matching the rule's "closely-coupled group" carve-out.
- **tests/ layout:** `src/tests/` split by topic (`browser_tests.rs`, `listener_tests.rs`, `server_tests.rs`) per the documented `<topic>_tests.rs` convention; the crate-root `tests/framing_round_trip.rs` mirrors the established sibling pattern (`shamir-transport-tcp/tests/framing.rs`) and is a genuine public-API integration test, so its placement conforms. Coverage is reasonable for the claims: 10 origin-policy tests, 9 listener-profile tests, 3 accept-path tests (including live-handshake wiring of the 16 MiB cap), 7 framing round-trip/negative tests.
- **Imports at top:** all `src/` files hoist every `use` to the header; grep finds zero indented (function-body) `use` statements crate-wide. Inline `#[allow(clippy::result_large_err)]` attributes at `server.rs:87` and `server.rs:122` each carry the required one-line justification comment.
- **Error handling:** all four error enums derive `thiserror::Error` with `#[from]` where natural; no `anyhow`, no `panic!` outside tests.

</details>
