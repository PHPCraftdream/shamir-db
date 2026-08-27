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
