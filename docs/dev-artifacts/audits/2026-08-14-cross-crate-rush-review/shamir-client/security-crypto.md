<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-client — security-crypto independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Unauthenticated ticket transmission and serialized-secret retention remain real. Full-connect checks and bounded serde recursion have positive counter-evidence. The inherited framing non-finding now has a concrete soundness violation.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 15 | 8 | 0 | 0 | 1 | 0 | 6 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — `Client::resume` hands the bearer ticket to an unverified peer — no server-identity check exists on the resume path

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Accepted substitute TLS endpoint receives the bearer ticket before any identity proof. The real server's strength/counter gates allow a valid stolen ticket's first use on a fresh TLS connection.

Evidence: [crates/shamir-client/src/client.rs:866](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L866); [crates/shamir-client/src/client.rs:895](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L895); [crates/shamir-connect/src/server/resume.rs:310](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/resume.rs#L310); [crates/shamir-connect/src/server/resume.rs:324](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/resume.rs#L324).

Grouping/duplicate: [SUMMARY.md#3.1](SUMMARY.md#review-3-1). This is not an additional independent defect.

<a id="review-2"></a>

### Claim 2 — SDK silently discards `rotation_in_progress` and `kdf_upgrade_required` from `auth_ok` — orphan-recovery (spec §6.5) and KDF-upgrade hints (§13) are unreachable

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Both transport mirrors omit the hints and Client supplies None. The full-handshake pin check rejects before rotation recovery. This is fail-closed availability/operational divergence, not automatic trust bypass.

Evidence: [crates/shamir-client/src/wire_frames.rs:66](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/wire_frames.rs#L66); [crates/shamir-client/src/client.rs:603](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L603); [crates/shamir-server/src/connection/handshake.rs:632](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L632); [crates/shamir-connect/src/client/handshake.rs:266](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/handshake.rs#L266).

Grouping/duplicate: [SUMMARY.md#3.2](SUMMARY.md#review-3-2). This is not an additional independent defect.

<a id="review-3"></a>

### Claim 3 — Cleartext password persists in unzeroized serialization buffers after `create_scram_user`

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Default SecretString wiping does not wipe scratch, cloned request, envelope or framing vectors. Process-memory disclosure is required; no direct remote password extraction is demonstrated.

Evidence: [crates/shamir-client/src/client.rs:1104](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1104); [crates/shamir-client/src/client.rs:1252](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1252); [crates/shamir-transport-tcp/src/framing.rs:160](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L160).

Grouping/duplicate: [SUMMARY.md#3.3](SUMMARY.md#review-3-3). This is not an additional independent defect.

<a id="review-4"></a>

### Claim 4 — Early-buffer push map is unbounded in distinct `sub_id`s (server-keyed memory growth)

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Fresh unknown sub IDs allocate retained vectors without a global cap. A hostile authenticated or resume-substituted peer can supply them; an ordinary disconnected user cannot remotely inject encrypted traffic.

Evidence: [crates/shamir-client/src/client.rs:349](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L349); [crates/shamir-client/src/subscription.rs:30](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/subscription.rs#L30).

Grouping/duplicate: [SUMMARY.md#4.3](SUMMARY.md#review-4-3). This is not an additional independent defect.

<a id="review-5"></a>

### Claim 5 — No depth guard on nested msgpack decode of peer frames

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

rmp-serde 1.3.1 guarded array/map dispatch handles the typed serde paths. IdBytes are separately traversed by RecordView with a 128-level guard. Neither guard establishes safe allocation sizes or stack behavior.

Evidence: [Cargo.lock:2949](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2949); [crates/shamir-client/src/client.rs:1292](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1292); [crates/shamir-types/src/record_view/lens.rs:38](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/record_view/lens.rs#L38).

Grouping/duplicate: [SUMMARY.md#3.4](SUMMARY.md#review-3-4). This is not an additional independent defect.

<a id="review-6"></a>

### Claim 6 — No negative-path coverage for pin verification or resume anywhere in the crate's suite

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Client-local fixtures never set a trusted pin or deny TOFU; resume unit tests are wire-only. Server-library rejection and rotation tests exist but do not test SDK identity wiring.

Evidence: [crates/shamir-client/src/tests/interner_cache_tests.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/interner_cache_tests.rs#L98); [crates/shamir-client/src/tests/resume_wire_tests.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/resume_wire_tests.rs#L1); [crates/shamir-connect/tests/integration_resume.rs:984](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/tests/integration_resume.rs#L984).

Grouping/duplicate: [SUMMARY.md#3.5](SUMMARY.md#review-3-5). This is not an additional independent defect.

<a id="review-nits-client-rs-603"></a>

### Claim Nits/client.rs:603 — `resume` extracts the TLS exporter solely to prove availability, then discards it while `WireResumeInit.binding_mode` declares `BindingMode::TlsExporter`

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

This intentionally verifies transport capability; the server derives its fresh exporter and attaches it to the new session. It is not a client possession proof and is not independently erroneous. The missing endpoint authentication remains a separate high defect.

Evidence: [crates/shamir-client/src/client.rs:877](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L877); [crates/shamir-client/src/client.rs:893](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L893); [crates/shamir-connect/src/server/resume.rs:310](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/resume.rs#L310).

Grouping/duplicate: [SUMMARY.md#3.1](SUMMARY.md#review-3-1). This is not an additional independent defect.

<a id="review-nits-client-rs-935-943"></a>

### Claim Nits/client.rs:935-943 — `get_ddl_op_status` comment says `not_supported` is treated as "feature unavailable rather than a hard error" but the code returns `Err(ClientError::Protocol(..))`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The Protocol reshaping branch is dead after roundtrip's typed Db conversion. Correct the comment/arm without removing the already machine-readable error code.

Evidence: [crates/shamir-client/src/client.rs:1209](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1209); [crates/shamir-client/src/client.rs:1293](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1293).

Grouping/duplicate: [SUMMARY.md#1.7](SUMMARY.md#review-1-7). This is not an additional independent defect.

<a id="review-nits-error-rs-28-29"></a>

### Claim Nits/error.rs:28-29 — `ClientError::RequestIdMismatch` is never constructed anywhere; dead public error surface

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The declaration is not constructed by demux, which drops unknown IDs. This is API taxonomy drift.

Evidence: [crates/shamir-client/src/error.rs:29](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/error.rs#L29); [crates/shamir-client/src/client.rs:400](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L400).

Grouping/duplicate: [SUMMARY.md#5.5](SUMMARY.md#review-5-5). This is not an additional independent defect.

<a id="review-nf-1"></a>

### Claim NF.1 — `connect` fail-closed

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Builder rejects no-pin/denied-TOFU, and successful auth checks mutual proof, pin and strict signature. TOFU remains explicitly authorized first-use trust.

Evidence: [crates/shamir-connect/src/client/handshake.rs:149](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/handshake.rs#L149); [crates/shamir-connect/src/client/handshake.rs:258](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/handshake.rs#L258).

Grouping/duplicate: [SUMMARY.md#NF.1](SUMMARY.md#review-nf-1). This is not an additional independent defect.

<a id="review-nf-2"></a>

### Claim NF.2 — KDF-DoS: server-supplied Argon2id params are double-capped before allocation

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Both cap checks precede derivation. The effective protocol ceiling is stricter than the outer safety ceiling; neither proves that multiple allowed derivations cannot exhaust resources.

Evidence: [crates/shamir-connect/src/client/handshake.rs:209](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/handshake.rs#L209); [crates/shamir-connect/src/common/types.rs:123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/types.rs#L123); [crates/shamir-connect/src/common/kdf_params.rs:77](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/kdf_params.rs#L77).

Grouping/duplicate: [SUMMARY.md#NF.2](SUMMARY.md#review-nf-2). This is not an additional independent defect.

<a id="review-nf-3"></a>

### Claim NF.3 — Comparison hygiene and CSPRNG-backed resume nonce generation

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Constant-time proof/pin comparison and strict Ed25519 verification are explicit. Exact rand 0.9.4 archive documents OS-seeded ChaCha12 thread RNG, with fork reseeding limitations. Source: https://docs.rs/crate/rand/0.9.4/source/src/rngs/thread.rs

Evidence: [crates/shamir-connect/src/common/crypto.rs:131](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/crypto.rs#L131); [crates/shamir-connect/src/common/crypto.rs:249](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/crypto.rs#L249); [Cargo.lock:2692](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2692).

Grouping/duplicate: [SUMMARY.md#NF.3](SUMMARY.md#review-nf-3). This is not an additional independent defect.

<a id="review-nf-4"></a>

### Claim NF.4 — Untrusted-input size bounds and fixed-size wire-field validation

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

The 16 MiB cap and fallible wire-field size checks are supported. They do not establish the stricter pre-authentication cap or an allocation bound after opaque IdBytes dispatch.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:110](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L110); [crates/shamir-client/src/client.rs:513](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L513); [crates/shamir-client/src/client.rs:905](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L905).

Grouping/duplicate: [SUMMARY.md#NF.4](SUMMARY.md#review-nf-4). This is not an additional independent defect.

<a id="review-nf-5"></a>

### Claim NF.5 — `unsafe`: none in this crate; dependency framing has a sound SAFETY argument

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `unverified`.

Client unsafe absence is true, but inherited framing is unsound: set_len exposes fresh uninitialized capacity before Tokio 1.49.0 read_exact constructs fully initialized ReadBuf::new. A safe AsyncRead may inspect initialized bytes; dropping a pending payload read also skips clear and returns access to an uninitialized Vec. Exact dependency source and pinned Rust 1.94 set_len contract provide positive counter-evidence, without a reproduction. Source: https://docs.rs/crate/tokio/1.49.0/source/src/io/util/read_exact.rs ; https://docs.rs/crate/tokio/1.49.0/source/src/io/read_buf.rs ; [Rust 1.94 contract](https://raw.githubusercontent.com/rust-lang/rust/1.94.0/library/alloc/src/vec/mod.rs)

Evidence: [crates/shamir-transport-tcp/src/framing.rs:118](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L118); [crates/shamir-transport-tcp/src/framing.rs:129](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L129); [crates/shamir-client/src/client.rs:314](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L314); [Cargo.lock:4195](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4195).

<a id="review-nf-6"></a>

### Claim NF.6 — Typed-builder query construction and §9.4 name/id discipline

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Normal SDK query construction and merges preserve typed builders and opaque string lookup. Public FieldMap mutation and repository recreation are outside the blanket only-server/no-invalidation assurance.

Evidence: [crates/shamir-client/src/interner_cache_ops.rs:268](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L268); [crates/shamir-client/src/interner_cache.rs:96](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache.rs#L96); [crates/shamir-client/src/interner_cache.rs:115](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache.rs#L115).

Grouping/duplicate: [SUMMARY.md#NF.5](SUMMARY.md#review-nf-5). This is not an additional independent defect.

## Evidence and recipe corrections

- The historical sound SAFETY argument is contradicted by initialized-buffer requirements and cancellation, not rescued by clearing only on returned I/O errors.
- Authenticate before ticket disclosure. Signed completion and metadata-only documentation do not close the attack; comparing exporters across connections breaks legitimate resume.
- KDF hints do not request an implemented self-service password/KDF upgrade in the current normative spec.
- A per-frame byte cap does not bound decoded allocations: IdBytes headers bypass serde container safeguards.
- Full-connect pin verification is supported by source but lacks SDK-level negative wiring oracles.
- CSPRNG-backed does not mean fork-reseed-independent, guaranteed nonzero, or immune to every RNG initialization failure.
- The actual server checks prevent downgrade and replay after consumption; these are not client-side endpoint-authentication checks.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-client -- Security & crypto boundary

## Summary

The full `connect` path is solid: TLS 1.3-only with an accept-any-cert verifier is compensated by three client-side checks (SCRAM mutual auth, TOFU/pin comparison, Ed25519 `verify_strict` over the TLS exporter), KDF parameters are double-capped before Argon2id runs, frame sizes are bounded, and there is no `unsafe` in this crate. The largest gap is the `resume` path, which re-uses the accept-any-cert TLS config but performs **none** of the server-identity checks before disclosing the bearer resumption ticket, and the SDK additionally drops the protocol's `rotation_in_progress` / `kdf_upgrade_required` signals, leaving pinned clients with no in-band recovery after server key rotation. Remaining findings are secret-hygiene (unzeroized serialization copies of the `create_scram_user` password) and bounded-but-real resource/recurse-hardening gaps.

## Findings

### 1. `Client::resume` hands the bearer ticket to an unverified peer — no server-identity check exists on the resume path
- **File:line:** `crates/shamir-client/src/client.rs:588-677` (esp. 591-604, 616-629, 663); `crates/shamir-client/src/wire_frames.rs:40-64`
- **Severity:** high
- **Issue:** `make_client_config_no_ca()` (shamir-transport-tcp/src/tls.rs:63-69) accepts *any* server certificate; all server authentication lives in the SCRAM handshake (`process_auth_ok`: mutual-auth proof + pin + Ed25519 over the TLS exporter). `Client::resume` re-uses that accept-any-cert TLS config but performs none of those checks: `WireResumeOk` carries no `server_pub_key`/`identity_sig`, `ResumeOptions::pinned_hash` is stored verbatim into the client (line 663) without ever being compared to anything, and the TLS exporter is extracted only to be discarded (`let _exporter`, line 603) while `WireResumeInit.binding_mode` still *claims* `TlsExporter`. The very first authenticated payload sent on this connection is the long-lived bearer ticket itself.
- **Failure scenario:** an active MITM terminates the TLS connection (any cert is accepted), reads `WireResumeInit.ticket` in cleartext, and now owns the victim's credential — it can open its own authenticated session against the real server (the server-side first-use-wins counter bounds *concurrent* use, but the attacker acts first and the victim's resume loudly fails) or transparently proxy the session. Every MITM protection the `connect` path provides is bypassed simply by inducing the client to resume. Note the server-side #512 / finding-1d analysis (shamir-connect/src/server/resume.rs:324-361) covers *already-stolen* tickets; it does not cover this path *creating* stolen tickets — `connect` would never disclose the ticket to a MITM, `resume` does.
- **Suggested fix:** authenticate the server before/at resume completion: extend `WireResumeOk` with `server_pub_key` + Ed25519 `identity_sig` over `(client_nonce, session_id, expires_at_ns, new-connection TLS exporter)` and verify strictly against `ResumeOptions::pinned_hash` before returning a usable `Client` (fail with a `ServerIdentityChanged`-style error on mismatch). This does not conflict with the #512 decision (it authenticates the server to the client; it does not bind the ticket to the exporter). Interim: document that `resume` must only run over networks where the server endpoint is otherwise authenticated.

### 2. SDK silently discards `rotation_in_progress` and `kdf_upgrade_required` from `auth_ok` — orphan-recovery (spec §6.5) and KDF-upgrade hints (§13) are unreachable
- **File:line:** `crates/shamir-client/src/wire_frames.rs:66-86` (`WireAuthOk` lacks both fields); `crates/shamir-client/src/client.rs:506-516` (hard-codes `rotation_in_progress: None, kdf_upgrade_required: None`)
- **Severity:** medium
- **Issue:** the protocol and the connect crate support both signals: `AuthOkView` carries them (shamir-connect/src/server/handshake.rs:99-103), the server library can attach them (`with_rotation_in_progress` / `with_kdf_upgrade_required`, `complete_auth_ok`), and the client side of shamir-connect has `verify_rotation_in_progress` for exactly the orphan-client case. `shamir-client` neither deserializes nor forwards either field — msgpack decode drops them, and the values handed to `process_auth_ok` are hard-coded `None`.
- **Failure scenario:** server performs an identity-key rotation (the mechanism §6.5 exists for) → every Rust-SDK client (and the napi/Node binding wrapping this crate 1:1) that persisted a pin gets `ClientError::Handshake("ServerIdentityChanged")` on every connect, forever, with no in-band recovery even though the protocol defines one; similarly clients are never told to upgrade stale Argon2id parameters. Availability/operational impact, fail-closed rather than exploitable — but it turns routine key rotation into a manual pin-wipe for all SDK users.
- **Suggested fix:** add both fields to `WireAuthOk` with `#[serde(default)]`, pass them through to `ServerAuthOk`, and surface them on the SDK result (e.g. `Client::connect` returns/exposes the rotation payload after `verify_rotation_in_progress` succeeds, plus a `kdf_upgrade_required()` accessor).

### 3. Cleartext password persists in unzeroized serialization buffers after `create_scram_user`
- **File:line:** `crates/shamir-client/src/client.rs:975-989` (thread-local `REQ_BUF`, `req_bytes` clone, `envelope_bytes`), `crates/shamir-client/src/client.rs:828-842`
- **Severity:** medium
- **Issue:** `roundtrip` serializes the `DbRequest` — for `CreateScramUser` the wire field is the plaintext password — into (a) the **thread-local** `REQ_BUF`, which retains the bytes after the call (only `clear()`ed at the *next* request on that thread, i.e. persists until overwrite/thread exit), (b) `buf.clone()` → `req_bytes`, a plain `Vec`, and (c) `envelope_bytes`, another plain `Vec`. The `drop(req)` at line 842 and its comment ("wipe ASAP") wipe only the typed `SecretString`; at least three unzeroized heap copies of the password outlive the call, defeating the zeroize discipline the surrounding code (and the crate's dependency posture: `Zeroizing` password, `Zeroizing` ticket) is written to provide.
- **Failure scenario:** heap inspection / core dump / swap capture after a `create_scram_user` recovers the new user's cleartext password long after the request completed — precisely what the wipe-on-drop effort elsewhere is meant to prevent.
- **Suggested fix:** zeroize the occupied region of `REQ_BUF` after the clone (or immediately after `write_frame` completes), and route secret-bearing requests' `req_bytes`/`envelope_bytes` through `Zeroizing` (or explicit `.zeroize()` on all exit paths of `roundtrip`).

### 4. Early-buffer push map is unbounded in distinct `sub_id`s (server-keyed memory growth)
- **File:line:** `crates/shamir-client/src/client.rs:259-270`; `crates/shamir-client/src/subscription.rs:30-33`
- **Severity:** low
- **Issue:** per-`sub` entries are capped (`EARLY_BUFFER_CAP = 256` envelopes each), but the `early_buffer` map itself has no cap on the number of distinct sub ids, and the keys come from the peer. A misbehaving (authenticated) server can push frames with fresh random sub ids → one map entry per id, up to 256 envelopes per entry, unbounded total client memory. The registered-subscription path is properly bounded by the mpsc cap; only the pre-registration buffer leaks growth.
- **Failure scenario:** hostile server floods pushes with unique sub ids on a long-lived connection → client RSS grows without bound until OOM.
- **Suggested fix:** cap total buffered envelopes (or total distinct buffered sub ids, e.g. 64) across the map; drop-and-`warn!` beyond it, as the per-sub path already does.

### 5. No depth guard on nested msgpack decode of peer frames
- **File:line:** `crates/shamir-client/src/client.rs:121-135` (`decode_frame`), `client.rs:240` (`PushEnvelope::from_slice`), `client.rs:1018` (`DbResponse` decode)
- **Severity:** low
- **Issue:** server-supplied frames (bounded at 16 MiB) are decoded via recursive serde deserialization into `DbResponse`/`PushEnvelope`, whose `QueryValue`/`Value` trees recurse per nesting level with no depth limit (no `recursion_limit`/depth check anywhere in the crate). A frame of deeply nested arrays/maps can exhaust the task's stack during decode. Threat requires an authenticated (or, per finding 1, MITM-substituted) server, and the exposure is symmetric server-side — but the client is the "untrusted-input handler" for everything the peer sends.
- **Failure scenario:** hostile peer sends a ~16 MiB frame of nested seqs → stack overflow → client process abort (DoS).
- **Suggested fix:** verify rmp-serde's current recursion behavior for the pinned version; if unguarded, add a pre-decode structural depth check (e.g. lightweight scanner rejecting nesting beyond a sane bound) or decode in a `spawn_blocking` with a large stack as mitigation.

### 6. No negative-path coverage for pin verification or resume anywhere in the crate's suite
- **File:line:** all tests use `accept_new_host: true, trusted_pin: None` (`tests/smoke.rs:97-98`, `src/tests/cursor_stream_tests.rs:99-100`, `src/tests/interner_cache_tests.rs:98-99`, `src/tests/ambient_sync_tests.rs:101-102`, `src/tests/v2_passthrough_tests.rs:94-95`, all `tests/*_e2e.rs`); `Client::resume` has only serialization-level tests in-crate (`src/tests/resume_wire_tests.rs`) and a single happy-path e2e outside the crate (`shamir-server/tests/duplex_e2e.rs:239`)
- **Severity:** low
- **Issue:** the SDK's own wiring of the security-critical knobs is untested: no test connects with `trusted_pin: Some(..)` (happy path), no test asserts a pin mismatch is refused with `ServerIdentityChanged`, no test asserts `trusted_pin: None + accept_new_host: false` fails closed, and the `pin_capture` callback + `.expect` at client.rs:539-542 and the `ResumeOptions::pinned_hash` plumbing (which finding 1 shows is a no-op check) are never exercised.
- **Suggested fix:** add e2e tests: (a) pinned connect against the pinned server succeeds; (b) pinned connect after server key rotation (or against a second server instance) fails with the identity-changed error; (c) no pin + TOFU refused fails closed; (d) an in-crate resume e2e (happy path + a wrong-pin variant once finding 1's fix lands).

### Nits
- `client.rs:603` — `resume` extracts the TLS exporter solely to prove availability, then discards it while `WireResumeInit.binding_mode` declares `BindingMode::TlsExporter`; the binding claim is decorative on this path (subsumed by finding 1's fix, which should actually use the exporter).
- `client.rs:935-943` — `get_ddl_op_status` comment says `not_supported` is treated as "feature unavailable rather than a hard error" but the code returns `Err(ClientError::Protocol(..))`; comment/behavior mismatch (correctness, not security).
- `error.rs:28-29` — `ClientError::RequestIdMismatch` is never constructed anywhere; dead public error surface.

## Non-findings (checked, clean)

- `connect` fail-closed: `HandshakeBuilder::build` errors when neither pin nor `accept_new_host` is set (shamir-connect/src/client/handshake.rs:148-153) — the SDK cannot silently skip server auth on the full path.
- KDF-DoS: server-supplied Argon2id params are double-capped *before* allocation (`validate_client_limits` + `validate_client_kdf_safe`, hard ceiling 512 MiB / 16 passes) inside `process_challenge` — a MITM cannot OOM the client via challenge params.
- Comparison hygiene: all secret comparisons (server signature, pin) use `constant_time_eq`; the client only *computes* HMAC tags (`compute_tag_hex`) and never compares them, so no client-side timing oracle exists. Nonce generation for resume uses `rand::rng()` (CSPRNG-backed).
- Untrusted-input size bounds: all frame reads enforce `MAX_FRAME_SIZE_DEFAULT` (16 MiB); fixed-size wire fields (`salt`, nonces, signatures, `session_id`) are `try_into`-checked with protocol errors on mismatch — no panics on malformed input.
- `unsafe`: none in this crate. (The one `unsafe` in the dependency path, `read_frame_into`'s `set_len` in shamir-transport-tcp/src/framing.rs:118-136, carries a sound SAFETY argument with an uninit-leak defense-in-depth reset.)
- Query construction goes through the typed builder everywhere (`interner_cache_ops.rs` uses QueryValue *accessors* only for parsing server responses) — CLAUDE.md's builder-only rule holds; §9.4 "ids only from server responses / names never parsed as numbers" discipline is consistently documented and implemented.

</details>
