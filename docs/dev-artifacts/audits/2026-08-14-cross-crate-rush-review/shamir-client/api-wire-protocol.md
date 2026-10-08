<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-client — api-wire-protocol revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Version fallback, resume asymmetry, and API hygiene issues remain. DDL errors already retain machine-readable Db codes despite the unreachable reshaping arm.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 13 | 12 | 0 | 0 | 0 | 0 | 1 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — `query_version: 2` stamped unconditionally -- the `server_query_version` negotiation is parsed but never applied to the request version

Status: `confirmed-open`. Current risk: `high`.

Execute stamps CURRENT=2; cursor creation uses the same constant. Historical pre-v2 server dispatch supported only 1, so negotiated fallback is not implemented.

Evidence: [crates/shamir-client/src/client.rs:1061](../../../../../crates/shamir-client/src/client.rs#L1061); [crates/shamir-client/src/cursor_stream.rs:126](../../../../../crates/shamir-client/src/cursor_stream.rs#L126); [crates/shamir-query-builder/src/cursor.rs:43](../../../../../crates/shamir-query-builder/src/cursor.rs#L43); [crates/shamir-server/src/version.rs:45](../../../../../crates/shamir-server/src/version.rs#L45).

Grouping/duplicate: `SUMMARY.md#5.1`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — Handshake/resume frames are positional (array) msgpack while everything post-handshake is named-map -- order is load-bearing, enforced only by duplicated struct definitions

Status: `confirmed-open`. Current risk: `medium`.

Client/server positional mirrors remain duplicated and ResumeInit lacks version. Current layouts match; this is evolution fragility, not current peer incompatibility.

Evidence: [crates/shamir-client/src/client.rs:504](../../../../../crates/shamir-client/src/client.rs#L504); [crates/shamir-client/src/client.rs:895](../../../../../crates/shamir-client/src/client.rs#L895); [crates/shamir-client/src/wire_frames.rs:40](../../../../../crates/shamir-client/src/wire_frames.rs#L40); [crates/shamir-server/src/connection/wire.rs:65](../../../../../crates/shamir-server/src/connection/wire.rs#L65).

Grouping/duplicate: `SUMMARY.md#5.2`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — `resume()` accepts `pinned_hash` but never verifies the server against it -- the pin is dead bookkeeping on the resume path

Status: `confirmed-open`. Current risk: `high`.

Resume stores the pin without comparison. The report's exporter-binding non-exploitability assurance is false: server checks strength, not old/new exporter equality.

Evidence: [crates/shamir-client/src/client.rs:937](../../../../../crates/shamir-client/src/client.rs#L937); [crates/shamir-connect/src/server/resume.rs:310](../../../../../crates/shamir-connect/src/server/resume.rs#L310); [crates/shamir-connect/src/server/resume.rs:324](../../../../../crates/shamir-connect/src/server/resume.rs#L324).

Grouping/duplicate: `SUMMARY.md#3.1`. This row is not another independent defect.

<a id="review-4"></a>

### Claim 4 — `ResumeOptions` has no `connect_timeout`/`request_timeout` -- resumed clients silently revert to unbounded waits

Status: `confirmed-open`. Current risk: `medium`.

ResumeOptions still lacks deadline fields; resume passes None to connect_tcp and stores request_timeout=None. History confirms intentional scope, not a fix.

Evidence: [crates/shamir-client/src/client.rs:99](../../../../../crates/shamir-client/src/client.rs#L99); [crates/shamir-client/src/client.rs:872](../../../../../crates/shamir-client/src/client.rs#L872); [crates/shamir-client/src/client.rs:949](../../../../../crates/shamir-client/src/client.rs#L949).

Grouping/duplicate: `SUMMARY.md#5.3`. This row is not another independent defect.

<a id="review-5"></a>

### Claim 5 — Envelope-level server errors are stringly-typed into `ClientError::Protocol` -- spec §14 codes are not machine-readable

Status: `confirmed-open`. Current risk: `low`.

ErrorEnvelope codes become formatted Protocol strings rather than structured error fields; the registered test explicitly relies on substring matching.

Evidence: [crates/shamir-client/src/client.rs:374](../../../../../crates/shamir-client/src/client.rs#L374); [crates/shamir-client/src/tests/demux_tests.rs:235](../../../../../crates/shamir-client/src/tests/demux_tests.rs#L235).

Grouping/duplicate: `SUMMARY.md#5.4`. This row is not another independent defect.

<a id="review-6"></a>

### Claim 6 — Dead public API: `ClientError::RequestIdMismatch` is never constructed anywhere in the workspace

Status: `confirmed-open`. Current risk: `low`.

The declaration remains; unknown response IDs are logged/dropped and no construction site was found.

Evidence: [crates/shamir-client/src/error.rs:29](../../../../../crates/shamir-client/src/error.rs#L29); [crates/shamir-client/src/client.rs:400](../../../../../crates/shamir-client/src/client.rs#L400).

Grouping/duplicate: `SUMMARY.md#5.5`. This row is not another independent defect.

<a id="review-7"></a>

### Claim 7 — Frame demux is shape-sniffing with no discriminator -- routing correctness relies on field-name disjointness

Status: `confirmed-open`. Current risk: `low`.

The reader tries serde shapes in sequence without a mandatory kind. Current shapes are distinct; overlapping future shapes remain a conditional extension hazard.

Evidence: [crates/shamir-client/src/client.rs:194](../../../../../crates/shamir-client/src/client.rs#L194); [crates/shamir-client/src/client.rs:329](../../../../../crates/shamir-client/src/client.rs#L329); [crates/shamir-connect/src/common/push_envelope.rs:25](../../../../../crates/shamir-connect/src/common/push_envelope.rs#L25).

Grouping/duplicate: `SUMMARY.md#5.6`. This row is not another independent defect.

<a id="review-8"></a>

### Claim 8 — Crate-root doc example no longer compiles: `ConnectOptions` literal missing `connect_timeout` / `request_timeout`

Status: `confirmed-open`. Current risk: `low`.

The example still supplies six fields for an eight-field struct; doctests are disabled. This mismatch is source-proven without compiling.

Evidence: [crates/shamir-client/src/lib.rs:10](../../../../../crates/shamir-client/src/lib.rs#L10); [crates/shamir-client/src/client.rs:85](../../../../../crates/shamir-client/src/client.rs#L85); [crates/shamir-client/Cargo.toml:52](../../../../../crates/shamir-client/Cargo.toml#L52).

Grouping/duplicate: `SUMMARY.md#7.1`. This row is not another independent defect.

<a id="review-9"></a>

### Claim 9 — `get_ddl_op_status`: comment says "feature unavailable rather than a hard error", code returns an error in both branches

Status: `confirmed-open`. Current risk: `low`.

The contradiction remains, but both branches are unreachable after roundtrip's error conversion; actual not_supported errors already expose Db.code.

Evidence: [crates/shamir-client/src/client.rs:1207](../../../../../crates/shamir-client/src/client.rs#L1207); [crates/shamir-client/src/client.rs:1210](../../../../../crates/shamir-client/src/client.rs#L1210); [crates/shamir-client/src/client.rs:1293](../../../../../crates/shamir-client/src/client.rs#L1293).

Grouping/duplicate: `SUMMARY.md#1.7`. This row is not another independent defect.

<a id="review-10"></a>

### Claim 10 — Ambient interner-epoch advertisement is all-or-nothing per batch

Status: `confirmed-open`. Current risk: `nit`.

Any caller-supplied epoch entry disables automatic insertion for every other referenced repo because the entire map must be empty.

Evidence: [crates/shamir-client/src/client.rs:1053](../../../../../crates/shamir-client/src/client.rs#L1053).

Grouping/duplicate: `SUMMARY.md#5.7`. This row is not another independent defect.

<a id="review-11"></a>

### Claim 11 — `ConnectOptions` has no `Default` impl -- every call site must spell out 8 fields

Status: `confirmed-open`. Current risk: `nit`.

No Default or partial-options constructor exists. This is optional ergonomics; mandatory endpoint/credentials make a blanket default questionable.

Evidence: [crates/shamir-client/src/client.rs:58](../../../../../crates/shamir-client/src/client.rs#L58); [crates/shamir-client-node/src/lib.rs:123](../../../../../crates/shamir-client-node/src/lib.rs#L123).

Grouping/duplicate: `SUMMARY.md#5.8`. This row is not another independent defect.

<a id="review-12"></a>

### Claim 12 — Tautological test: `atomic_u8_plumbing_stores_and_reads_correctly` asserts on a locally created `AtomicU8`, not on any crate code

Status: `confirmed-open`. Current risk: `nit`.

The reachable test checks only standard-library loads and never invokes Client's getter or handshake wiring.

Evidence: [crates/shamir-client/src/tests/wire_version_tests.rs:135](../../../../../crates/shamir-client/src/tests/wire_version_tests.rs#L135); [crates/shamir-client/src/tests/mod.rs:9](../../../../../crates/shamir-client/src/tests/mod.rs#L9).

Grouping/duplicate: `SUMMARY.md#1.6`. This row is not another independent defect.

<a id="review-nf-1"></a>

### Claim NF.1 — Builder-only public query construction

Status: `not-applicable`. Current risk: —.

Typed query/cursor builders are used; direct DbRequest construction wraps transport/session operations. No serde_json/json construction exists in this crate.

Evidence: [crates/shamir-query-builder/src/lib.rs:12](../../../../../crates/shamir-query-builder/src/lib.rs#L12); [crates/shamir-client/src/interner_cache_ops.rs:24](../../../../../crates/shamir-client/src/interner_cache_ops.rs#L24); [crates/shamir-client/src/cursor_stream.rs:41](../../../../../crates/shamir-client/src/cursor_stream.rs#L41).

Grouping/duplicate: `SUMMARY.md#NF.5`. This row is not another independent defect.

## Corrections and qualified non-findings

- Current server supports [1,2]; the unconditional-version defect concerns supported fallback to historical/older servers, not current-server rejection.
- Named wire-version tests establish map-field defaults, not complete positional cross-version handshake compatibility.
- DDL unsupported is distinguishable through ClientError::Db.code today; claims requiring Protocol-string matching describe an unreachable branch.
- An unknown operation ID and an unsupported protocol operation are different conditions; docs do not inherently require treating both as Ok(None).
- Default is an optional API design choice; prefer safe constructor/options-builder ergonomics over silently defaulting credentials or enabling TOFU.
- The TypeScript client is an independent implementation, not a wrapper around this Rust Client.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-client -- API & wire-protocol design

## Summary

The crate's public surface is clean and the CLAUDE.md "builder only" query-construction rule is fully honored: zero `serde_json`/`json!` anywhere, all batch/query/cursor/admin ops flow through the re-exported `shamir_query_builder` (`shamir_client::builder`), and the top-level `DbRequest` variants the client does build by hand (`Ping`, `CreateScramUser`, `SetReplicator`, `Repl`, `GetDdlOpStatus`) are exactly the ones the builder crate's module doc explicitly reserves for the client SDK. The one genuine versioning defect: the client stamps `query_version: CURRENT_QUERY_LANG_VERSION` (2) unconditionally, so the `server_query_version` handshake negotiation it carefully parses and exposes is never used to gate the version field itself -- the documented v1-fallback path is unreachable against a pre-v2 server. Secondary theme issues are protocol-shape fragility (positional vs named msgpack split across handshake vs post-handshake frames) and API-contract asymmetries on the resume path. Wire tests are substantive (rid-demux injection via duplex streams, old/new server serde-default compat, live-server v2 e2e), with one tautological exception noted below.

## Findings

### 1. `query_version: 2` stamped unconditionally -- the `server_query_version` negotiation is parsed but never applied to the request version
- **File:line:** `crates/shamir-client/src/client.rs:786-790` (`Client::execute`); also `crates/shamir-client/src/cursor_stream.rs:126` (via `builder::cursor::create_cursor`, which stamps `CURRENT_QUERY_LANG_VERSION`); contrast `client.rs:389` where `server_query_version() >= 2` gates only the id-keyed encoding.
- **Severity:** high
- **Issue:** The client reads `auth_ok.server_query_version` / `resume_ok.server_query_version`, stores it, documents it ("emit v2 protocol only when `server_query_version() >= 2`"), and gates the v2 id-keyed write path on it -- but every `DbRequest::Execute` and `DbRequest::CreateCursor` still carries `query_version: CURRENT_QUERY_LANG_VERSION` (2). The server (`shamir-server/src/db_handler/handler.rs:484`, `cursor_handlers.rs:1156`) rejects unknown versions with `unsupported_query_version` *before* any DB work, and a pre-v2 server build's `SUPPORTED_QUERY_LANG_VERSIONS` is `[1]` only (`shamir-server/src/version.rs`).
- **Failure scenario:** Current client connects to an older deployed server: `server_query_version == 0` is correctly detected, `execute_with_touch` takes its "v1 path: send batch unchanged" branch -- and then every request still fails with `ClientError::Db { code: "unsupported_query_version" }`, because the version stamp itself is the v2 opt-in the protocol docs say to gate. The entire graceful-degradation ladder (`#[serde(default)]` fields, "v1 path: send batch unchanged", `ResultEncoding::Name` fallback) is dead code against the exact server generation it was built for.
- **Suggested fix:** Stamp `query_version = min(CURRENT_QUERY_LANG_VERSION, server_query_version.max(1))` in `Client::execute` (and pass an explicit version to `create_cursor_with_version` from `CursorStream`, or overload `builder::cursor::create_cursor` with the client's negotiated version). Add a regression test connecting the current client to a `SUPPORTED_QUERY_LANG_VERSIONS = [1]`-only stub.

### 2. Handshake/resume frames are positional (array) msgpack while everything post-handshake is named-map -- order is load-bearing, enforced only by duplicated struct definitions
- **File:line:** `crates/shamir-client/src/wire_frames.rs:13-86`; encodes at `crates/shamir-client/src/client.rs:415, 464, 621` (`rmp_serde::to_vec`, positional) vs `client.rs:981` (`encode::write_named`) for `DbRequest`; server-side mirror `shamir-server/src/connection/wire.rs:35-92` (whose comments warn "positional msgpack -- omitting a field shifts array indices").
- **Severity:** medium
- **Issue:** `WireAuthInit`/`WireChallenge`/`WireClientProof`/`WireResumeInit`/`WireAuthOk`/`WireResumeOk` serialize positionally; all post-handshake traffic uses named map encoding. Positional correctness rests on field order staying identical between two independently-maintained struct definitions (client `wire_frames.rs`, server `connection/wire.rs`); nothing in `wire_frames.rs` carries the server side's "append new fields as trailing `#[serde(default)]`" warning. Additionally, the resume path has no version field at all (`WireResumeInit` lacks the `version: u8` that `WireAuthInit` carries), so a future resume-wire shape change has no negotiation axis.
- **Failure scenario:** A developer inserts a field mid-struct in one of the two mirrored definitions (or in the TS/napi clients, which must replicate exact positional order by hand). Decodes fail at handshake with an opaque rmp-serde error -- or, with a trailing-but-optional field added on one side only, silently misparse.
- **Suggested fix:** Either switch handshake frames to `to_vec_named` (rmp-serde's `from_slice` already accepts both shapes on decode, so this is a one-sided-encode change -- coordinate with the server), or at minimum copy the server's positional-compat warning into `wire_frames.rs` and add a version field to the resume frames for future evolution.

### 3. `resume()` accepts `pinned_hash` but never verifies the server against it -- the pin is dead bookkeeping on the resume path
- **File:line:** `crates/shamir-client/src/client.rs:663` (`pinned_hash: opts.pinned_hash` stored verbatim), `95-105` (`ResumeOptions` doc: "The resumed session will carry the same pin"), `592-604`; TLS context: `shamir-transport-tcp/src/tls.rs:63-129` (`make_client_config_no_ca` accepts any certificate); server rationale: `shamir-server/src/connection/wire.rs:76-78` ("the client already has the server's Ed25519 pub-key ... from the original SCRAM handshake").
- **Severity:** medium
- **Issue:** On `connect`, the pin is actively validated by the handshake (`hb.pinned_hash(pin)` -> `process_auth_ok`). On `resume`, `ResumeOkWire` carries no server pubkey/identity material and the client performs zero verification -- `server_pub_key_pin()` on the resumed client returns whatever the caller passed in. With no-CA TLS, the pin is the *only* client-side identity check in the protocol, and it is skipped precisely on the path that reuses a long-lived bearer credential.
- **Failure scenario:** Not currently exploitable (the resumption ticket's exporter binding makes relay/impersonation fail server-side), but the API contract miscommunicates: a caller reading the `pinned_hash` doc reasonably believes identity is verified. Any future loosening of the exporter-binding check would leave nothing client-side to catch it, and a MITM-downgrade of `binding_mode` in `WireResumeInit` is indistinguishable to the caller.
- **Suggested fix:** Either have the server echo `server_pub_key`/`identity_sig` in `resume_ok` and verify the pin client-side (symmetric with `auth_ok`), or rename/document the `ResumeOptions.pinned_hash` field to say explicitly that it is carry-through metadata only and identity is enforced server-side via ticket+exporter binding.

### 4. `ResumeOptions` has no `connect_timeout`/`request_timeout` -- resumed clients silently revert to unbounded waits
- **File:line:** `crates/shamir-client/src/client.rs:596-598` (comment: "ResumeOptions carries no timeout knobs"), `675` (`request_timeout: None`), `95-105` (`ResumeOptions` struct).
- **Severity:** medium
- **Issue:** `ConnectOptions` grew `connect_timeout` and `request_timeout` (task #520), but `ResumeOptions` was not given the knobs, so a client built via `resume()` always runs with unbounded connect *and* per-request waits -- including a server that accepts the resumption and then never answers.
- **Failure scenario:** An app that hardened its primary connections with `request_timeout = Some(5s)` resumes from a ticket (e.g. reconnect after network blip) and hangs forever on the first request against a wedged server; the fix for #520 silently does not apply.
- **Suggested fix:** Add `connect_timeout: Option<Duration>` and `request_timeout: Option<Duration>` to `ResumeOptions`, threading them exactly as `connect()` does.

### 5. Envelope-level server errors are stringly-typed into `ClientError::Protocol` -- spec §14 codes are not machine-readable
- **File:line:** `crates/shamir-client/src/client.rs:283-289` (`ClientError::Protocol(format!("server error envelope: {error}"))`); contrast the structured `ClientError::Db { code, message }` used for `DbResponse::Error` at `client.rs:1019-1024`.
- **Severity:** low
- **Issue:** `ErrorEnvelope.error` carries spec §14 codes (`session_expired`, `session_invalidated`, `authentication_failed`), which are exactly the events a client must react to programmatically (drop the client, re-auth, refresh ticket). The demux flattens them into an unstructured `Protocol(String)`.
- **Failure scenario:** A caller cannot implement "on `session_expired`, resume with the ticket" without `format!`-string matching against the error text, which breaks the moment the message wording changes.
- **Suggested fix:** Add a `ClientError::Session { code: String }` (or reuse `Db { code, message: String::new() }`) for envelope errors, matching on the known code vocabulary.

### 6. Dead public API: `ClientError::RequestIdMismatch` is never constructed anywhere in the workspace
- **File:line:** `crates/shamir-client/src/error.rs:27-29`.
- **Severity:** low
- **Issue:** The variant documents "Server returned a request_id that doesn't match what we sent", but the demux routes purely by `rid` lookup (`client.rs:300-315`); a mismatched/unknown rid is logged and dropped, never turned into this error. No caller in the workspace constructs it.
- **Failure scenario:** API consumers write `matches!(err, ClientError::RequestIdMismatch { .. })` arms that are unreachable dead code; the enum implies a demux behavior that does not exist.
- **Suggested fix:** Remove the variant, or implement rid validation (the reader knows the rid it looked up; mismatch is only possible if envelopes ever carry a second correlation field) -- removal is the honest option.

### 7. Frame demux is shape-sniffing with no discriminator -- routing correctness relies on field-name disjointness
- **File:line:** `crates/shamir-client/src/client.rs:121-135` (`decode_frame`: try `ResponseEnvelope`, then `ErrorEnvelope`), `236-279` (fall through to `PushEnvelope`).
- **Severity:** low
- **Issue:** Incoming frames have no type tag; the reader identifies them by *trying* each serde shape in order. It works today only because `res` / `error` / `push`+`sub`+`seq` field names are disjoint. Any future server->client envelope that happens to contain a bytes field named `res` (streaming chunks, gossip frames, ...) silently decodes as a regular response and is misrouted to a pending oneshot (or dropped as "frame without rid"), with no error anywhere. It also costs up to three decode attempts per unknown frame.
- **Failure scenario:** A v3 server adds a `progress` envelope `{ rid, res: bytes, pct }`; every such frame is demuxed as a `ResponseEnvelope` for that rid, corrupting the in-flight request's payload -- decode then fails in `roundtrip` with an unrelated rmp-serde error.
- **Suggested fix:** Prepend a one-byte envelope kind tag (or a mandatory `t` string field) to every server->client frame and switch on it; until the wire format changes, at least reorder/document the sniff chain and add a test asserting a new-envelope-shaped frame is dropped, not misrouted.

### 8. Crate-root doc example no longer compiles: `ConnectOptions` literal missing `connect_timeout` / `request_timeout`
- **File:line:** `crates/shamir-client/src/lib.rs:10-17` vs `ConnectOptions` (`client.rs:54-89`, 8 fields).
- **Severity:** low
- **Issue:** The primary usage example constructs `ConnectOptions` without the two fields added in task #520. The drift is invisible because `doctest = false` (per workspace policy), so the example is never even compile-checked.
- **Failure scenario:** Users copy the example from docs.rs/source and hit a compile error of missing fields; the crate's first impression is stale.
- **Suggested fix:** Update the literal (add `connect_timeout: None, request_timeout: None`), and consider a `Default` impl for `ConnectOptions` (see finding 11) so the example stays 4 lines and cannot drift field-by-field.

### 9. `get_ddl_op_status`: comment says "feature unavailable rather than a hard error", code returns an error in both branches
- **File:line:** `crates/shamir-client/src/client.rs:935-948`.
- **Severity:** low
- **Issue:** Both the `not_supported` branch and the generic branch return `Err`; only the variant/message differ. The doc contract ("Returns ... `None` if the operation is unknown") is satisfiable via `DdlOpStatus { status: None }`, but "server too old to know this op" is only distinguishable from other DB errors by string-matching the `Protocol` message.
- **Failure scenario:** A caller wanting "poll status; if unsupported, skip" must match on `Err(ClientError::Protocol(m)) if m.contains("not supported by server")` -- brittle, and contradicted by the code's own comment.
- **Suggested fix:** Either return `Ok(None)` for `not_supported` (matching the comment's stated intent), or add a dedicated `ClientError::NotSupported` variant callers can match on.

### 10. Ambient interner-epoch advertisement is all-or-nothing per batch
- **File:line:** `crates/shamir-client/src/client.rs:779-784` (`if batch.interner_epochs.is_empty()`).
- **Severity:** nit
- **Issue:** `Client::execute` populates `interner_epochs` for every distinct repo only when the caller left the map entirely empty. A caller that pre-fills one repo's epoch (e.g. because it just ran `refresh_repo` for a hot repo) silently disables ambient delta advertisement for every *other* repo in the same batch.
- **Suggested fix:** Insert per-repo epochs only for repos not already present in `batch.interner_epochs` (per-repo `entry` API instead of the whole-map guard).

### 11. `ConnectOptions` has no `Default` impl -- every call site must spell out 8 fields
- **File:line:** `crates/shamir-client/src/client.rs:54-89`.
- **Severity:** nit
- **Issue:** `addr`, `server_name`, `username`, `password` genuinely have no default, but `accept_new_host`/`trusted_pin`/`connect_timeout`/`request_timeout` do (documented in-line). The absence of `Default` is what allowed the lib.rs example drift (finding 8) and makes the napi/TS bindings' option plumbing noisier than it needs to be.
- **Suggested fix:** `#[derive(Default)]` (with `accept_new_host: true` needing a manual impl) plus struct-update syntax `ConnectOptions { addr, server_name, username, password, ..Default::default() }`.

### 12. Tautological test: `atomic_u8_plumbing_stores_and_reads_correctly` asserts on a locally created `AtomicU8`, not on any crate code
- **File:line:** `crates/shamir-client/src/tests/wire_version_tests.rs:135-142`.
- **Severity:** nit
- **Issue:** The test creates its own `AtomicU8`, stores 2, loads, asserts equality -- it exercises `std` semantics, not `Client::server_query_version` plumbing, despite its doc claiming to verify "the AtomicU8 plumbing". It inflates the apparent coverage of the `server_query_version` field path (whose real wiring -- `client.rs:573`, `674` -- is only covered by live-server e2e).
- **Suggested fix:** Delete it or replace with a real assertion against a `Client` (e.g. via the existing live-server e2e asserting `client.server_query_version() == 2` after connect).


</details>
