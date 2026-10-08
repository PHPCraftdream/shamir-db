<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-tx — security-crypto revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Journal silent omissions and variable-length-key vacuum isolation remain conditional integrity concerns. Two panic exploits are refuted by current reachability; shared-journal disclosure requires violating supported per-repo wiring.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 9 | 3 | 0 | 0 | 3 | 1 | 2 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Corrupt changefeed journal entries are silently skipped, contradicting CF-1

Status: `confirmed-open`. Current risk: `medium`.

A decode failure only warns and does not set a resync signal. Threat sources are corrupt/incompatible stored payloads or custom stores, not a demonstrated unauthenticated remote injection path.

Evidence: [crates/shamir-tx/src/changefeed.rs:409](../../../../../crates/shamir-tx/src/changefeed.rs#L409); [crates/shamir-tx/src/changefeed.rs:411](../../../../../crates/shamir-tx/src/changefeed.rs#L411); [crates/shamir-tx/src/changefeed.rs:416](../../../../../crates/shamir-tx/src/changefeed.rs#L416).

Grouping/duplicate: `api-wire-protocol.md#1`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — LayeredInterner::touch_sync panics on a failure its sibling merge path treats as recoverable

Status: `refuted`. Current risk: —.

Every current touch_ind return is Ok; there is no field-name-conditioned Err to trigger this expect. The proposed remote input DoS lacks its required failure mechanism.

Evidence: [crates/shamir-types/src/core/interner/interner.rs:146](../../../../../crates/shamir-types/src/core/interner/interner.rs#L146); [crates/shamir-types/src/core/interner/interner.rs:166](../../../../../crates/shamir-types/src/core/interner/interner.rs#L166); [crates/shamir-types/src/core/interner/interner.rs:176](../../../../../crates/shamir-types/src/core/interner/interner.rs#L176); [crates/shamir-tx/src/layered_interner.rs:84](../../../../../crates/shamir-tx/src/layered_interner.rs#L84).

<a id="review-3"></a>

### Claim 3 — Separator invariant is doc-only, probability wrong, and vacuum scan uses the wrong cur_v for prefix-matched keys

Status: `confirmed-open`. Current risk: `low`.

vacuum decodes but discards orig, then applies the queried key's current-version guard to every matching row. Arbitrary public variable-length keys can cross namespaces; fixed-width production data keys avoid this case.

Evidence: [crates/shamir-tx/src/version_codec.rs:20](../../../../../crates/shamir-tx/src/version_codec.rs#L20); [crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:175](../../../../../crates/shamir-tx/src/mvcc_store/mvcc_gc.rs#L175); [crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:185](../../../../../crates/shamir-tx/src/mvcc_store/mvcc_gc.rs#L185); [crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:214](../../../../../crates/shamir-tx/src/mvcc_store/mvcc_gc.rs#L214); [crates/shamir-types/src/types/record_id.rs:42](../../../../../crates/shamir-types/src/types/record_id.rs#L42).

<a id="review-4"></a>

### Claim 4 — StagedRow::as_inner panics on malformed staged bytes; invariant is a pub API doc

Status: `refuted`. Current risk: —.

No reachable public accessor exposes StagedRow and no in-tree call invokes as_inner. Staging reads and remapping use bytes directly; malformed remap data produces an error, not this panic.

Evidence: [crates/shamir-tx/src/staging_store.rs:32](../../../../../crates/shamir-tx/src/staging_store.rs#L32); [crates/shamir-tx/src/staging_store.rs:81](../../../../../crates/shamir-tx/src/staging_store.rs#L81); [crates/shamir-tx/src/staging_store.rs:141](../../../../../crates/shamir-tx/src/staging_store.rs#L141); [crates/shamir-tx/src/staging_store.rs:314](../../../../../crates/shamir-tx/src/staging_store.rs#L314); [crates/shamir-tx/src/id_remap.rs:77](../../../../../crates/shamir-tx/src/id_remap.rs#L77).

<a id="review-5"></a>

### Claim 5 — Changefeed journal keys carry no repo namespace — shared stores cross-wire streams

Status: `not-applicable`. Current risk: —.

Aliasing is possible if callers deliberately share one store, but the trait explicitly requires a per-repo store and production opens each repository's __changelog__ namespace. No supported cross-tenant disclosure path was found.

Evidence: [crates/shamir-tx/src/changefeed.rs:147](../../../../../crates/shamir-tx/src/changefeed.rs#L147); [crates/shamir-tx/src/changefeed.rs:160](../../../../../crates/shamir-tx/src/changefeed.rs#L160); [crates/shamir-tx/src/changefeed.rs:428](../../../../../crates/shamir-tx/src/changefeed.rs#L428); [crates/shamir-engine/src/repo/repo_instance.rs:1207](../../../../../crates/shamir-engine/src/repo/repo_instance.rs#L1207).

<a id="review-6"></a>

### Claim 6 — Non-keyed THasher on engine-supplied keys — no-untrusted-input premise is upstream

Status: `confirmed-open`. Current risk: `nit`.

Hasher choice and unvalidated public key APIs remain. The stronger premise is not established: layered interner maps also hash caller field-name Strings. Exploitable collision sets and claimed quadratic cost remain unverified.

Evidence: [crates/shamir-tx/src/mvcc_store/mod.rs:138](../../../../../crates/shamir-tx/src/mvcc_store/mod.rs#L138); [crates/shamir-tx/src/layered_interner.rs:95](../../../../../crates/shamir-tx/src/layered_interner.rs#L95); [crates/shamir-tx/src/tx_context.rs:538](../../../../../crates/shamir-tx/src/tx_context.rs#L538); [CLAUDE.md:366](../../../../../CLAUDE.md#L366).

<a id="review-scope-notes-crate-crypto-and-injection-boundary"></a>

### Claim Scope notes: crate crypto and injection boundary — No direct unsafe, cryptography, secret comparison or assembled command/query surface

Status: `not-applicable`. Current risk: —.

The claimed absence is supported for shamir-tx's implementation and direct dependency surface; it is not a transitive dependency or whole-workspace security certification.

Evidence: [crates/shamir-tx/Cargo.toml:9](../../../../../crates/shamir-tx/Cargo.toml#L9); [crates/shamir-tx/src/version_codec.rs:42](../../../../../crates/shamir-tx/src/version_codec.rs#L42); [crates/shamir-tx/src/changefeed.rs:152](../../../../../crates/shamir-tx/src/changefeed.rs#L152).

<a id="review-summary-workspace-wide-zero-unsafe"></a>

### Claim Summary: workspace-wide zero unsafe — Workspace-wide sweep found zero unsafe blocks

Status: `refuted`. Current risk: —.

Neighbouring workspace crates contain explicit unsafe blocks. Narrow the assurance to shamir-tx rather than repeating the workspace-wide assertion.

Evidence: [crates/shamir-numa/src/linux.rs:109](../../../../../crates/shamir-numa/src/linux.rs#L109); [crates/shamir-index/src/vector/simd.rs:57](../../../../../crates/shamir-index/src/vector/simd.rs#L57).

<a id="review-summary-checksummed-authenticated-inputs"></a>

### Claim Summary: checksummed/authenticated inputs — Stored inputs are checksummed and the crate is behind server authentication

Status: `unverified`. Current risk: —.

MvccStore and ChangelogStore accept arbitrary Store implementations and public callers. The inspected interfaces do not enforce authentication or checksums; universal protection was not established.

Evidence: [crates/shamir-tx/src/mvcc_store/mod.rs:126](../../../../../crates/shamir-tx/src/mvcc_store/mod.rs#L126); [crates/shamir-tx/src/changefeed.rs:152](../../../../../crates/shamir-tx/src/changefeed.rs#L152); [crates/shamir-tx/src/changefeed.rs:232](../../../../../crates/shamir-tx/src/changefeed.rs#L232).

## Corrections and qualified non-findings

- For uniformly random bytes, the tested-position separator probability is 1/256, not 2^-72; current RecordIds are timestamp-prefixed with a random tail, not uniformly random 16-byte strings.
- The suffix decoder itself remains bijective for arbitrary keys. The integrity issue is cross-key prefix collection; an orig == requested_key filter directly addresses vacuum isolation.
- The corrupt entry's journal key is not returned by ChangelogStore::range_from, so the proposed key-derived marker is unavailable without changing the seam.
- Do not describe the interner or unused StagedRow expect as current remotely reachable process-wide DoS.
- Shared-store aliasing is a caller-contract violation under current per-repo production wiring, not an established tenant-boundary bypass.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-tx -- Security & crypto boundary

## Summary

`shamir-tx` contains **no authentication, HMAC/SCRAM/TLS, or cryptographic code** — `Cargo.toml` pulls no crypto dependency, the SCRAM/TLS surface lives in `shamir-connect`/`shamir-server`, and a workspace-wide sweep found **zero `unsafe` blocks**, no secret material, and no string-assembled command/query surface (keys are binary throughout, so there is no classic injection vector). The theme therefore reduces to three residual surfaces: panic-on-input robustness at `pub` APIs that accept caller-supplied bytes/strings, untrusted/corrupt durable-input handling (history-log and changefeed-journal reads), and the key-encoding boundary (`version_codec`) whose safety rests on an unenforced caller convention. Findings below are ranked; none is critical/high — the crate sits behind the server's auth boundary and its stored inputs are checksummed at the storage layer.

## Findings

### 1. Corrupt changefeed journal entries are silently skipped, contradicting CF-1's own "silent omission is not acceptable" contract
**File:** `crates/shamir-tx/src/changefeed.rs:407-421` (skip loop), `:180-204` (`first_gap_version` / `JournalRead`)
**Severity:** medium
**Issue:** `RepoChangefeed::read_from` decodes each raw journal record with `rmp_serde::from_slice::<ChangelogEvent>`; on decode failure it logs a warning and skips the entry. `gap_at` is populated **only** from `first_gap_version`, which tracks channel-overflow drops at emit time (`journal_send`, `:306-341`). A byte-level corruption in the journal store therefore produces exactly the failure mode the file's own comment rules out (`:414-415`: "Conservative over-signal is acceptable; silent omission is not") — a hole in the event stream with **no resync signal** to the consumer.
**Failure scenario:** A replication/subscription consumer resumes via `read_from(V + 1, n)`. N records after `V` are corrupt on disk (partial write, bit rot beyond the storage layer's checksum coverage, or a hand-edited store). The consumer receives `events` with entries missing and `gap_at: None`, advances its cursor past the hole, and permanently diverges from the source without ever being told to snapshot-resync — the CF-1 mechanism exists but does not cover this hole class.
**Suggested fix:** On a decode failure in `read_from`, set `gap_at = Some(corrupt_key_version)` (decode the 8-byte key to a version; it is always available regardless of payload corruption) so consumers take the documented full-resync path. Alternatively track a `first_decode_failure_version` watermark alongside `first_gap_version` with the same min-CAS discipline.

### 2. `LayeredInterner::touch_sync` panics on the same failure its sibling merge path treats as a recoverable error
**File:** `crates/shamir-tx/src/layered_interner.rs:82-86` (Direct branch) vs `:256-263` (`commit_interner_overlay`)
**Severity:** medium
**Issue:** Both call sites invoke `Interner::touch_ind` on field names — user-supplied data that reaches this crate from the engine's write path. `commit_interner_overlay` maps the `Err` to `DbError::Codec` and propagates with `?`; `touch_sync`'s `Direct` (non-tx write) branch instead does `.expect("Interner::touch_ind is infallible for valid input")` — a message that itself concedes invalid input exists. If any input characteristic (key length bound, interner capacity, encoding constraint) can make `touch_ind` fail, the **non-tx write path converts a client-triggerable error into a process-wide panic**, violating CLAUDE.md's error-handling rule ("avoid `panic!` outside invariant violations") — this is not an invariant violation, it is an input-conditioned failure the sibling path already models as `Err`.
**Failure scenario:** A client write using a field name that trips `touch_ind`'s failure mode (whatever "invalid input" covers — e.g. pathological length) crashes the whole server process on the non-tx path, where the tx path would have returned a clean `Codec` error. Remote DoS by input, contingent on `Interner`'s out-of-crate failure modes.
**Suggested fix:** Return `Result<u64, DbError>` from `touch_sync` (or `Option`/`DbResult`), mirroring `commit_interner_overlay`. If the interner truly cannot fail on any reachable input, delete the `expect`'s "for valid input" hedge and assert unconditionality — the current half-infallible phrasing is the hazard.

### 3. `version_codec`'s separator invariant is a doc comment, not a check — and the doc's probability claim is wrong; `vacuum_key`'s scan path guards prefix-matched entries with the wrong `cur_v`
**File:** `crates/shamir-tx/src/version_codec.rs:10-30` (invariant + "negligible" claim), `crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:162-199` (prefix scan), `:185` + `:212-214` (single-`cur_v` SACRED check)
**Severity:** low
**Issue:** Three stacked weaknesses at the MVCC key-encoding boundary:
(a) The invariant ("original key must not contain `0xFF` + 8 trailing bytes") is enforced nowhere — `encode_version_key` is `pub` and accepts any `&[u8]`.
(b) The doc claims a uniformly random 16-byte `RecordId` has "negligible" chance of a tail `0xFF + 8 bytes`; the actual probability that byte `len-9` is `0xFF` is **1/256**, not negligible — only the further requirement of a live prefix key keeps it harmless.
(c) `vacuum_key`'s scan path prefix-scans `key || 0xFF` and applies **one** `cur_v = current_version(key)` SACRED check to every collected entry. Under a keyspace where one logical key equals another key plus `0xFF` + 8 bytes (i.e. the documented invariant is ever violated), entries belonging to the *longer* key fall inside the shorter key's prefix scan and are reclaimed against the *shorter* key's current-version guard — the C1 protection does not apply to them. `gc_below` (`mvcc_gc.rs:329`) and `purge_below_ts` (`:426`) correctly re-derive `cur_v` per decoded original key; `vacuum_key` is the outlier.
**Failure scenario:** A future caller stores variable-length user keys (anything not a fixed 16-byte `RecordId`/typed `SysKey`) in an MVCC-backed store where key `A` and key `A||0xFF||W` coexist. A retention-triggered `vacuum_key(A, …)` scan deletes `A||0xFF||W`'s live current version — silent data loss attributed to GC. Not reachable with today's fixed-width keyspaces; this is defense-in-depth against the crate's own documented invariant being violated by a downstream caller.
**Suggested fix:** (1) Add a `debug_assert!` in `encode_version_key` that `key[len-9] != VERSION_SEP` (guarded for `len >= 9`) so invariant violations surface in tests. (2) Correct the doc's probability arithmetic. (3) In `vacuum_key`'s scan path, group collected entries by decoded original key and check each against `current_version(orig)` exactly as `gc_below` does.

### 4. `StagedRow::as_inner` panics on malformed staged bytes; the invariant is a doc comment on a `pub` API
**File:** `crates/shamir-tx/src/staging_store.rs:31-49` (`StagedRow::as_inner` `.expect`), `:135-137` (`pub fn set` accepting arbitrary `Bytes`)
**Severity:** low
**Issue:** `StagingStore::set` is public and accepts any `Bytes`; `StagedRow` documents "always holds already-encoded msgpack `Bytes`" but nothing enforces it, and `as_inner()` does `InnerValue::from_bytes(...).expect("StagedRow always holds valid msgpack")`. The crate's own remap path (`id_remap.rs:77-80`) handles the *identical* decode failure by returning `Err` — so the same malformed payload either panics or errors depending on which API touches it first. CLAUDE.md permits `panic!` for invariant violations; the point is that this invariant is unenforced at a public boundary, and a later read-your-own-write (cold path) is where the panic detonates, far from the buggy staging call.
**Failure scenario:** An engine caller stages bytes not produced by `query_value_to_storage_bytes` (a future write path, a test fixture, a WAL-replay shortcut). Every staged read/commit-remap touching that row panics the process. Requires a caller bug, not attacker input — hence low.
**Suggested fix:** Either make `as_inner` infallible by construction (store `InnerValue` alongside the bytes, or validate at `StagingStore::set`), or return `Result`/`Option` and let callers propagate — matching `remap_inner_value_bytes`' existing behavior for the same decode.

### 5. Changefeed journal keys carry no repo namespace — shared stores silently cross-wire event streams
**File:** `crates/shamir-tx/src/changefeed.rs:426-430` (`version_key`), `:232-257` (`RepoChangefeed::new(store)`)
**Severity:** low
**Issue:** Journal records are keyed by the bare 8-byte big-endian `commit_version` — no repo id, no table token. The "one store per repo" assumption exists only in the wiring; `RepoChangefeed::new` accepts any `Arc<dyn ChangelogStore>` and nothing detects or rejects sharing. Two repos' feeds pointed at one store overwrite each other's entries (same key space, `put` is an upsert) with no error and no gap marker — a silent integrity/aliasing failure at the trust boundary, not a crash.
**Failure scenario:** A deployment or test harness reuses one changelog store for two repos. Repo B's event at version V overwrites repo A's event at version V; A's subscribers receive B's records (cross-tenant record disclosure if the repos have different readers) and neither feed reports a gap.
**Suggested fix:** Include a per-repo discriminator in the journal key (e.g. hash of the repo name as a key prefix), or at minimum `debug_assert!`/document the exclusive-store contract at `RepoChangefeed::new`.

### 6. Non-keyed `THasher` on engine-supplied keys — the "no untrusted hash inputs" premise is enforced only upstream
**File:** `crates/shamir-tx/src/tx_context.rs:233` (`read_set`), `:240` (`cas_set`), `crates/shamir-tx/src/mvcc_store/mod.rs:138` (`cells`), `:143` (`locks`)
**Severity:** nit
**Issue:** CLAUDE.md pillar 4 deliberately trades SipHash's DOS protection for Fx speed "and we don't accept untrusted hash inputs here". Within shamir-tx that premise holds only because upstream callers constrain record keys to producer-generated random 16-byte ids — the crate's own `pub` APIs (`record_read_shared`, `record_cas`, `MvccStore` keyed probes) accept arbitrary `Bytes`/`RecordKey` with no validation. A future engine feature letting clients choose their own record ids converts client-chosen keys into Fx-hash collisions (O(n²) probe chains in `scc` buckets) — a CPU-DoS surface. No action needed now; recorded so the premise is known to live outside this crate.
**Suggested fix:** None required while keys are system-generated. If client-chosen record ids ever land, the hash-keyed tx structures here need a keyed hasher or a key-shape validation at the boundary.

---

**Scope notes (things checked and found clean):** zero `unsafe` blocks in the crate (grep-verified; the only "unsafe" hits are prose in doc comments); no crypto/auth dependencies in `Cargo.toml`; no secret values, timing-sensitive comparisons, or constant-time requirements anywhere (nothing to side-channel); no string-assembled queries/commands — all keys are binary slices passed to typed `Store` traits, so there is no injection grammar; WAL entries are encoded/decoded via `shamir-wal`'s typed `WalEntryV2` (out of scope here); the two `.lock().unwrap()` sites (`repo_tx_gate.rs:753/761`) sit on the `pending_commits` field explicitly sanctioned as dead scaffolding in CLAUDE.md and are not re-litigated. Test coverage for the codec boundary is strong (round-trip + separator-rejection + proptest in `tests/version_codec_tests.rs`); the changefeed tests cover projection/live-push/journal but have **no corrupt-entry case**, which is why finding 1's behavior went unpin­ned.

</details>
