<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-wal — style-claude-md independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The actual layout violation and stale architecture prose remain Low/Nit work. Registered tests are not orphaned, and closely coupled types are explicitly allowed.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 9 | 7 | 0 | 0 | 0 | 0 | 2 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Inline #[cfg(test)] mod tests in an implementation file

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Five tests reside in an inline cfg(test) implementation-file module, contrary to the explicit repository rule; Rust registration remains intact.

Evidence: [crates/shamir-wal/src/segment_meta.rs:175](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_meta.rs#L175); [AGENTS.md:158](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/AGENTS.md#L158).

<a id="review-2"></a>

### Claim 2 — Module docs describe the retired KV-marker design as current, with broken intra-doc links

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Entry/segment docs describe removed KV dispatch and link absent WalManager/wal_entry types, contradicting current exports and construction.

Evidence: [crates/shamir-wal/src/wal_entry_v2.rs:3](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_entry_v2.rs#L3); [crates/shamir-wal/src/wal_segment.rs:3](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L3).

Grouping/duplicate: [api-wire-protocol.md#3](api-wire-protocol.md#review-3). This is not an additional independent defect.

<a id="review-3"></a>

### Claim 3 — segment_set.rs module doc claims it is unwired scaffold

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The F6a unwired paragraph remains although File stores SegmentSet and RepoInstance constructs it.

Evidence: [crates/shamir-wal/src/segment_set.rs:15](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_set.rs#L15); [crates/shamir-wal/src/wal_sink.rs:86](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_sink.rs#L86); [crates/shamir-engine/src/repo/repo_instance.rs:830](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/repo/repo_instance.rs#L830).

<a id="review-4"></a>

### Claim 4 — WalActiveKey: exported, documented-as-live module with zero production callers

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The three-callsites/recovery-scan prose remains, but source consumers are its own tests. Retention as a documented legacy helper is valid; export alone does not assert current use.

Evidence: [crates/shamir-wal/src/active_key.rs:4](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/active_key.rs#L4); [crates/shamir-wal/src/lib.rs:54](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/lib.rs#L54).

Grouping/duplicate: [api-wire-protocol.md#3](api-wire-protocol.md#review-3). This is not an additional independent defect.

<a id="review-5"></a>

### Claim 5 — Mid-function use statements in tests (imports-at-top rule)

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Four test-body imports have no collision or cfg rationale and violate imports-at-top.

Evidence: [crates/shamir-wal/src/tests/wal_group_commit_tests.rs:222](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/tests/wal_group_commit_tests.rs#L222); [crates/shamir-wal/src/tests/wal_group_commit_tests.rs:253](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/tests/wal_group_commit_tests.rs#L253); [crates/shamir-wal/src/tests/wal_group_commit_tests.rs:270](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/tests/wal_group_commit_tests.rs#L270); [crates/shamir-wal/src/tests/wal_group_commit_tests.rs:447](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/tests/wal_group_commit_tests.rs#L447).

<a id="review-6"></a>

### Claim 6 — pub mod segment_meta exports nothing public

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The public module exposes only private/crate-private functions and links private read_blocking from its docs. This is namespace/documentation debt, not a runtime defect or observed rustdoc run.

Evidence: [crates/shamir-wal/src/lib.rs:47](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/lib.rs#L47); [crates/shamir-wal/src/segment_meta.rs:11](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_meta.rs#L11); [crates/shamir-wal/src/segment_meta.rs:120](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_meta.rs#L120).

<a id="review-7"></a>

### Claim 7 — Vestigial, unexplained #[allow(dead_code)] on a public type

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Both broad allowances remain without rationale. The impl allowance can suppress private-helper warnings, so 'does nothing' is inaccurate; no runtime consequence was demonstrated.

Evidence: [crates/shamir-wal/src/wal_segment.rs:108](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L108); [crates/shamir-wal/src/wal_segment.rs:132](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L132).

<a id="review-8"></a>

### Claim 8 — wal_sink.rs carries two public types with separate impl blocks (borderline)

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

MemSink is the enum variant's payload and fits the expressly permitted closely-coupled group. Additional module prose or splitting is optional.

Evidence: [crates/shamir-wal/src/wal_sink.rs:17](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_sink.rs#L17); [crates/shamir-wal/src/wal_sink.rs:89](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_sink.rs#L89); [CLAUDE.md:497](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L497).

<a id="review-conformant-manifests-and-registration"></a>

### Claim Conformant/manifests-and-registration — Manifest-only roots and topic-split test registration conform

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

lib.rs declares/reexports; cfg(test) mounts a manifest containing six topic modules. Coupled entry/op and coordinator/tier types fit the structural exception.

Evidence: [crates/shamir-wal/src/lib.rs:43](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/lib.rs#L43); [crates/shamir-wal/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/tests/mod.rs#L1).

## Evidence and recipe corrections

- Inline placement does not prevent Rust discovery and is not runtime High.
- Private encode/decode are not reachable from arbitrary sibling test modules. Mount extracted tests as a descendant of segment_meta, or deliberately widen only crate visibility.
- doctest=false does not disable ordinary rustdoc link resolution.
- Additional stale guarantees include SegmentSet::recover instead of replay, wal.commit advancing truncation although it is a no-op, and max_committed spanning active contents although reopen resets that watermark.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-wal -- Style & CLAUDE.md structural conformance

## Summary

The crate's skeleton largely conforms: `lib.rs`/`tests/mod.rs` are re-export-only manifests, modules are topic-split with one primary export each, tests live in `src/tests/` as fixture-only topic files, and imports are otherwise hoisted (including the sanctioned cfg-gated import in `wal_sink.rs`). The clear structural breach is `segment_meta.rs`, which embeds an inline `#[cfg(test)] mod tests` — the only file in the crate violating the "never inline test modules" rule. The dominant comment-discipline problem is stale documentation: several module docs still describe the retired KV-marker (`__wal_active_`/info_store) WAL design and a pre-F6b "wired into nothing yet" state as current, including two broken intra-doc links to symbols that do not exist in this crate; `WalActiveKey` is exported and tested despite having zero production callers.

## Findings

### 1. Inline `#[cfg(test)] mod tests` in an implementation file
- **File:** `crates/shamir-wal/src/segment_meta.rs:175-218`
- **Severity:** high
- **Issue:** CLAUDE.md test-organisation rule 5: "Never embed `#[cfg(test)] mod tests { ... }` inline inside implementation files. Move them to the `tests/` directory." `segment_meta.rs` carries a 44-line inline test module (5 tests: `roundtrip_encode_decode`, `decode_rejects_bad_magic`, `decode_rejects_bad_version`, `decode_rejects_bad_crc`, `decode_rejects_wrong_length`). Every other module in this crate correctly puts its tests in `src/tests/` (6 topic files behind the manifest-only `tests/mod.rs`); this is the sole outlier, and `src/tests/segment_meta_tests.rs` does not exist. `segment_meta`'s private fns (`encode`/`decode`) are reachable from a sibling test file via `crate::segment_meta::…` or `pub(crate)` shims, same pattern other test files already use for `pub(crate)` knobs.
- **Failure scenario:** the next contributor adding sidecar tests appends to the inline module (it is the visible local precedent inside this file), and the crate's test layout forks; test discovery by file no longer works for this module.
- **Suggested fix:** move the five tests to `crates/shamir-wal/src/tests/segment_meta_tests.rs` (marked `pub mod segment_meta_tests;` in `tests/mod.rs`), widening `encode`/`decode` to `pub(crate)` only if needed.

### 2. Module docs describe the retired KV-marker design as current, with broken intra-doc links
- **File:** `crates/shamir-wal/src/wal_entry_v2.rs:1-16` (esp. 3, 14-16); `crates/shamir-wal/src/wal_segment.rs:3-7`; `crates/shamir-wal/src/wal_entry_v2.rs:259-264`
- **Severity:** medium
- **Issue:** Comment discipline. `wal_entry_v2.rs`'s module doc says V1/V2 entries "live under the same `WalActiveKey` prefix in info_store; recovery distinguishes them by sniffing the magic prefix on each value (stage 0.8 will wire this)" and links `[`super::wal_entry::WalEntry`]` — but this crate has no `wal_entry` module, `lib.rs`'s own doc states the F5c/F6 cutover "retired the earlier KV-marker design … production no longer uses such markers", and entries now go to file segments via `SegmentSet`. `wal_segment.rs:3` opens with "The existing [`crate::WalManager`] is KV-backed" — no `WalManager` exists in this crate's API (the manager lives in `shamir-tx`/`shamir-engine` history). `WalEntryV2::looks_like_v2`'s doc ("Used by `WalManager` (stage 0.8) to dispatch between V1 and V2") is likewise stale: the method's only caller anywhere in the workspace is its own unit test. Doctests are banned crate-wide (`doctest = false` in Cargo.toml) and rustdoc is not in the pre-commit gate, so the broken links and false claims are never surfaced mechanically.
- **Failure scenario:** an engineer reading `wal_entry_v2.rs`/`wal_segment.rs` top-down (exactly what module docs are for) reconstructs the wrong storage model — markers in info_store with magic-sniff dispatch — and "corrects" recovery/append code toward a design that was retired.
- **Suggested fix:** rewrite the two module-doc preambles in past tense ("Historically … retired by the F5c/F6 file-segment cutover"), delete or fix the broken links (`super::wal_entry::WalEntry`, `crate::WalManager`), and either delete `looks_like_v2` or annotate it as a retained decode helper with no live dispatcher.

### 3. `segment_set.rs` module doc claims it is unwired scaffold ("wired into nothing yet")
- **File:** `crates/shamir-wal/src/segment_set.rs:15-16`
- **Severity:** medium
- **Issue:** Comment discipline. The doc says: "PURELY ADDITIVE (F6a): wired into nothing yet — production still runs a single [`WalSegment`] via `WalSink::File`. F6b cuts `repo_instance` over." This is false on two counts: (a) `shamir-engine/src/repo/repo_instance.rs:800-801` already calls `shamir_wal::SegmentSet::open(...)` and wraps it in `WalSink::File(segset)` — the cutover landed; (b) `WalSink::File` holds a `SegmentSet` (`wal_sink.rs:86`), not a single `WalSegment` — the type shape the comment describes no longer exists. It directly contradicts sibling docs (`wal_group_commit.rs:65-68` "Wired in … production commit path (W3/W4 landed), not an unwired scaffold"; `wal_segment.rs:15-18` "Live production primitive"; `lib.rs` architecture section).
- **Failure scenario:** a reviewer or contributor assessing whether `SegmentSet` is safe to change skips impact analysis on the commit path, believing production bypasses it; or removes it as dead scaffold.
- **Suggested fix:** replace the paragraph with the current truth (production sink since F6b; constructed by `repo_instance.rs`) or delete it outright.

### 4. `WalActiveKey`: exported, documented-as-live module with zero production callers
- **File:** `crates/shamir-wal/src/active_key.rs` (whole file); `crates/shamir-wal/src/lib.rs:46,54`
- **Severity:** medium
- **Issue:** Comment discipline + structural. Workspace grep shows `WalActiveKey` is referenced only by its own module, its test file (`tests/active_key_tests.rs`), and prose in comments/docs of other crates (`shamir-engine/src/tx/pre_commit.rs:2695`, `recovery_tests.rs:576`, `shamir-storage/src/key_bytes.rs:37`) — no production call site anywhere. Its module doc still claims the encoding "lives in one place instead of being recomputed at three callsites" and that `scan_prefix` serves "recovery's `scan_prefix → sorted by oldest first` flow", all of which belonged to the KV-marker design `lib.rs` declares retired. The crate nonetheless ships it as `pub mod active_key` + `pub use active_key::WalActiveKey`, plus a dedicated test file asserting byte-compatibility with on-disk data nothing reads anymore.
- **Failure scenario:** readers assume active markers are part of the live WAL protocol (the crate's own exports say so) and build new code against them; the module's "three callsites" claim sends archaeologists hunting for code that doesn't exist.
- **Suggested fix:** owner decision — delete module + `lib.rs` exports + `active_key_tests.rs`, or, if deliberately retained as a legacy on-disk-format decoder, rewrite the doc to say exactly that ("retained to parse pre-F5c corpora; no live callers") so the export stops lying.

### 5. Mid-function `use` statements in tests (imports-at-top rule)
- **File:** `crates/shamir-wal/src/tests/wal_group_commit_tests.rs:222, 253, 270, 447`
- **Severity:** low
- **Issue:** Four test bodies each open with a local `use std::time::Duration;`. CLAUDE.md "Imports at the top" bans `use` inside function bodies unless one of three documented exceptions applies (module-local `super::*` in a test mod, trait-name collision, cfg-gated validity) — none does here; there is no name collision, `Duration` is used freely elsewhere in the same file. Inconsistently, the file header does *not* import `Duration` and instead spells it fully-qualified inside the shared `poll_until` helper (lines 42, 51).
- **Suggested fix:** add `use std::time::Duration;` to the header import block and delete the four local imports (also shortening `poll_until`'s signatures).

### 6. `pub mod segment_meta` exports nothing public
- **File:** `crates/shamir-wal/src/lib.rs:47`; `crates/shamir-wal/src/segment_meta.rs:62, 89, 120, 164`
- **Severity:** low
- **Issue:** Structural/API-surface. The module is declared `pub` but every item in it is `pub(crate)` (`meta_path_for`, `write_blocking`, `read_blocking`, `remove_blocking`), so the crate's public API contains an empty module. Side effect: the module doc's intra-doc links (`[`read_blocking`]`, `[`crate::SegmentSet::open`]` context) point from a public doc into private items, which rustdoc flags as "public documentation links to private item" whenever docs are built.
- **Suggested fix:** demote to `mod segment_meta;` in `lib.rs` (internal helper module of `segment_set`), keeping `lib.rs` re-exports unchanged (there are none for it today).

### 7. Vestigial, unexplained `#[allow(dead_code)]` on a public type
- **File:** `crates/shamir-wal/src/wal_segment.rs:108, 132`
- **Severity:** nit
- **Issue:** `#[allow(dead_code)]` sits on `pub struct WalSegment` and its `impl`. Public items in a library crate cannot be dead code (reachable via the public API), so the attributes do nothing today — but they would silently mask genuinely dead private helpers if visibility ever narrows, and they carry no inline justification, contra the workspace pattern for allows (e.g. the `#[allow(clippy::disallowed_methods)] // O(N) ack: <why>` convention in CLAUDE.md, and the justified `#![allow(clippy::disallowed_types)]` header in `wal_group_commit_tests.rs:1-2`).
- **Suggested fix:** delete both attributes; if one was load-bearing in the pre-extraction `shamir-engine` location, that history stayed behind.

### 8. `wal_sink.rs` carries two public types with separate impl blocks (borderline)
- **File:** `crates/shamir-wal/src/wal_sink.rs:17, 82`
- **Severity:** nit
- **Issue:** One-file-one-export: this is the only src file with two public types (`WalSink` enum + `MemSink` struct, each with its own `impl`, plus a separate `Default` impl). Defensible as a "closely-coupled group" — `MemSink` exists solely as `WalSink::Mem`'s payload and mirrors its interface — so this is flagged as borderline, not a violation. Relatedly, `wal_sink.rs` is the only src module without a `//!` module-level doc, which makes the coupling rationale live only in scattered item docs.
- **Suggested fix:** optional: move `MemSink` to `mem_sink.rs` and give `wal_sink.rs` a module doc stating the enum-not-trait ("no dyn dispatch on the hot path") design; or just add the module doc and leave the layout as a documented coupled pair.

## Conformant-by-design (checked, no action)

- `lib.rs` and `src/tests/mod.rs` are re-export/manifest-only — no logic in either.
- `#[cfg(test)] mod tests;` wiring in `lib.rs:43-44` follows the documented layout.
- All other imports are at file/module headers, including the sanctioned cfg-gated `#[cfg(test)] use std::sync::atomic::AtomicBool;` (`wal_sink.rs:1-2`) and `use super::*;` inside the (misplaced, see #1) inline test module.
- Test files are topic-split, contain fixtures + tests only, headers clean; benches (`benches/*.rs`) have no mid-function imports.
- Per-file primary exports hold elsewhere: `active_key.rs`→`WalActiveKey`, `segment_set.rs`→`SegmentSet` (+private `SealedMeta`/`Inner`), `wal_entry_v2.rs`→`WalEntryV2` (+coupled `WalOpV2`, private legacy/serde helpers), `wal_group_commit.rs`→`WalGroupCommit` (+coupled `WalDurability`, private `Waiter`), `wal_segment.rs`→`WalSegment`, `segment_meta.rs`→ cohesive pub(crate) free-function group.

</details>
