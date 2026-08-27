# shamir-tx -- API & wire-protocol design

## Summary

shamir-tx's physical key codecs (`version_codec`, ts-key namespace, changefeed journal keys) are well-reasoned and genuinely property-tested, and the builder-only query rule is trivially satisfied (the crate sits below the query layer; zero `serde_json` usage). The weak spot is the changefeed wire format: `ChangelogEvent` msgpack journal entries carry no format/version envelope and decode failures are silently skipped, the CF-1 gap signal is in-memory only with no contiguity check on reads, and `read_from` re-demands the store the constructor already consumed with no same-store guarantee. Secondary issues: the `SORTED_TAG` posting-key layout constant is duplicated across crates behind an illusory "pinned by test" claim, Stringly-typed `Result<_, String>` errors pervade the public API against CLAUDE.md's thiserror rule, and the empty-`Bytes`-as-tombstone sentinel is unguarded on the public write path.

## Findings

### 1. Durable journal events have no schema/version envelope; decode failures are silently skipped
- **File:** crates/shamir-tx/src/changefeed.rs:86-101 (`ChangelogEvent`), :542-546 (`serialize_event`), :409-411 (corrupt-entry skip)
- **Severity:** high
- **Issue:** `ChangelogEvent` is serialized with bare `rmp_serde::to_vec` into the *durable* per-repo journal — an artifact that survives restarts and format upgrades. There is no version field, no format tag, no envelope. Adding/renaming/removing a field silently changes the on-disk layout. Worse, `read_from` skips entries that fail msgpack decode with only a `log::warn!` — an incompatible format change manifests as per-entry silent loss, not an error.
- **Failure scenario:** An upgrade adds a field to `ChangelogEvent`; a not-yet-upgraded replica (or a rollback after upgrade) calls `read_from`: every entry fails `from_slice`, is skipped with a warn, and the caller receives an empty/sparse `JournalRead { gap_at: None }` — indistinguishable from an empty journal. Downstream replication/subscription silently diverges with no error surfaced through the API.
- **Suggested fix:** Wrap the payload in an envelope (`{ v: u8, event: ChangelogEvent }` or a leading format-tag byte before the msgpack body); on decode failure of a known version, return a count/flag in `JournalRead` (see finding 4) instead of silently skipping.

### 2. `SORTED_TAG` posting-key layout duplicated across crates with an illusory test pin
- **File:** crates/shamir-tx/src/predicate_set.rs:159-181; crates/shamir-tx/src/tests/predicate_set_tests.rs:196
- **Severity:** medium
- **Issue:** `SORTED_TAG`/`SORTED_PREFIX_LEN` mirror `shamir-engine/src/index/sorted_index_manager.rs:60/:574` "kept local so shamir-tx stays decoupled", with the comment claiming this is "Pinned by `key_in_interval_prefix_tag_matches` test". But that test only asserts the local constant equals `0x80` — it cannot fail if the *engine-side* constant or key layout drifts. The pin protects the wrong crate; the coupling is real (the predicate layer must interpret posting keys exactly as the engine composes them).
- **Failure scenario:** The engine changes `SORTED_TAG` or its posting-key layout. shamir-tx still compiles, still passes its local pin test, and `key_in_interval` returns `false` for every posting — `predicate_conflicts` finds no phantom, Serializable txs stop aborting on phantoms. The degradation is completely silent (missing aborts, no error anywhere).
- **Suggested fix:** Move the tag byte / prefix layout into a crate both already depend on (`shamir-types` or `shamir-collections`), or add a cross-crate test in `shamir-engine` asserting its constant equals the re-exported `shamir_tx::SORTED_TAG`. Also fix the comment to say the current pin is local-only.

### 3. CF-1 gap signal is volatile (in-memory only); `read_from` never checks contiguity
- **File:** crates/shamir-tx/src/changefeed.rs:190 (`first_gap_version` field), :414-422 (gap_at computation)
- **Severity:** medium
- **Issue:** The documented contract is "`gap_at = Some(v)` ⇒ the journal is not contiguous, resync". But `first_gap_version` is a plain in-memory atomic: it resets on restart, so both overflow-drops and the documented crash-window tail loss become undetectable after a process restart. Additionally, `read_from` holds the returned events (each carrying `commit_version`) yet never verifies their contiguity — a cheap check that would catch every hole regardless of how it was created.
- **Failure scenario:** A burst overflows the 4096-deep journal channel and drops commit_version 100 (`gap_at = Some(100)` correctly signalled in-process). The process restarts (routine deploy) before the consumer catches up; on the new process `first_gap_version == 0`, so `read_from(1, …)` returns events 1..99,101.. with `gap_at: None`. A consumer honouring the contract trusts an unbroken history and permanently misses v100.
- **Suggested fix:** In `read_from`, scan the returned events' `commit_version`s and synthesise `gap_at` from the first hole (covers corrupt-skips and crash-window losses too); optionally persist a durable gap tombstone key at the dropped version.

