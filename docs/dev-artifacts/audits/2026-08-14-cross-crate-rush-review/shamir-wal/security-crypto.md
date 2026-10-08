<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-wal — security-crypto independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Local-file corruption and storage ownership are the relevant boundaries. The specific tiny-frame exabyte-allocation allegation is refuted by the exact published dependency paths, including the previously missing ByteBuf implementation.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 11 | 6 | 0 | 1 | 2 | 0 | 2 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — repair_torn_tail silently and irreversibly truncates on a mid-file CRC mismatch in the ACTIVE segment

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

A complete CRC-bad frame causes physical suffix truncation during active open. Existing positive-frame suffixes can be lost after local corruption; this is not a demonstrated remote input path.

Evidence: [crates/shamir-wal/src/wal_segment.rs:391](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L391); [crates/shamir-wal/src/wal_segment.rs:404](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L404).

Grouping/duplicate: [correctness-tdd.md#4](correctness-tdd.md#review-4). This is not an additional independent defect.

<a id="review-2"></a>

### Claim 2 — bincode 1.x decodes recovery data with no allocation bounds — small crafted/corrupt frame can OOM the process

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `unverified`.

The missing pinned serde_bytes 0.11.19 source and archive are available. ByteBuf calls deserialize_byte_buf; bincode 1.3.3 read_vec reaches SliceReader.get_byte_buffer, which checks remaining bytes before copying. Strings and borrowed RecordId follow checked slice paths. serde_core 1.0.228 collection hints are capped at 1 MiB, with incremental element reads. The claimed tiny-input speculative-exabyte mechanism is absent. Published sources: https://docs.rs/crate/serde_bytes/0.11.19/source/src/bytebuf.rs and https://docs.rs/crate/bincode/1.3.3/source/src/de/read.rs. This is not a universal memory bound for large corpora.

Evidence: [Cargo.lock:413](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L413); [Cargo.lock:3214](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3214); [Cargo.lock:3224](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3224); [crates/shamir-wal/src/wal_entry_v2.rs:121](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_entry_v2.rs#L121); [crates/shamir-wal/src/wal_entry_v2.rs:240](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_entry_v2.rs#L240); [crates/shamir-wal/src/wal_entry_v2.rs:251](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_entry_v2.rs#L251); [crates/shamir-types/src/types/record_id.rs:167](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/record_id.rs#L167).

<a id="review-2-ops-length-allocation"></a>

### Claim 2/ops-length-allocation — Huge ops length causes immediate multi-exabyte Vec preallocation

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

serde_core 1.0.228 VecVisitor uses cautious&lt;T&gt;, capped at 1 MiB, then decodes elements. All reachable WAL collection element shapes consume input. Published source: https://docs.rs/crate/serde_core/1.0.228/source/src/de/impls.rs and source/src/private/size_hint.rs.

Evidence: [Cargo.lock:3224](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3224); [crates/shamir-wal/src/wal_entry_v2.rs:153](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_entry_v2.rs#L153).

Grouping/duplicate: [security-crypto.md#2](security-crypto.md#review-2). This is not an additional independent defect.

<a id="review-2-unbounded-file-buffering"></a>

### Claim 2/unbounded-file-buffering — Recovery reads whole segment files without a size cap

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Repair, replay and sidecar reading use read_to_end without a byte ceiling. A large legitimate backlog or locally enlarged file can exhaust memory; max_bytes is checked after a complete batch and is not a hard cap.

Evidence: [crates/shamir-wal/src/wal_segment.rs:370](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L370); [crates/shamir-wal/src/wal_segment.rs:527](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L527); [crates/shamir-wal/src/segment_meta.rs:131](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_meta.rs#L131).

<a id="review-3"></a>

### Claim 3 — Unauthenticated WAL replay is an injection surface; the data directory is trusted assumption is implicit and undocumented

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

A local writer can forge CRC-valid entries or sidecar maxima; no keyed authentication binds them to a repo. This documents missing integrity trust assumptions, not privilege escalation by ordinary authenticated requests. At-rest encryption delegation does not authenticate a writable directory.

Evidence: [crates/shamir-wal/src/wal_segment.rs:230](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L230); [crates/shamir-wal/src/segment_set.rs:145](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_set.rs#L145); [docs/guide-docs/security/data-protection.md:23](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/security/data-protection.md#L23).

<a id="review-4"></a>

### Claim 4 — Frame-length arithmetic can wrap on 32-bit targets → panic on crafted 4-byte header

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

With len=u32::MAX and pos=0, wrapping frame_end is 7. Four bytes alone break in release; seven bytes pass the guard and create the invalid 4..3 payload slice. Debug overflow checks panic earlier. wasm32 deployment is not established.

Evidence: [crates/shamir-wal/src/wal_segment.rs:380](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L380); [crates/shamir-wal/src/wal_segment.rs:384](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L384); [crates/shamir-wal/src/wal_segment.rs:535](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L535).

<a id="review-5"></a>

### Claim 5 — A single CRC-valid-but-undecodable frame aborts the entire recovery (version skew == corruption)

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Fail-closed replay is appropriate and unknown versions have distinct text. A concrete documentation/compatibility defect exists for pre-5575ad59 version-1 bodies: positional insertion of commit_version was unversioned and current legacy decoding cannot recover an old empty-ops body. The synthetic legacy test serializes the newer shape. bincode 1.3.3 source: https://docs.rs/crate/bincode/1.3.3/source/src/de/mod.rs. General alpha upgrade support is not promised.

Evidence: [crates/shamir-wal/src/wal_entry_v2.rs:137](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_entry_v2.rs#L137); [crates/shamir-wal/src/wal_entry_v2.rs:165](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_entry_v2.rs#L165); [crates/shamir-wal/src/tests/wal_entry_v2_tests.rs:163](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/tests/wal_entry_v2_tests.rs#L163); [README.md:33](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/README.md#L33).

<a id="review-6"></a>

### Claim 6 — Error strings embed absolute filesystem paths

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Supplied absolute paths appear in Storage errors. Grouped append replaces most such causes with generic text; the server serializes other error messages, but a particular WAL-to-client path was not established.

Evidence: [crates/shamir-wal/src/wal_segment.rs:155](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L155); [crates/shamir-wal/src/wal_group_commit.rs:201](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_group_commit.rs#L201); [crates/shamir-server/src/db_handler/handler.rs:611](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/handler.rs#L611).

<a id="review-positive-sidecar-and-envelope-bounds"></a>

### Claim Positive/sidecar-and-envelope-bounds — Sidecar, active-key, and entry-envelope rejection guards are defensive

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Sidecar exact length precedes indexed extraction; key length/prefix and envelope length/magic/version are checked. These guards do not bound the preceding whole-file read.

Evidence: [crates/shamir-wal/src/segment_meta.rs:138](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_meta.rs#L138); [crates/shamir-wal/src/active_key.rs:49](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/active_key.rs#L49); [crates/shamir-wal/src/wal_entry_v2.rs:228](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_entry_v2.rs#L228).

<a id="review-positive-sealed-and-startup-hardening"></a>

### Claim Positive/sealed-and-startup-hardening — Sealed CRC and startup PermissionDenied distinctions are tested

Status: `partially-fixed`. Current risk: `medium`.

Prior-cycle decision: `partially-fixed`.

The sealed payload-flip test detects removal of the CRC Err branch. Startup denial is not induced, and sealed corrupted-length/incomplete-tail cases bypass that CRC oracle entirely.

Evidence: [crates/shamir-wal/src/tests/wal_segment_tests.rs:174](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/tests/wal_segment_tests.rs#L174); [crates/shamir-wal/src/tests/wal_segment_tests.rs:218](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/tests/wal_segment_tests.rs#L218); [crates/shamir-wal/src/wal_segment.rs:536](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L536).

Grouping/duplicate: [correctness-tdd.md#3](correctness-tdd.md#review-3). This is not an additional independent defect.

<a id="review-positive-no-crypto-or-unsafe-surface"></a>

### Claim Positive/no-crypto-or-unsafe-surface — No authentication, secret comparison, unsafe block, or interpreter injection surface

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

The inspected production crate contains no authentication primitive, unsafe block, secret comparison or command/SQL interpreter. Absence of secret comparison does not prove absence of every possible side channel.

Evidence: [crates/shamir-wal/Cargo.toml:9](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/Cargo.toml#L9); [crates/shamir-wal/src/lib.rs:46](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/lib.rs#L46).

## Evidence and recipe corrections

- Replace the previous missing-serde_bytes limitation with the checked pinned source/archive result; the tiny-input exabyte scenario is refuted, not merely still unverified.
- bincode 1.3.3 with_limit is available without a dependency upgrade, but an explicit configuration must preserve fixint/little-endian wire semantics. A byte budget is not a universal preallocation bound for every serde dispatch.
- Changing to bincode 2 or postcard is not a drop-in decoder fix; it requires explicitly supported layouts and a compatibility decision.
- Use bounded reading, not only metadata().len() checks, for mutable files. Sidecars can be read with a small fixed-length-plus-one bound.
- Do not classify every complete CRC failure as proven bit rot; a failed/torn write can also produce it.

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
