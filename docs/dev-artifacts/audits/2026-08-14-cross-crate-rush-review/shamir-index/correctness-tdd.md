<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-index — correctness-tdd independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Unsupported-leaf hashing, Unicode folding, replay acceptance, stats accounting, cache publication, and staged dimensions remain concrete defects. Existing tests miss the discriminating boundaries. Dot counterexamples violate the public normalization contract.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 14 | 11 | 0 | 0 | 1 | 0 | 2 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — FunctionalBackend hash collapses every Dec/Big/Bin value to an identical posting hash

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

A Field expression over any unsupported leaf hashes only 255 in both passes. Different values share postings and direct lookup returns all matching hashes; engine treats index2 answers as complete. Current functional tests use string/int-derived paths. The lower(decimal) example is invalid because Lower type-errors to Null.

Evidence: [crates/shamir-index/src/functional_backend.rs:138](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/functional_backend.rs#L138); [crates/shamir-index/src/functional_backend.rs:173](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/functional_backend.rs#L173); [crates/shamir-engine/src/table/read_exec.rs:462](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/read_exec.rs#L462); [crates/shamir-index/src/tests/functional_backend_tests.rs:73](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/tests/functional_backend_tests.rs#L73).

<a id="review-2"></a>

### Claim 2 — Whitespace/Full tokenizers never case-fold words whose uppercase letters are all non-ASCII (Russian/Greek FTS broken)

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Москва contains no ASCII alphabetic uppercase bytes and remains borrowed, unlike москва. Full uses the same predicate before stemming/stopword filtering. Lowercase Russian tests exist but do not compare affected uppercase/lowercase inputs.

Evidence: [crates/shamir-index/src/tokenizer.rs:55](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/tokenizer.rs#L55); [crates/shamir-index/src/tokenizer.rs:277](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/tokenizer.rs#L277); [crates/shamir-index/src/tests/tokenizer_tests.rs:70](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/tests/tokenizer_tests.rs#L70).

<a id="review-3"></a>

### Claim 3 — Vector delta-replay failure is warned away, then permanently baked in by the next background snapshot

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

A decode/read/upsert failure stops replay, but restore returns Ok with partial graph state. A subsequent snapshot can prune its missing recovery record. The oracle needs the failing delta path, exact recovered membership, then another flip/restart; chunk-corruption tests exercise a different dispatch.

Evidence: [crates/shamir-index/src/vector/vector_backend.rs:683](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/vector_backend.rs#L683); [crates/shamir-index/src/vector/snapshot.rs:1251](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/snapshot.rs#L1251); [crates/shamir-index/src/vector/snapshot.rs:1314](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/snapshot.rs#L1314).

<a id="review-4"></a>

### Claim 4 — `FtsRankedBackend::plan_update` emits unguarded BumpFtsStats on empty↔non-empty transitions — permanent doc_count/avg_doc_len drift

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

An empty document was never counted, yet update emits -old and +new unconditionally, so empty→nonempty fails to add a document and the reverse fails to remove one. Rebuild does not replace counters. The registered rebuild test explicitly zeroes them first, and no affected plan_update oracle exists.

Evidence: [crates/shamir-index/src/fts_ranked_backend.rs:223](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/fts_ranked_backend.rs#L223); [crates/shamir-index/src/fts_ranked_backend.rs:382](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/fts_ranked_backend.rs#L382); [crates/shamir-index/src/tests/fts_ranked_backend_tests.rs:215](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/tests/fts_ranked_backend_tests.rs#L215).

<a id="review-5"></a>

### Claim 5 — `lookup_by_index` posting-cache miss→scan→insert race can pin a stale entry past the writer's invalidation

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Reader completes an old store scan, writer applies and invalidates, then reader inserts the old slice. ReaderDrainGate only coordinates DROP. Existing hook parks before scanning, so it does not force the necessary post-scan/pre-publication boundary.

Evidence: [crates/shamir-index/src/base_index/index_manager.rs:2835](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/index_manager.rs#L2835); [crates/shamir-index/src/base_index/index_manager.rs:2847](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/index_manager.rs#L2847); [crates/shamir-index/src/base_index/index_manager.rs:2879](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/index_manager.rs#L2879); [crates/shamir-engine/src/tx/commit_phases.rs:769](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/commit_phases.rs#L769).

<a id="review-6"></a>

### Claim 6 — DROP INDEX of a vector index leaks the entire `__vec_snap__<id>` snapshot keyspace

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

drop_all is a no-op and recovery removes only the compact binary-id prefix, not __vec_snap__&lt;id&gt;. records. Ordinary IDs are monotonic, so resurrection is not a normal DROP consequence. Cleanup must also drain detached writers.

Evidence: [crates/shamir-index/src/vector/vector_backend.rs:739](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/vector_backend.rs#L739); [crates/shamir-index/src/persistence.rs:633](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/persistence.rs#L633).

Grouping/duplicate: [error-handling-lifecycle.md#3](error-handling-lifecycle.md#review-3). This is not an additional independent defect.

<a id="review-7"></a>

### Claim 7 — Staged vectors bypass dim validation on the in-tx merge paths (debug panic / silent truncation)

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Numeric extraction stages any length; HNSW and BruteForce merge slices without checks. Kernels assert in debug and use minimum length in release. Valid-dimension merge tests cannot discriminate this defect; stage a wrong-length row and query before commit.

Evidence: [crates/shamir-index/src/vector/vector_backend.rs:451](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/vector_backend.rs#L451); [crates/shamir-index/src/vector/hnsw_adapter.rs:2952](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/hnsw_adapter.rs#L2952); [crates/shamir-index/src/vector/brute_force.rs:309](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/brute_force.rs#L309); [crates/shamir-index/src/vector/simd.rs:105](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/simd.rs#L105).

<a id="review-8"></a>

### Claim 8 — `build_index2_backend` silently discards the persisted HNSW config (`m`, `ef_construct`)

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Builder and compaction build_config use fixed m/ef values. Valid graph snapshots can restore original parameters; fallback reconstruction cannot. Test a nondefault descriptor with no usable snapshot.

Evidence: [crates/shamir-index/src/build_backend.rs:52](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/build_backend.rs#L52); [crates/shamir-index/src/vector/hnsw_adapter.rs:952](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/hnsw_adapter.rs#L952).

Grouping/duplicate: [api-wire-protocol.md#1](api-wire-protocol.md#review-1). This is not an additional independent defect.

<a id="review-9"></a>

### Claim 9 — FunctionalBackend Map hashing is insertion-order dependent (byte-identity floor violation)

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Map entries feed the hash in IndexMap order. Equal maps built in opposite insertion order can miss the same functional lookup. Canonical ordering is safer than blindly copying XOR aggregation, which has its own collision properties; migrate persisted hashes.

Evidence: [crates/shamir-index/src/functional_backend.rs:164](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/functional_backend.rs#L164); [crates/shamir-collections/src/lib.rs:20](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/src/lib.rs#L20); [crates/shamir-index/src/base_index/index_keys.rs:161](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/index_keys.rs#L161).

<a id="review-10"></a>

### Claim 10 — `Dot` metric silently clamps distances to 0 for unnormalized vectors in HNSW (inconsistent with BruteForce)

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

The public search guide specifies prenormalized Dot vectors and implementation repeats that requirement. Raw magnitudes producing dot&gt;1 violate the supported precondition. HNSW's small-index branch also clamps, so the alleged size crossover is false. Validation may be added; automatic normalization would redefine raw inner product.

Evidence: [docs/guide-docs/guide/06-search.md:169](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/guide/06-search.md#L169); [crates/shamir-index/src/vector/hnsw_adapter.rs:153](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/hnsw_adapter.rs#L153); [crates/shamir-index/src/vector/hnsw_adapter.rs:2850](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/hnsw_adapter.rs#L2850).

<a id="review-11"></a>

### Claim 11 — `FtsStats`: torn (count, sum) reads and a divide-by-zero window in `avg_doc_len`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

A reader between count increment and length increment sees avg=0. With positive posting length, the term score becomes zero; zero-length combinations can produce NaN. This is not an integer division panic. A deterministic seam must sample between updates.

Evidence: [crates/shamir-index/src/bm25.rs:72](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/bm25.rs#L72); [crates/shamir-index/src/bm25.rs:80](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/bm25.rs#L80); [crates/shamir-index/src/bm25.rs:38](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/bm25.rs#L38).

<a id="review-12"></a>

### Claim 12 — Unbounded sorted-range upper bound `prefix \|\| 0xFF×64` excludes values with ≥64 leading 0xFF encoded bytes

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Bin encoding prepends 0x60 and every supported value tag is below 0xFF. The first compared value byte already orders below the sentinel, regardless of following FF payload length. This is pre-existing counter-evidence, not a later fix.

Evidence: [crates/shamir-types/src/core/sort_codec.rs:39](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/core/sort_codec.rs#L39); [crates/shamir-types/src/core/sort_codec.rs:135](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/core/sort_codec.rs#L135); [crates/shamir-index/src/base_index/sorted_index_manager.rs:2627](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/sorted_index_manager.rs#L2627).

<a id="review-13"></a>

### Claim 13 — `apply_index_ops_at_commit` silently drops any non-`BumpFtsStats` in-memory op

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

No such current variant exists. The three-variant enum routes the two postings to transact and BumpFtsStats to in-memory grouping. Exhaustive future routing is useful hardening, not a present lost operation.

Evidence: [crates/shamir-tx/src/index_write_op.rs:90](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/index_write_op.rs#L90); [crates/shamir-index/src/write_ops.rs:170](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/write_ops.rs#L170).

<a id="review-14"></a>

### Claim 14 — Inline `#[cfg(test)] mod tests` in `quant_meta.rs` violates the documented test layout

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The ordinary cfg(test) module remains inline and is reachable through vector::quant_meta. Its symmetric parameter roundtrip does not test malformed metadata or guarantee loader failure behavior.

Evidence: [crates/shamir-index/src/vector/quant_meta.rs:83](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/quant_meta.rs#L83); [crates/shamir-index/src/vector/mod.rs:7](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/mod.rs#L7).

Grouping/duplicate: [style-claude-md.md#2](style-claude-md.md#review-2). This is not an additional independent defect.

## Evidence and recipe corrections

- Change Claim 10 to not-applicable under the explicit normalized-input contract.
- Case-fold/hash fixes change persisted token/value identities and require rebuilding existing postings.
- The paired update bumps create persistent count drift but do not by themselves leave permanent u64::MAX after both operations finish.
- The existing lookup hook precedes the scan; it cannot force stale post-invalidation publication without another seam.
- Replacing replay warning with rebuild on the same partially restored adapter is insufficient for deleted/no-longer-vector rows.
- Graph CRC coverage must not be attributed to plain delta envelopes.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-index — Correctness & TDD-coverage

## Summary

The crate's infrastructure layers (reader-drain gates, lifecycle tombstones, bincode forward-compat, SQ8/HNSW concurrency) are exceptionally well-engineered and tested, but several value-level logic bugs survive in the less-trafficked paths: the functional-index hash collapses all Dec/Big/Bin values to one constant (guaranteed false positives), the Whitespace/Full tokenizers never case-fold words whose uppercase letters are all non-ASCII (breaking Russian/Greek FTS matching), and the vector snapshot delta-replay failure path permanently bakes lost mutations into the next snapshot. TDD discipline is strong overall (pause-hooks, drain-wait oracles, adversarial-race tests), yet the exact edges where the bugs live are the ones the suites skip — the ranked-FTS `plan_update` has zero tests, Cyrillic case-folding is only tested on the one tokenizer that handles it, and the functional-backend suite never leaves Str/Int.

## Findings

### 1. FunctionalBackend hash collapses every Dec/Big/Bin value to an identical posting hash
- **File:** `crates/shamir-index/src/functional_backend.rs:173` (`hash_inner`, `_ => h.write_u8(255)`)
- **Severity:** high
- **Issue:** `hash_inner` explicitly covers Null/Bool/Int/F64/Str/List/Map, and every other `InnerValue` variant — `Dec`, `Big`, `Bin` (all real variants of `InnerValue`, as `shamir-types/src/record_view/scalar_ref.rs` and the crate's own `rust_decimal`/`num-bigint` dev-deps confirm) — falls into the catch-all that writes ONLY the tag byte `0xFF` and no content. Both the h1 and h2 passes hash just that constant, so **every distinct Decimal/BigInt/binary value produces the same 128-bit posting hash** — a guaranteed collision by construction, not a probabilistic hash collision.
- **Failure scenario:** `CREATE INDEX ... FUNCTIONAL(lower(price))` or a plain `IndexExpr::Field` over a Decimal/BigInt/bytes column (eval returns the raw leaf; `eval_or_null` only collapses *errors* to Null). Insert 1,000 rows with 1,000 distinct decimals; a point query for `price = 10.5` scans the shared hash prefix and returns **all 1,000 rows** regardless of value. Update/delete likewise tombstone/remove the shared key.
- **Suggested fix:** Extend the tag scheme with content-hashing arms for `Dec`/`Big`/`Bin` (mirroring base_index `index_keys.rs`'s exhaustive 11-tag scheme, which already handles them via `hash_inner_value`), or fail closed (return `IndexError::TypeMismatch` at plan time / reject at index creation) for unsupported leaf types. Add a Red test: two distinct Dec/Big/Bin values must produce different posting keys.
- **TDD note:** `tests/functional_backend_tests.rs` exercises only Str/Int fields (`make_rec(email, age)`) — the hole is untested.

### 2. Whitespace/Full tokenizers never case-fold words whose uppercase letters are all non-ASCII (Russian/Greek FTS broken)
- **File:** `crates/shamir-index/src/tokenizer.rs:55-57` (WhitespaceTokenizer) and `tokenizer.rs:277-284` (FullTokenizer); the correct Unicode-aware check exists only in `lowercase_cow` (`tokenizer.rs:93-99`, used by UnicodeTokenizer)
- **Severity:** high
- **Issue:** The borrowed-vs-owned decision is `word.bytes().all(|b| b.is_ascii_lowercase() || !b.is_ascii_alphabetic())`. A word with **no ASCII letters** — e.g. `Москва`, `МОСКВА`, `ΣΟΦΟΣ` — satisfies this predicate (all bytes ≥ 0x80 are "not ASCII alphabetic") and is kept as `Cow::Borrowed` **without lowercasing**. The document is then indexed with the mixed/uppercase token, while a lowercase query (`москва`) tokenizes to a different token; `token_hash` differs → **no match ever**. In `FullTokenizer` this also feeds the uppercase word to the Snowball stemmer, which expects lowercase input, compounding the mismatch. Every properly-capitalized Russian word (sentence-initial words, all proper nouns) is affected.
- **Failure scenario:** FTS index with `TokenizerKind::Full{language: Russian, ..}`; doc body `"Москва — столица"`; query `"москва"` → 0 hits. Same for Whitespace tokenizer.
- **Suggested fix:** Replace the ASCII-only predicate with a Unicode-aware one (`word.chars().all(|c| !c.is_uppercase())`, exactly `lowercase_cow`'s logic) in both tokenizers. Red test: `FullTokenizer::new(Russian,…).tokenize("Москва")` must equal `tokenize("москва")`; same for `WhitespaceTokenizer`.
- **TDD note:** `tests/tokenizer_tests.rs:37-41` (`unicode_cyrillic`) tests Cyrillic lowercasing **only on UnicodeTokenizer** (the one tokenizer that handles it); all Whitespace/Full tests use ASCII. The suite is vacuous for this bug — classic green-test-sidesteps-the-red-case.

### 3. Vector delta-replay failure is warned away, then permanently baked in by the next background snapshot
- **File:** `crates/shamir-index/src/vector/vector_backend.rs:683-698` (`restore_on_open`); interacts with `snapshot.rs:1287-1330` (`flip_generation` pruning `0..delta_applied_upto` chunks)
- **Severity:** high
- **Issue:** On a successful snapshot load, `restore_on_open` replays delta chunks `>= manifest.delta_applied_upto`. If `replay_delta` errors (one corrupt/unreadable chunk), the code logs `warn!` and **continues serving the incomplete graph**. The comment claims the mutations are only "missing until the next snapshot" — but the next background snapshot (`run_background_snapshot`) dumps the **in-memory adapter** (still missing those mutations), sets `delta_applied_upto = next_delta_idx`, and `flip_generation` **prunes every absorbed delta chunk**. The committed mutations are now permanently absent from the index with no self-heal path (the data store still has the rows; the index silently disagrees).
- **Failure scenario:** One delta chunk gets a bad byte (the per-chunk crc32 exists precisely to catch this) → open loads base + partial replay → after `VECTOR_SNAPSHOT_DELTA_THRESHOLD` further mutations the flip prunes the unreadable chunk → affected rows vanish from all future vector queries, permanently.
- **Suggested fix:** On replay failure, fall back to the branch-3 semantics: `self.rebuild(data_store).await` (full re-derivation from the source of truth) instead of warn-and-continue. Add a Red test: corrupt one delta chunk, restart, assert `rebuild_count() == 1` and that post-snapshot rows are still findable after a forced snapshot flip.

### 4. `FtsRankedBackend::plan_update` emits unguarded BumpFtsStats on empty↔non-empty transitions — permanent doc_count/avg_doc_len drift
- **File:** `crates/shamir-index/src/fts_ranked_backend.rs:223-232` (contrast `plan_insert`'s guard at 164-166 and `plan_delete`'s guard at 252-258)
- **Severity:** medium
- **Issue:** `plan_insert` returns early when `doc_len == 0` (no Bump) and `plan_delete` wraps its Bump in `if doc_len > 0`, but `plan_update` **unconditionally** pushes `Bump{doc_len: old, sign: -1}` and `Bump{doc_len: new, sign: +1}`. For old-empty→new-non-empty the `-1` decrements a doc that was never counted (undercount, forever); for non-empty→empty the `+1` counts a doc that now has zero postings and can never match a query (overcount, forever). Both skew `doc_count` and `avg_doc_len` permanently; on a freshly-rebuilt backend where `doc_count == 0`, the spurious decrement wraps to `u64::MAX` (`on_delete`'s `fetch_sub`), making `avg_doc_len ≈ 0` and BM25 norms explode/NaN.
- **Failure scenario:** Table with an FTS index; update a row setting the indexed text field from `""` to `"hello world"`; `doc_count` stays at N instead of N+1 while the doc is queryable; subsequent idf/avgdl computations are wrong for every query.
- **Suggested fix:** Guard each Bump as in insert/delete: emit the `-old` Bump only `if old_doc_len > 0`, the `+new` Bump only `if new_doc_len > 0`. Red test: insert empty-field doc → update to non-empty → assert `doc_count` increments by exactly 1; reverse direction decrements by exactly 1.
- **TDD note:** `tests/fts_ranked_backend_tests.rs` contains **no `plan_update` test at all** (only insert/delete/lookup/rebuild) — the drift path is untested. Related: `rebuild()` (382-399) never zeroes stats before re-deriving them, so calling it on a live backend double-counts; the test `rebuild_restores_stats_from_data_store` (lines 214-216) **manually zeroes the counters first**, working around the bug rather than asserting the invariant.

### 5. `lookup_by_index` posting-cache miss→scan→insert race can pin a stale entry past the writer's invalidation
- **File:** `crates/shamir-index/src/base_index/index_manager.rs:2847-2882` (miss path + insert); invalidation at `apply_ops`/commit in `2889-2902`
- **Severity:** medium
- **Issue:** A reader that (a) misses the cache, (b) scans the store **before** a concurrent tx commit's `transact`, and (c) inserts its scan result **after** the commit's `invalidate_posting_cache_for_ops` ran, installs a stale `Arc<[RecordId]>` that no later invalidation touches. Every subsequent equality lookup for that key returns the stale set (silently missing newly-committed rows) until the 512-entry cache evicts it. There is no epoch/version re-check between scan and insert.
- **Failure scenario:** Reader A starts `lookup_by_index(k)`; tx B commits a new row posting for `k` and invalidates; A finishes its pre-commit scan and caches the old row list. All `k` lookups miss the new row indefinitely.
- **Suggested fix:** Version the cache (global `AtomicU64` epoch bumped by every invalidate; reader records epoch before scan and skips the insert if it advanced), or invalidate-after-insert from the writer side by re-probing. A Red test can deterministically interleave via the existing `lookup_pause_hook` seam (park the reader between scan and insert).

### 6. DROP INDEX of a vector index leaks the entire `__vec_snap__<id>` snapshot keyspace
- **File:** `crates/shamir-index/src/vector/vector_backend.rs:739-743` (`drop_all` no-op); `persistence.rs:531-548` (`sweep_index2_postings_by_id` scans only the 4-byte posting prefix); snapshot keys are string-keyed (`snapshot.rs:151-170`, `delta_chunk_key` 1150-1158)
- **Severity:** medium
- **Issue:** `VectorBackend::drop_all` is explicitly a no-op and the crash-recovery sweep scans only `[index_id LE]`-prefixed posting keys. The snapshot chunks (`__vec_snap__<id>.gN.graph.KKKKKK`, `.sidecar`), `.manifest`, and all `.delta.NNNNNNNNNN` chunks live under a **string** keyspace the sweep never touches — they survive every DROP as unbounded dead space. If an id is ever recycled (`set_next_id` is caller-controlled), a fresh index would also resurrect a stale manifest/graph for the reused id.
- **Suggested fix:** On drop (and in the tombstone recovery sweep), also `scan_prefix_stream` + `remove_many` the `__vec_snap__<id>` prefix. Red test: create → insert → drop → assert zero keys under the snapshot keyspace.

### 7. Staged vectors bypass dim validation on the in-tx merge paths (debug panic / silent truncation)
- **File:** `crates/shamir-index/src/vector/vector_backend.rs:451-457` (`staged_vector` returns `extract_vec` with no length check); `hnsw_adapter.rs:2948-2956` and `brute_force.rs:308-319` (merge loops call `dist.eval(query, vec)` unguarded); the correct guard exists only in `score_staged_candidates` (`hnsw_adapter.rs:2133-2138`)
- **Severity:** medium
- **Issue:** `extract_vec` accepts any all-numeric list regardless of length, so a malformed record (wrong-dim vector field) stages cleanly; at commit it only fails at `upsert` (after the tx was acknowledged as staged), and worse, an in-tx `lookup_tx` merges the staged vector via `ShamirDist::eval` → the SIMD kernels (`simd.rs:105/127` etc.) `debug_assert_eq!(a.len(), b.len())` — **panic in debug/test builds** — and in release compute a silently **truncated** distance (kernels use `a.len().min(b.len())`), returning wrong in-tx top-k. `score_staged_candidates` documents and guards exactly this hazard ("one bad row cannot poison the whole query"); the other two merge paths lack the guard.
- **Suggested fix:** Skip wrong-dim staged vectors in both merge loops (mirror `score_staged_candidates`), and/or validate dim in `staged_vector` so bad rows fail at stage time with a typed error.

### 8. `build_index2_backend` silently discards the persisted HNSW config (`m`, `ef_construct`)
- **File:** `crates/shamir-index/src/build_backend.rs:52-64`
- **Severity:** medium
- **Issue:** The reopen/migration path constructs `HnswAdapter::new` with hardcoded `max_elements: 100_000, m: 16, ef_construction: 200, ef_search: 50`, ignoring `VectorConfig::backend = InProcessHnsw { ef_construct, m }` that the DDL recorded and the descriptor persists. After a restart the index silently rebuilds/loads with different graph parameters than it was created with (recall/latency characteristics change with no signal). Only `quantization` is documented as deliberately not persisted; the ef/m divergence is undocumented. (`HnswAdapter::build_config`, used by compaction, likewise hardcodes `m: 16, ef_construction: 200`.)
- **Suggested fix:** Thread `cfg.backend`'s `m`/`ef_construct` (and a sane ef_search) into `HnswConfig` on the reopen path, or persist/restore them via the snapshot sidecar (which already records the live graph's build params).

### 9. FunctionalBackend Map hashing is insertion-order dependent (byte-identity floor violation)
- **File:** `crates/shamir-index/src/functional_backend.rs:164-172`
- **Severity:** low
- **Issue:** `InnerValue::Map(m)` is hashed by iterating `m.iter()`; `TMap` is an IndexMap, so iteration follows **insertion order**. Two logically-equal maps serialized with different key orders (msgpack map order is writer-dependent) produce different posting hashes → false negatives on lookup plus remove/set churn on update. Base_index's `hash_inner_value` (`index_keys.rs:161-173`) deliberately makes Map/Set order-independent (per-element hash XOR) and documents the property; the functional scheme lacks it.
- **Suggested fix:** Mirror the XOR-of-per-entry-hash scheme (or canonical sort by key id) for Map/Set in `hash_inner`. Red test: two maps with equal content built in opposite insert order must hash identically.

### 10. `Dot` metric silently clamps distances to 0 for unnormalized vectors in HNSW (inconsistent with BruteForce)
- **File:** `crates/shamir-index/src/vector/hnsw_adapter.rs:150-157` (`(1.0 - dot).max(0.0)`), vs `brute_force.rs:125` (`-dot_product`, exact for any magnitudes); same clamp in `quantized_dist.rs:274-277` and `RescoreCtx::score` (440-443)
- **Severity:** low
- **Issue:** The HNSW path's non-negativity clamp collapses every vector pair with `dot >= 1.0` to distance 0, so for unnormalized (legal) inputs the ordering is destroyed by arbitrary tie-breaking while BruteForce (small indexes, `BRUTE_FORCE_MAX`) returns exact ordering — results silently change character as the index grows past 256. The "callers must normalize" precondition is documented but unenforced (no validation at insert or query).
- **Suggested fix:** Normalize on insert for the Dot metric (store normalized vectors, or reject non-normalized with a typed error), or return `-dot` and use max-heap semantics consistently; at minimum validate and surface a `DimMismatch`-style error instead of clamping.

### 11. `FtsStats`: torn (count, sum) reads and a divide-by-zero window in `avg_doc_len`
- **File:** `crates/shamir-index/src/bm25.rs:72-78` (`avg_doc_len`), `80-92` (`on_insert` two separate fetch_adds)
- **Severity:** low
- **Issue:** `on_insert` increments `doc_count` then `sum_doc_len` as two independent Relaxed RMWs; a scoring reader between them observes `count > 0, sum == 0` → `avg_doc_len() == 0` → in `term_score`, `dl / avg_doc_len` is `inf` (or `NaN` when `dl == 0`), producing transiently wrong/NaN BM25 scores under concurrent insert+query. The `count == 0 → 1.0` guard exists but not a `sum == 0 && count > 0` guard.
- **Suggested fix:** Clamp: `if count == 0 || sum == 0 { return 1.0; }` (a posting can only exist when sum ≥ 1, so this never masks a legitimate average), or pack both counters into one atomic. Cheap Red test: interleave on_insert between the two RMWs (deterministically via a test seam) and assert a finite score.

### 12. Unbounded sorted-range upper bound `prefix || 0xFF×64` excludes values with ≥64 leading 0xFF encoded bytes
- **File:** `crates/shamir-index/src/base_index/sorted_index_manager.rs:2627-2632` (`range_bounds`, `None` end arm)
- **Severity:** low
- **Issue:** The "infinite" upper bound is `prefix + [0xFF; 64]`. A physical key whose encoded value begins with 64+ `0xFF` bytes (possible for `Bin`-typed indexed fields — `sort_codec::encode_bytes` never escapes `0xFF`, only `0x00`) compares **greater** than this bound and is silently excluded from `lookup_range(None, None)`, `lookup_min`, `lookup_max`, `lookup_first_k`, `lookup_last_k`. (`entry_count` uses a true prefix scan, so `doctor::verify()` would report the entries the lookups can't see.) Str is safe (UTF-8 cannot contain 0xFF).
- **Suggested fix:** Compute the true successor bound (`prefix` with its last byte incremented / `prefix + 0xFF` repeated to the backend's max key length contract), or document + enforce a max encoded-value length at index creation.

### 13. `apply_index_ops_at_commit` silently drops any non-`BumpFtsStats` in-memory op
- **File:** `crates/shamir-index/src/write_ops.rs:160-196`
- **Severity:** low
- **Issue:** Ops that are neither Set/RemovePosting are collected into `in_memory_ops`, but the grouping loop (170-176) only inserts `BumpFtsStats` variants into `ops_by_backend`; any other future in-memory op variant reaching this path would be silently discarded at commit (while the non-tx `apply_index_ops` applies everything to `backend.apply_in_memory`). Latent today (BumpFtsStats is the only variant) but a one-line drift away from a silent-loss bug.
- **Suggested fix:** Handle the residual explicitly (log/error on ungrouped in-memory ops), or route all in-memory ops through the backend that produced them.

### 14. Inline `#[cfg(test)] mod tests` in `quant_meta.rs` violates the documented test layout
- **File:** `crates/shamir-index/src/vector/quant_meta.rs:83-111`
- **Severity:** nit
- **Issue:** CLAUDE.md's test-organisation rules ("Never embed `#[cfg(test)] mod tests { ... }` inline inside implementation files. Move them to the `tests/` directory") — this is the only production file in the crate with an inline test module (every other module uses the `tests/` directory layout).
- **Suggested fix:** Move the round-trip test to `vector/tests/quant_meta_tests.rs` and wire it via `vector/tests/mod.rs`.

</details>
