# P34-12 - index identity v2: design

Read-only design for stage P34-12. Code read at upstream master `8793791a`
(checkout `worktrees/review-p34-12`). All paths are relative to the checkout
root. Cards: RV-INDEX-9E472725 (P2, strongest), RV-INDEX-53CEF405 (P2),
L3-RV-INDEX-605EEC5D (P3), L3-RV-INDEX-FBB72544 (P3). Target file for the
orchestrator copy: `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P34-12-index-identity-v2-design.md`.

## 0. Current state (observed)

Three families persist hash-only value identities in the per-table
`info_store`:

| Family | Key layout today | Identity | Code |
|---|---|---|---|
| base regular | `[0][name_interned u64 LE][h1 u64 LE][h2 u64 LE][rid 16]` (41 B), value empty | two FxHasher streams over tagged leaves | `crates/shamir-index/src/base_index/infra/index_keys.rs:224-286`, `index_record_key.rs:84-101`, `index_keys.rs:352-357` |
| base unique | `[1][name_interned LE][h1][h2]` (25 B), value = RecordId (16 B) | same | `index_manager_unique/validate.rs:310-350` |
| FTS (index2) | `[id u32 LE][tag 1][token_hash u64 LE][rid 16]`, value = bincode `FtsPostingValue` | `FxHasher::write(token)` | `fts/tokenizer.rs:429-435`, `backends/fts_ranked_backend.rs:136-178` |
| functional (index2) | `[id u32 LE][tag 2][hash 16 B][rid 16]`, value empty | `hash_value` = two chained FxHashers | `backends/functional_backend.rs:70-88,152-226` |

Defects confirmed in the code:

