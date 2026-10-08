<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-types — error-handling-lifecycle revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Header allocation, malformed-mode fallback, conflicting hydration, and conditional cross-API divergence remain. Duplicate errors and stringly results are API hygiene. Several lifecycle and diagnostic claims need qualification.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 10 | 9 | 0 | 0 | 0 | 0 | 1 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Unbounded preallocation from attacker-controlled msgpack headers -- allocator abort, unlike the capped tree visitor

Status: `confirmed-open`. Current risk: `high`.

Array/map custom decoding and merge entries/id maps still allocate directly from unvalidated counts. Source proves pathological requested capacity; allocation failure depends on platform/allocator. No huge-count short-body regression covers these sites. The current reachable S-write validator conversion separately preallocates from view.len().

Evidence: [crates/shamir-types/src/codecs/interned/messagepack.rs:305](../../../../../crates/shamir-types/src/codecs/interned/messagepack.rs#L305); [crates/shamir-types/src/codecs/interned/messagepack.rs:318](../../../../../crates/shamir-types/src/codecs/interned/messagepack.rs#L318); [crates/shamir-types/src/codecs/interned/messagepack.rs:581](../../../../../crates/shamir-types/src/codecs/interned/messagepack.rs#L581); [crates/shamir-types/src/codecs/interned/messagepack.rs:585](../../../../../crates/shamir-types/src/codecs/interned/messagepack.rs#L585); [crates/shamir-types/src/types/value.rs:191](../../../../../crates/shamir-types/src/types/value.rs#L191); [crates/shamir-types/src/codecs/interned/codec.rs:154](../../../../../crates/shamir-types/src/codecs/interned/codec.rs#L154); [crates/shamir-engine/src/table/write_exec.rs:382](../../../../../crates/shamir-engine/src/table/write_exec.rs#L382).

<a id="review-2"></a>

### Claim 2 — Two public, rival `CodecError` enums; the basic/bincode one is hand-rolled, violating the documented thiserror rule

Status: `confirmed-open`. Current risk: `low`.

The manual Serialize/Deserialize enum still differs from the thiserror Encode/Decode enum adjacent to convenience reexports. No conversion joins them. This is a source/API consistency issue, not a runtime High defect.

Evidence: [crates/shamir-types/src/codecs/basic/bincode.rs:8](../../../../../crates/shamir-types/src/codecs/basic/bincode.rs#L8); [crates/shamir-types/src/codecs/basic/bincode.rs:22](../../../../../crates/shamir-types/src/codecs/basic/bincode.rs#L22); [crates/shamir-types/src/codecs/error.rs:4](../../../../../crates/shamir-types/src/codecs/error.rs#L4); [crates/shamir-types/src/codecs/mod.rs:12](../../../../../crates/shamir-types/src/codecs/mod.rs#L12).

<a id="review-3"></a>

### Claim 3 — `Interner::touch_ind` returns a `Result` with no reachable `Err`; `touch_with_id` returns stringly-typed errors

Status: `confirmed-open`. Current risk: `low`.

touch_ind's production return branches are both Ok; touch_with_id's conflicts remain formatted strings. intern_string_key's error mapper is therefore dead under current implementation, not a mechanism that can actually emit fake decode errors. Inspected recovery callers uniformly propagate conflicts rather than substring-classify them.

Evidence: [crates/shamir-types/src/core/interner/interner.rs:146](../../../../../crates/shamir-types/src/core/interner/interner.rs#L146); [crates/shamir-types/src/core/interner/interner.rs:166](../../../../../crates/shamir-types/src/core/interner/interner.rs#L166); [crates/shamir-types/src/core/interner/interner.rs:348](../../../../../crates/shamir-types/src/core/interner/interner.rs#L348); [crates/shamir-types/src/codecs/interned/common.rs:17](../../../../../crates/shamir-types/src/codecs/interned/common.rs#L17); [crates/shamir-engine/src/tx/recovery.rs:491](../../../../../crates/shamir-engine/src/tx/recovery.rs#L491).

<a id="review-4"></a>

### Claim 4 — `ResourceMeta::from_record` fails open: invalid `mode` silently becomes `Mode::OPEN` (0o777)

Status: `confirmed-open`. Current risk: `medium`.

The parser still conflates invalid present metadata with intentionally compatible absent metadata and selects OPEN. Existing types tests cover absent and valid modes, not invalid-present modes.

Evidence: [crates/shamir-types/src/access.rs:275](../../../../../crates/shamir-types/src/access.rs#L275); [crates/shamir-types/src/tests/access_tests.rs:238](../../../../../crates/shamir-types/src/tests/access_tests.rs#L238); [crates/shamir-types/src/tests/access_tests.rs:257](../../../../../crates/shamir-types/src/tests/access_tests.rs#L257).

Grouping/duplicate: `security-crypto.md:2`. This row is not another independent defect.

<a id="review-5"></a>

### Claim 5 — `Interner::with_state` silently collapses duplicate ids/names in hydrated state

Status: `confirmed-open`. Current risk: `medium`.

Conflicting mappings are not validated. Forward insert is last-wins; reverse OnceLock is first-wins, contrary to the report's later-name claim. Production hydration concatenates persisted chunks then calls with_state. Identical repeated pairs are legitimate under gap recapture and must remain idempotent.

Evidence: [crates/shamir-types/src/core/interner/interner.rs:121](../../../../../crates/shamir-types/src/core/interner/interner.rs#L121); [crates/shamir-types/src/core/interner/interner.rs:124](../../../../../crates/shamir-types/src/core/interner/interner.rs#L124); [crates/shamir-types/src/core/interner/interner.rs:126](../../../../../crates/shamir-types/src/core/interner/interner.rs#L126); [crates/shamir-types/src/core/interner/interner.rs:509](../../../../../crates/shamir-types/src/core/interner/interner.rs#L509); [crates/shamir-engine/src/table/interner_manager.rs:207](../../../../../crates/shamir-engine/src/table/interner_manager.rs#L207); [crates/shamir-engine/src/table/interner_manager.rs:220](../../../../../crates/shamir-engine/src/table/interner_manager.rs#L220).

<a id="review-6"></a>

### Claim 6 — `RecordId::system` truncates names to 12 bytes with no collision detection

Status: `confirmed-open`. Current risk: `medium`.

Constructor truncation and its aliasing test remain. Some caller-level collision fixes do not change this public constructor; arbitrary distinct string names are not injectively encoded.

Evidence: [crates/shamir-types/src/types/record_id.rs:100](../../../../../crates/shamir-types/src/types/record_id.rs#L100); [crates/shamir-types/src/types/tests/record_id_tests.rs:48](../../../../../crates/shamir-types/src/types/tests/record_id_tests.rs#L48); [crates/shamir-index/src/persistence.rs:529](../../../../../crates/shamir-index/src/persistence.rs#L529).

Grouping/duplicate: `api-wire-protocol.md:3`. This row is not another independent defect.

<a id="review-7"></a>

### Claim 7 — Race-between-touch_ind-and-touch_with_id guarded only by `debug_assert!` -- release path silently drops the write

Status: `confirmed-open`. Current risk: `low`.

A losing reverse OnceLock set remains silent in release while the forward map retains the new mapping. The invariant is not enforced by a release-time verdict. Production collision reachability remains conditional; the live drainer means startup-only exclusion cannot simply be assumed.

Evidence: [crates/shamir-types/src/core/interner/interner.rs:223](../../../../../crates/shamir-types/src/core/interner/interner.rs#L223); [crates/shamir-types/src/core/interner/interner.rs:224](../../../../../crates/shamir-types/src/core/interner/interner.rs#L224); [crates/shamir-engine/src/tx/drainer.rs:1116](../../../../../crates/shamir-engine/src/tx/drainer.rs#L1116).

Grouping/duplicate: `correctness-tdd.md:8`. This row is not another independent defect.

<a id="review-8"></a>

### Claim 8 — `SecretString::Drop` uses hand-written `unsafe` where the safe std/trait path exists; lifecycle behaviour untested

Status: `confirmed-open`. Current risk: `low`.

The current feature-gated Drop still zeroizes an exclusive mutable byte slice; lifecycle tests remain absent. into_inner transfers its buffer using mem::take, so source supports ownership handoff. A freed-buffer inspection would be invalid evidence; use a safe controlled lifecycle oracle. Exact zeroize 1.8.2 String replacement source was unavailable.

Evidence: [crates/shamir-types/src/secret.rs:40](../../../../../crates/shamir-types/src/secret.rs#L40); [crates/shamir-types/src/secret.rs:67](../../../../../crates/shamir-types/src/secret.rs#L67); [crates/shamir-types/src/secret.rs:72](../../../../../crates/shamir-types/src/secret.rs#L72); [crates/shamir-types/src/tests/secret_tests.rs:1](../../../../../crates/shamir-types/src/tests/secret_tests.rs#L1); [Cargo.lock:5473](../../../../../Cargo.lock#L5473).

Grouping/duplicate: `security-crypto.md:6`. This row is not another independent defect.

<a id="review-9"></a>

### Claim 9 — `trace_access` -- a `Result` that is always `Ok`, with an error type the crate itself never constructs

Status: `not-applicable`. Current risk: —.

Always-Ok observability is an explicit, extensively cross-linked contract, not a missing error path. Real AccessError construction exists in the facade. Returning unit would be optional API simplification.

Evidence: [crates/shamir-types/src/access.rs:632](../../../../../crates/shamir-types/src/access.rs#L632); [crates/shamir-types/src/access.rs:640](../../../../../crates/shamir-types/src/access.rs#L640); [crates/shamir-types/src/access.rs:657](../../../../../crates/shamir-types/src/access.rs#L657); [crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:863](../../../../../crates/shamir-db/src/shamir_db/shamir_db/access_control.rs#L863).

<a id="review-10"></a>

### Claim 10 — `pos + len` unchecked additions in the tree decoder's `read_str`/`read_bin`

Status: `confirmed-open`. Current risk: `low`.

Both unchecked additions remain. Header lengths fit u32, so the alleged overflow is not reachable on normal 64-bit slices; on 32-bit targets a large Str32/Bin32 length plus cursor can overflow before the bounds check and panic. No source-test coverage pins that target case.

Evidence: [crates/shamir-types/src/codecs/interned/messagepack.rs:133](../../../../../crates/shamir-types/src/codecs/interned/messagepack.rs#L133); [crates/shamir-types/src/codecs/interned/messagepack.rs:147](../../../../../crates/shamir-types/src/codecs/interned/messagepack.rs#L147); [crates/shamir-types/src/codecs/interned/messagepack.rs:215](../../../../../crates/shamir-types/src/codecs/interned/messagepack.rs#L215); [crates/shamir-types/src/codecs/interned/messagepack.rs:230](../../../../../crates/shamir-types/src/codecs/interned/messagepack.rs#L230); [crates/shamir-types/src/record_view/lens.rs:208](../../../../../crates/shamir-types/src/record_view/lens.rs#L208).

## Corrections and qualified non-findings

- with_state's reverse winner is the first name, not the last. Reject conflicts, not harmless identical repeated pairs.
- A Result with an impossible error branch cannot currently generate phantom runtime errors, and String errors do not inherently allocate on success.
- Wrong CodecError imports cause type errors; they do not silently bypass exhaustive matching.
- The bincode rustdoc examples are stale unfenced text, not executable doctests masked only by doctest=false.
- The lens's checked reads and depth cap are source-supported, but index allocation and silent iterator termination qualify the broad untrusted-input guarantee.
- Do not inspect memory after SecretString destruction as a zeroization test oracle.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-types -- Error handling & resource lifecycle

## Summary

The crate's error-type hygiene is largely exemplary: `ValueError`, `Base58Error`, `RecordIdError`, `SortCodecError` and `RecordViewError` are proper `thiserror` enums, the RecordView lens lives up to its "never panics on untrusted bytes" doc contract (bounds-checked reads, depth cap, reserved-marker rejection) and has genuinely load-bearing error-path tests. The weak spots cluster elsewhere: three decode paths allocate upfront capacity straight from attacker-controllable MessagePack headers with no `SANE_PREALLOC_CAP`-style guard (allocator-abort class, which `types/value.rs` explicitly guards against and documents), one module ships a second, hand-rolled `CodecError` that violates the documented thiserror rule, the `Interner` exposes a `Result` that has no possible `Err` arm alongside stringly-typed errors, and one access-control parser fails open by silently coercing an invalid mode to `0o777`. Error-path test coverage is uneven: strong for the lens/codecs, absent for the fail-open mode fallback and the `SecretString` zeroize lifecycle.

## Findings

### 1. Unbounded preallocation from attacker-controlled msgpack headers -- allocator abort, unlike the capped tree visitor

- **File:line:** `crates/shamir-types/src/codecs/interned/messagepack.rs:305` (`Vec::with_capacity(len)` in `decode_array`), `:318` (`new_map_wc(len)` in `decode_map`), `:581` + `:584-585` (`Vec::<Entry>::with_capacity(n_old)` and `TFxMap::with_capacity_and_hasher(n_old, ..)` in `merge_storage_bytes`)
- **Severity:** high
- **Issue:** `Array16/Array32/Map16/Map32` headers declare element counts up to `u32::MAX` (~4.29e9). All three sites allocate `count * elem_size` bytes *before* validating that any corresponding payload exists. `value.rs` faced the exact same hazard and added `SANE_PREALLOC_CAP = 4096` with the rationale "driving `Vec::with_capacity(size_hint)` to multi-GB / abort" (`src/types/value.rs:117-122`), applied in its visitor (`value.rs:191,204,212`). The zero-copy tree decoder and the byte-level merge encoder were never given the equivalent treatment, despite decoding the same storage/WAL byte format. Depth-capping (`MAX_MSGPACK_DEPTH`) does not help here: a *flat* Array32 with a 5-byte body trips nothing before the allocation.
- **Failure scenario:** a corrupted, truncated, or hostile record (WAL replay body, storage payload reachable from client-supplied field data) carries `0xDD FF FF FF FF` (Array32, ~4.29e9 elems) followed by almost nothing. `msgpack_to_inner` attempts a >100 GB allocation before reading the first element; the allocator aborts the process (uncatchable), taking down recovery/server rather than surfacing `Err(CodecError::Decode(..))`. Same abort applies to `merge_storage_bytes` via a Map32 top-level header (`Entry` is 5 usizes ~= 40 B/elem).
- **Suggested fix:** mirror the established pattern: clamp header-driven `with_capacity`/`new_map_wc` calls with a `.min(SANE_PREALLOC_CAP)`-style constant (hoist one shared const, or export `value.rs`'s), letting the collection grow on demand as elements are actually decoded. Apply to all three functions (including `merge_storage_bytes`' `entries` vec and `old_ids` map). Add regression tests feeding small buffers with huge declared counts, asserting `Err` -- not abort -- for both `msgpack_to_inner` and `merge_storage_bytes`.

### 2. Two public, rival `CodecError` enums; the basic/bincode one is hand-rolled, violating the documented thiserror rule

- **File:line:** `crates/shamir-types/src/codecs/error.rs:3-9` (thiserror `Encode/Decode`) vs `crates/shamir-types/src/codecs/basic/bincode.rs:7-22` (manual `pub enum CodecError { Serialize(String), Deserialize(String) }` with hand-written `Display`/`Error`)
- **Severity:** medium
- **Issue:** CLAUDE.md mandates "`thiserror` for library error enums". One crate exports two unrelated error types with identical names: `codecs::CodecError` (thiserror) and `codecs::basic::bincode::CodecError` (hand-rolled, no `PartialEq`, no `#[from]`, different Display wording). Worse, `codecs/mod.rs:12` re-exports `to_bytes`/`from_bytes` right next to the *other* `CodecError`, so a caller importing `codecs::{to_bytes, CodecError}` gets functions whose `Err` type does not unify with their imported error -- no `?`-propagation, no `From`, easy mis-mapping.
- **Failure scenario:** downstream glue does `codecs::to_bytes(&x)?` against `codecs::CodecError` and fails to compile, or hand-wraps via strings, losing variant information on the bincode path only.
- **Suggested fix:** either convert the bincode wrapper onto the shared thiserror enum (add `Serialize(String)`/`Deserialize(String)` variants or map into existing `Encode`/`Decode`), or rename it `BincodeCodecError` and keep it private to the module.

### 3. `Interner::touch_ind` returns a `Result` with no reachable `Err`; `touch_with_id` returns stringly-typed errors

- **File:line:** `crates/shamir-types/src/core/interner/interner.rs:138` (signature; both match arms `:146` and `:166` return `Ok`), `interner.rs:348` (`Result<(), String>`; error construction sites `:352, :363, :375, :396, :441-446`), amplified by `codecs/interned/common.rs:13-18` (`intern_string_key` formats the phantom error)
- **Severity:** medium
- **Issue:** Workspace rules require `Result<T,E>` with thiserror library enums. `touch_ind` promises failure it can never deliver (`&'static str` with zero production sites), forcing every caller through `unwrap_or_else`/`?` plumbing for an impossible branch -- and the phantom string gets laundered into `CodecError::Decode("Failed to intern key ...")`, i.e. fake decode errors can appear in logs. Meanwhile `touch_with_id`, whose failures are real (reserved id 0, remap conflict, id collision, race collision + rollback), reports them as formatted `String`s: no exhaustiveness, no typed matching by consumers such as WAL recovery.
- **Failure scenario:** recovery code wants to distinguish "benign replay idempotence" from "id collision -- persistent-state divergence" and can only substring-match English messages.
- **Suggested fix:** make `touch_ind` infallible (`-> TouchInd`) and delete the dead `Err` arm plus the phantom formatting in `intern_string_key`; introduce a small `thiserror::Error` enum (e.g. `InternerError::{ReservedId, NameRemap{..}, IdCollision{..}}`) for `touch_with_id`.

### 4. `ResourceMeta::from_record` fails open: invalid `mode` silently becomes `Mode::OPEN` (0o777)

- **File:line:** `crates/shamir-types/src/access.rs:275-279`
- **Severity:** medium
- **Issue:** A persisted catalogue record whose `mode` field is out of range (`u16::try_from(m).ok()` yields `None`) falls back to `Mode::OPEN` -- the most permissive possible value -- rather than an error or a restrictive default. Unlike the owner-field collapse (deliberately documented at `access.rs:283-300`, with `owner_field` provided as the escalation-safe alternative), the mode fallback is undocumented and is the security-relevant direction to fail. There is no test exercising a malformed mode (access_tests cover round-trip and absent-field cases only).
- **Failure scenario:** bit rot / partial write / buggy writer stores `mode = 70000` (or negative); the object reloads world-writable (`0o777`) and `permits()` grants every class full rwx, silently widening access until someone notices.
- **Suggested fix:** treat unparsable mode as a fault: return `Result<ResourceMeta, _>` or fall back to a deny-by-default mode (System-owned, owner-only `0o700`), and add a red test pinning the chosen behaviour. At minimum, `log::warn!` and document the fail-open choice next to the owner-collapse documentation.

### 5. `Interner::with_state` silently collapses duplicate ids/names in hydrated state

- **File:line:** `crates/shamir-types/src/core/interner/interner.rs:121-127` (`map_user_to_interned.insert(...)` last-wins; `let _ = reverse[id].set(arc);` discards both `OnceLock` overwrite failures)
- **Severity:** low
- **Issue:** Hydration (persist-file / recovery input) performs no consistency validation. Two entries sharing an id leave the reverse spine holding whichever `Arc<str>` came later while the forward map keeps a name whose resolution de-interns to a *different* string; duplicate `(name, id)` pairs similarly overwrite without error. The function cannot express "input was inconsistent".
- **Failure scenario:** a torn/corrupted interner persist file hydrates "successfully"; post-restart, `get_ind("email")` returns an id whose `get_str` yields `"email_backup"` -- silent cross-field data mixing with no error signal at boot.
- **Suggested fix:** scan `initial_data` once (it is already iterated) and return `Result<Self, String/InternerError>` on duplicate ids or conflicting mappings, or at minimum `log::warn!` and skip deterministically.

### 6. `RecordId::system` truncates names to 12 bytes with no collision detection

- **File:line:** `crates/shamir-types/src/types/record_id.rs:95-103`
- **Severity:** low
- **Issue:** Any two distinct system names sharing a 12-byte prefix mint the *same* `RecordId`, silently addressing one logical record where two were intended. Unlike every other fallibility in the crate, this conversion offers neither `Result` nor an assertion; the collision is undetectable by the caller or victim.
- **Failure scenario:** `"catalog::users_primary_index_stats_v2"` and `"catalog::users_primary_index_stats_v3"` collapse onto one id; second write overwrites the first record, quietly, forever.
- **Suggested fix:** return `Result<Self, RecordIdError::NameTooLong>` (or take `&[u8; <=12]` / split hash suffix), keeping `system()` total only for short canonical names; add a debug_assert so violations surface in tests at least.

### 7. Race-between-touch_ind-and-touch_with_id guarded only by `debug_assert!` -- release path silently drops the write

- **File:line:** `crates/shamir-types/src/core/interner/interner.rs:222-229` (`set_reverse_slot`: failed `OnceLock::set` acknowledged only via `debug_assert!`; success value discarded), cf. the hand-rolled rollback at `:438-447` in `touch_with_id`
- **Severity:** low
- **Issue:** The doc comments correctly classify a concurrent `touch_ind`/`touch_with_id` hit on the same id as a violation of the recovery model ("cannot happen"). But the enforcement mechanism evaporates in release builds: `debug_assert!` compiles out and the newer mapping is dropped without any observable effect. Per project rules, `panic!` is reserved for exactly this class (programmer/invariant bugs) yet here the invariant breach is invisible in production.
- **Failure scenario:** WAL recovery overlapping live traffic (future refactor, scheduler change) assigns a name to an already-set slot; `get_str(id)` resolves to the *older* name; everything downstream silently disagrees.
- **Suggested fix:** escalate to an unconditional `panic!`/invariant failure (matching how the codebase treats programmer bugs), or at minimum `log::error!` on the discarded write so release deployments surface the broken assumption.

### 8. `SecretString::Drop` uses hand-written `unsafe` where the safe std/trait path exists; lifecycle behaviour untested

- **File:line:** `crates/shamir-types/src/secret.rs:67-75`
- **Severity:** low
- **Issue:** `zeroize` implements `Zeroize for String`; `self.inner.zeroize()` achieves the same wipe without `unsafe { self.inner.as_bytes_mut() }` and the accompanying safety justification (however sound it looks today). Additionally, no test exercises the resource-lifecycle contract at all: zeroize-on-drop (observable via `into_inner`-vs-drop comparison of the buffer, or a custom-zeroize probe) and `into_inner`'s "caller now owns the cleanup duty" transfer are both unverified (`secret_tests.rs` covers Debug-redaction, serde round-trip, conversions only). Without the `crypto` feature the type silently downgrades to non-zeroizing (documented, but untestable as shipped).
- **Failure scenario:** a refactor moves `inner` behind another indirection; the `unsafe` block's "no other references" premise erodes and the soundness argument rots invisibly, with no test failing.
- **Suggested fix:** replace the manual `Drop` body with `self.inner.zeroize();` (keeping `impl Drop` so the wipe happens on all destruction paths), and add a lifecycle test for `into_inner` + drop semantics.

### 9. `trace_access` -- a `Result` that is always `Ok`, with an error type the crate itself never constructs

- **File:line:** `crates/shamir-types/src/access.rs:657-660`; `AccessError` defined `:621-630` (zero construction sites in-crate)
- **Severity:** nit
- **Issue:** Deliberate and extensively documented (renamed from `authorize` precisely to telegraph "not a gate"), so this is shape, not bug: an infallible `fn` returning `Result<(), AccessError>` invites `?`-chains at call sites against an error value that can only originate in `shamir-db`'s real gate, blurring which layer denied access. No error-path test is possible for this symbol in-crate.
- **Suggested fix:** long-term, have observability tracing return `()` and let `authorize_access` own the `Result`; interim, a doc cross-link from `AccessError` noting "produced only by `shamir-db`'s facade gate" would prevent `use`-of-the-wrong-gate mistakes.

### 10. `pos + len` unchecked additions in the tree decoder's `read_str`/`read_bin`

- **File:line:** `crates/shamir-types/src/codecs/interned/messagepack.rs:133` (`pos + len > data.len()`), `:147`
- **Severity:** nit
- **Issue:** `len` derives from u8/u16/u32 headers (`<= u32::MAX`); on 64-bit targets the addition cannot wrap, but the sibling lens code (`record_view/lens.rs:202-215` `borrow_bytes`) uses `checked_add` uniformly as its stated untrusted-input discipline. The asymmetry is a latent panic (slice-index) should either site ever feed a larger cursor or the cast widen.
- **Suggested fix:** align with the lens: `pos.checked_add(len)` + explicit `Truncated`-equivalent `CodecError`.

## Error-path test coverage observed (context for finding 4 / 8)

Good: `record_view/tests/error_tests.rs` (11 targeted tests incl. depth cap, reserved marker, mid-skip truncation, garbage-bytes no-panic); `codecs/interned/tests/messagepack_tests.rs` error section (`:604-644`: truncated/empty input, non-string key, depth rejection); `bincode_tests.rs` (`:136,:148` decode/serialize failures); `base_tests.rs` (`Base58Error` variants); `sort_codec_tests.rs:82` (NaN refusal); `value_api_tests.rs` (`ValueError::NotAMap`/`TypeMismatch` shapes); `validate_keys` unresolved-id suite; `interner_tests.rs:872-885` (`touch_with_id` remap/collision errors) plus the load-bearing concurrent-growth stress (`:922`).
Gaps found: malformed/oversized `mode` in `ResourceMeta::from_record` (finding 4), huge-header/short-body allocations (finding 1), `SecretString` lifecycle (finding 8), post-error rollback state after `touch_with_id` raced collisions (finding 7), duplicate-id hydration (finding 5).

</details>
