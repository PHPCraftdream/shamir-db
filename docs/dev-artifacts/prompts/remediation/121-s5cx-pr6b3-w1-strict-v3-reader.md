# S5c-X PR6b-3 / W1 — strict v3 reader, typed refusals, v3-only bookmark module

Task 121 brief: `docs/dev-artifacts/prompts/remediation/121-s5cx-retire-legacy-follower.md`
(read it first — you implement its sections 1 (bookmark rows), 2 (refusal semantics) at the
metadata layer, and the strict-reader parts of its test disposition).

## Hard rules (repeat verbatim in any child brief)
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git
  command that mutates the working tree or index. Only edit files; the orchestrator commits.
  Delete files by emptying/rewriting them with your file tools, never with git.
- NO `cargo test` / `build` / `check` / `clippy` / `nextest` or any other compile or test
  command. Only `cargo fmt -p <crate>` for touched crates as your LAST step.
- Never silence a lint with `#[allow]` — remove the dead code instead. CI clippy runs with
  `-D warnings` (dead_code, unused imports, `clone_on_copy`, `needless_borrow`,
  `type_complexity`, `await_holding_lock`).
- Layout ratchet: ≤7 entries per folder, ≤1000 lines per file; files/folders already over a
  limit must not grow.
- All `use` statements at the top of the file. `mod.rs` files only re-export. Tests live in
  `tests/` folders, one topic per file, wired into the enclosing `tests/mod.rs`; delete
  removed test files from their manifests.
- No machine-specific absolute paths in any file you write.
- Every `impl Trait` function returns one concrete type on every branch.

## Context you need

The receipt protocol becomes the ONLY follower apply protocol. Repositories whose durable
bookmark record is legacy (fixed-width u64, v1 or v2 — including bookmark zero) are REFUSED
with a typed error; there is no migration and no legacy-read fallback anywhere.

Production bookmark code currently lives inside the tests directory and is loaded with
`#[path]`: `crates/shamir-engine/src/meta/repl/bookmark.rs:7` loads
`crates/shamir-engine/src/meta/tests/follower_bookmark/v2.rs`. That file hosts BOTH the
legacy codec (to delete) and the public loaders/writers (to keep, reimplemented strictly).
`FenceRecord` lives in `crates/shamir-engine/src/meta/tests/follower_bookmark/fence_record.rs`.

## Deliverables

### 1. `crates/shamir-storage/src/error.rs` — typed refusal
Add next to the existing variants (style-match the file):
```rust
/// Why durable follower state was refused. The receipt protocol is the only
/// follower apply protocol; unsupported states are refused, never migrated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowerRefusalReason {
    /// A legacy fixed-width/u64, v1 or v2 bookmark record is present.
    LegacyBookmarkFormat,
    /// The bookmark record is malformed, truncated, or of an unknown version.
    MalformedBookmark,
    /// A nonzero allocation floor (or other pre-receipt installation state)
    /// exists without a v3 bookmark record; reseeding is required.
    LegacyFollowerState,
}

/// Durable follower state this version refuses (S5c-X PR6b-3). No in-place
/// migration exists; the operator reseeds the affected subscription.
#[error("follower repository state refused ({reason:?}); no migration exists — reseed required")]
FollowerStateRefused { reason: FollowerRefusalReason },
```
Match the file's existing derive/error-attr style exactly (it uses thiserror).

### 2. `crates/shamir-engine/src/meta/repl/fence_record.rs` — NEW
Move `meta/tests/follower_bookmark/fence_record.rs` here VERBATIM (content unchanged except
its module doc if it names the old path). Production code must not live under `tests/`.

### 3. `crates/shamir-engine/src/meta/repl/bookmark_v3.rs` — strict loader + classifier
- DELETE `has_v3_record` (redundant raw sniff; callers move to the classifier — W2 fixes the
  two engine callers `receipt_recovery.rs` and (former) `activation.rs`; leave them broken).
