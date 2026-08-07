# Brief — #1025: DROP INDEX must resolve the index family from the catalog, not trust the client's `unique: bool`

## Context

S.H.A.M.I.R. Database. Source: both 2026-08-05 review reports (fh §8 R1:
"unified DROP INDEX name без клиентского unique: bool — DropIndexOp это
последний stringly-рудимент, каталог уже классифицирует"). #1010 (closed
this session) unified the per-table index-name namespace across ALL FOUR
index families (base_index regular, base_index unique, sorted, index2) —
`handle_create_index`'s `any_index_exists` preflight (`crates/shamir-db/
src/shamir_db/execute/admin_table_index.rs:401`) now refuses to CREATE a
name that already exists in ANY family, so on any table built after #1010,
a given index name can exist in **at most one** family at a time.

`DropIndexOp` (`crates/shamir-query-types/src/admin/types/index_ops.rs:
117-145`) still carries a required-in-practice `unique: bool` the CLIENT
must set correctly before calling DROP INDEX, because
`handle_drop_index`'s resolution logic (`admin_table_index.rs:592-738`)
branches on `op.unique` to decide which ONE of the two base_index
sub-families to probe (`unique_index_exists` vs `index_exists`) — see
lines 607-618 (the `if_exists` early-exit) and 648-654 (the main
resolution). Since names are now globally unique per table across all
four families (post-#1010), this client-supplied hint is redundant and,
worse, a WRONG value silently mis-resolves: if a caller passes
`unique: false` to drop an index that is actually unique, `index_exists`
(the regular-family probe) returns false, `unique_index_exists` is never
even checked, and the drop either no-ops (`if_exists` path) or falls
through to sorted/index2 checks and eventually errors as "not found" —
even though the index genuinely exists, just in the family the client
guessed wrong.

## Already investigated — the fix already exists as a precedent, for RENAME

`TableManager::rename_index` (`crates/shamir-engine/src/table/
table_manager_index_mgmt.rs:1546-1605`) already solves EXACTLY this
problem, and its own doc comment names this task explicitly:

```rust
// R0-C (#1010): refuse instead of silently resolving when `old_name`
// is a PRE-EXISTING cross-family collision... A full redesign (explicit
// per-family disambiguator) is out of scope here (tracked as #1025)...
let is_regular = self.index_manager.index_exists(old_id);
let is_unique = self.index_manager.unique_index_exists(old_id);
let is_sorted = self.sorted_indexes.find_by_name_interned(old_id).is_some();
let is_index2 = self.index2_registry.get_by_name(old_id).await.is_some();
let matching_families = [is_regular, is_unique, is_sorted, is_index2]
    .iter().filter(|&&m| m).count();
