<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-types — security-crypto revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Allocation hardening and malformed-mode handling remain open. The username-hash production identity bridge is source-proven retired. Other findings are conditional boundary risks or hygiene, not demonstrated remote exploits.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 8 | 7 | 1 | 0 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Unbounded preallocation from attacker-controlled msgpack array/map headers (zerocopy decoder)

Status: `confirmed-open`. Current risk: `high`.

Header counts still reach uncapped custom-decoder and merge allocations. A five-byte header proves an enormous allocation request, not an experimentally observed abort. Current S-write uses the lens rather than this custom decoder; validator de-interning has its own uncapped top-level map allocation.

Evidence: [crates/shamir-types/src/codecs/interned/messagepack.rs:305](../../../../../crates/shamir-types/src/codecs/interned/messagepack.rs#L305); [crates/shamir-types/src/codecs/interned/messagepack.rs:318](../../../../../crates/shamir-types/src/codecs/interned/messagepack.rs#L318); [crates/shamir-types/src/codecs/interned/messagepack.rs:581](../../../../../crates/shamir-types/src/codecs/interned/messagepack.rs#L581); [crates/shamir-types/src/codecs/interned/codec.rs:154](../../../../../crates/shamir-types/src/codecs/interned/codec.rs#L154); [crates/shamir-engine/src/table/write_exec.rs:362](../../../../../crates/shamir-engine/src/table/write_exec.rs#L362); [crates/shamir-engine/src/table/write_exec.rs:382](../../../../../crates/shamir-engine/src/table/write_exec.rs#L382).

Grouping/duplicate: `error-handling-lifecycle.md:1`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — Fail-open mode fallback in `ResourceMeta::from_record`

Status: `confirmed-open`. Current risk: `medium`.

Negative, oversized, wrong-type, and absent modes all collapse to OPEN. Current facade metadata reads use this parser. Broad access requires the remaining ancestor checks to allow traversal and does not grant non-owner Manage; malformed values must be distinguished from intentionally compatible absent fields.

Evidence: [crates/shamir-types/src/access.rs:275](../../../../../crates/shamir-types/src/access.rs#L275); [crates/shamir-types/src/access.rs:702](../../../../../crates/shamir-types/src/access.rs#L702); [crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:57](../../../../../crates/shamir-db/src/shamir_db/shamir_db/access_control.rs#L57); [crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:850](../../../../../crates/shamir-db/src/shamir_db/shamir_db/access_control.rs#L850).

<a id="review-3"></a>

### Claim 3 — Append-only interner accepts unlimited untrusted key names; FxHash rationale inverted

Status: `confirmed-open`. Current risk: `medium`.

No distinct-name quota exists in Interner. The live InternerTouch handler interns and persists caller-supplied names after Store Write authorization. Thus cumulative growth is reachable by a principal permitted to write that store, not an arbitrary unauthenticated client. FxHash collision construction speed is unverified.

Evidence: [crates/shamir-types/src/core/interner/interner.rs:138](../../../../../crates/shamir-types/src/core/interner/interner.rs#L138); [crates/shamir-types/src/types/common.rs:8](../../../../../crates/shamir-types/src/types/common.rs#L8); [crates/shamir-db/src/shamir_db/execute/admin_interner.rs:129](../../../../../crates/shamir-db/src/shamir_db/execute/admin_interner.rs#L129); [crates/shamir-db/src/shamir_db/execute/admin_interner.rs:159](../../../../../crates/shamir-db/src/shamir_db/execute/admin_interner.rs#L159); [crates/shamir-db/src/shamir_db/execute/admin_interner.rs:168](../../../../../crates/shamir-db/src/shamir_db/execute/admin_interner.rs#L168); [CLAUDE.md:351](../../../../../CLAUDE.md#L351).

<a id="review-4"></a>

### Claim 4 — `principal64_from_username` mints principal ids with seedless FxHasher

Status: `fixed`. Current risk: —.

The helper remains for fixtures, but no non-test/non-bench production caller was found. Both alleged live bridge sites now resolve directory-minted principal ids through PrincipalResolver; server session actors use principal64(session.user_id). Directory minting rejects zero and occupied projections. The helper's rustdoc is stale.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:195](../../../../../crates/shamir-db/src/shamir_db/shamir_db/access_control.rs#L195); [crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:1056](../../../../../crates/shamir-db/src/shamir_db/shamir_db/access_control.rs#L1056); [crates/shamir-server/src/db_handler/handler.rs:128](../../../../../crates/shamir-server/src/db_handler/handler.rs#L128); [crates/shamir-server/src/ports.rs:45](../../../../../crates/shamir-server/src/ports.rs#L45); [crates/shamir-server/src/user_directory.rs:526](../../../../../crates/shamir-server/src/user_directory.rs#L526); [crates/shamir-types/src/access.rs:47](../../../../../crates/shamir-types/src/access.rs#L47).

<a id="review-5"></a>

### Claim 5 — Generic `bincode::from_bytes` helper lacks the depth caps its sibling decoders have

Status: `confirmed-open`. Current risk: `low`.

The pinned bincode 1.3.3 helper has no wrapper depth/size policy; compatible recursive types remain a conditional stack risk. Inspected production callers decode bounded-shape counters or interner entries from storage. The proposed QueryValue nested-list exploit is refuted: Value requests deserialize_any, unsupported by bincode 1.3.3.

Evidence: [crates/shamir-types/src/codecs/basic/bincode.rs:51](../../../../../crates/shamir-types/src/codecs/basic/bincode.rs#L51); [crates/shamir-types/src/types/value.rs:283](../../../../../crates/shamir-types/src/types/value.rs#L283); [Cargo.lock:413](../../../../../Cargo.lock#L413); [crates/shamir-engine/src/table/interner_manager.rs:200](../../../../../crates/shamir-engine/src/table/interner_manager.rs#L200); [crates/shamir-engine/src/table/record_counter.rs:201](../../../../../crates/shamir-engine/src/table/record_counter.rs#L201).

<a id="review-6"></a>

### Claim 6 — `SecretString`: constant-time-comparison footgun plus avoidable `unsafe`

Status: `confirmed-open`. Current risk: `low`.

Derived equality is not a constant-time contract; unsafe UTF-8-preserving zeroization and missing lifecycle tests remain. The current inspected password consumer derives SCRAM material, not a cleartext equality check. Exact first-byte timing, remote prefix extraction, and the pinned replacement trait implementation were not established.

Evidence: [crates/shamir-types/src/secret.rs:21](../../../../../crates/shamir-types/src/secret.rs#L21); [crates/shamir-types/src/secret.rs:40](../../../../../crates/shamir-types/src/secret.rs#L40); [crates/shamir-types/src/secret.rs:67](../../../../../crates/shamir-types/src/secret.rs#L67); [crates/shamir-types/src/tests/secret_tests.rs:1](../../../../../crates/shamir-types/src/tests/secret_tests.rs#L1); [crates/shamir-server/src/db_handler/admin.rs:183](../../../../../crates/shamir-server/src/db_handler/admin.rs#L183); [Cargo.lock:5473](../../../../../Cargo.lock#L5473).

<a id="review-7"></a>

### Claim 7 — Predictable RecordId random tail and embedded timestamps — invariant undocumented

Status: `confirmed-open`. Current risk: `low`.

RecordId still embeds timestamps and uses Xoshiro rather than a cryptographic RNG, without a non-capability warning. No possession-only authorization path was established. Exact recovery from a handful of ids, especially truncated from_ts_seq tails and interleaved threads, remains unverified.

Evidence: [crates/shamir-types/src/types/record_id.rs:45](../../../../../crates/shamir-types/src/types/record_id.rs#L45); [crates/shamir-types/src/types/record_id.rs:48](../../../../../crates/shamir-types/src/types/record_id.rs#L48); [crates/shamir-types/src/types/record_id.rs:74](../../../../../crates/shamir-types/src/types/record_id.rs#L74); [crates/shamir-types/src/types/record_id.rs:84](../../../../../crates/shamir-types/src/types/record_id.rs#L84); [Cargo.lock:2749](../../../../../Cargo.lock#L2749).

<a id="review-8"></a>

### Claim 8 — Log-forging surface: raw resource names rendered into trace/denial lines

Status: `confirmed-open`. Current risk: `low`.

ResourcePath accepts arbitrary strings and Display/trace interpolate them without control-character escaping. Exploitability depends on the construction path: database/repository and function-folder creation already apply restrictive name validation. Ordinary quotes are not themselves fabricated log-line proof.

Evidence: [crates/shamir-types/src/access.rs:565](../../../../../crates/shamir-types/src/access.rs#L565); [crates/shamir-types/src/access.rs:622](../../../../../crates/shamir-types/src/access.rs#L622); [crates/shamir-types/src/access.rs:658](../../../../../crates/shamir-types/src/access.rs#L658); [crates/shamir-db/src/shamir_db/execute/helpers.rs:84](../../../../../crates/shamir-db/src/shamir_db/execute/helpers.rs#L84); [crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs:28](../../../../../crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs#L28).

## Corrections and qualified non-findings

- The #559 migration predates this review. The old two-live-call-sites claim was already stale, not merely a subsequently completed task.
- Security severity must reflect authorized Store Write access for interner growth and conditional recursive-type exposure for bincode.
- Bincode 2.x configuration is not evidence of a recursion limit; that migration recommendation needs a pinned reference before making such a guarantee.
- The sole executable unsafe block is feature-gated and source-sound under its current exclusive borrow and zero-byte UTF-8 preservation. Its existence is hygiene debt, not proven undefined behavior.
- SecretString has no zeroizing Drop without crypto. Cargo.toml documents that feature contract; this is not an unexpected runtime downgrade.
- Depth caps and checked lens reads exist, but universal panic-free/allocation-safe claims are too broad: index preallocation is uncapped, and fields() silently terminates on malformed entries.
- validate_keys_resolve rejects unresolved ids that its iterators yield; because malformed iteration can terminate silently, it is not proof of complete structural validation of every input.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-types -- Security & crypto boundary

## Summary

Security posture is strong overall: the crate contains exactly one `unsafe` block (a correct-but-avoidable zeroize in `secret.rs`), both purpose-built MessagePack decoders enforce a 128-deep recursion cap, the RecordView lens bounds-checks every read (`checked_add` throughout) and documents itself as untrusted-input safe without panicking, and the S-write spine (`validate_keys_resolve`) explicitly refuses to persist records whose interner ids are forged. The findings below concentrate on three weak spots: (1) allocation-size hardening was applied to the serde visitor path (`SANE_PREALLOC_CAP`, value.rs) but **not** to the hand-rolled zerocopy decoder or the byte-level merge path, leaving a tiny-payload remote OOM/abort; (2) two fail-open defaults (`ResourceMeta::from_record` mode fallback, seedless-hash principal bridging); (3) footguns on the secret/id primitives (non-constant-time comparison derive, predictable RecordId PRNG). Test coverage is good for access semantics (`permits`/`class_of`/mode/principal64 all exercised in `src/tests/access_tests.rs`) but there is no test driving a huge declared `Array32`/`Map32` header through the zerocopy decoder, and `SecretString`'s zeroize-on-drop behavior is untested.

## Findings

### 1. Unbounded preallocation from attacker-controlled msgpack array/map headers (zerocopy decoder)

- File:line: `crates/shamir-types/src/codecs/interned/messagepack.rs:305` (`decode_array`: `Vec::with_capacity(len)`), `:318` (`decode_map`: `new_map_wc(len)`); same pattern storage-side at `:581-585` (`merge_storage_bytes`: `Vec::with_capacity(n_old)` + `TFxMap::with_capacity_and_hasher(n_old, ..)`).
- Severity: high
- Issue: `len` comes straight from the wire marker — `Array32`/`Map32` declare up to `2^32 - 1` elements from just 5 bytes. Each `InnerValue` slot is ~50+ bytes (IndexMap/Blob variants), so a 5-byte header requests hundreds of GB. `value.rs:117-122` documents fixing this exact bug class for the serde visitor path ("driving `Vec::with_capacity(size_hint)` to multi-GB / abort") via `SANE_PREALLOC_CAP`, but the caps were never carried over to the hand-rolled decoder — and this is the decoder facing *untrusted* payloads (S-write decode of client-supplied id-msgpack; WAL replay).
- Failure scenario: a client submits a record whose first bytes are `0xDD FF FF FF FF` (Array32, 4294967295 elements) followed by junk; `msgpack_to_inner_zerocopy` panics with `capacity overflow` or the allocator aborts → whole-process crash from a 5-byte message (DoS).
- Suggested fix: clamp every wire-derived count through the existing cap before preallocating (export/share `SANE_PREALLOC_CAP`, e.g. `Vec::with_capacity(len.min(SANE_PREALLOC_CAP))`), letting the loop grow on demand. Apply identically in `merge_storage_bytes`. Add a regression test decoding a huge-header buffer.

### 2. Fail-open mode fallback in `ResourceMeta::from_record`

- File:line: `crates/shamir-types/src/access.rs:275-279`
- Severity: medium
- Issue: a stored `mode` value that fails to parse as `u16` (negative int, > 65535 — corruption, buggy writer, or hostile edit of a catalogue record) silently falls back to `Mode::OPEN` (`0o777`, everyone rwx). Every other malformed-field outcome in this function falls back to System/open too, which compounds it. Parse failure should never make a resource *more* accessible than it was.
- Failure scenario: catalogue record carries `mode: 999999999` after partial corruption; `from_record` maps it to world-writable instead of rejecting or tightening, and subsequent `permits()` checks grant broad access that the original object never had.
- Suggested fix: fail closed — `.unwrap_or(Mode::from_rwx(true, false, false))` (owner-only enforced default) or return an error/sentinel flag so the facade gate rejects unparsable security metadata.

### 3. Append-only interner accepts unlimited untrusted key names; FxHash rationale inverted

- File:line: `crates/shamir-types/src/core/interner/interner.rs:138-179` (`touch_ind`, monotonic, no eviction by design); reached for every client map key via `crates/shamir-types/src/codecs/interned/common.rs:13` (`intern_string_key`).
- Severity: medium
- Issue: CLAUDE.md pillar 4 mandates `THasher`(FxHash) with the stated rationale "we don't accept untrusted hash inputs here" — but the interner hashes fully attacker-chosen field-name strings on the write path, and every distinct name becomes a *permanent* entry (Arc\<str\> in the reverse spine + forward DashMap row; append-only, no clear/remove). Additionally FxHash collisions are trivially forgeable, so crafted names can pile onto one DashMap shard/bucket chain.
- Failure scenario: low-privilege session streams records containing millions of unique field names → unreclaimable server memory growth across restarts (names persist via WAL/storage); second-order effect: Fx-colliding names degrade lookup latency. Neither has a cap at this layer.
- Suggested fix: enforce a tunable ceiling (distinct-keys quota at the gate that calls `touch_ind`, surfaced as `CodecError`), and/or annotate the specific deviation from pillar 4's rationale where the keys are untrusted. If neither is wanted, document the invariant ("write principals are trusted to bound field-name cardinality") next to `Interner` so it's an explicit, owned risk.

### 4. `principal64_from_username` mints principal ids with seedless FxHasher

- File:line: `crates/shamir-types/src/access.rs:59-66`
- Severity: medium
- Issue: deterministic, non-cryptographic, seed-less hash of a *user-chosen* name projected into the owner/principal id space. FxHash(64-bit, no seed) collisions over usernames can be constructed in seconds offline, aliasing two names to one principal id. The doc correctly labels it interim with two live production call sites pending `PrincipalResolver` (#559), but nothing tracks that removal.
- Failure scenario: any permission/ownership decision keyed on `principal64_from_username(name)` can be steered by registering a name chosen to collide with a victim's name → shared owner-class bits or audit attribution confusion.
- Suggested fix: treat #559 as a blocking-security migration (deadline in the doc), and until then whitelist this bridge to exactly the two documented call sites via clippy-disallowed-methods or a `#[doc(hidden)]` + lint-bait wrapper.

### 5. Generic `bincode::from_bytes` helper lacks the depth caps its sibling decoders have

- File:line: `crates/shamir-types/src/codecs/basic/bincode.rs:51-56` (`from_bytes`, bincode 1.3.3); exposed crate-wide via `codecs::basic::{from_bytes, ..}` re-export
- Severity: low
- Issue: bincode 1.x applies no nesting/recursion limit, while both custom msgpack decoders (`MAX_MSGPACK_DEPTH = 128` in messagepack.rs and lens.rs) cap deliberately. Decoding deeply nested untrusted bytes into a recursive type (`Value<..>` recurses per container) can exhaust the stack (SIGSEGV/abort, unwinding unreliable under OOM-style exhaustion). Exposure depends entirely on call sites outside this crate — if this helper only ever sees engine-produced frames it is defense-in-depth debt, not a live hole.
- Failure scenario: future caller wires `from_bytes::<QueryValue>` onto a network-facing path; a megabyte of nested list markers ends the process.
- Suggested fix: either document the helper as trusted-input-only at the definition, or route untrusted callers through the depth-capped msgpack codec; long-term prefer migrating to bincode 2.x which supports configuration.

### 6. `SecretString`: constant-time-comparison footgun plus avoidable `unsafe`

- File:line: `crates/shamir-types/src/secret.rs:21` (`#[derive(Clone, PartialEq, Eq)]`), `secret.rs:67-75` (manual `Drop` with `unsafe { self.inner.as_bytes_mut() }`)
- Severity: low
- Issue: (a) derived `PartialEq` early-exits on first differing byte — appropriate nowhere for secret material, and the type's very name invites auth code to write `provided.reveal() == expected.reveal()` for password verification (timing oracle). Within this crate nothing compares, so it is a footprint hazard for consumers. (b) The single `unsafe` in the crate is sound today (zero bytes preserve UTF-8 validity; `&mut self` in `drop` is exclusive) but unnecessary: `zeroize::Zeroize` is already implemented for `String` (wipes bytes in place, keeps capacity semantics) — `self.inner.zeroize();` needs no `unsafe`.
- Failure scenario: downstream SCRAM/HMAC flow compares cleartexts via `==`; microsecond-scale timing differentials leak secret prefix bytes (mitigated in practice once upstream hashes, hence low).
- Suggested fix: replace the unsafe block with `Zeroize for String`; add a doc warning on the type ("never implement auth decisions by comparing `reveal()` output; compare digests/MACs"). Optionally add a test asserting the wiped state (e.g. take `into_inner()`-adjacent path or verify capacity-retention zeroing in debug via `String`'s vec).

### 7. Predictable RecordId random tail and embedded timestamps — invariant undocumented

- File:line: `crates/shamir-types/src/types/record_id.rs:48-52, 80-90`
- Severity: low
- Issue: the tail comes from a thread-local Xoshiro256++ seeded once from OsRng, and bytes [0..8] carry the wall-clock microsecond. For collision resistance the comment's claim ("CSPRNG is unnecessary") is fair. But ids are then *predictable* and *time-leaking*: xoshiro outputs are state-recoverable, so observing a handful of ids lets an attacker compute past/future ids. That is harmless **only** as long as record ids never act as unguessable handles, shareable capability links, pagination tokens tied to secrecy, or rate-limit keys. Nothing in the crate states that invariant, yet this is the canonical id minted for user rows.
- Failure scenario: a future endpoint authorizes "GET /records/{id}" by id possession alone (unguessability assumed, snowflake-style) → ids harvested from one response enumerate the table.
- Suggested fix: document in `RecordId`'s rustdoc that ids are NOT secret/capability material and authorization must never key on possession of an id; revisit `getrandom`-based tails if such a use ever lands.

### 8. Log-forging surface: raw resource names rendered into trace/denial lines

- File:line: `crates/shamir-types/src/access.rs:561-588` (`Display for ResourcePath`), `:622` (`AccessError` Display), `:657-660` (`trace_access` → `log::trace!`)
- Severity: nit
- Issue: every segment of `ResourcePath` (db/store/table/user/group/function names — user-chosen strings) is interpolated verbatim; newlines/ANSI escapes pass through, so denial messages and `shomer:` trace lines can contain forged entries. Actual validation presumably lives in the directory/catalogue layer upstream, but this crate controls the rendering.
- Failure scenario: attacker creates table `x"; DROP` or name containing `\n2026-08-14 INFO admin granted...` → downstream log aggregation shows fabricated audit lines.
- Suggested fix: escape control characters in these Display impls (`char::escape_debug` / strip `c.is_control()`) — cheap since this is not the hot path, and it makes observability lines tamper-evident at the source.

---

Notes for the reviewer: read-only static review per constraints — nothing was built, tested, or linted; severities are relative to this crate's trust boundaries as documented in-source. Findings 3 and 5 depend partly on out-of-crate call sites (engine/server gates feeding these APIs); they are framed conditional on those boundaries holding as the doc comments claim.

</details>