- REWRITE `load_bookmark_v3`: read the key; `NotFound` → `Ok(None)`; sniffed tuple version 3
  → `decode_bookmark_v3` (strict, unchanged); ANYTHING else → typed refusal error (reason
  `LegacyBookmarkFormat` when the bytes look like the legacy encodings: length 22, or sniffed
  version 1 or 2; `MalformedBookmark` otherwise). NO legacy fallback projection anymore.
- ADD a public classifier used by open/admission (W2 consumes it):
```rust
/// Durable follower-role classification of the bookmark record at open /
/// first follower admission.
pub enum FollowerBookmarkState {
    /// A valid v3 record exists: the repo is a durably identified follower.
    Native(BookmarkV3),
    /// No bookmark record and no durable allocation floor: eligible for
    /// explicit fresh follower initialization (adoption writes v3 first).
    Fresh,
    /// No bookmark record with a durable `LastCommittedVersion` floor > 0:
    /// unowned history. A LEADER with this state must still open; an attempt
    /// to adopt/apply as a follower is refused (reseed required).
    FloorWithoutBookmark { floor: u64 },
}
pub async fn classify_follower_bookmark(info: &Arc<dyn Store>) -> DbResult<FollowerBookmarkState>
```
  Classification: read the record; `NotFound` → floor via
  `crate::meta::recovery_marker::load_last_committed(info)` (`unwrap_or(0)`); floor > 0 →
  `FloorWithoutBookmark`, else `Fresh`. Some bytes → v3 → `Native(decoded)`; legacy-shaped →
  `Err(DbError::FollowerStateRefused { reason: LegacyBookmarkFormat })`; anything else →
  `MalformedBookmark`. Malformed/unknown records are NEVER treated as absent (fail closed).
- Keep `encode_bookmark_v3`, `decode_bookmark_v3`, `save_bookmark_v3` unchanged.
- Update the module doc (drop the "PR6b-2 replaces the read path" narrative; state the
  strict, refusal-based contract).

