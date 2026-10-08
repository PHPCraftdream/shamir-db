<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-wal — security-crypto revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Recovery robustness and local-directory trust documentation remain concerns. No remotely supplied WAL input boundary was established. The small-file speculative-exabyte claim lacks complete pinned dependency proof and has substantial counter-evidence.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 11 | 6 | 0 | 1 | 1 | 1 | 2 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — repair_torn_tail silently and irreversibly truncates on a mid-file CRC mismatch in the ACTIVE segment

Status: `confirmed-open`. Current risk: `medium`.

A complete bad-CRC active frame triggers physical truncation of its entire suffix with only a warning. Trigger requires on-disk corruption or filesystem write access, not ordinary network input.

Evidence: [crates/shamir-wal/src/wal_segment.rs:391](../../../../../crates/shamir-wal/src/wal_segment.rs#L391); [crates/shamir-wal/src/wal_segment.rs:404](../../../../../crates/shamir-wal/src/wal_segment.rs#L404); [crates/shamir-wal/src/segment_set.rs:173](../../../../../crates/shamir-wal/src/segment_set.rs#L173).

Grouping/duplicate: `correctness-tdd.md#4`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — bincode 1.x decodes recovery data with no allocation bounds — small crafted/corrupt frame can OOM the process

Status: `unverified`. Current risk: `low` (provisional; not a confirmed defect).

The exact tiny-frame exabyte scenario is not established. Pinned bincode uses a bounds-checked slice reader, and pinned serde caps Vec preallocation at 1 MiB, contradicting the stated ops-length mechanism. Pinned serde_bytes source was unavailable. Whole-file uncapped reads are separately confirmed.

Evidence: [Cargo.lock:413](../../../../../Cargo.lock#L413); [Cargo.lock:3204](../../../../../Cargo.lock#L3204); [Cargo.lock:3214](../../../../../Cargo.lock#L3214); [crates/shamir-wal/src/wal_entry_v2.rs:240](../../../../../crates/shamir-wal/src/wal_entry_v2.rs#L240); [crates/shamir-wal/src/wal_entry_v2.rs:251](../../../../../crates/shamir-wal/src/wal_entry_v2.rs#L251).

<a id="review-2-ops-length-allocation"></a>

### Claim 2/ops-length-allocation — Huge ops length causes immediate multi-exabyte Vec preallocation

Status: `refuted`. Current risk: —.

The inspected pinned serde Vec visitor uses cautious size hints capped at 1 MiB, then decodes elements incrementally. Pinned bincode's slice byte-buffer reader checks available input before copying; the report conflates slice and I/O reader behavior.

Evidence: [Cargo.lock:413](../../../../../Cargo.lock#L413); [Cargo.lock:3204](../../../../../Cargo.lock#L3204); [Cargo.lock:3224](../../../../../Cargo.lock#L3224); [crates/shamir-wal/src/wal_entry_v2.rs:153](../../../../../crates/shamir-wal/src/wal_entry_v2.rs#L153); [crates/shamir-wal/src/wal_entry_v2.rs:251](../../../../../crates/shamir-wal/src/wal_entry_v2.rs#L251).

<a id="review-2-unbounded-file-buffering"></a>

### Claim 2/unbounded-file-buffering — Recovery reads whole segment files without a size cap

Status: `confirmed-open`. Current risk: `low`.

Repair and replay read entire files into growing Vecs; sidecar reading is also uncapped. Rotation is a post-batch threshold, not a hard file-size bound. Large input can exhaust memory, conditional on corpus size or local write access.

Evidence: [crates/shamir-wal/src/wal_segment.rs:370](../../../../../crates/shamir-wal/src/wal_segment.rs#L370); [crates/shamir-wal/src/wal_segment.rs:527](../../../../../crates/shamir-wal/src/wal_segment.rs#L527); [crates/shamir-wal/src/segment_meta.rs:130](../../../../../crates/shamir-wal/src/segment_meta.rs#L130); [crates/shamir-wal/src/segment_set.rs:250](../../../../../crates/shamir-wal/src/segment_set.rs#L250).

<a id="review-3"></a>

### Claim 3 — Unauthenticated WAL replay is an injection surface; the data directory is trusted assumption is implicit and undocumented

Status: `confirmed-open`. Current risk: `low`.

WAL frames and sidecars use unkeyed CRC, with no repository authentication. A principal already able to modify the WAL corpus can forge entries or sidecar maxima. This is an undocumented filesystem trust boundary, not demonstrated remote privilege escalation; CounterDelta is currently ignored on recovery.

Evidence: [crates/shamir-wal/src/wal_segment.rs:230](../../../../../crates/shamir-wal/src/wal_segment.rs#L230); [crates/shamir-wal/src/segment_meta.rs:149](../../../../../crates/shamir-wal/src/segment_meta.rs#L149); [crates/shamir-wal/src/segment_set.rs:145](../../../../../crates/shamir-wal/src/segment_set.rs#L145); [crates/shamir-engine/src/tx/recovery.rs:58](../../../../../crates/shamir-engine/src/tx/recovery.rs#L58); [crates/shamir-engine/src/tx/recovery.rs:120](../../../../../crates/shamir-engine/src/tx/recovery.rs#L120).

<a id="review-4"></a>

### Claim 4 — Frame-length arithmetic can wrap on 32-bit targets → panic on crafted 4-byte header

Status: `confirmed-open`. Current risk: `low`.

Unchecked usize addition can overflow on 32-bit. A four-byte maximum-length header can panic with overflow checks; wrapping release mode needs enough bytes to pass the wrapped frame-end guard before the invalid slice. A four-byte-only release file is insufficient.

Evidence: [crates/shamir-wal/src/wal_segment.rs:377](../../../../../crates/shamir-wal/src/wal_segment.rs#L377); [crates/shamir-wal/src/wal_segment.rs:380](../../../../../crates/shamir-wal/src/wal_segment.rs#L380); [crates/shamir-wal/src/wal_segment.rs:535](../../../../../crates/shamir-wal/src/wal_segment.rs#L535); [crates/shamir-wal/src/wal_segment.rs:539](../../../../../crates/shamir-wal/src/wal_segment.rs#L539).

<a id="review-5"></a>

### Claim 5 — A single CRC-valid-but-undecodable frame aborts the entire recovery (version skew == corruption)

Status: `confirmed-open`. Current risk: `nit`.

Decode errors still abort replay as Internal. Unsupported envelope versions already have distinct text; same-version body drift does not. History 5575ad59 added commit_version without a version bump, whereas f6ebd0ef correctly bumped for interner_delta. Fail-closed recovery itself is not a vulnerability.

Evidence: [crates/shamir-wal/src/wal_segment.rs:572](../../../../../crates/shamir-wal/src/wal_segment.rs#L572); [crates/shamir-wal/src/wal_entry_v2.rs:165](../../../../../crates/shamir-wal/src/wal_entry_v2.rs#L165); [crates/shamir-wal/src/wal_entry_v2.rs:251](../../../../../crates/shamir-wal/src/wal_entry_v2.rs#L251); [crates/shamir-wal/src/wal_entry_v2.rs:253](../../../../../crates/shamir-wal/src/wal_entry_v2.rs#L253).

<a id="review-6"></a>

### Claim 6 — Error strings embed absolute filesystem paths

Status: `confirmed-open`. Current risk: `nit`.

Errors preserve supplied PathBuf values, including absolute paths when configured. Ordinary grouped append errors are replaced by generic text; generic server error serialization exists, but a specific path-bearing WAL error's remote reach remains unverified.

Evidence: [crates/shamir-wal/src/wal_segment.rs:155](../../../../../crates/shamir-wal/src/wal_segment.rs#L155); [crates/shamir-wal/src/wal_segment.rs:519](../../../../../crates/shamir-wal/src/wal_segment.rs#L519); [crates/shamir-wal/src/segment_set.rs:512](../../../../../crates/shamir-wal/src/segment_set.rs#L512); [crates/shamir-wal/src/wal_group_commit.rs:201](../../../../../crates/shamir-wal/src/wal_group_commit.rs#L201); [crates/shamir-server/src/db_handler/handler.rs:611](../../../../../crates/shamir-server/src/db_handler/handler.rs#L611).

<a id="review-positive-sidecar-and-envelope-bounds"></a>

### Claim Positive/sidecar-and-envelope-bounds — Sidecar, active-key, and entry-envelope rejection guards are defensive

Status: `not-applicable`. Current risk: —.

Exact sidecar length precedes indexed extraction; key parsing checks length and prefix; entry decode checks five-byte envelope length. Registered tests cover these rejection branches. This does not imply bounded file reading.

Evidence: [crates/shamir-wal/src/segment_meta.rs:138](../../../../../crates/shamir-wal/src/segment_meta.rs#L138); [crates/shamir-wal/src/segment_meta.rs:175](../../../../../crates/shamir-wal/src/segment_meta.rs#L175); [crates/shamir-wal/src/active_key.rs:49](../../../../../crates/shamir-wal/src/active_key.rs#L49); [crates/shamir-wal/src/tests/wal_entry_v2_tests.rs:69](../../../../../crates/shamir-wal/src/tests/wal_entry_v2_tests.rs#L69).

<a id="review-positive-sealed-and-startup-hardening"></a>

### Claim Positive/sealed-and-startup-hardening — Sealed CRC and startup PermissionDenied distinctions are tested

Status: `partially-fixed`. Current risk: `medium`.

Both distinctions exist in source. The sealed CRC test genuinely corrupts bytes and asserts Err; the startup PermissionDenied test never induces denial and cannot protect the flag selection.

Evidence: [crates/shamir-wal/src/wal_segment.rs:512](../../../../../crates/shamir-wal/src/wal_segment.rs#L512); [crates/shamir-wal/src/wal_segment.rs:546](../../../../../crates/shamir-wal/src/wal_segment.rs#L546); [crates/shamir-wal/src/tests/wal_segment_tests.rs:156](../../../../../crates/shamir-wal/src/tests/wal_segment_tests.rs#L156); [crates/shamir-wal/src/tests/wal_segment_tests.rs:218](../../../../../crates/shamir-wal/src/tests/wal_segment_tests.rs#L218).

Grouping/duplicate: `correctness-tdd.md#3`. This row is not another independent defect.

<a id="review-positive-no-crypto-or-unsafe-surface"></a>

### Claim Positive/no-crypto-or-unsafe-surface — No authentication, secret comparison, unsafe block, or interpreter injection surface

Status: `not-applicable`. Current risk: —.

The inspected crate has no such production code or direct crypto dependency. This supports absence of a local secret-comparison surface, not an unrestricted claim that the component has no possible timing leakage.

Evidence: [crates/shamir-wal/Cargo.toml:9](../../../../../crates/shamir-wal/Cargo.toml#L9); [crates/shamir-wal/src/lib.rs:46](../../../../../crates/shamir-wal/src/lib.rs#L46).

## Corrections and qualified non-findings

- Separate local filesystem tampering from remotely reachable security defects.
- Remove the unsupported generalization that all bincode collection lengths cause speculative huge allocations; retain the independently proven uncapped-file issue.
- Do not impose max_bytes as a strict file cap without allowing legitimate post-batch threshold overshoot.
- CRC mismatch can result from a torn/sub-frame write as well as bit-rot or tampering; the code cannot conclusively identify its origin.
- Returning an error without repairing a corrupt active segment is compatible with safety if appends remain gated; preserving corruption does not require continuing to append.
- WASM-first describes hosted user logic here, not proof of a supported wasm32 WAL host.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-wal -- Security & crypto boundary

## Summary

`shamir-wal` has **no authentication, HMAC/SCRAM/TLS, or secret-handling surface** (grep confirms: zero crypto deps in `Cargo.toml`, zero `unsafe` blocks, no secret comparisons — hence no timing side-channels). Its entire security-relevant surface is the **recovery-time untrusted-input boundary**: replay of on-disk segment files (`WalSegment::replay_inner` / `repair_torn_tail` frame walkers), envelope decode (`WalEntryV2::decode`, bincode 1.x), sidecar decode (`segment_meta`), and key parse (`WalActiveKey::parse`). Integrity rests solely on **unkeyed CRC32** — adequate against torn writes/bit-rot, transparent to any adversary who can write to the data directory. Parsing is mostly defensive (length checks before slicing, magic/version rejection with direct unit tests), with two robustness gaps: silent truncation of mid-file corruption on the active segment, and length-arithmetic/allocator behavior that assumes a trusted data directory — an assumption the crate never states.

## Findings

### 1. `repair_torn_tail` silently and irreversibly truncates on a mid-file CRC mismatch in the ACTIVE segment
- **File:line:** `crates/shamir-wal/src/wal_segment.rs:391-393` (break on CRC mismatch), `:402-411` (`set_len(pos)` truncation); the read-side twin at `:546-570`
- **Severity:** medium
- **Issue:** The self-heal loop treats a *complete-frame CRC mismatch* identically to a *torn trailing frame*: break + truncate the file back to the last good boundary (`set_len(pos)`), with only a `log::warn`. But a full frame with a bad CRC is bit-rot (or tampering), not a crash tail — this is exactly the distinction the crate itself draws for sealed segments (audit §1.8, `replay_sealed` makes it a loud operator-facing `Err`, with a dedicated regression test `sealed_segment_crc_failure_is_loud_error`). The active segment's open-time path instead **destroys data silently**: every entry after the corrupt byte — including `Synced`-acked, power-loss-durable commits — is deleted from disk with no error surfaced to `SegmentSet::open`'s caller.
- **Failure scenario:** Bit-rot (failing disk sector) flips one byte in frame 3 of a 500-frame active segment. On next open, `repair_torn_tail` truncates 497 durable frames; the database opens "successfully" and the data is gone permanently — no `Err`, no operator decision, contradicting the crate's own §1.8 philosophy that corrupt-but-complete frames are corruption, not crash tails.
- **Suggested fix:** In `repair_torn_tail`, only truncate when the damage is genuinely a TAIL condition (`frame_end > buf.len()` — bytes run out mid-frame). A complete frame whose CRC fails should return a loud error (mirroring `replay_sealed`) or, at minimum, only stop replay without mutating the file — leaving the intact prefix replayable and the corruption operator-visible. Add a regression test for the mid-file-CRC-mismatch case (current tests cover only a genuine partial tail and the clean no-op: `wal_segment_poison_tests.rs:48,124`).

### 2. bincode 1.x decodes recovery data with no allocation bounds — small crafted/corrupt frame can OOM the process
- **File:line:** `crates/shamir-wal/src/wal_entry_v2.rs:240,251` (`bincode::deserialize`); `crates/shamir-wal/Cargo.toml:12` (`bincode = "1.3.3"`); amplified by unbounded `read_to_end` at `wal_segment.rs:527-529,370-374`
- **Severity:** low
- **Issue:** bincode 1.x is documented as targeting *trusted* input: collection/`ByteBuf` length prefixes are read as raw `u64`s and used to size allocations before the reader validates that many bytes exist. A frame passes the CRC gate and reaches `WalEntryV2::decode` with an inner length field claiming ~2^60 elements/bytes → huge speculative allocation → abort/OOM at recovery from a ~40-byte file. Separately, `replay_inner`/`repair_torn_tail` `read_to_end` the whole segment file into RAM with no size cap — a legitimately-bounded segment is the rotation threshold (`max_bytes`), but nothing at open validates that a `.wal` file on disk respects it.
- **Failure scenario:** Corrupt or planted WAL file with a forged (CRC32 is unkeyed — trivially recomputed) bincode body containing `ops`/`body` with a `u64` length of `0x0100_0000_0000_0000`. Recovery attempts a multi-exabyte `Vec` allocation and the process aborts — a DoS on database open. The same holds for an accidentally giant segment (e.g. after a rotation bug) that `read_to_end` swallows whole.
- **Suggested fix:** Bound the trust: (a) cap per-frame `len` against a max-entry-size tunable before slicing/decoding; (b) prefer a deserializer with a recursion/byte limit on this boundary (bincode 2 `with_limit`, or postcard) — the envelope already carries a version byte so a bounded decode path can be gated on it; (c) check file `metadata().len()` against a segment-size bound before `read_to_end`. A small `cargo-fuzz` target over `WalEntryV2::decode` + the frame walker would lock this boundary in cheaply.

### 3. Unauthenticated WAL replay is an injection surface; the "data directory is trusted" assumption is implicit and undocumented
- **File:line:** `crates/shamir-wal/src/segment_set.rs:74-77,100-104,128-129` (`parse_seg_seq` accepts any numeric `.wal`; highest seq becomes the append tail); `crates/shamir-wal/src/wal_entry_v2.rs:49-110` (replayed `Put`/`Delete`/`InternerOverlayMerge`/`CounterDelta` ops); `crates/shamir-wal/src/segment_set.rs:470-477` (truncation trusts sidecar/filename-derived `max_version`)
- **Severity:** low
- **Issue:** Recovery replays whatever `NNNNNNNN.wal` files exist in the directory — there is no manifest, seq-contiguity check, or per-entry MAC binding entries to the repo. `WalOpV2::Put.body` is *arbitrary bytes replayed verbatim into data_store*; a planted or edited segment (payload + recomputed CRC32 — the sole integrity check, and unkeyed) silently injects, deletes, or rewrites records, and a forged `.meta` sidecar can drive `truncate_below` to delete segments whose data is *not* yet durable in history. This is sound **iff** the data directory is writable only by the DB's own OS user — but that trust boundary is nowhere stated in the crate or module docs, and CLAUDE.md's Fx-hash pillar already leans on the same "we don't accept untrusted inputs here" assumption without connecting it to the WAL.
- **Failure scenario:** Multi-user host or shared/network-mounted WAL directory (NFS, container volume shared between services): any principal with write access to the directory owns the database's post-crash state — silent record injection/erasure with zero tamper evidence.
- **Suggested fix:** Document the threat boundary explicitly in `lib.rs` / `segment_set.rs` ("WAL files are trusted input; directory permissions are the security boundary"). If WAL storage is ever exposed to weaker trust (shared volumes, backups restored from untrusted sources), add a keyed checksum (e.g. HMAC-SHA256 over each frame, keyed by a per-repo secret held outside the WAL dir) — naturally done at the already-planned WAL format-version bump.

### 4. Frame-length arithmetic can wrap on 32-bit targets → panic on crafted 4-byte header
- **File:line:** `crates/shamir-wal/src/wal_segment.rs:532-539` (`replay_inner`) and `:377-384` (`repair_torn_tail`)
- **Severity:** low
- **Issue:** `let len = u32::from_le_bytes(..) as usize; let frame_end = pos + 4 + len + 4;` — on a target where `usize == u32`, a header of `0xFFFF_FFFF` wraps `frame_end` to a small value, the `frame_end > buf.len()` guard passes, and `&buf[pos + 4..pos + 4 + len]` panics (`slice index starts at 4 but ends at 3`). Untrusted on-disk input causing a library panic violates the CLAUDE.md error rule ("avoid `panic!` outside invariant violations"). Not reachable on the production 64-bit targets (`pos` ≤ file size, `len` ≤ 4 GiB, no `usize` wrap) — hence low — but the crate is a library and the walker runs on raw file bytes.
- **Failure scenario:** 32-bit build opens a segment whose first 4 bytes are `FF FF FF FF`: recovery panics instead of returning `Err`/stopping at the torn tail.
- **Suggested fix:** Compute with checked/saturating arithmetic (`pos.checked_add(4 + 4).and_then(|x| x.checked_add(len))`) and treat overflow as a torn tail (`break`). The fuzz target from finding 2 would catch this class permanently.

### 5. A single CRC-valid-but-undecodable frame aborts the entire recovery (version skew == corruption)
- **File:line:** `crates/shamir-wal/src/wal_segment.rs:572` (`out.push(WalEntryV2::decode(payload)?)` — the `?` aborts the whole `SegmentSet::replay`)
- **Severity:** nit
- **Issue:** Fail-closed on undecodable data is the right instinct (and loudly so), but the error conflates corruption with **version skew**: an entry written by a newer build (same envelope version byte, evolved bincode schema) makes the entire database unopenable after a downgrade, with an `Internal` error that doesn't name the skew. The envelope exists precisely to dispatch migrations (`wal_entry_v2.rs:20-23`) yet an in-version schema change has no distinct signal.
- **Suggested fix:** Distinguish decode-failure kinds (unsupported/foreign payload vs. truncation) in the error, and consider bumping `WAL_V2_VERSION` on any in-body schema change as a documented rule so skew is at least self-identifying.

### 6. Error strings embed absolute filesystem paths
- **File:line:** e.g. `crates/shamir-wal/src/wal_segment.rs:155,201-204,519-523,556-563`; `segment_set.rs:92,512`
- **Severity:** nit
- **Issue:** `DbError::Storage(format!("... {path:?} ..."))` bakes server-side absolute paths into error text. Whether this crosses the network depends on how `shamir-server`/`shamir-connect` map `DbError` onto wire responses (out of this crate's view); if any path reaches a client, it discloses host directory layout.
- **Suggested fix:** Keep paths in `tracing`/`log` output; return path-free (or basename-only) messages in the `DbError` variants that can reach callers outside the process.

## Positive observations (for balance)

- `segment_meta::decode` (`segment_meta.rs:138-155`) is exemplary untrusted-input parsing: exact-length check → magic → version → CRC, all before any field extraction; no panic path; every rejection branch unit-tested.
- `WalActiveKey::parse` (`active_key.rs:49-56`) validates length + prefix before slicing; `WalEntryV2::decode` rejects short/bad-magic/unknown-version with direct tests (`wal_entry_v2_tests.rs:69-87`).
- The sealed-vs-active and startup-vs-live `PermissionDenied`/CRC distinctions (`replay_sealed*`, `replay_at_startup`, audit §1.8/§2.4) show deliberate, tested untrusted-input hardening — finding 1 is the one place the same rigor was not carried through.
- No `unsafe`, no secret-dependent branching, no injection into command/SQL-style interpreters anywhere in the crate.

</details>