1. Unique enforcement is hash-only. `check_unique_key` returns the owner
   RecordId of whatever posting sits at the 25-byte key and every caller turns
   a hit into `DuplicateKey` without looking at the value
   (`validate.rs:107-117,310-350`). Both FxHasher streams consume the same
   leaves with the same multiplier, so a seed-independent collision family
   exists for two Int leaves (card 9E472725: `(0,0)` vs `(t, -t*K^2 mod 2^64)`
   under rustc-hash 2.1.2's `H <- (H + x) * K`). There is ONE physical slot per
   25-byte key, so even a value check could not admit both colliding values:
   verification without a new key layout only renames the error.
2. FTS: a token is one unkeyed u64; postings keep no token text
   (`fts_ranked_backend.rs:169-178`); the ranked scan does not even check the
   decoded value length (`fts_ranked_backend.rs:191-192`).
3. Functional Map hashing feeds entries in IndexMap insertion order and feeds
   interner keys as variable-width LE bytes with no length frame
   (`functional_backend.rs:182-190`); Set hashing XORs element hashes, so two
   numerically-equal elements of different types cancel
   (`functional_backend.rs:213-224`, same XOR in `index_keys.rs:187-209`).
4. Portability: every identity goes through `std::hash::Hash` impls and
   rustc-hash, whose multiplier, rotation and `usize` state depend on pointer
   width; slice/`Box<[u8]>` hashing writes a `usize` length prefix
   (`NumericKey::hash_tagged`, `crates/shamir-types/src/types/basic/numeric_key.rs:154-179`).
   The dependency is caret-ranged (`crates/shamir-index/Cargo.toml:36`); the
   only guard is 64-bit golden tests.
5. Versioning: base family has `_m.idx.lfv` (`model/meta/persistence.rs:32-47,316-354`)
   and the rebuild-on-open gate is `stored < CURRENT`
   (`persistence.rs:349-354`, called at
   `crates/shamir-engine/src/table/table_manager/admission/domain/barrier/write_domain_barrier.rs:519-534`).
   A NEWER stamp is accepted silently. index2 (FTS/functional) has no identity
   version at all.
6. Results are trusted without re-filtering: index2 candidates are returned as
   the authoritative answer (`crates/shamir-engine/src/table/read/execution/dispatch.rs:509-545`,
   `postprocess_candidates` passes `filter_cb = None` at
   `table/read/dispatch/vector.rs:804`), and a base Eq lookup without residual
   (including the count fast path) is authoritative too (`dispatch.rs:549-620`).
   So any identity collision is a wrong answer, not a slow path.

## (a) Identity scheme

### Decision: one canonical codec for all three families ("IDV2")

One scheme serves all three families. Identity = canonical bytes `C(v)` of the
value's equality class; the posting key carries `C(v)` itself when it is short
and a BLAKE3 digest of it when it is long, and in the digest case the posting
VALUE carries `C(v)` so equality is always decided on canonical bytes, never
on a hash alone. Per-family differences are only the key namespace / type tag
and the posting value payload.

Not reused: the funclib canonical stream
(`crates/shamir-funclib/src/functions/value/canonical/core.rs:1-21,99-127`)
encodes `Int` and `F64` with different tags and `Dec`/`Big` as `Str`, which
contradicts the index equality rule (Int(1) == F64(1.0) == Dec(1) == Big(1)).
Its conventions (explicit tags, fixed big-endian widths, sorted maps/sets,
frozen format) are followed.

### Canonical bytes `C` (frozen; any change is an identity-version bump)

Leaf codes. Multi-byte integers are big-endian; lengths and counts are `u32`
BE; no `usize`, no `Hash` impl, no `mem::discriminant`. The equality classes
are exactly today's (the ONE numeric rule of `numeric_key.rs`), so v2 changes
no equality decision except the defects listed in section 0.

| Tag | Class | Payload |
|---|---|---|
| `0x00` | Null | - |
| `0x01` | Bool | 1 byte `0x00`/`0x01` |
| `0x02` | integer (`NumericKey::Int`: Int, integral F64 in i64 range, -0.0, Big/Dec fitting i64) | i64 BE (8) |
| `0x03` | big integer (`NumericKey::BigBits`) | sign byte (`0x00` non-negative, `0x01` negative) + u32 len + minimal BE magnitude |
| `0x04` | float bits (`NumericKey::F64Bits`: non-integral finite, +-inf, nested NaN) | u64 BE bits |
| `0x05` | exact decimal (non-integral Dec, `decimal_exact_parts`) | i128 BE mantissa (16) + u32 BE scale |
| `0x06` | Str | u32 len + UTF-8 |
| `0x07` | Bin | u32 len + bytes |
| `0x08` | List | u32 count + element codes in order |
| `0x09` | Set | u32 count + element codes sorted bytewise ascending, duplicates (equal codes) removed |
| `0x0A` | Map | u32 count + entries sorted by key id ascending: u64 BE interner key id + value code |

- Top-level NaN keeps today's rule: no posting, a NaN probe matches nothing
  (`index_keys.rs:244-258`, `functional_backend.rs:146-148,251-253`).
- Composite (multi-path) base key: concatenation of the leaf codes in index
  path order; absent path = no key (unchanged, `index_keys.rs:221-262`).
- The write path (`ScalarRef`, zero-copy) and the lookup path (`InnerValue`)
  call the same encoder per class; a parity test pins it.
- `C` is self-delimiting (every variable part is length- or count-framed), so
  no codeword of a given arity is a proper prefix of another. Prefix scans on
  `[prefix][I]` are therefore exact (needed by `lookup.rs:477-487` and the
  index2 prefix scans).
- Encoding a length or count above `u32::MAX` returns a typed error (cannot
  occur under the 16 MiB frame budget; never truncates).

### Identity bytes `I` (what goes into the key)

- `|C| <= 256` bytes: `I = C` (inline form, no hash at all).
- `|C| > 256`: `I = 0xFF || BLAKE3-256(derive_key context "shamir-db index identity v2 posting digest", C)`
  (digest form, 33 bytes). `0xFF` is never a leaf tag, so inline and digest
  identities are disjoint and both self-delimiting.
- `INLINE_MAX = 256` is part of the frozen format. Rationale: inline total
  bytes are always smaller than digest form (digest form stores 33 B in the key
  plus `C` in the value), so the cap exists only to bound key length (LSM key
  comparisons, the posting-cache charge); 256 B keeps typical strings, all
  composite numeric tuples up to 28 leaves and almost all FTS tokens inline.

Example vectors (also the first golden vectors):
- `Int(1)` = `F64(1.0)` = `Dec(1)` = `Big(1)`: `02 00 00 00 00 00 00 00 01`
- `Str("run")`: `06 00 00 00 03 72 75 6E`
- composite `(Int 0, Int 0)`: `02 00x8 02 00x8` (18 B); the 9E472725 witness
  `(1, -(K*K))` encodes to different bytes by construction.
- `Map{k5: 1, k7: 2}` in either insertion order:
  `0A 00 00 00 02 | 00 00 00 00 00 00 00 05 02 00 00 00 00 00 00 00 01 | 00 00 00 00 00 00 00 07 02 00 00 00 00 00 00 00 02`

### Per-family layouts (v2)

| Family | v2 key | v2 value |
|---|---|---|
| base regular | `[0x02][name_interned u64 LE][I][rid 16]` | empty (inline) / `C` (digest) |
| base unique | `[0x03][name_interned u64 LE][I]` | RecordId 16 B (inline) / RecordId 16 B + `C` (digest) |
| FTS | `[id u32 LE][0x11][I(Str token)][rid 16]` | `tf u32 LE, doc_len u32 LE` (8 B, byte-identical to today's bincode) + `C` when digest |
| functional | `[id u32 LE][0x12][I(value)][rid 16]` | empty / `C` when digest |

The existing LE prefix fields (`name_interned`, `index_id`) stay as they are:
they are already explicit-endian. New tags/namespaces (`0x02`/`0x03` for base,
`0x11`/`0x12` in `posting_layout::type_tag`, today `0..=3` at
`model/query/posting_layout.rs:11-16`) put v2 postings in keyspaces that no v1
reader scans, which is what makes the migration crash-safe and lets old and
new postings coexist during it.

### Why it is stable

- Endianness: every field is written with an explicit `to_be_bytes`/`to_le_bytes`.
- 32/64-bit: no `usize` is written; no `Hasher` is involved in the inline form.
- Toolchain: no `std::hash::Hash` impl is used (their byte feed, such as the
  `str` terminator or slice length prefix, is an implementation detail).
- Dependency versions: the inline form depends on no crate. The digest form
  depends on the BLAKE3 specification (fixed output, official test vectors),
  not on crate internals; the golden vectors fail CI on any drift. rustc-hash
  leaves the persisted path completely, so the "do not pin rustc-hash" P2
  decision becomes moot instead of being a risk.
- Remaining semantic inputs, each guarded by golden vectors: `NumericKey`
  classes (`numeric_key.rs:39-147`), `decimal_exact_parts` normalization,
  num-bigint magnitude bytes, interner key ids for Map keys. Interner ids are
  database-local, as today: identities are stable for a database, not across
  databases.

### Collision handling and how DuplicateKey is decided

- Inline form: the key IS the canonical encoding, and the encoding is
  injective on equality classes (tagged, fixed-width or framed fields parse
  uniquely), so equal keys mean equal values. No hash participates.
- Digest form: the key is only a locator. Every reader compares the posting's
  `C` tail with the probe's `C`:
  - regular / functional / FTS lookups drop postings whose tail differs (two
    different long values sharing a digest each keep their own postings,
    distinguished by the trailing rid, so even a BLAKE3 collision returns
    exact results);
  - unique: see below.
- `DuplicateKey` is raised iff a posting exists at the exact v2 unique key AND
  (inline form, or digest form with tail equal to the probe's `C`) AND the
  existing owner is not the record itself / not released-and-touched in the
  same tx (the #1096 rules in `validate.rs:155-308` are unchanged).
- Digest form with a different tail = BLAKE3-256 collision of two distinct
  values: the write fails closed with `DbError::Internal("unique index identity
  digest collision ...")` plus `log::error!`; never `DuplicateKey`, never an
  overwrite. Same rule at commit time where `prelock.rs` would see two owners
  for one key (it already aborts with `UniqueViolation`; acceptable because it
  needs a BLAKE3 collision).
- "Unchanged key" short-circuits on UPDATE (`validate.rs:218-222,281-285`,
  `functional_backend.rs:300-302`) compare `(key, tail)`, not the key alone.
- Batch-local duplicate sets in the engine (`table_manager_crud.rs:353-377`,
  `tx_insert.rs:233-265` and its twin near `tx_insert.rs:471-490`) are keyed by
  `(name_interned, C)` instead of key bytes.
- Corruption policy (P0-4, `validate.rs:316-345`) is kept and extended: an
  inline unique posting must be exactly 16 B, a digest one must be
  `16 + |C|` with `|C| > 256`; anything else is `DbError::Codec`.

Rejected alternatives: keyed/seeded 64- or 128-bit hashes (still hash-only
proof, seed management); "verify the value on every unique hit" without a new
layout (P2 decision's first step: impossible to admit both values in one
25-byte slot, so it only renames the error; skipped as a stage); digest-only
keys (bigger postings for every short value and a BLAKE3 call per FTS token).

## (b) Persisted identity version

Recorded at three levels:

1. Per posting key: namespace byte (base `0x02`/`0x03` vs `0x00`/`0x01`) and
   index2 type tag (`0x11`/`0x12` vs `0x01`/`0x02`); inside `I` the `0xFF`
   marker distinguishes digest form.
2. Per table, base family: new system key `system:_m.idx.idv` = `u32 LE 2`
   (`"_m.idx.idv"` is 10 bytes, no truncation, and diverges from `.lfv`,
   `.drop`, `.ren` at byte 7, same check as `persistence.rs:615-626`). Rule:
   the base family is current iff `idv == 2` AND `_m.idx.lfv` is ABSENT. New
   binaries never write `_m.idx.lfv` again.
3. Per table, index2 family (per backend through its descriptor): the
   `_m.idx` `MetaEnvelope` is written with envelope version 2 iff the registry
   holds at least one FTS or functional descriptor; otherwise it stays version
   1 (vector-only tables are unaffected). Envelope v2 means every FTS /
   functional descriptor in it uses IDV2 postings. `MetaEnvelope::open` stays
   strict (version 1 only) for every other payload; a new
   `open_accepting(&[1, 2]) -> (version, T)` and `new_with_version` are used
   only by `persistence.rs` (`model/meta/meta_envelope.rs:62-79`).

Why not the P2-suggested `LEGACY_INDEX_FORMAT_VERSION 3 -> 4`: an old binary
reads `4`, finds `4 < 3` false and skips the rebuild (`persistence.rs:349-354`),
then hashes v3 keys against v4 postings: silent misses and silent duplicates.
That bump is fail-OPEN for every binary already shipped.

### Old binary opens NEW data

- Base family: `_m.idx.lfv` is absent, so the old S9b gate reads version 0 and
  runs its own `repair()` (`write_domain_barrier.rs:519-534`): drops and
  rebuilds every base index in the v3 keyspace from the data store and stamps
  `lfv = 3`. Result: correct v3 behaviour (fail-safe). The v2 postings
  (namespaces 2/3) become unread garbage. Exception: if v2 admitted two tuples
  that collide under v3 (the 9E472725 family), the old unique rebuild returns
  `UniqueIndexCreationFailed` and the old binary cannot open the table
  (fail-closed).
- index2 family: the old binary's `MetaEnvelope::open` reads the header first
  and returns `UnsupportedVersion(2)` (`meta_envelope.rs:73-75`);
  `decode_persisted_indexes` maps it to `DbError::Internal`
  (`persistence.rs:243`), `load_index2_metadata` fails and
  `TableManager::create` fails (`write_domain_barrier.rs:292-293`). The old
  binary refuses the table (fail-closed). index2 is loaded before the base
  rebuild in the old open path, so a table with both families is refused
  before any base rebuild runs.

### New binary opens OLD data

- Base family: `idv` absent or `lfv` present (written by any pre-v2 binary,
  including one that touched the table after an upgrade) -> base migration.
- index2: envelope version 1 with FTS/functional descriptors -> index2
  migration.
- Newer-than-me: `idv > 2` or envelope version > 2 -> typed open error
  (fail-closed). This is the forward gate the current code lacks.

## (c) Migration

Policy (recommended, see OQ-3): automatic, blocking, open-time rebuild from
the data store, reusing the two existing mechanisms: S9b `repair_admitted` for
the base family and the F-50 Step 3b Building self-heal for index2. Postings
are never rewritten in place (a v3 hash cannot be inverted to a value).

### Base family protocol (in `TableManager::create`, replacing `write_domain_barrier.rs:510-534`)

`need = !(idv == 2 && lfv absent)`. If `need`:

1. One atomic `info_store.transact([Remove lfv, Remove idv])`. From here every
   old binary will rebuild (lfv absent) and every new binary will migrate
   (idv absent).
2. `repair_admitted(open_permit)` (`table/table_manager/diagnostics/doctor.rs:563-668`)
   with `drop_index` / `drop_unique_index` sweeping BOTH namespaces of the
   index (`[0|2][name]` regular, `[1|3][name]` unique; today a single prefix at
   `base_index/index_manager/flags.rs:529-532`), then recreating in v2.
3. `set idv = 2`. Commit point.

Crash safety: crash after 1 or during 2 leaves `idv` absent -> the next open
repeats 2 from scratch (drops are idempotent and cover both namespaces). Crash
after 3 is complete. An old binary between 1 and 3 rebuilds in v3 and stamps
lfv, which forces the next new open to migrate again. The ordering matters:
deleting `lfv` first closes the window in which an old binary would trust a
stale `lfv = 3` over v2 postings.

### index2 protocol (in the F-50 block, `write_domain_barrier.rs:272-410`)

If the loaded envelope is version 1 and holds FTS/functional descriptors:

1. Write `_m.idx` once with envelope version 2 and every FTS/functional
   descriptor in state `Building` (vector descriptors untouched). Commit point
   for "this table is v2"; from now on old binaries refuse it.
2. The existing self-heal runs for those descriptors: `drop_all` sweeps the
   whole `[id]` prefix, i.e. v1 and any partial v2 postings
   (`fts_ranked_backend.rs:499-514`, `functional_backend.rs:377-387`),
   backfills in v2 (`table_manager_index_mgmt.rs:178-193`), flips to `Ready`
   and re-persists. `restore_on_open` is already skipped for recovered ids, so
   FTS stats are not double-counted (`write_domain_barrier.rs:471-480`).

Crash safety: crash before 1 = still v1 everywhere, retried next open. Crash
after 1 = v2 envelope with `Building` descriptors = the existing restart-from-
scratch path, retried on every open until it succeeds.

Failure rule: the self-heal currently propagates any backfill error except
`TokenBudgetExceeded` and fails the table open (`write_domain_barrier.rs:355-373`).
A functional index over a user `.trusted_pure()` scalar cannot be backfilled
at open because the per-DB resolver is installed later by
`set_scalar_resolver` (`table/table_manager/accessors/table_manager_validators.rs:116-129`;
open uses `builtins_only`, `write_domain_barrier.rs:219-221`). For
identity-migration descriptors only, every non-storage backfill error marks
the backend `Failed` with an actionable reason (register the scalar, then
`TableManager::repair()`, which already heals `Building`/`Failed` index2 at
`doctor.rs:716-770`) instead of failing the table open. The index is
planner-invisible meanwhile; FTS/functional filters fall back to the full scan,
which evaluates the same predicate, so results stay correct.

### Pre-upgrade WAL entries

WAL replay applies `IndexPut`/`IndexDel` with literal keys
(`crates/shamir-engine/src/tx/recovery/v2.rs:181-300`) and runs after the
tables are reachable (`repo/instance/repo_instance/lifecycle.rs:247`,
`lifecycle/durability_repo_impl.rs:77`). A crash of an old binary followed by
the first new open would replay v3/v1 keys AFTER the open-time migration,
leaving those records missing from the v2 index. Rule: the open-time migration
sets a per-table `identity_migrated_this_open` flag; replay records every table
that received an index op (broadcast ops: all tables); after the replay loop,
every table with both marks re-runs the identity migration (base: steps 1-3;
index2: flip descriptors to `Building` + self-heal). The data ops have been
replayed by then, so the rebuild is complete. This also closes the same latent
hole for the S9b v2->v3 bump. No WAL format change.

### Cost per index size (N records, P old postings, T total tokens)

- Base regular: one streaming scan of N records per index, N posting writes,
  sweep of P old postings (`doctor.rs:597-646`).
- Base unique: the existing repair materializes all N current records in
  memory once for all unique definitions (`doctor.rs:653`), then N writes per
  index. Memory is O(N record values) - the existing repair cost.
- FTS: tokenization of N records, one posting per distinct (token, record), up
  to T writes; `drop_all` deletes P postings one `remove` per key (could use
  `remove_many` like `persistence.rs:720-737`; implementation choice).
- Functional: N expression evaluations, N writes.
- Key size: inline base keys grow from 25/41 B to `9 + |C| (+16)` with
  `|C| <= 256`; FTS keys go from `4+1+8+16` to `4+1+(5+len)+16`; digest-form
  postings store `C` once in the value. Total bytes per posting never exceed
  one copy of `C` plus 33 B.

### Behaviour during the rebuild

It runs inside `TableManager::create` before the table is returned: no read or
write of that table can observe a half-migrated index (planner invisibility is
trivial). The open holds the repo write-domain admission permit, so writes to
the repo stall for the duration (same as today's S9b repair and F-50
self-heal). Outside the open path, index2 `Failed` backends are
planner-invisible and served by the full scan.

### Rollback

- Base: automatic. An old binary self-heals to v3 (above). Exception:
  v3-colliding unique tuples admitted under v2 make the old unique rebuild
  fail -> table refused by the old binary; remedy = delete one of the tuples
  with the new binary, or restore a backup.
- index2: the new binary writes envelope version 1 again as soon as no FTS or
  functional descriptor remains. Downgrade = `DROP INDEX` of the FTS/functional
  indexes with the new binary, downgrade, `CREATE INDEX` with the old binary;
  or restore a pre-upgrade backup. No automatic path (OQ-1).

## (d) Format breaks without a safe migration

- No step lacks a FORWARD migration: every old format is migrated
  automatically on first open.
- BACKWARD (owner decisions):
  1. index2 (stage S3): after the first new open, tables with FTS/functional
     indexes are refused by every older binary until those indexes are dropped
     with the new binary (OQ-1). This is deliberate: the only alternative an
     old binary allows is silent wrong FTS/functional results.
  2. Base (stage S2): backward-safe through the old binary's own rebuild,
     except the v3-colliding unique tuples case (fail-closed in the old binary).
- Rejected because it is unsafe: the `LEGACY_INDEX_FORMAT_VERSION 3 -> 4` bump
  (fail-open, section b).
- Not affected: WAL format, `MetaEnvelope` header layout, record format,
  sorted indexes (no hashing), vector indexes, the `_m.idx` payload shape
  (`PersistedIndexes`/`IndexDescriptor` are unchanged; only the envelope
  version number changes). `grep` finds no WAL index-op application in
  `crates/shamir-server/src`.
- Rust API (not persisted, workspace-internal): `IndexQuery::Fts { tokens }`
  changes from `Vec<u64>` to identity byte strings
  (`backends/backend.rs:68-74`), `IndexBackend::tokenize_query`
  (`backend.rs:329`), `FunctionalBackend::hash_value` (used by the engine
  planner, `table/read/planning/read_planner.rs:136-141`), `IndexRecordKey`
  public fields `hash1`/`hash2`.

## (e) Staged plan

Order: S1 -> S2 -> S3 -> S4. S2 and S3 are independent of each other (separate
families, separate markers); S2 goes first because 9E472725 is the strongest
risk. P34-12 starts after P34-07 is merged (shared `fts/tokenizer.rs`,
`model/meta/persistence.rs`). Layout: `src/` of shamir-index is at 7 entries,
`tests/backends/`, `base_index/infra/`, `tests/model/`, `tests/runtime/` and
`index_manager_tests/data/` are at 7; new code goes to `src/model/identity/`
(model/ has 6) and new tests to `src/tests/identity/` (tests/ has 5) and
`index_manager_tests/unique/` (3).

### S1 - IDV2 codec and envelope versioning (no wiring)

- Files: `crates/shamir-index/Cargo.toml` (blake3, OQ-2);
  `src/model/mod.rs`; new `src/model/identity/{mod.rs, canonical.rs, identity_bytes.rs, markers.rs}`
  (encoder for `InnerValue` and `ScalarRef`, inline/digest split, tail
  verification helper, unique-posting value encode/decode incl.
  `unique_posting_owner(&[u8]) -> Option<RecordId>`, marker constants);
  `src/model/meta/meta_envelope.rs` (`new_with_version`, `open_accepting`);
  new `src/tests/identity/{mod.rs, codec_golden_tests.rs, codec_parity_tests.rs}`.
  Items are `pub` (crate API) so nothing is dead code.
- Tests: `p34_12_codec_golden_vectors` (literal hex, no `target_pointer_width`
  gate: they must hold on every target); `p34_12_codec_map_reversed_insertion_parity`;
  `p34_12_codec_set_order_parity_and_dedup`; `p34_12_codec_numeric_class_parity`
  (Int/F64/Big/Dec equal, `F64(0.5) != Dec(0.5)`, `-0.0 == 0`,
  `F64(2^63) == Big(2^63)`); `p34_12_codec_prefix_free` (deterministic
  generated corpus, no new dev-dependency); `p34_12_codec_scalar_ref_inner_value_parity`;
  `p34_12_identity_inline_digest_boundary` (256 inline, 257 digest, digest
  golden); `p34_12_codec_9e472725_witness_distinct`;
  `p34_12_envelope_open_accepting` and `p34_12_envelope_strict_open_rejects_v2`
  (the old-binary code path returns `UnsupportedVersion(2)`).
- Compat: additive (new API, new dependency). No persisted byte changes.
- Effort: M.

### S2 - base regular + unique on IDV2, with base migration (closes 9E472725; 605EEC5D for the base family)

- shamir-index files: `base_index/infra/index_keys.rs` (rewrite on the codec,
  FxHasher removed), `base_index/infra/index_record_key.rs` (namespace byte,
  identity bytes, value tail), `base_index/index_manager_unique/validate.rs`
  (decision rule of section a), `base_index/index_manager/lookup.rs` (exact
  `len == prefix + 16`, tail check for digest form), `index_manager/create.rs`
  and `index_manager/flags.rs` (posting values; sweep both namespaces),
  `model/meta/persistence.rs` (`idv` load/save, lfv removal, need rule; S9b
  helpers kept for the old-data check).
- shamir-engine files: new `table/table_manager/admission/domain/barrier/identity_migration.rs`
  (base protocol; folder has 3 entries); `write_domain_barrier.rs` (call it
  instead of the S9b block); `tx/recovery/v2.rs` (post-replay re-run);
  `tx/preparation/pre_commit/prelock.rs` (owner decode via
  `unique_posting_owner` at lines 515, 578, 677, 699);
  `table/table_manager/operations/table_manager_crud.rs` and
  `table/table_manager/transactions/tx_insert.rs` (batch sets keyed by `C`);
  `table/table_manager/admission/domain/index_admission/write_domain_online_index.rs:196-201`
  (posting value with tail).
- Tests (red on old code where marked):
  - RED `rv_index_9e472725_composite_int_unique_no_false_duplicate`: unique
    index on `(a, b)` Int; insert `(0, 0)` then `(1, (0u64.wrapping_sub(K.wrapping_mul(K))) as i64)`
    with K = the 64-bit rustc-hash 2.1.2 multiplier copied as a literal from
    the pinned source; old code: `DuplicateKey`; new: both rows, both found by
    lookup. Index-level and engine-level (non-tx `insert_many`, tx insert +
    commit).
  - `..._true_duplicate_still_rejected` (`(1, 2)` vs `(F64 1.0, Dec 2)` ->
    `DuplicateKey`); long-string (digest form) equal -> `DuplicateKey`,
    distinct -> Ok; injected digest posting with a foreign tail -> `Internal`
    collision error, not `DuplicateKey`.
  - regular lookup exactness: `"a"`/`"ab"`, inline and digest values, count
    fast path equals scan count.
  - golden: `rv_index_605eec5d_golden_regular_unique_keys` rewritten in the
    same change to pin v2 bytes (the P2 decision requires updating goldens with
    the bump; the test is kept, not deleted).
  - migration: seed literal v3 postings (bytes taken from the current golden
    test) + `lfv = 3`, open -> v2 keys present, v3 keys gone, `lfv` absent,
    `idv = 2`, lookups correct; old-binary simulation (after migration write
    `lfv = 3` + stale v3 postings) -> reopen re-migrates; crash points via a
    failing store after step 1, during step 2, before step 3 -> reopen
    converges; `idv = 3` -> open fails closed; replay of a pre-upgrade v3
    `IndexPut` after migration -> record found (red against a migration
    without the post-replay rule); the existing `s9b_rebuild_on_open_tests`
    rewritten for the new marker semantics.
- Compat: breaking persisted format for base indexes, with automatic forward
  migration; old binaries self-heal (section b).
- Effort: L.

### S3 - FTS + functional on IDV2, with index2 migration (closes 53CEF405, FBB72544; 605EEC5D for index2)

- shamir-index files: `fts/tokenizer.rs` (`token_identity` replaces
  `token_hash`), `backends/fts_ranked_backend.rs` and `backends/fts_backend.rs`
  (freq map keyed by token string, encoded once per distinct token; exact
  value-bytes check on scan; value tail), `backends/functional_backend.rs`
  (codec instead of `hash_inner`/`hash_value`), `backends/backend.rs`
  (`IndexQuery::Fts` tokens, `tokenize_query`), `model/query/posting_layout.rs`
  (`FTS_V2 = 0x11`, `FUNCTIONAL_V2 = 0x12`), `model/meta/persistence.rs`
  (envelope v2 rule on save; `open_accepting` on load, version returned).
- shamir-engine files: `identity_migration.rs` (index2 flip),
  `write_domain_barrier.rs` (failure rule for migration descriptors),
  `table/read/planning/read_planner.rs:65-80,136-141`, `tx/recovery/v2.rs`
  (index2 part of the re-run).
- Tests:
  - RED `rv_index_fbb72544_functional_map_reversed_insertion_hits_same_posting`
    (functional index over a Map-valued expression; insert with keys a,b;
    lookup with a Map built b,a; old code: miss).
  - `rv_index_53cef405_fts_token_identity_exact`: `run`/`running` never share
    postings; a long token (digest form) round-trips; a forged posting with the
    right digest and a foreign tail is not returned.
  - golden: `rv_index_605eec5d_golden_token_hash`, `..._golden_fts_posting`,
    `..._golden_functional_postings` rewritten to v2 bytes in the same change.
  - migration: seed a v1 envelope + literal v1 FTS/functional postings, open ->
    envelope v2, descriptors `Ready`, postings only under `0x11`/`0x12`, ranked
    results and BM25 stats as before (no double count); crash after the flip
    -> reopen completes; v2 blob through strict `MetaEnvelope::open` ->
    `UnsupportedVersion(2)`; envelope back to v1 after dropping the last FTS
    index; vector-only table stays v1; user-scalar functional index -> table
    opens, index `Failed`, `set_scalar_resolver` + `repair()` -> `Ready` and
    correct lookups; replayed v1 FTS `IndexPut` after the flip -> complete
    results.
- Compat: breaking persisted format for FTS/functional indexes, automatic
  forward migration, old binaries refuse (OQ-1).
- Effort: L.

### S4 - documentation

- Files: `docs/guide-docs/KNOWN_LIMITATIONS.md:417-426` (replace the accepted
  risk with: identity v2, downgrade rule for FTS/functional, user-scalar
  functional indexes `Failed` until `repair()` after upgrade, BLAKE3 residual),
  module docs in `index_keys.rs:6-48` and `persistence.rs:4-37` (version
  history), `functional_backend.rs` doc on the persisted format.
- Tests: none (documentation only).
- Compat: compatible. Effort: S.

## (f) Not done, residual risk

- Not done: vector and sorted indexes (no hash identity); an automatic
  downgrade tool for index2 (OQ-1); online/background migration (OQ-3);
  automatic re-backfill of user-scalar functional indexes when the resolver
  arrives (they stay `Failed` until `repair()`); removing
  `IndexRecordKey::with_values` and its tests (deprecated, test-only, not
  persisted) - so `rustc-hash` stays a dependency of shamir-index, outside
  every persisted path; replication followers (no index-op application found
  in shamir-server).
- Residual: a BLAKE3-256 collision on a long unique value fails that write
  closed (needs about 2^128 work); never a false `DuplicateKey`.
- Residual: identities depend on interner ids, so copying an `info_store`
  between databases without its interner still requires a rebuild (as today).
- Residual (pre-existing, not changed in class): base keys `[ns][name_interned LE]`
  and index2 keys `[id u32 LE]` share one `info_store`; an index2 prefix
  `[id LE]` also matches base keys whose namespace byte equals the low id byte
  and whose `name_interned` has zero low 24 bits (index2 ids start at 1,
  `model/runtime/registry.rs:172`). The new namespace bytes 2/3 add the same
  condition for index2 ids 2/3 that 0/1 already have for ids 0/1.
- Residual: storage per posting grows for long inline values (up to 256 B of
  key); the open-time migration blocks repo writes for its duration and the
  unique rebuild holds all records in memory (existing repair behaviour).
- Residual: a set mixing numerically equal elements of different numeric types
  is canonicalized with duplicates removed; the codec test pins this choice.

## Owner decisions (2026-10-30)

Decided by the design decider (@oh) on its own recommendations, as directed by the owner on 2026-10-30 (technical and housekeeping questions are decided, not asked). The owner can override any of them before the stage that depends on it starts.

- **OQ-1 (S3 refuses on old binaries):** accepted. Tables with FTS or functional indexes written by identity v2 are refused by older binaries (`UnsupportedVersion(2)`) instead of returning silent wrong results. Downgrade is `DROP INDEX` of those indexes on the new binary or a pre-upgrade backup. No downgrade command.
- **OQ-2 (digest dependency):** `blake3` with the `pure` feature, the same version as `shamir-funclib`, as a direct dependency of `shamir-index`.
- **OQ-3 (rebuild policy):** automatic blocking open-time rebuild, reusing the S9b repair and the F-50 Building self-heal.

Stages S1-S4 above are executed as four sequential executor runs after P34-07 lands; each gets its own brief under `docs/dev-artifacts/prompts/remediation/p34/12a-...` to `12d-...` derived from sections (a)-(e).