### 4. `read_from` has no error channel — store failure and corruption are indistinguishable from "empty"
- **File:** crates/shamir-tx/src/changefeed.rs:397-413
- **Severity:** medium
- **Issue:** A `ChangelogStore::range_from` error is logged and returns `JournalRead { events: vec![], gap_at: None }`; corrupt entries are skipped. The caller cannot distinguish "no events yet", "storage broken", and "entries dropped as undecodable". CLAUDE.md's error rules (return `Result`, propagate with `?`, thiserror enums) are sidestepped entirely on this read path.
- **Failure scenario:** A misconfigured or failed changelog store makes every pull look like an empty feed. Monitoring built on `JournalRead` sees nothing wrong; replication falls behind with zero signal.
- **Suggested fix:** Return `Result<JournalRead, ChangefeedError>` (or at minimum add `truncated: bool` / `decode_failures: usize` to `JournalRead`) so the three cases are distinguishable.

### 5. `read_from` re-demands the store the constructor already consumed; no same-store guarantee
- **File:** crates/shamir-tx/src/changefeed.rs:232 (`new(store: Arc<dyn ChangelogStore>)`), :390-395 (`read_from(&self, store: &Arc<dyn ChangelogStore>, …)`)
- **Severity:** medium
- **Issue:** `RepoChangefeed::new` moves the store `Arc` into the background writer task; `read_from` then requires the caller to pass a *second* handle to the same store. Nothing enforces it is the same store: passing a different `ChangelogStore` silently returns wrong/empty results with no error. The caller must also keep a duplicate `Arc` alive for the feed's whole lifetime or reads break.
- **Failure scenario:** A caller builds the feed from the repo's changelog store but later passes a fresh/other store handle (easy in DI or test wiring) — `read_from` reads the wrong journal and the API reports success with empty results.
- **Suggested fix:** Keep `Arc<dyn ChangelogStore>` in `Self` (clone into the writer task) and drop the `store` parameter from `read_from`.

### 6. Stringly-typed `Result<_, String>` across the public API
- **File:** crates/shamir-tx/src/changefeed.rs:154-157 (`ChangelogStore` trait), :542 (`serialize_event`); crates/shamir-tx/src/mvcc_store/retention.rs:60 (`validate`); crates/shamir-tx/src/mvcc_store/mod.rs:500 (`set_retention`); crates/shamir-tx/src/staging_store.rs:249-251 (`rewrite_set_bytes`); crates/shamir-tx/src/tx_context.rs:916 (`apply_id_remap`)
- **Severity:** low
- **Issue:** CLAUDE.md mandates `thiserror` for library error enums and `Result<T, E>` propagation. The public `ChangelogStore` trait — the crate's main extension point — types its errors as `String`, as do `Retention::validate`/`set_retention` and the staging/remap paths. Downstream implementors cannot match on error kinds; `format!`-built strings are the error contract.
- **Suggested fix:** Introduce small thiserror enums (`ChangelogStoreError`, `RetentionError`) at the trait/validation boundaries; keep `String` only inside internal helpers if at all.

### 7. Empty-`Bytes`-as-tombstone sentinel is unguarded on the public write path
- **File:** crates/shamir-tx/src/mvcc_store/mod.rs:766 (`set_versioned`), :724-728 (`resolve_read`: `Ok(val) if val.is_empty() => Ok(None)`), :1058-1060 (the only documentation of the convention, at `delete_versioned`)
- **Severity:** low
- **Issue:** The wire convention "empty value bytes = tombstone" is sound for msgpack records (never zero-length) but is documented only on `delete_versioned`, while `set_versioned`/`set_versioned_many`/`set_versioned_many_append_only` accept arbitrary `Bytes` with no `debug_assert!` rejecting empty values. A non-record caller writing `Bytes::new()` creates an implicit delete.
- **Failure scenario:** Any present or future caller stores a legitimately empty blob through `MvccStore::set_versioned`: every read path (`resolve_read`, `get_current_bytes`, `get_at_many`, `current_stream`) interprets it as a tombstone and the row vanishes from scans — silently, since nothing rejected the write.
- **Suggested fix:** Add `debug_assert!(!value.is_empty())` to the three `set_versioned*` entry points (and the `KvOp::Set` arm of `apply_committed_visible`), plus one contract line on `set_versioned`'s doc.

