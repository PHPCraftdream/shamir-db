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

