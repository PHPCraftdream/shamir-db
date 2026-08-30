# shamir-types — Cross-Lens Review (all 7 lenses, synthesized)

Crate: `crates/shamir-types/` — the workspace's shared value/type foundation: the
`Value`/`QueryValue` model, the id-keyed MessagePack codecs (tree + zerocopy +
storage-bytes merge), the zero-copy `RecordView` lens, the lock-free `Interner`
(DashMap + ArcSwap + AtomicU64), `RecordId`, `mpack!`, and the access-model
primitives (`ResourcePath`/`ResourceMeta`/`Mode`/`permits`).

Review basis: the seven 2026-08-14 lens reports under this directory —
`correctness-tdd.md`, `concurrency-lockfree.md`, `security-crypto.md`,
`performance-hotpath.md`, `api-wire-protocol.md`, `error-handling-lifecycle.md`,
`style-claude-md.md` — read in full and deduplicated. Structure/tone/rigor
calibrated on the two completed exemplar syntheses:
`shamir-client-node/SUMMARY.md` and `shamir-transport-ipc/SUMMARY.md`.
Workspace context: the sweep's per-crate row credits this crate 64 lens-tagged
findings (0c/5h/23m/21l/15n) — independently recounted from the 7 files below,
and it matches. Read-only synthesis: no build/test/lint commands; no source file
modified. A handful of file:line anchors and one inter-file contradiction were
spot-checked against the source (noted inline where it changes a claim).

## Executive summary

The crate's foundations are genuinely strong — the ArcSwap/DashMap interner, the
depth-capped panic-free lens, and byte-parity codec test suites are the best in
the sweep — but it currently ships two process-killers and a family of
silent-wrong-results primitives. Fix first: (1) **the 5-byte abort DoS** —
`SANE_PREALLOC_CAP` was added to the serde visitor only; the zerocopy decoder and
`merge_storage_bytes` still preallocate straight from wire headers, so a corrupt
or hostile `Array32`/`Map32` header aborts WAL recovery / the S-write path
(findings 3.1/2.2/6.1); (2) **the silent-wrong-results primitives** — ±0.0
violates the Hash/Eq contract with the bug *asserted by its own test* (1.1),
`Dec`/`Big`/`Set` cannot round-trip the wire so `WHERE price = 123.45::dec`
silently never matches rows written through Dec literals (5.1), and
`RecordId::is_system()` misclassifies real records minted near/before the custom
epoch (1.3); (3) **the hidden O(k·f)/O(f²) scans** in the projection codec and
`RecordRef::for_each_field`, which bypass the crate's own `FieldIndex` and
quadraticize the engine's per-row read path (4.1/4.2).

---

## 1. correctness-tdd