if matching_families > 1 { return Err(...); }
// ...then dispatches to whichever ONE family actually matched.
```

Note `RenameIndexOp` has **no `unique` field at all** — `rename_index`
classifies unconditionally, every time, from the catalog. This is direct
proof the pattern works with zero client foreknowledge of the family.
`handle_rename_index`'s own `if_exists` early-exit (`admin_table_index.rs:
790-799`) is ALSO already unconditional (`table.index_exists(..) ||
table.unique_index_exists(..) || ...`) — it is `handle_drop_index`
specifically that still branches on the client hint.

**Recommendation:** bring `handle_drop_index` in line with
`rename_index`'s already-correct pattern — classify unconditionally
across all 4 families, refuse on >1 match (extending the EXISTING
`matching_families` guard at lines 648-671, which today undercounts
because `base_index_has_it` collapses two families into one client-hint-
gated check), and dispatch the drop to whichever ONE family matched, not
to whichever the client declared.

## The HMAC wrinkle — investigate, but this should NOT block the fix

`DropIndexOp.hmac` is a required-in-practice confirmation signature over
`canonical_drop_index(db, repo, table, index, unique)` (`crates/
shamir-query-types/src/hmac.rs:118-134`), which embeds `unique` as a
`0`/`1` byte. The server recomputes this canonical form FROM the
client-submitted `op.unique` and compares against the client-submitted
tag (`crates/shamir-server/src/db_handler/admin.rs:666-675`) — i.e. the
signature is **self-consistent** (binds whatever the client declared,
tamper-evident against a MITM flipping the bit in transit) and is **not**
a proof that the declared value matches the index's TRUE family. Verify
this reading yourself before proceeding (read `canonical_drop_index` and
its call site in `admin.rs` end to end), but if confirmed: decoupling
DROP INDEX's family *resolution* from `op.unique` does NOT weaken the
HMAC gate at all — a client can keep signing with `unique: false` always
(or any fixed convention) and the signature still validates exactly as
today. **Do not change `canonical_drop_index`'s shape or the wire
`DropIndexOp.hmac` field** — that is a separate, more sensitive
authorization-format change, out of scope here, and not needed to close
this task.

## What to implement

1. **`handle_drop_index`'s `if_exists` early-exit** (`admin_table_index.rs:
   601-626`): replace the `if op.unique { unique_index_exists } else {
   index_exists }` branch with an unconditional check of both (mirror
   `handle_rename_index`'s own early-exit at lines 790-799 exactly —
   `table.index_exists(..).await || table.unique_index_exists(..).await
   || table.sorted_index_exists(..).await || table.index2_exists(..).await`).

2. **`handle_drop_index`'s main resolution** (`admin_table_index.rs:
   648-700`): classify unconditionally into four named booleans
   (`is_regular`, `is_unique`, `is_sorted`, `is_index2`), mirroring
   `rename_index`'s exact structure. Extend the existing
   `matching_families` collision guard (currently built from
   `[base_index_has_it, sorted_has_it, index2_has_it]`, effectively
   3-way because `base_index_has_it` conflates regular+unique) to the
   true 4-way `[is_regular, is_unique, is_sorted, is_index2]`. Then
   dispatch to the ONE matching family's drop call (`table.drop_index`
   / `table.drop_unique_index` / `table.drop_sorted_index` /
   `table.drop_index2`) — not based on `op.unique`.

3. **`DdlOpKind` selection** (`admin_table_index.rs:706-714`): currently
   `if op.unique { DropUniqueHashIndex } else { DropHashIndex }` — must
   switch to branching on the ACTUAL resolved `is_unique` from step 2,
   not the client's declared flag, so the DDL op-status log records the
   truth even when a caller's hint was wrong.

4. **`op.unique` becomes informational-only** in `handle_drop_index` —
   confirm (by reading, not assuming) that after steps 1-3 nothing in
   this function still reads `op.unique` for a resolution decision (it
   may still legitimately be read for logging/telemetry, but not
   branching). Do NOT remove the field from `DropIndexOp` itself (wire
   compat + HMAC canonical form both still reference it) — this is a
   server-side resolution fix, not a wire-shape change.

5. **Rust + TS builder doc updates** — `crates/shamir-query-builder/src/
   ddl/drop_index.rs`'s `.unique()` builder method and `crates/
   shamir-client-ts/src/core/builders/ddl.ts`'s `dropIndex(...,
   { unique })` option: update their doc comments to state plainly that
   `unique` is now optional/advisory (kept only for the HMAC canonical
   form's input, does not affect which index family actually gets
   dropped — the server resolves that from the catalog). Don't change
   their signatures/defaults (both already default `unique` to `false`
   when omitted) — this is a documentation-accuracy fix reflecting the
   new server behavior, not an API break.

## Tests

- **Mismatched-flag drop succeeds correctly** — create a UNIQUE index,
  then call DROP INDEX with `unique: false` (the default/wrong value);
  must succeed and drop the correct (unique) index, not no-op or error.
  Symmetric case: create a REGULAR index, drop with `unique: true`; must
  still resolve and drop the correct (regular) one.
- Same mismatched-flag proof through the **`if_exists = true` early-exit
  path** specifically (not just the main resolution branch) — a
  mismatched flag must not cause a false "existed: false" no-op.
- **`DdlOpKind` in the op-status log reflects the true family**, not the
  client's declared flag — poll the op status after a mismatched-flag
  drop (via #1015's `get_ddl_op_status`) and assert the logged kind
  matches the index's ACTUAL family.
- **Cross-family collision guard still correctly refuses** a genuine
  pre-existing collision (a name present in >1 family, reachable only on
  pre-#1010 data) — extend/confirm the existing collision test now
  exercises the true 4-way count, not the old 3-way-with-hint-gating.
- **HMAC gate tests unaffected** — run `shamir-server/tests/hmac_gate.rs`
  as part of your gate; if `canonical_drop_index`'s shape genuinely
  needs to change (only if your own investigation in the "HMAC wrinkle"
  section above concludes otherwise), STOP and report back with your
  reasoning instead of implementing a wire/security-format change
  unreviewed.

## Constraints

- Follow `CLAUDE.md`: `Result<T, E>` conventions, tests in `tests/`
  directories, imports at top of file, one-file-one-primary-export.
- This is a server-side resolution-robustness fix, not a wire-breaking
  change — `DropIndexOp`'s shape and the HMAC canonical form must stay
  byte-identical on the wire.
- Gate: `cargo fmt -p shamir-db -p shamir-engine -p shamir-query-builder
  -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `./scripts/test.sh -p shamir-db -p shamir-engine -p shamir-query-builder
  -p shamir-server -p shamir-client --full`. Use the wrapper, never raw
  `cargo test`/`cargo nextest run`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.
⛔ Do not create scratch files at the repo root.

## Definition of done

- [ ] Verified (or refuted, with a clear counter-argument) the HMAC
      self-consistency reading above before touching anything HMAC-related.
- [ ] `handle_drop_index`'s `if_exists` early-exit and main resolution
      both classify unconditionally across all 4 families, mirroring
      `rename_index`'s existing pattern.
- [ ] `matching_families` collision guard is a true 4-way check.
- [ ] `DdlOpKind` selection uses the resolved family, not `op.unique`.
- [ ] Mismatched-flag tests (both directions, both early-exit and main
      path) prove resolution no longer depends on the client's hint.
- [ ] Builder doc comments (Rust + TS) updated to reflect `unique` is
      now advisory/HMAC-input-only.
- [ ] fmt/clippy/test gates green, real output reported.