### 8. `VERSION_SEP` invariant is probabilistic, self-contradictory in its doc, and dodged by its own prop tests
- **File:** crates/shamir-tx/src/version_codec.rs:10-30; crates/shamir-tx/src/tests/version_codec_tests.rs:53-67, :110-117
- **Severity:** low
- **Issue:** The module doc claims `0xFF` "cannot appear in a `RecordId`", then immediately argues the collision chance is merely "negligible" for 16 random bytes (a random id contains *some* `0xFF` with p≈6%; only the exact tail shape is 2⁻⁷²). The prop tests exclude `0xFF` from generated keys "so that the invariant … is upheld by construction" — i.e. the suite never exercises the fragile case it documents, and the doc's "verified by … round-trip property tests below" reference is stale (tests live in `tests/version_codec_tests.rs`, not below). The tests also correctly admit cross-length keys can interleave, constraining the non-interleaving property to same-length keys. Correctness is preserved in practice by the defensive `orig == key` filter after decode in the scan paths (`mvcc_history.rs:36`, `:197`) and by fixed-length 16-byte RecordIds, so this is a documentation/latent-hygiene issue, not an active bug.
- **Suggested fix:** State the real invariant precisely (data keys are fixed 16-byte RecordIds, so cross-key range intrusion is structurally impossible; variable-length keys must be engine-typed encodings that never end in `0xFF` + 8 bytes), fix the stale "tests below" reference, and consider a `debug_assert` on key length at encode time.

### 9. `tx_id = 0` "non-tx write" sentinel is unenforced against 0-seeded id allocators
- **File:** crates/shamir-tx/src/changefeed.rs:491-494 (`project_event`), :513-514 (`nontx_event`); crates/shamir-tx/src/repo_wal_manager.rs:30-40
- **Severity:** low
- **Issue:** The external changefeed contract ("0 = non-tx write", per LIVE_SUBSCRIPTIONS.md) relies on every real tx id being ≥ 1. `RepoTxGate::fresh()` seeds 1, but `RepoWalManager::new(initial_txn_id, …)` accepts any `u64` — a 0 seed mints a real tx whose projected event is indistinguishable from a non-tx write. Nothing in this crate enforces the sentinel's precondition.
- **Failure scenario:** A recovery path seeds `RepoWalManager` with 0; the first real transaction gets id 0; its `ChangelogEvent.tx_id == 0` and every downstream consumer classifies a genuine tx as a non-tx write.
- **Suggested fix:** `debug_assert!` (or clamp) in the id allocators so 0 is never handed out, or document the precondition on `project_event`/`nontx_event`.

### 10. Dead public group-commit API remains in the exported surface
- **File:** crates/shamir-tx/src/lib.rs:69 (`pub use pending_commit::PendingCommit`); crates/shamir-tx/src/repo_tx_gate.rs:752-763 (`enqueue_pending`/`drain_pending`)
- **Severity:** nit
- **Issue:** F-79/#906 sanctions the dead `pending_commits` field as scaffolding, but the crate still *exports* `PendingCommit` and both zero-caller accessors in its public API. The exported surface invites use of a path whose contention model is explicitly documented as "never audited for a live call".
- **Suggested fix:** Demote `enqueue_pending`/`drain_pending` to `pub(crate)` (or `#[doc(hidden)]`) and drop `PendingCommit` from `lib.rs` re-exports until group-commit is revived.

### 11. `serde_bytes_compat` deserializer accepts any sequence, not just byte arrays
- **File:** crates/shamir-tx/src/changefeed.rs:104-116
- **Severity:** nit
- **Issue:** `serialize` emits `serialize_bytes` (msgpack bin) but `deserialize` goes through `Vec::<u8>::deserialize`, which also accepts a msgpack array-of-integers encoding. The pair is asymmetric: round-trip of self-produced payloads is fine (and tested), but hand-crafted/adversarial journal entries get a second accepted encoding for the same field.
- **Suggested fix:** Implement a small `Visitor` calling `deserialize_byte_buf` so only the bin encoding is accepted.

## Positive notes (theme-relevant, no action required)

- **Builder-only rule: compliant.** No `serde_json`/`json!`/`serde_json::Value` anywhere in the crate (grep-verified); this layer never constructs queries, so no exception comments are needed.
- `IsolationLevel`'s wire names are snake_case *and* pinned by test (`types_tests.rs:23-29`), including the exact strings the `shamir-db` tx match arms consume.
- `version_codec`'s property suite is genuinely thorough for the invariant-respecting domain (round-trip, sort-order equivalence, monotonicity, prefix dominance), and the ts-key namespace split (`TS_TAG = 0x00`, 9 bytes vs ≥10-byte version keys, `mvcc_store/mod.rs:52-88`) is a clean, well-argued collision-free design; ts values are consistently LE on both write and read.
- The changefeed journal key (`version_key`, BE-8) ordering and msgpack round-trip both have direct tests.