### 4. `crates/shamir-engine/src/meta/repl/bookmark.rs` — rewrite strict, v3-only
Keep the module (the public path `crate::meta::repl_bookmark` and
`shamir_db::engine::meta::repl_bookmark` must keep resolving: `load_replication_bookmark`,
`load_fence_record`, `load_clean_horizon`, `save_fence_record_v3_aware`, `FenceRecord`).
- `mod fence_record; pub use fence_record::FenceRecord;` (no `#[path]` tricks).
- Reimplement the three loaders as STRICT V3 PROJECTIONS: read the key; `NotFound` → default
  (`0` / `None` / `None`); v3 → `decode_bookmark_v3` → project the field; anything else →
  the same typed refusal errors as `load_bookmark_v3` (share one private strict-read helper
  so the reason logic lives in exactly one place per crate; a small private byte-sniff helper
  duplicated in `bookmark_v3.rs` is fine, but the CLASSIFICATION decision must not be
  duplicated — have this module call `bookmark_v3`'s classifier or decode, not re-decide).
- REWRITE `save_fence_record_v3_aware` as v3-only identity adoption:
  - record present and v3 → in-place fence update preserving `bookmark`, `clean_through`
    and `outstanding_receipt` exactly as today (sync `fence.bookmark` to the record's
    bookmark when they disagree), set+flush via `save_bookmark_v3`.
  - record absent → if `load_last_committed(info) > 0` → `Err(DbError::FollowerStateRefused {
    reason: LegacyFollowerState })` (floor without v3 is pre-receipt follower state; adoption
    must not manufacture a v3 over it). Otherwise write the fresh v3 baseline
    `{ fence: Some(FenceRecord { bookmark: 0, ..record }), bookmark: 0, clean_through: None,
    outstanding_receipt: None }`.
  - record present but legacy/malformed → the matching typed refusal (never rewrite).
- DELETE: `Version2Record`, the legacy `decode`, `load` legacy internals, `save_record`,
  `get_previous`, `refuse_v3`, `save_fence_record`, `save_replication_bookmark`,
  `save_bookmark_and_clean_horizon`, `is_v3`, and the `#[path]`/`#[cfg(test)]` blocks that
  pulled code from `meta/tests/follower_bookmark/`.
- Update the module doc: v3-only contract, refusal semantics, no legacy writers.

### 5. Delete `crates/shamir-engine/src/meta/tests/follower_bookmark/` entirely
(v2.rs production codec gone; fence_record.rs moved; v2_tests.rs superseded — its REPLACEMENT
is deliverable 7). Make sure nothing references the directory anymore.

### 6. Meta test rewrites (wire through manifests)
- `crates/shamir-engine/src/meta/repl/tests/bookmark_v3_tests.rs`: update for the strict
  loader — decode/encode round trip unchanged; loader tests now assert `Ok(None)` on absent
  and the typed refusal on legacy/corrupt bytes.
- NEW `crates/shamir-engine/src/meta/repl/tests/bookmark_refusal_tests.rs`: encode legacy
  fixtures IN THE TEST (raw `MetaEnvelope::new((1u16, fence))` / `((2u16, record))` /
  fixed-width u64 envelope — the tests own the fixture bytes now, no production writer
  exists) and assert:
  - u64 / v1 / v2 records, INCLUDING bookmark zero, refuse `load_bookmark_v3`,
    `classify_follower_bookmark`, `load_replication_bookmark`, `load_fence_record`,
    `load_clean_horizon` with the exact typed reason `LegacyBookmarkFormat`;
  - truncated/unknown-version bytes refuse with `MalformedBookmark`;
  - corrupt records are never overwritten "as fresh": `save_fence_record_v3_aware` over a
    legacy record errors and leaves the stored bytes untouched (read back and compare);
  - floor-marker-without-bookmark: `classify_follower_bookmark` → `FloorWithoutBookmark`;
    `save_fence_record_v3_aware` refuses with `LegacyFollowerState`;
  - absent+no-floor: `classify_follower_bookmark` → `Fresh`; `save_fence_record_v3_aware`
    writes the v3 baseline (bookmark 0, clean None, receipt None, fence synced to 0).
  Wire it into `crates/shamir-engine/src/meta/repl/tests/mod.rs`.
- `crates/shamir-engine/src/meta/tests/repl_bookmark_tests.rs`: rewrite for the v3-only API
  (round trip through `save_bookmark_v3` + projections; refusal fixtures live in the new
  file — don't duplicate them here).
- `crates/shamir-engine/src/meta/tests/follower_durability_tests.rs`: rewrite the
  fence/bookmark assertions onto `save_fence_record_v3_aware` + v3 records (fence write
  preserves bookmark/clean horizon/receipt pointer).
- Keep `meta/tests/mod.rs` manifests consistent (nothing references follower_bookmark).

### 7. Reference sweep (report, don't fix out of scope)
`rg` the whole workspace for every deleted name (`save_replication_bookmark`,
`save_fence_record\b`, `save_bookmark_and_clean_horizon`, `has_v3_record`,
`Version2Record`, `refuse_v3`) and for `follower_bookmark`. Known broken-until-W2/W3/W4
production callers: `crates/shamir-engine/src/repo/seed_install/writer.rs` (W2),
`crates/shamir-engine/src/repo/repo_instance.rs` advance_replication_bookmark (W3),
`crates/shamir-engine/src/tx/follower_durability/batch.rs` (W3),
`receipt/resume/activation.rs` + `cold_reconcile/receipt_recovery.rs` (`has_v3_record`, W2),
plus receipt/gap/activation tests (W4). List ALL residue you find with file:line in your
report; fix only what is inside deliverables 1–6.

## Last step
`cargo fmt -p shamir-storage -p shamir-engine` (only if it changes files you touched; report
the diff summary).

## Report back
Files changed/created/deleted; the typed-error names you introduced and where; the full
residue list from deliverable 7 (file:line); line counts of any file you touched that now
exceeds 900 lines; anything you had to deviate on, with the reason.