### 1.1 — high — `F64(0.0)` and `F64(-0.0)` compare equal but hash differently; a test locks the violation in
- File:line: `src/types/value.rs:697-711` (Hash), `:293-299` (PartialEq); `src/types/tests/value_tests.rs:525-530`. *(also flagged by api-wire-protocol #4 — one defect, two lenses)*
- Issue: `PartialEq` uses IEEE `==` so `F64(0.0) == F64(-0.0)`; `Hash` canonicalizes only NaN and hashes raw `f.to_bits()` otherwise, and `0.0`/`-0.0` bits differ. The NaN branch's own comment states the `k1 == k2 ⇒ hash(k1) == hash(k2)` contract (added for the 2026-07-06 `distinct()` regression) — then misses ±0.0. `test_f64_neg_zero_hash` **asserts the hashes differ**, so the suite is green on wrong semantics and any fix must flip the test.
- Failure scenario: `TSet<Value<String>>` dedup (DISTINCT / IN-lists / set membership): `Set([0.0]) == Set([-0.0])` compares unequal, membership of one zero misses the other's bucket, and ±0.0 dedup counts twice — the same dedup-regression class as the fixed NaN bug, one rotation away.
- Suggested fix: canonicalize `-0.0 → +0.0` bits next to the NaN arm; flip the test to assert equal hashes and both-zeros-in-one-set.

### 1.2 — medium — `HavingView::materialize_at` returns `Some(Null)` for containers, contradicting its own safety comment
- File:line: `src/record_view/record_ref.rs:514-539` (impl), `:559-582` (`query_value_to_inner_value`). *(adjacent but distinct from 5.5's de-intern swallow — same module, different root)*
- Issue: the comment promises containers return `None` so InSet/Contains gets the "no leaf" fallback; the code unconditionally collapses `List`/`Set`/`Map` leaves to `InnerValue::Null`.
- Failure scenario: a HAVING predicate probing aggregate output receives `Null` instead of "no leaf": it silently evaluates as no-match-with-a-value, indistinguishable from a genuine Null aggregate — an observable divergence hidden behind a comment claiming it cannot happen.
- Suggested fix: honor the comment (type-check before conversion) or fix the comment and pin the chosen semantic with a test.

### 1.3 — medium — `RecordId::is_system()` misclassifies real records minted near/before CUSTOM_EPOCH; the comment claims this cannot happen
- File:line: `src/types/record_id.rs:16-18` (prefix rationale), `:41-54` (`from_ts`), `:95-109` (`system`/`is_system`). *(also flagged by api-wire-protocol #11b — same root, folded here)*
- Issue: `is_system()` tests "first 4 bytes zero"; user ids store `(timestamp_micros − CUSTOM_EPOCH_MICROS).to_be_bytes()`, whose top 4 bytes are zero for any relative time < 2³² µs ≈ 71.6 min after 2026-01-31T00:00Z. The constant's doc ("a real timestamp will never be zero") is false at the epoch boundary. **Resolved during synthesis:** correctness-tdd #3 claimed pre-epoch times "saturate to negative → 0xFF…", while api-wire #11b claimed `saturating_sub` clamps them to zero — the source (`record_id.rs:46`, `i64::saturating_sub`) confirms **api-wire is right**: pre-epoch timestamps clamp to exactly 0, producing an all-zero timestamp half and `is_system() == true` (the worse of the two claims). No test pins either boundary (all tests use "now", past the window).
- Failure scenario: records minted in the first ~72 minutes after the epoch (cold-restore replaying old timestamps, clock set near the epoch) or *any* pre-epoch timestamp (clock skew, imported data) collide with the system-record convention; downstream branches on `is_system()` treat user data as system metadata.
- Suggested fix: reserve a distinguishing pattern beyond "4 zero bytes" (range check or a reserved bit), or clamp/validate `from_ts` inputs and document+test both boundaries. Residual: the deliberate 12-byte truncation aliasing of `system()` names has no collision guard at construction (see 5.3).
- Residual note (api #11a adjacency): `Self::default()` minting a fresh random id each call is filed under 5.11.

### 1.4 — medium — TDD gap: `HavingView` (a full public `RecordRef` impl, ~180 lines) has zero tests anywhere
- File:line: `src/record_view/record_ref.rs:354-557`; no `HavingView|having` matches under any `tests/` directory.
- Issue: nothing constructs a `HavingView`. Its distinctive behaviors are exactly where bugs hide: single-segment-only resolution (multi-segment paths silently `None` = "predicate does not match"), `key_index` built by interning row keys at construction (unknown keys silently dropped), `for_each_field` materializing through `query_value_to_inner_value`, and finding 1.2. CLAUDE.md's Red/Green protocol presumes failing-first coverage; this impl shipped without any.
- Failure scenario: refactors of `key_index`/descend logic break HAVING evaluation with no test signal; the multi-segment `None` and unknown-key drop remain untested, possibly unintended semantics.
- Suggested fix: `record_view/tests/having_view_tests.rs`: flat-row scalar hit/miss, unknown-key drop, multi-segment `None`, List/Set `any_seq_elem` paths, `materialize_at` scalar-vs-container (pinning 1.2's decision), `to_query_value` clone identity.

### 1.5 — medium — *(primary: same as 4.2)* — `RecordView::for_each_field` hides an O(fields²) rescan
- Listed under the correctness lens because it violates CLAUDE.md pillar 3 in a helper; full write-up at **4.2** (also flagged by concurrency-lockfree #1 — one defect, three lenses).

### 1.6 — low — `mpack!` rejects negative (or any multi-token) literals as OBJECT values — asymmetric with lists, undocumented, untested
- File:line: `src/macros/mpack.rs:249-261` (object value arms consume exactly one token tree), module doc `:90-96` promises `-7` works; `src/macros/tests/mpack_tests.rs` covers negatives only top-level/in-lists.
- Issue: `"k": -7` tokenizes as two tokens; the single-tt value arms match neither and the muncher dies with "no rules expected" — a compile error. Lists work because their flush arm re-expands into the dedicated `(- $n:literal)` arm. The doc sells negative-literal support without carving out objects, and no test exercises a negative object value, so CI cannot see the asymmetry.
- Failure scenario: a user writes `mpack!({"delta": -7})` per the doc and hits an inscrutable macro error (the `@` workaround exists but is undocumented for this case).
- Suggested fix: add object-value arms mirroring the array flush (or widen to `$($value:tt)+`); add tests: `{"profit": -7}`, nested `{-2.5}` in lists/maps.

### 1.7 — low — Untested public surface with documented contracts: `Interner::generation`, `ResourceMeta::owner_field`, `principal64_from_username`, `SecretString::into_inner`, `ResourcePath::WasmCompiler`
- File:line: `src/core/interner/interner.rs:303-305`; `src/access.rs:296-300`, `:59-66`, `:408/:523/:581` (WasmCompiler arms); `src/secret.rs:40-43`; definitions only — no test references. *(the security defect behind `principal64_from_username` itself is 3.4 — this is its missing-coverage half)*
- Issue: each carries a behavioral promise nothing pins: `generation()` is the documented staleness signal for cached filters; `owner_field` exists specifically to distinguish "explicitly System-owned" from "field absent" in privilege-escalation decisions; `principal64_from_username` determinism/distinctness is unpinned; `into_inner`'s ownership handoff (drop must NOT zeroize the taken buffer) is unverified and the non-`crypto` build silently skips zeroization entirely; `WasmCompiler` is absent even from the exhaustive-looking `trace_access_transparent_for_all_variants` loop.
- Failure scenario: a refactor (e.g. reordering the `current_id` bump in `touch_with_id`, changing `from_owner_id`) silently invalidates cached-filter completeness or ownership decisions with green tests.
- Suggested fix: one small test file per area: generation monotonicity, `owner_field` present/absent/null parity vs `from_record`, username-hash determinism + distinctness sanity, `into_inner` post-move validity, `WasmCompiler` added to the variant loops.

### 1.8 — low — `touch_ind` racing `touch_with_id` on the same id is guarded only by `debug_assert!` — silent divergence in release
- File:line: `src/core/interner/interner.rs:216-233` (`set_reverse_slot` debug_assert), `:200-207` (documented out-of-scope hazard). *(also flagged by error-handling-lifecycle #7 — one defect, two lenses)*
- Issue: the single-writer lock serializes reverse writes among themselves, but cross-API exclusion (monotonic `touch_ind` vs WAL-recovery `touch_with_id`) rests on the "recovery never runs concurrently with live traffic" assumption, enforced only in debug builds. In release, a lost race fails `OnceLock::set` and keeps the OTHER name in the reverse slot while the forward map holds THIS name's id — permanent forward/reverse divergence, silently. Tests cover each API separately, never the interaction (declared out of scope, hence low/residual).
- Failure scenario: only reachable if recovery ever overlaps live traffic — the exact condition whose enforcement mechanism is the assumption itself; then `get_str(id)` resolves to the wrong name everywhere, with no error.
- Suggested fix: promote to logged+`Err` (or store-only-if-empty with a returned verdict) so a future caller that violates the model fails loudly instead of corrupting the namespace.

### 1.9 — nit — Vacuous self-comparison assertion in `from_ts_produces_unique_ids_with_same_timestamp`
- File:line: `src/types/tests/record_id_tests.rs:107-113`.
- Issue: asserts `id.as_bytes()[..8] == RecordId::from_ts(ts).as_bytes()[..8]` — re-encoding the same input and comparing encoders to themselves; always true regardless of layout bugs. (The subsequent shared-prefix loop does carry weight.)
- Suggested fix: compare against the hand-computed `relative.to_be_bytes()` like `from_ts_preserves_byte_layout` does, or drop the tautology.

### 1.10 — nit — Duplicated `CodecError` name, malformed doctests, dead error variant, misnamed error variant
- File:line: `src/codecs/basic/bincode.rs:8-22` (second `CodecError{Serialize,Deserialize}`) with broken pseudo-doctests at `:26-32`/`:43-50` referencing nonexistent `shamir_db::types::codec` (masked only by `[lib] doctest = false`; also flagged by api #11d — folded here); `src/types/value_error.rs:19-24` (`PathNotFound` produced by no API); `src/record_view/lens.rs:196` (`read_str_len` reports string-read failures as `NonBinKey`). *(the name-shadowing half is the same defect as 6.2 — counted once there)*
- Issue: name-shadowed error enums invite wrong-type imports; the doctests would fail the moment doctests are re-enabled; `PathNotFound` implies a path-based error surface that doesn't exist; `NonBinKey` fires for string reads too.
- Suggested fix: rename the bincode enum (see 6.2), fix/delete the broken examples, remove or start producing `PathNotFound`, add a `NotAStr` variant.

## 2. concurrency-lockfree

**General verdict: pillar-clean on the shared-state surface.** The interner is `DashMap(THasher)` + `ArcSwap` RCU reads + `AtomicU64` ids; hash-keyed structures are uniformly Fx; the single `std::sync::Mutex` is the registered F-9 category-3 first-touch-only exception with an inline contention-model comment and a documented data-loss rationale; zero `async`/`.await` (no guard-across-await class), no `scc::*` banned `len()`, per-thread RNG. Findings below are medium-and-below (the two O(N²)-shaped items are deduped to 3.1/4.2).

### 2.1 — medium — *(primary: same as 4.2)* — repeated full-scan lookups + double decode in `for_each_field`
- Concurrency framing of **4.2** (also flagged by correctness-tdd #5 — one defect, three lenses).

### 2.2 — medium — *(primary: same as 3.1)* — header-driven unbounded preallocation in the zerocopy decoder and merge encoder
- Concurrency framing of **3.1** (also flagged by error-handling-lifecycle #1 — one defect, three lenses).

### 2.3 — low — Sanctioned `reverse_write_lock` is also held across the doubling-growth clone-forward — write-stall grows with spine length
- File:line: `src/core/interner/interner.rs:216-233` (`set_reverse_slot`), `:243-260` (`grown_reverse`).
- Issue: the registered F-9 exception's instantaneous critical section includes the full `grown_reverse` sweep — cloning every spine cell under the mutex. With N distinct fields interned, one unlucky first touch stalls all concurrent first touches (~N Arc refcount bumps). Reads are unaffected (ArcSwap); steady-state contention is nil (first-touch is once-per-distinct-name-ever); the exposed window is cold-start/WAL-hydration bursts into a large interner.
- Failure scenario: hydrating ~1M persisted field names serializes all writers behind N successive O(spine-length) clones at growth boundaries — latency spikes, nothing corrupts.
- Suggested fix: keep the design (its data-loss justification is sound) but note the growth-boundary stall inline next to the exception comment; if hydration ever runs hot, revisit the struct doc's own seqlock-style generation-counter sketch.

### 2.4 — low — Transient forward-before-reverse publication window (and rollback window) visible to racing third-party readers
- File:line: `src/core/interner/interner.rs:168-177` (`touch_ind` commits forward before `set_reverse_slot`), `:402-447` (`touch_with_id`'s collision rollback removes the forward entry afterward). *(adjacent to 1.8's cross-API race but a distinct defect: a nanosecond-scale read-side transient vs permanent divergence)*
- Issue: between a writer's forward insert and reverse-slot population (or after rollback), a thread that resolved `get_ind(name)` and immediately calls `get_str(id)` observes `None` → upstream `CodecError::Decode("Interned key not found")`. It self-heals in nanoseconds and the owning caller never sees it, but nothing tells codec consumers `None` can be transient, so it may be treated as permanent corruption instead of retried.
- Failure scenario: a reader interleaves into the window under load → one de-intern error / rejected row; the rollback window additionally exposes a name→id mapping that then disappears.
- Suggested fix: one sentence on `get_str`/`get_ind` docs ("a just-touched key may briefly resolve forward-only; `None` is transient") and/or a retry-at-call-site helper in `codecs::interned::common`; optionally encode the recovery-vs-live exclusion in code (AtomicBool probe epoch) rather than convention.

### 2.5 — nit — README describes an obsolete locking model for `Interner`
- File:line: `src/core/README.md:64-67`, `:83-86`, `:128-131`.
- Issue: the README claims `TDashMap<InternerKey, UserKey>` + "Current ID: `Mutex<u64>`" + "fine-grained writes via DashMap"; the actual model is `ArcSwap<Vec<OnceLock<Arc<str>>>>` + `AtomicU64` + the F-9-cited `Mutex<()>` write gate. A stale doc claiming a `Mutex` where there is none (and vice versa) actively misleads the F-9 exception audits that depend on accurate per-site documentation.
- Failure scenario: a future reviewer trusts the README, misses the real sanctioned-mutex site, or hunts a phantom `Mutex<u64>`.
- Suggested fix: refresh the §Interner/Thread-Safety sections to name the ArcSwap spine, `OnceLock` slots, `AtomicU64` counter, and the `Mutex<()>` gate with a pointer to the struct doc.

## 3. security-crypto

**Posture is strong overall:** exactly one `unsafe` in the crate (sound but avoidable — 3.6), both purpose-built decoders enforce a 128-deep recursion cap, the lens bounds-checks every read and never panics on untrusted bytes, and `validate_keys_resolve` refuses to persist forged interner ids. The weak spots: the allocation hardening never left the serde visitor (3.1), two fail-open defaults (3.2, 3.4), and secret/id footguns (3.6, 3.7).

### 3.1 — high — Unbounded preallocation from attacker-controlled msgpack array/map headers → allocator-abort DoS on the decode/merge path
- File:line: `src/codecs/interned/messagepack.rs:305` (`decode_array`: `Vec::with_capacity(len)`), `:318` (`decode_map`: `new_map_wc(len)`), `:581`/`:584-585` (`merge_storage_bytes`: `Vec::<Entry>::with_capacity(n_old)` + `TFxMap::with_capacity_and_hasher(n_old, ..)`); headers parsed straight from wire markers at `:236-254`/`:680-718`. *(also flagged by concurrency-lockfree #2 and error-handling-lifecycle #1 — one defect, three lenses; all three anchor sets verified against source)*
- Issue: `Array16/32`/`Map16/32` headers declare up to `u32::MAX` (~4.29e9) elements from 5 bytes; all three sites allocate `count × elem_size` (~50+ B/slot; `Entry` ≈ 40 B) *before* validating any payload exists. `src/types/value.rs:117-122` fixed this exact hazard for the serde visitor with `SANE_PREALLOC_CAP = 4096` ("driving `Vec::with_capacity(size_hint)` to multi-GB / abort") — the caps were never carried to the hand-rolled decoder, the WAL-recovery/S-write decode target, or the storage merge. Depth capping doesn't help: a *flat* `Array32` with a 5-byte body trips nothing before the allocation. (The `:582-583` comment "no untrusted input risk" addresses the hasher, not the header-driven capacity.)
- Failure scenario: a torn/corrupt (or hostile, for client-supplied id-msgpack — `validate_keys.rs` exists precisely because client data reaches here) record starting `0xDD FF FF FF FF` drives a >100 GB allocation → `handle_alloc_error` aborts the whole process during recovery or the write path, taking down all in-flight sessions instead of returning `CodecError`.
- Suggested fix: clamp every wire-derived count through a shared `min(header, SANE_PREALLOC_CAP)` at all three sites (collections grow on demand for legitimately large records); add regression tests feeding huge-header/short-body buffers to `msgpack_to_inner` and `merge_storage_bytes`, asserting `Err` — not abort.

### 3.2 — medium — Fail-open mode fallback in `ResourceMeta::from_record` — unparsable `mode` silently becomes `Mode::OPEN` (0o777)
- File:line: `src/access.rs:275-279` (`.unwrap_or(Mode::OPEN)`). *(also flagged by error-handling-lifecycle #4 — one defect, two lenses)*
- Issue: a stored `mode` that fails `u16` parsing (corruption, buggy writer, hostile catalogue edit) silently widens to world-rwx. Every other malformed-field fallback in the function is System/open too, which compounds it; unlike the owner-collapse (deliberately documented at `:283-300` with `owner_field` as the escalation-safe alternative), this fallback is undocumented — and parse failure should never make a resource *more* accessible. No test exercises a malformed mode.
- Failure scenario: a catalogue record carries `mode: 999999999` after partial corruption → reloads world-writable → `permits()` grants every class full access the original object never had.
- Suggested fix: fail closed — owner-only `Mode` fallback, or return an error/sentinel the facade gate rejects on; add a red test pinning the behavior; at minimum `log::warn!` + document the fail-open choice.

### 3.3 — medium — Append-only interner accepts unlimited untrusted key names; FxHash rationale inverted
- File:line: `src/core/interner/interner.rs:138-179` (`touch_ind`, monotonic, no eviction by design); reached per client map key via `src/codecs/interned/common.rs:13`.
- Issue: CLAUDE.md pillar 4 mandates FxHash with the rationale "we don't accept untrusted hash inputs here" — but the interner hashes fully attacker-chosen field names on the write path, and every distinct name becomes a *permanent* entry (`Arc<str>` spine + forward row; append-only). FxHash collisions are trivially forgeable, so crafted names can pile onto one shard/bucket chain.
- Failure scenario: a low-privilege session streams records containing millions of unique field names → unreclaimable server memory growth that persists via WAL/storage across restarts; second-order, Fx-colliding names degrade lookup latency. No cap exists at this layer.
- Suggested fix: enforce a tunable distinct-keys quota at the gate calling `touch_ind` (surfaced as `CodecError`), and/or annotate the deviation from pillar 4's rationale where keys are untrusted; if neither is wanted, document the invariant ("write principals are trusted to bound field-name cardinality") as an explicit owned risk.

### 3.4 — medium — `principal64_from_username` mints principal ids with seedless FxHasher
- File:line: `src/access.rs:59-66`. *(its missing test coverage is 1.7)*
- Issue: deterministic, non-cryptographic, seed-less hash of a *user-chosen* name projected into the owner/principal id space. FxHash-64 collisions over usernames are constructible offline in seconds, aliasing two names to one principal id. The doc correctly labels it interim with two live production call sites pending `PrincipalResolver` (#559) — but nothing tracks that removal.
- Failure scenario: any permission/ownership decision keyed on `principal64_from_username(name)` is steered by registering a colliding name → shared owner-class bits or audit-attribution confusion.
- Suggested fix: treat #559 as a blocking-security migration with a deadline in the doc; until then whitelist the bridge to the two documented call sites (clippy disallowed-methods or `#[doc(hidden)]` wrapper).

### 3.5 — low — Generic `bincode::from_bytes` helper lacks the depth caps its sibling decoders have
- File:line: `src/codecs/basic/bincode.rs:51-56` (bincode 1.3.3), re-exported crate-wide via `codecs::basic`.
- Issue: bincode 1.x applies no nesting limit while both custom msgpack decoders cap at `MAX_MSGPACK_DEPTH = 128`. Decoding deeply nested untrusted bytes into a recursive type can exhaust the stack. Exposure depends on out-of-crate call sites — if the helper only sees engine-produced frames, this is defense-in-depth debt, not a live hole.
- Failure scenario: a future caller wires `from_bytes::<QueryValue>` onto a network-facing path; a megabyte of nested list markers ends the process.
- Suggested fix: document the helper trusted-input-only at the definition, or route untrusted callers through the depth-capped msgpack codec; long-term migrate to bincode 2.x (configurable limits).

### 3.6 — low — `SecretString`: derived `PartialEq` timing footgun, avoidable `unsafe` in `Drop`, untested lifecycle
- File:line: `src/secret.rs:21` (`#[derive(Clone, PartialEq, Eq)]`), `:67-75` (manual `Drop` with `unsafe { self.inner.as_bytes_mut() }`). *(also flagged by error-handling-lifecycle #8, which covers the same unsafe/lifecycle-test half — one defect, two lenses)*
- Issue: (a) derived `PartialEq` early-exits on the first differing byte — appropriate nowhere for secret material; the type's name invites `provided.reveal() == expected.reveal()` password checks (timing oracle). (b) The crate's single `unsafe` is sound today (zero bytes preserve UTF-8 validity; `&mut self` in `drop` is exclusive) but unnecessary — `zeroize::Zeroize` is implemented for `String`. (c) Zeroize-on-drop and `into_inner`'s duty-transfer are untested (`secret_tests.rs` covers redaction/serde/conversions only); the non-`crypto` build silently downgrades to non-zeroizing (documented, untestable as shipped).
- Failure scenario: downstream auth code compares cleartexts via `==` (microsecond-scale prefix leak, mitigated once upstream hashes); later, a refactor moving `inner` behind an indirection erodes the unsafe block's premises invisibly — no test fails.
- Suggested fix: replace the `Drop` body with `self.inner.zeroize()`; add a doc warning ("never compare `reveal()` output; compare digests/MACs"); add a lifecycle test for `into_inner` + drop semantics.

### 3.7 — low — Predictable RecordId random tail and embedded timestamps — the "ids are not capability material" invariant is undocumented
- File:line: `src/types/record_id.rs:48-52` (thread-local Xoshiro256++ tail), `:80-90`.
- Issue: ids are predictable (xoshiro is state-recoverable from a handful of outputs) and time-leaking (wall-clock micros in bytes [0..8]). That is harmless **only** as long as ids never act as unguessable handles, capability links, secrecy-tied pagination tokens, or rate-limit keys — nothing in the crate states that invariant, yet this is the canonical id minted for user rows. (The comment's "CSPRNG is unnecessary" is fair for collision resistance.)
- Failure scenario: a future endpoint authorizes "GET /records/{id}" by possession alone → ids harvested from one response enumerate the table.
- Suggested fix: rustdoc on `RecordId`: ids are NOT secret/capability material; authorization must never key on possession. Revisit `getrandom` tails if such a use lands.

### 3.8 — nit — Log-forging surface: raw resource names rendered into trace/denial lines
- File:line: `src/access.rs:561-588` (`Display for ResourcePath`), `:622` (`AccessError` Display), `:657-660` (`trace_access` → `log::trace!`). *(same render sites as 5.9 but a different defect: forgery vs missing round-trip)*
- Issue: every user-chosen segment (db/store/table/user/group/function names) is interpolated verbatim; newlines/ANSI escapes pass through, so denial messages and `shomer:` trace lines can contain forged entries. Upstream validation presumably lives in the directory/catalogue layer, but this crate controls the rendering.
- Failure scenario: attacker creates a table named `x"; DROP` or `\n2026-08-14 INFO admin granted...` → downstream log aggregation shows fabricated audit lines.
- Suggested fix: escape control characters in these `Display` impls (`char::escape_debug` / strip `is_control()`) — cheap off the hot path, makes observability lines tamper-evident at the source.

## 4. performance-hotpath

**Largely exemplary on pillar 3** — the zero-copy lens with its amortizing `FieldIndex`, the doubling-growth interner spine, streaming storage encoders, and single-pass `merge_storage_bytes` all push per-op cost toward O(1)/O(N). The residuals are two re-scan patterns that re-introduce O(N²)-shaped products on per-row paths, a scratch-buffer API whose stated win its own `mem::take` defeats, and one avoidable per-key allocation. Tests cover parity, not scaling, so none would surface as a failure today.

### 4.1 — high — Projection codec re-scans the whole record once PER selected field id — O(fields × selected) per row
- File:line: `src/codecs/interned/projection.rs:62-67` (loop calling `view.field_value_bytes(id.clone())`); underlying linear scan `src/record_view/lens.rs:1131-1168` (`value_bytes_at` restarts at offset 0).
- Issue: `record_view_to_id_msgpack` calls `field_value_bytes` per selected id; every call walks key markers + `skip_value`s from body offset 0. A k-id projection over an f-field record costs O(k·f) per row — bypassing exactly the `FieldIndex` primitive whose doc promises "reading N fields is O(fields) amortised, not O(fields²)".
- Failure scenario: `shamir-engine/src/table/read_exec.rs:1538,1579` invokes this per row on the S-read path: `SELECT` of 20 columns over a 60-field record pays ~1200 marker reads + value skips per row instead of ~80; cost grows multiplicatively with record and projection width, invisible to correctness tests (`projection_tests.rs` uses tiny fixtures).
- Suggested fix: build a one-pass span index per view (extend `FieldIndex::index()` to record value end offsets), then emit each selected id in O(1) — total O(f + k). Existing byte-parity `projection_tests` verify output is unchanged.

### 4.2 — medium — `RecordRef::for_each_field` (lens impl) is O(f²): one full map re-scan per field
- File:line: `src/record_view/record_ref.rs:338-346` (per-field `materialize_at(&[key])` discarding the value already decoded by `fields()`); scan primitive `src/record_view/lens.rs:1123-1125` → `:1131-1168`. *(primary of the three-lens group: also correctness-tdd #5 and concurrency-lockfree #1)*
- Issue: iterates `fields()` (one full pass), throws the decoded value away (`for (key, _val)`), then re-scans from offset 0 for every field via `materialize_at` → `value_bytes_at`; each value's marker is processed twice. Documented as "Used for SELECT * / full-record projection" — the cross-crate use case where quadratic bites hardest.
- Failure scenario: currently exercised only by `record_view/tests/record_ref_tests.rs` (`for_each_field_parity`); no shamir-engine caller yet — which is why this is a medium next to 4.1's high. When Stage-3 wires SELECT * onto it, a 100-field record pays ~5,000 extra key decodes + skips per row vs 100.
- Suggested fix: single pass capturing `(id, val_start, val_end)` spans while walking `fields()`' cursor, then `InnerValue::from_bytes(&body[span])` per field — or share the span index from 4.1's fix between both call sites.

### 4.3 — medium — Zerocopy decoder allocates an owned `String` per map KEY although keys go straight into the interner
- File:line: `src/codecs/interned/messagepack.rs:130-140` (`read_str` always `Ok(s.to_string())`), consumed at `:324-341` (`decode_map` → `intern_string_key`).
- Issue: each wire key is materialized as a heap `String` purely to hash it — against an interner whose hot path (`interner.rs:141-147`, `UserKey: Borrow<str>`, "no String alloc on cache hits, the 99% case") exists precisely to make lookups allocation-free. On stored-form decodes the field-name set is warm, so every row re-pays f allocations that buy nothing. Values must be owned anyway; keys are the pure waste.
- Failure scenario: whole-record decodes (WAL recovery replays; benches `codec_msgpack.rs:106` as the reference decode) allocate one transient `String` per key per row — tens of thousands of short-lived allocs/sec for a 30-field table at batch-recovery speed.
- Suggested fix: make `read_str` borrow (`fn read_str<'a>(cur: &mut Cursor<&'a [u8]>, len) -> Result<&'a str, CodecError>` via `std::str::from_utf8` on the subslice, then advance); pass `&str` to `intern_string_key` (no signature change). Storage-bytes round-trip tests already pin behavior.

### 4.4 — medium — `query_value_to_storage_bytes_into` defeats its own scratch-buffer purpose — capacity resets to 0 every call
- File:line: `src/codecs/interned/messagepack.rs:890-910` (`Ok(Bytes::from(std::mem::take(scratch)))` at `:909`).
- Issue: the doc promises capacity retention ("eliminates the +1 alloc + memcpy per row that was causing the L12 regression on N=1"), but `mem::take` moves the grown Vec into the returned `Bytes`, leaving `scratch` at capacity 0 — from call #2 the writer reallocates from nothing and regrows geometrically. Steady-state allocation churn equals plain `to_vec`; only round 1 benefits.
- Failure scenario: `shamir-engine/src/query/batch/query_runner.rs:1792` and `shamir-engine/src/table/write_exec.rs:250` pass `&mut scratch` inside per-row loops exactly for the promised reuse; a batch INSERT of N rows still performs N independent growth sequences. No test asserts capacity retention, so the regression this function was written to fix is silently back.
- Suggested fix: restore the buffer before returning (capture `capacity()`, `mem::take`, then `scratch.reserve_exact(cap.max(64))`) — not `Bytes::copy_from_slice`, which reintroduces the memcpy; add a test asserting `scratch.capacity()` survives a second call.

### 4.5 — low — Raced `touch_ind` cold-misses permanently leak interned ids — interner memory tracks racing touches, not distinct names
- File:line: `src/core/interner/interner.rs:154-166` (`fetch_add` before the forward-map CAS; lost races "silently leaked"), `:243-260` (`grown_reverse` sized to the max leaked id). *(same race family as 2.4 but a distinct consequence: leaked slots vs transient `None`)*
- Issue: documented and accepted ("small leaks are harmless"), but under a thundering-herd cold touch (many threads racing the FIRST touch of the same new name — schema-migration fan-out, replay burst) every loser burns a `fetch_add` slot: id space, the doubling-grown spine (length ≥ max raced id), and one wasted `Arc<str>` per race. Ids never recycle, so growth is proportional to total touches including races, forever; amortized per-op cost stays O(1).
- Failure scenario: K threads cold-touch one new field name → up to K−1 leaked slots; repeated across hundreds of racing names during recovery, the spine inflates well past the distinct-name count, growing every later `entries_after` persist-delta scan.
- Suggested fix: keep the leak for simplicity but move the id reservation inside the `Entry::Vacant` arm (reserved-but-unused ids stop advancing the high-water mark consumed by persistence scans), or document a measured bound on cold-miss races.

### 4.6 — low — `merge_storage_bytes` allocates intermediate Vecs per NEW set_map entry before copying into the output buffer
- File:line: `src/codecs/interned/messagepack.rs:658-671` (`rmp_serde::to_vec(key)` + `val.to_bytes()`, then `buf.extend_from_slice`).
- Issue: the rest of the function is allocation-free (verbatim span copies, capacity pre-estimate at `:634`), but each new-entry encode builds a throwaway heap Vec, memcpy'd once and dropped — the exact pattern `query_value_to_storage_bytes_into` was created to remove (#61/L12 history).
- Failure scenario: an UPDATE adding many fields to a wide record allocates/frees 2 Vecs per new field; churn scales with new-field count × batch size. Correctness unaffected.
- Suggested fix: serialize directly into `buf` via `rmp_serde::encode::write(&mut buf, key)` and a small streaming `Serialize` wrapper for values.

### 4.7 — low — Authorization helpers allocate fresh Strings + Vecs per check on the access-gate path
- File:line: `src/access.rs:504-546` (`parent()` clones db/store/table/name Strings per level), `:550-558` (`ancestors()` builds a `Vec<ResourcePath>`).
- Issue: every traversal decision re-materializes the ancestor chain even though callers typically hold the originals. Depth is bounded (~≤5), so constant-factor, not asymptotic — but it runs per-op in front of `permits()` on a path the code itself calls "on the hot path" (`AccessError` doc, `:626-628`).
- Failure scenario: per-request authorization does ~4 Vec+String-set allocations that exist only to be pattern-matched and dropped; visible as allocator overhead under high rps, never as a wrong answer.
- Suggested fix: an `ancestors_with<&F>` closure-callback variant (walk without collecting), or borrow-based ancestors returning segment slices.

### 4.8 — nit — Lazy aggregate cursors pay an eager full-subtree validation walk, then walk again when consumed
- File:line: `src/record_view/lens.rs:625-659` (`borrow_seq_body`/`borrow_map_body` run `skip_value` over every element/pair just to bound the slice).
- Issue: `read_value` returns "lazy" cursors only after eagerly skipping the entire subtree for truncation validation; consumers then decode the same bytes twice. This buys untrusted-input safety and exact bounds — a deliberate tradeoff recorded here only so it isn't double-counted when profiling filter-heavy array workloads.
- Failure scenario: none (correctness-driven); ~2× bytes-touched per consumed aggregate.
- Suggested fix: optionally cache validated spans, or document the double-walk in module docs so lens consumers budget for it.

### 4.9 — nit — `QueryValue::set_path` builds the error-message path prefix during successful traversals too
- File:line: `src/types/value.rs:591-594` (`walked.push_str(segment)` on the success path per intermediate segment).
- Issue: `walked` exists solely to render `ValueError::NotAMap` but is incrementally built (allocation + copy per segment) even when traversal succeeds.
- Failure scenario: none — constant-factor waste in a mutation helper used by update-paths.
- Suggested fix: assemble `walked` lazily on error branches, or accept the cost with a one-line comment stating the tradeoff.

## 5. api-wire-protocol

**The wire layer is mechanically sound** — the id-keyed encoder/decoder pairs and the lens are byte-exact, parity-tested, depth-capped, untrusted-input safe; zero `serde_json` anywhere (builder-rule clean; `mpack!` is the sanctioned typed-literal constructor). The theme-level weakness: the value model advertises 11 typed variants but the wire carries ~7 shapes, pinned by tests yet undocumented at the API level and contradicted by the codecs README — around which sit split-brain decode semantics, a silent-truncating id constructor, error-swallowing trait methods, and a README documenting APIs that no longer exist.

### 5.1 — high — Wire format cannot represent Dec / Big / Set — types silently degrade to Str / List on every encode
- File:line: `src/types/value.rs:72-73` (`Dec`/`Big` → `serialize_str`); `src/codecs/interned/messagepack.rs:375-376`, `:954-955` (same mapping in `InternedRef`/`QvInternedRef`); `:389-398`, `:965-971` (`Set` → seq); decoders map str→`Str` (`messagepack.rs:200-218`) and seq→`List` (`value.rs:187-196`, `lens.rs:519-533`).
- Issue: all three serializers share the contract, so no path round-trips `Value::Dec`, `Value::Big`, or `Value::Set`: write → persist → read always yields `Str(decimal_string)`/`List`. Deliberate and test-pinned (`value_tests.rs:419-496`, `messagepack_tests.rs:437-500`; `kind.rs:19-24` acknowledges it) — but the public API is not honest about it: `mpack!`'s `@` escape invites constructing `Dec`/`Big` (`macros/mpack.rs:31-42`), `query_value_to_inner` preserves them in memory, and `codecs/README.md` claims "MessagePack roundtrip for all types" with a type table omitting Dec/Big entirely.
- Failure scenario: a record written with `field = Dec(123.45)` stores `"123.45"`; after reload it's `Str` — and `scalar_ref_cmp`/`scalar_ref_cmp_qv` have NO `Str`↔`Dec`/`Big` bridge (`scalar_ref.rs:151-202` returns `None`), so `WHERE price = 123.45::dec` silently never matches a row written through exactly such a literal. A persisted `Set{3,1,2}` re-reads as `List[3,1,2]`: dedup/ordering semantics change shape silently and `PartialEq` calls List ≠ Set, breaking pre/post-reload equality.
- Suggested fix: tag the flattened variants on the wire (MessagePack ext codes — ext already collapses to Bin on read, so a versioned escape exists) or a per-record schema byte. Minimum viable honesty: correct the README claim + type table, document the lossy rule on `Value`, and either add exact Str→Dec/Big fallback arms to `scalar_ref_cmp(_qv)` or reject Dec/Big literals at the builder boundary.

### 5.2 — medium — Split-brain decode contract for msgpack u64 > i64::MAX (Big vs Str depending on decoder)
- File:line: `src/types/value.rs:142-155` (serde visitor promotes to `Big` — the "Unified u64 contract, fix FG-1"); `src/codecs/interned/messagepack.rs:183-190` (storage zerocopy decoder → `Str`); `src/record_view/lens.rs:610-620` (`uint_to_record_value` → `RecordValue::Str`).
- Issue: identical wire bytes (raw uint above `i64::MAX`) decode as `InnerValue::Big` via the serde/`from_bytes` path but `InnerValue::Str` via storage/lens. The lens doc ("mirrors the tree") is true only vs the zerocopy decoder. Same file also still does the wrap-cast the FG-1 comment condemns: `From<usize> for Value<String>` uses `v as i64` (`value.rs:660-664`), inconsistent with `From<u64>` directly above.
- Failure scenario: a client/WASM guest emits a native uint > i64::MAX; reading via `QueryValue::from_bytes` yields `Big(...)`, reading the same record from storage yields `Str("...")` — cross-type `Value::eq` is false, so one logical field compares unequal across paths and type-dispatch sees different discriminants for one input.
- Suggested fix: pick one contract (recommend `Str` everywhere — the encoder can never emit raw >i64::MAX ints anyway), update `visit_u64` + the FG-1 comments to say why, align `From<usize>` with `From<u64>`, and add a parity test pinning Big-vs-Str agreement across both decoders.

### 5.3 — medium — `RecordId::system()` silently truncates names longer than 12 bytes → durable-ID collisions
- File:line: `src/types/record_id.rs:95-103` (truncation, verified), `:18`/`:107-109` (`SYSTEM_RECORD_PREFIX`/`is_system`). *(also flagged by error-handling-lifecycle #6 — one defect, two lenses)*
- Issue: system ids copy the name into 12 bytes; anything longer is truncated with no signal (`Self`, not `Result`/`Option`). Two distinct system names sharing a 12-byte prefix alias to one identity — and being deterministic persistent metadata ids, the collision is undetectable by caller or victim. No test asserts distinct names stay collision-free.
- Failure scenario: `RecordId::system("index_build_meta_v2")` and `system("index_build_meta_v3")` mint the identical 16-byte id; catalogue/metadata writes under the second name land on the first identity, quietly, forever.
- Suggested fix: validate length at construction (`Result<RecordId, RecordIdError>` or invariant `panic!` per house rules for `>12`), keep a `system_truncating()` if callers genuinely rely on prefix aliasing; add a distinct-names-never-alias test.

### 5.4 — medium — *(primary: same as 1.1)* — ±0.0 violates the Hash/Eq contract as a wire/dedup-visible behavior
- API framing of **1.1** (one defect, two lenses).

### 5.5 — medium — `RecordRef::to_query_value` swallows de-intern errors into `QueryValue::Null`
- File:line: `src/record_view/record_ref.rs:222-224` (impl for `InnerValue`), `:348-350` (impl for `RecordView`); trait doc `:106-108` documents the swallow; related: `HavingView::materialize_at`'s container→`Null` (finding 1.2) and `query_value_to_inner_value` (`:567-582`).
- Issue: the codec functions correctly return `Result`, but the public trait wrapper flattens a missing interner key (stale reverse-snapshot, genuine corruption) into an empty result — and `Null` is also a legal data value, so failure is indistinguishable from a legitimately null record.
- Failure scenario: a cache-stale interner during failover renders every projected row as `QueryValue::Null`; callers log/store empty rows instead of surfacing an error and retrying — while the closure twin `record_view_deintern_with` explicitly designs FOR retry-on-cache-miss, making the trait-level swallowing inconsistent within the same module.
- Suggested fix: change the trait method to `Result<QueryValue, CodecError>` (pre-1.0, `publish = false`), or add `try_to_query_value` alongside and deprecate the swallowing form.

### 5.6 — medium — `src/codecs/README.md` documents APIs that no longer exist (and wrong semantics)
- File:line: `src/codecs/README.md:13-25` (file tree listing `legacy_text.rs`, `legacy/tools.rs`), `:63-101` (`InternedCodec` trait, `CodecFormat` enum), `:225-300` (`text_to_inner`; "deintern_key … Panics if key not found"), `:332-344` (type table omits Dec/Big), `:426-439` ("MessagePack roundtrip for all types").
- Issue: the actual surface (`codecs/mod.rs`, `interned/mod.rs`) has no `InternedCodec`/`CodecFormat`, no legacy files, no `TransformResult`; `deintern_key` returns `Result<_, CodecError>` (`common.rs:24-28`) rather than panicking (`codec.rs:7-9` confirms the removals). A live README inside `src/` claiming phantom APIs and wrong panic semantics is interface drift future work will copy from.
- Failure scenario: a contributor implements an ACL/decode feature against `CodecFormat::LegacyText` or relies on `deintern_key` panicking on corruption; neither matches reality; review time is wasted rediscovering the real API.
- Suggested fix: rewrite the README around the current tree (`Codec<T>` + interned free functions + projection/validate_keys + `merge_storage_bytes`), delete the Legacy sections, include the Dec/Big/Set flattening table from 5.1.

### 5.7 — low — *(primary: same as 6.2)* — two rival public `CodecError` enums
- API framing of **6.2** (one defect, three lenses; the nit-bundle mention in correctness-tdd #10 folds there too).

### 5.8 — low — *(primary: same as 6.3)* — stringly-typed errors on public interner APIs
- API framing of **6.3** (one defect, two lenses).

### 5.9 — low — `ResourcePath` renders URIs (`Display`) but has no parser; rendering duplicated cross-crate
- File:line: `src/access.rs:561-588` (`db://`, `fn://`, `user://`, `group://` formats); duplicated independently in `crates/shamir-query-types/src/hmac.rs:186-189` (`db://` rebuilt for HMAC canonical strings); no `FromStr for ResourcePath` anywhere (workspace grep: 0 hits). *(same render sites as 3.8 but a different defect: round-trip vs forgery)*
- Issue: one-way encoding, encoded twice. The HMAC signing format (a security surface) and the display format are maintained in two crates with no shared definition or parse round-trip.
- Failure scenario: an added variant or formatting tweak in one renderer desyncs signature computation from audit/error output; wire clients receiving `err.path` strings cannot reconstruct the typed path.
- Suggested fix: move canonical encoding (+ a total `parse` if the grammar is closed) into shamir-types beside `Display`, and make `hmac.rs` delegate to it.

### 5.10 — low — `ResourceMeta::inject_into` silently no-ops on non-map records
- File:line: `src/access.rs:245-261` (plus `:304-319` duplicate insert logic in `to_query_value`).
- Issue: if `rec` is not a `Map`, `inject_into` returns `Ok(())`-shaped `()` having written nothing: ACL owner/group/mode fields vanish from the persisted catalogue record with no signal. The mutation cannot be observed missing until a permission check reads absent defaults.
- Failure scenario: a caller passes a freshly-built non-map catalogue row (variant change upstream) → resource silently persists open/System-owned instead of creator-owned; privilege decisions then run on wrong metadata.
- Suggested fix: return `Result<(), ValueError>` (`NotAMap`) mirroring `Value::set_path`'s convention; share one insertion helper between `inject_into`/`to_query_value`.

### 5.11 — nit — API-polish bundle (five items; two folded into other findings)
- (a) **`Default for RecordId` generates a fresh random ID** — `record_id.rs:127-131`: `Self::default()` on a `Copy`/hash-keyed id type minting a new random identifier per call invites silent divergence (`..Default::default()` clones differently each time). Deprecate in favor of explicit `new()`/`nil()`. Nit.
- (b) **`from_ts` before CUSTOM_EPOCH produces system-prefixed ids** — `record_id.rs:41-53`: same root cause as **1.3**, counted once there (and verified during synthesis: `saturating_sub` clamps pre-epoch times to 0 → all-zero timestamp half → `is_system() == true`). Folded.
- (c) **`UserValue` deprecated, twin `QueryValue` is not** — `value.rs:25-31`: `QueryValue` is the identical alias used pervasively in production (`access.rs`, shamir-query-types wire structs), contradicting the deprecation note's "production should use InnerValue directly". Either un-deprecate the string-keyed family or state precisely who must migrate. Nit.
- (d) **bincode.rs malformed stale doctests** — `basic/bincode.rs:24-33`, `:42-51`: tripled duplicated `# #[derive]` lines and nonexistent paths; same defect as **1.10**, counted once there. Folded.
- (e) **`MpackIntoValue` documented as sealed but isn't** — `macros/mpack.rs:286-293`: no private supertrait binds the seal; downstream impls would compile. Either add `: __Sealed` or drop the word. Nit.

## 6. error-handling-lifecycle

**Error-type hygiene is largely exemplary** — `ValueError`, `Base58Error`, `RecordIdError`, `SortCodecError`, `RecordViewError` are proper thiserror enums; the lens lives up to "never panics on untrusted bytes" with load-bearing error-path tests (`record_view/tests/error_tests.rs`: depth cap, reserved marker, mid-skip truncation, garbage-bytes no-panic). Weak spots: the abort-class decode paths (3.1), a hand-rolled rival error enum (6.2), stringly-typed interner errors (6.3), the fail-open mode fallback (3.2), and uneven error-path coverage (no test drives a huge-header decode, malformed mode, `SecretString` lifecycle, post-rollback state, or duplicate-id hydration).

### 6.1 — high — *(primary: same as 3.1)* — unbounded preallocation from msgpack headers (abort, unlike the capped tree visitor)
- Error-path framing of **3.1** (one defect, three lenses).

### 6.2 — medium — Two public, rival `CodecError` enums; the basic/bincode one is hand-rolled, violating the thiserror rule
- File:line: `src/codecs/error.rs:3-9` (thiserror `Encode/Decode`) vs `src/codecs/basic/bincode.rs:7-22` (manual `pub enum CodecError { Serialize(String), Deserialize(String) }` with hand-written `Display`/`Error`). *(primary of the group: also style-claude-md #2, api-wire-protocol #8, and the name-shadowing half of correctness-tdd #10 — one defect, three lenses + a nit overlap; consumed cross-crate via `shamir-engine/src/table/interner_manager.rs:12`, `record_counter.rs:20`, shamir-index tests)*
- Issue: CLAUDE.md mandates thiserror for library error enums; one crate exports two unrelated enums with identical simple names, and `codecs/mod.rs:12` re-exports `to_bytes`/`from_bytes` right next to the *other* `CodecError` — a caller importing `codecs::{to_bytes, CodecError}` gets functions whose `Err` type does not unify with the imported error: no `?`-propagation, no `From`, easy mis-mapping. The bincode variant has no structured fields and bypasses workspace error hygiene.
- Failure scenario: downstream glue does `codecs::to_bytes(&x)?` against `codecs::CodecError` and fails to compile, or hand-wraps via strings losing variant information on the bincode path only; a caller pattern-matching `CodecError::Encode(..)` silently misses bincode failures styled `Serialize(..)`.
- Suggested fix: converge on one type — fold the bincode wrapper onto the shared thiserror enum (map Serialize→Encode, Deserialize→Decode) or rename it `BincodeError` (private to the module if API stability requires); either way, thiserror.

### 6.3 — medium — `Interner::touch_ind` returns a `Result` with no reachable `Err`; `touch_with_id` returns stringly-typed errors
- File:line: `src/core/interner/interner.rs:138` (signature; both match arms `:146`, `:166` return `Ok`), `:348` (`Result<(), String>`; error sites `:352`, `:363`, `:375`, `:396`, `:441-446`); amplified by `codecs/interned/common.rs:13-18` formatting the phantom error. *(also flagged by api-wire-protocol #9 — one defect, two lenses)*
- Issue: `touch_ind` promises failure it can never deliver (`&'static str`, zero production sites), forcing every caller through `unwrap_or_else`/`?` plumbing for an impossible branch — and the phantom string gets laundered into `CodecError::Decode("Failed to intern key …")`, so fake decode errors can appear in logs. Meanwhile `touch_with_id`, whose failures are real (reserved id 0, remap conflict, id collision, race + rollback), reports them as formatted `String`s: no exhaustiveness, no typed matching by WAL recovery.
- Failure scenario: recovery code needs to distinguish "benign replay idempotence" from "id collision — persistent-state divergence" and can only substring-match English messages; message edits break recovery handling invisibly.
- Suggested fix: make `touch_ind` infallible (`-> TouchInd`) and delete the dead `Err` arm + phantom formatting; introduce a small `InternerError::{ReservedId, NameRemap{..}, IdCollision{..}, Race{..}}` thiserror enum for `touch_with_id`, keeping prose in `#[error]`.

### 6.4 — medium — *(primary: same as 3.2)* — `ResourceMeta::from_record` fails open on unparsable mode
- Error-handling framing of **3.2** (one defect, two lenses).

### 6.5 — low — `Interner::with_state` silently collapses duplicate ids/names in hydrated state
- File:line: `src/core/interner/interner.rs:121-127` (`map_user_to_interned.insert(...)` last-wins; `let _ = reverse[id].set(arc);` discards both `OnceLock` overwrite failures).
- Issue: hydration (persist-file/recovery input) performs no consistency validation. Two entries sharing an id leave the reverse spine holding whichever `Arc<str>` came later while the forward map keeps a name that de-interns to a *different* string; duplicate `(name, id)` pairs likewise overwrite without error. The function cannot express "input was inconsistent".
- Failure scenario: a torn/corrupted interner persist file hydrates "successfully"; post-restart, `get_ind("email")` returns an id whose `get_str` yields `"email_backup"` — silent cross-field data mixing with no error signal at boot.
- Suggested fix: scan `initial_data` once (already iterated) and return `Result<Self, InternerError>` on duplicate ids or conflicting mappings, or at minimum `log::warn!` and skip deterministically.

### 6.6 — low — *(primary: same as 5.3)* — `RecordId::system` truncates names to 12 bytes with no collision detection
- Error-handling framing of **5.3** (one defect, two lenses).

### 6.7 — low — *(primary: same as 1.8)* — `touch_ind`/`touch_with_id` race guarded only by `debug_assert!`; release drops the write silently
- Error-handling framing of **1.8** (one defect, two lenses). Additional emphasis from this lens: per project rules `panic!` is reserved for exactly this class (programmer/invariant bugs), yet the breach is invisible in production — escalate to an unconditional invariant failure or at minimum `log::error!` on the discarded write.

### 6.8 — low — *(primary: same as 3.6)* — `SecretString::Drop` hand-written `unsafe` + untested lifecycle
- Error-handling framing of **3.6** (one defect, two lenses).

### 6.9 — nit — `trace_access` — a `Result` that is always `Ok`, with an error type the crate itself never constructs
- File:line: `src/access.rs:657-660`; `AccessError` defined `:621-630` (zero in-crate construction sites).
- Issue: deliberate and extensively documented (renamed from `authorize` precisely to telegraph "not a gate") — shape, not bug: an infallible fn returning `Result<(), AccessError>` invites `?`-chains at call sites against an error only `shamir-db`'s real gate can produce, blurring which layer denied access. No error-path test is possible for this symbol in-crate.
- Suggested fix: long-term, have observability tracing return `()` and let `authorize_access` own the `Result`; interim, a doc cross-link from `AccessError` ("produced only by shamir-db's facade gate").

### 6.10 — nit — `pos + len` unchecked additions in the tree decoder's `read_str`/`read_bin`
- File:line: `src/codecs/interned/messagepack.rs:133`, `:147`.
- Issue: `len` derives from u8/u16/u32 headers; on 64-bit targets the addition cannot wrap, but the sibling lens code (`lens.rs:202-215`, `borrow_bytes`) uses `checked_add` uniformly as its stated untrusted-input discipline. The asymmetry is a latent panic should either site ever feed a larger cursor or the cast widen.
- Suggested fix: align with the lens: `pos.checked_add(len)` + explicit truncated `CodecError`.

## 7. style-claude-md

**Largely conformant:** every `mod.rs` (lib, types, codecs, core, core/interner, record_view, macros, basic, interned) is re-export-only; tests live in per-module `tests/` directories wired through manifest `mod.rs` files with topic-split coverage across all 21 test-file groups; no inline `#[cfg(test)]` block survives in any implementation file. The structural outliers: `access.rs`'s mega-file, the rival `CodecError` (6.2), and mid-function imports.

### 7.1 — medium — `access.rs` bundles identity, mode-bits, policy and error types into one 716-line file — one-file-one-export violation
- File:line: `src/access.rs:1-716` (15 public items across three loosely-coupled domains: principal/identity projection `OWNER_SYSTEM`/`principal64`/`Actor`; POSIX mode math `Mode`/`MODE_SETUID`/`PermClass`/`Perm`; resource addressing `ResourcePath`; metadata envelope `ResourceMeta`; policy evaluation `Action`/`AccessError`/`trace_access`/`action_perm`/`class_of`/`permits`).
- Issue: CLAUDE.md permits one primary export plus a closely-coupled group; sibling modules in this same crate (`touch_ind.rs`, `value_error.rs`, `record_view/kind.rs`) demonstrate the intended granularity — `access.rs` is the outlier.
- Failure scenario: unrelated access-model edits (e.g. adding an `Action` variant) force re-diffs of a file whose other sections are stable; `git blame` mixes identity-minting with policy changes; reviewers can't tell at a glance which concern a hunk belongs to.
- Suggested fix: split into sibling files (`actor.rs`, `principal.rs`, `resource_path.rs`, `action.rs`, `mode.rs`, `resource_meta.rs`, `policy.rs`, `access_error.rs`) under `src/access/` with a re-export-only `mod.rs` preserving today's `crate::access::*` paths so `shamir-engine`/`shamir-db` keep compiling unchanged.

### 7.2 — medium — *(primary: same as 6.2)* — second public `CodecError` enum with manual impls duplicates the crate's thiserror error type
- Style framing of **6.2** (one defect, three lenses).

### 7.3 — medium — Mid-function imports in production code violate imports-at-top
- File:line: `src/core/interner/interner.rs:161` and `:349` (`use dashmap::mapref::entry::Entry;` duplicated inside `touch_ind` and `touch_with_id`); `src/types/record_id.rs:85` (`use rand::SeedableRng;` inside `fill_random_tail`'s `thread_local!` initializer).
- Issue: none of CLAUDE.md's three narrow exceptions applies; hoisting compiles identically (no other `Entry`/`SeedableRng` referenced in either file's header). The duplicated local `Entry` import invites divergence.
- Failure scenario: readers scanning headers miss trait deps; duplicated local imports drift apart when one is edited; import-audit automation reports false negatives.
- Suggested fix: hoist `Entry` once to `interner.rs`'s header block and delete both locals; hoist `SeedableRng` next to `use rand::RngCore;` in `record_id.rs`.

### 7.4 — low — `types/tests/value_tests.rs` retains the legacy inline `#[cfg(test)] mod tests { ... }` wrapper shape
- File:line: `src/types/tests/value_tests.rs:1-13`.
- Issue: every other test file in the crate (~20) uses flat top-level `#[test]` fns; this lone file nests everything in `#[cfg(test)] #[allow(deprecated)] mod tests { ... }` — precisely the shape CLAUDE.md bans in implementation files and flags as mid-migration to `tests/`. The module-level `#[allow(deprecated)]` also silently widens suppression over the whole file.
- Failure scenario: the file gets copied as a template, propagating the deprecated pattern; the blanket allow masks genuine new uses of deprecated APIs.
- Suggested fix: flatten to top-level `#[test]` fns like siblings, narrowing `#[allow(deprecated)]` to only the `UserValue`-exercising items.

### 7.5 — low — Mid-function imports scattered through test files
- File:line: `src/tests/access_tests.rs:265-266`; `src/core/interner/tests/interner_tests.rs:534,804`; `src/codecs/interned/tests/messagepack_tests.rs:667`; `src/codecs/interned/tests/storage_bytes_tests.rs:438`; `src/codecs/interned/tests/merge_storage_bytes_tests.rs:296,318` (repeats the identical `use crate::record_view::RecordView;` in two adjacent functions); `src/record_view/tests/scalar_ref_cmp_tests.rs:193`; `src/macros/tests/mpack_tests.rs:332`.
- Issue: function-local `use` inside individual `#[test]` fns; none falls under the documented exceptions (`use super::*` / collision-with-comment / cfg-gated macro body).
- Failure scenario: duplicate imports drift out of sync; long test bodies lose readability.
- Suggested fix: hoist each into the owning test file's header.

### 7.6 — nit — Dead "Tests" section banner left behind after inline-test extraction
- File:line: `src/core/sort_codec.rs:152-154`.
- Issue: a `// Tests` divider banner followed by nothing — scaffolding left from before the tests moved to `core/tests/sort_codec_tests.rs`.
- Failure scenario: minor — misleads a reader into expecting content below.
- Suggested fix: delete the banner (or replace with a pointer doc-comment to the test file).

### 7.7 — nit — Inconsistent test-manifest visibility across `tests/mod.rs` files
- File:line: e.g. `src/tests/mod.rs`, `src/core/tests/mod.rs`, `src/core/interner/tests/mod.rs`, `src/codecs/basic/tests/mod.rs`, `src/codecs/interned/tests/mod.rs` use `pub mod x_tests;`; `src/types/tests/mod.rs`, `src/record_view/tests/mod.rs`, `src/macros/tests/mod.rs` use private `mod` (some with redundant extra `#[cfg(test)]` on top of the parent's existing gate); `core/interner/mod.rs` wires tests as `pub mod tests` while every other parent uses private `mod tests`.
- Issue: CLAUDE.md's example shows uniform `pub mod value_tests;` manifests; the crate mixes freely. Purely cosmetic — visibility differences are unobservable under the parent's `#[cfg(test)]` gate.
- Failure scenario: none functional; a reader comparing modules cannot infer convention.
- Suggested fix: pick one form and normalize in a style-only commit per CLAUDE.md's style-commit rule.

---

## Finding counts

Raw lens-tagged findings across the 7 files (matches the sweep's per-crate row: 64):

| Severity | Lens-tagged findings | Deduped distinct defects | Dedup groups (primary noted; secondaries listed) |
|---|---|---|---|
| critical | 0 | 0 | — |
| high | 5 | 4 | 1.1 (±0.0 Hash/Eq) ← also 5.4 · 3.1 (prealloc DoS) ← also 2.2, 6.1 · 4.1 (projection O(k·f)) · 5.1 (Dec/Big/Set lossy) |
| medium | 23 | 17 | 1.2 · 1.3 ← also 5.11b · 1.4 · 3.2 ← also 6.4 · 3.3 · 3.4 · 4.2 (for_each_field O(f²)) ← also 1.5, 2.1 · 4.3 · 4.4 · 5.2 · 5.3 ← also 6.6 · 5.5 · 5.6 · 6.2 (CodecError dup) ← also 7.2, 5.7, 1.10-part · 6.3 (stringly interner errors) ← also 5.8 · 7.1 · 7.3 |
| low | 21 | 16 | 1.6 · 1.7 · 1.8 (debug_assert race) ← also 6.7 · 2.3 · 2.4 · 3.5 · 3.6 (SecretString) ← also 6.8 · 3.7 · 4.5 · 4.6 · 4.7 · 5.9 · 5.10 · 6.5 · 7.4 · 7.5 |
| nit | 15 | 13 | 1.9 · 1.10 ← also 5.11d · 2.5 · 3.8 · 4.8 · 4.9 · 5.11 (3 surviving: a, c, e) · 6.9 · 6.10 · 7.6 · 7.7 |
| **total** | **64** | **50** | |

Deduplicated defect census: **0 critical, 4 high, 17 medium, 16 low, 13 nit = 50
distinct defects** (64 lens-tagged findings). Ten dedup groups collapsed 14
secondary tags; the api #11 bundle's five nits count as three distinct after two
folds (b→1.3, d→1.10).

## Fix Plan

**P0 — before anything else ships from this crate**
1. **Clamp header-driven preallocation everywhere.** Hoist/share `SANE_PREALLOC_CAP` and apply `min(header, cap)` at `messagepack.rs:305`, `:318`, and `merge_storage_bytes`' `:581`/`:584-585`; add huge-header/short-body regression tests asserting `Err` for both `msgpack_to_inner` and `merge_storage_bytes`. Closes **3.1/2.2/6.1** — the 5-byte process-abort on the WAL-recovery and S-write decode paths.
2. **Canonicalize ±0.0 in `Value::F64`'s `Hash`** next to the NaN arm and flip `test_f64_neg_zero_hash` to assert hash equality (Red first: the current test asserts the bug). Closes **1.1/5.4**.
3. **Make the Dec/Big/Set wire contract honest.** Minimum: fix `scalar_ref.rs` to bridge `Str`↔`Dec`/`Big` (or reject Dec/Big literals at the builder boundary), correct `codecs/README.md`'s round-trip claim and type table, document the lossy rule on `Value`. Closes the wrong-results half of **5.1**; the full ext-tag round-trip format is P1 item 8.
4. **Kill the per-row re-scans with a shared span index.** Extend/reuse `FieldIndex::index()` to record value byte spans; emit projections by probing the index and materialize `for_each_field` from recorded spans (decode each value exactly once). Byte-parity tests already pin output. Closes **4.1, 4.2** (and the correctness/concurrency framings 1.5/2.1).

**P1 — soon**
5. **Fix the `RecordId` epoch/zero-prefix hole:** stop deriving `is_system` from bare leading zeros (reserved bit or range check), validate or clamp `from_ts`/`from_ts_seq` inputs, document both boundaries, and add boundary tests. Closes **1.3** (incl. 5.11b).
6. **Fail closed in `ResourceMeta::from_record`:** owner-only fallback (or `Result`) for unparsable `mode`, plus a red test and a doc note beside the owner-collapse rationale. Closes **3.2/6.4**.
7. **Put a deadline on `principal64_from_username`:** track #559 as blocking-security in the doc; whitelist the two call sites until the migration lands. Closes **3.4** (test coverage per **1.7**).
8. **Versioned Dec/Big/Set wire tags** (MessagePack ext codes or schema byte) for true round-trip. Completes **5.1**.
9. **Unify the u64>i64::MAX decode contract** (recommend `Str` everywhere), align `From<usize>`, add the cross-decoder parity test. Closes **5.2**.
10. **Stop swallowing de-intern errors:** `try_to_query_value` (or change the trait signature) so stale-interner failover is visible and retryable. Closes **5.5**.
11. **Interner error hygiene:** make `touch_ind` infallible, introduce the `InternerError` thiserror enum for `touch_with_id`, delete the phantom `intern_string_key` formatting. Closes **6.3/5.8**.
12. **Converge the `CodecError` enums** onto one thiserror type (or rename `BincodeError`); also fix/delete the malformed doctests. Closes **6.2/7.2/5.7** and the **1.10**/5.11d doctest half.
13. **Validate `RecordId::system` name length** (Result or invariant panic) + distinct-names-never-alias test. Closes **5.3/6.6**.
14. **Bound untrusted interner growth:** distinct-keys quota at the `touch_ind` gate (or an explicit documented invariant owning the deviation from pillar 4's rationale). Closes **3.3**.
15. **Escalate the cross-API interner race** from `debug_assert!` to a loud release-time failure (typed Err via item 11, or `log::error!` minimum), and document the transient forward-only window on `get_str`/`get_ind`. Closes **1.8/6.7** and **2.4**.
16. **Cover the untested contracts:** `HavingView` suite (pinning 1.2's chosen semantic), generation/owner_field/principal64/into_inner/WasmCompiler (1.7's list), malformed-mode test (with item 6), capacity-retention test (with item 17). Closes **1.4, 1.7, 1.2** (decision-pinning part).
17. **Restore scratch-buffer reuse** in `query_value_to_storage_bytes_into` (reserve-before-return) + capacity test. Closes **4.4**.
18. **Borrow map keys in the zerocopy decoder** (`read_str` → `&str`). Closes **4.3**.
19. **Split `access.rs`** into `src/access/` sibling files behind a re-export-only `mod.rs`; hoist the mid-function imports. Closes **7.1, 7.3**.
20. **Rewrite `src/codecs/README.md`** around the current surface (delete phantom Legacy APIs, fix the `deintern_key` panic claim, include the flattening table). Closes **5.6**.

**P2 — backlog**
21. `mpack!` object-value arms for multi-token literals + tests. Closes **1.6**.
22. `SecretString` hygiene: `Zeroize for String` instead of the `unsafe` block, comparison-footgun doc warning, lifecycle test. Closes **3.6/6.8**.
23. Trusted-input-only doc (or depth cap) for `bincode::from_bytes`; bincode 2.x migration note. Closes **3.5**.
24. Document `RecordId` ids as non-capability material. Closes **3.7**.
25. Hydration validation in `Interner::with_state` (detect duplicate ids/conflicting mappings). Closes **6.5**.
26. `ResourcePath` canonical encoding + parser, with `hmac.rs` delegating; escape control chars in the `Display`/trace paths. Closes **5.9, 3.8**.
27. `inject_into` returns `Result` on non-map records; share the insertion helper. Closes **5.10**.
28. Perf-polish batch: stream new-entry encodes in `merge_storage_bytes` (**4.6**), closure-based `ancestors` (**4.7**), interner id-reservation inside the Vacant arm (**4.5**), lazy `walked` in `set_path` (**4.9**), double-walk note for lazy cursors (**4.8**).
29. Doc/test nits sweep: refresh `core/README.md`'s locking model (**2.5**), fix the vacuous record_id test (**1.9**), bincode enum rename/dead variants/`NotAStr` (**1.10** remainder), `trace_access` doc cross-link (**6.9**), `checked_add` in `read_str`/`read_bin` (**6.10**), flatten `value_tests.rs` (**7.4**), hoist test imports (**7.5**), delete the sort_codec banner (**7.6**), normalize test manifests (**7.7**), `Default for RecordId` deprecation (**5.11a**), `UserValue` deprecation decision (**5.11c**), `MpackIntoValue` sealing (**5.11e**), growth-stall comment next to the F-9 exception (**2.3**).
