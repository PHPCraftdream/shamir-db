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
