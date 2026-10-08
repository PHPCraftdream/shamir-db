<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-types — error-handling-lifecycle independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Allocation, malformed metadata, and conflicting hydration remain genuine failure-path problems. Safe secret replacement is now version-verified; several proposed API changes are preferences rather than lifecycle corrections.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 10 | 8 | 0 | 0 | 0 | 0 | 2 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Unbounded preallocation from attacker-controlled msgpack headers -- allocator abort, unlike the capped tree visitor

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Custom arrays/maps and merge storage allocate directly from counts before body validation. Live de-intern conversion has an additional unchecked map capacity; no directly registered huge-count controls cover these sites.

Evidence: [crates/shamir-types/src/codecs/interned/messagepack.rs:305](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/interned/messagepack.rs#L305); [crates/shamir-types/src/codecs/interned/messagepack.rs:585](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/interned/messagepack.rs#L585); [crates/shamir-types/src/codecs/interned/codec.rs:154](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/interned/codec.rs#L154).

<a id="review-2"></a>

### Claim 2 — Two public, rival `CodecError` enums; the basic/bincode one is hand-rolled, violating the documented thiserror rule

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The standalone helpers return a manual Serialize/Deserialize enum, whereas adjacent exports expose another thiserror type; this violates the library-error convention but not memory safety.

Evidence: [crates/shamir-types/src/codecs/basic/bincode.rs:8](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/basic/bincode.rs#L8); [crates/shamir-types/src/codecs/error.rs:4](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/error.rs#L4); [crates/shamir-types/src/codecs/mod.rs:12](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/mod.rs#L12).

<a id="review-3"></a>

### Claim 3 — `Interner::touch_ind` returns a `Result` with no reachable `Err`; `touch_with_id` returns stringly-typed errors

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

touch_ind has only Ok branches; touch_with_id has real formatted conflict errors. The impossible mapper cannot emit fake errors, and current recovery propagates every failure.

Evidence: [crates/shamir-types/src/core/interner/interner.rs:146](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/core/interner/interner.rs#L146); [crates/shamir-types/src/core/interner/interner.rs:348](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/core/interner/interner.rs#L348); [crates/shamir-types/src/codecs/interned/common.rs:17](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/interned/common.rs#L17).

<a id="review-4"></a>

### Claim 4 — `ResourceMeta::from_record` fails open: invalid `mode` silently becomes `Mode::OPEN` (0o777)

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Invalid values fall through to 0o777 in a live metadata parser; absent/valid-mode tests cannot detect invalid-present widening.

Evidence: [crates/shamir-types/src/access.rs:275](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/access.rs#L275); [crates/shamir-types/src/tests/access_tests.rs:238](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/tests/access_tests.rs#L238).

Grouping/duplicate: [security-crypto.md#2](security-crypto.md#review-2). This is not an additional independent defect.

<a id="review-5"></a>

### Claim 5 — `Interner::with_state` silently collapses duplicate ids/names in hydrated state

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Duplicate IDs with different names create two forward entries and a first-wins reverse slot; repeated names with different IDs are forward-last-wins. Production concatenates chunks without checking these conflicts.

Evidence: [crates/shamir-types/src/core/interner/interner.rs:124](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/core/interner/interner.rs#L124); [crates/shamir-types/src/core/interner/interner.rs:126](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/core/interner/interner.rs#L126); [crates/shamir-engine/src/table/interner_manager.rs:220](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/interner_manager.rs#L220).

<a id="review-6"></a>

### Claim 6 — `RecordId::system` truncates names to 12 bytes with no collision detection

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

The constructor documents and tests truncation; arbitrary-name injectivity is not its contract. No current colliding production pair was established, and existing long tags constrain any replacement API.

Evidence: [crates/shamir-types/src/types/record_id.rs:94](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/record_id.rs#L94); [crates/shamir-types/src/types/tests/record_id_tests.rs:48](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/tests/record_id_tests.rs#L48); [crates/shamir-index/src/base_index/index_manager.rs:498](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/index_manager.rs#L498).

Grouping/duplicate: [api-wire-protocol.md#3](api-wire-protocol.md#review-3). This is not an additional independent defect.

<a id="review-7"></a>

### Claim 7 — Race-between-touch_ind-and-touch_with_id guarded only by `debug_assert!` -- release path silently drops the write

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

OnceLock set failure is debug-only while the forward association survives. The drainer is live, although a conflicting production schedule is not established merely by that fact.

Evidence: [crates/shamir-types/src/core/interner/interner.rs:223](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/core/interner/interner.rs#L223); [crates/shamir-engine/src/tx/drainer.rs:1116](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/drainer.rs#L1116).

Grouping/duplicate: [correctness-tdd.md#8](correctness-tdd.md#review-8). This is not an additional independent defect.

<a id="review-8"></a>

### Claim 8 — `SecretString::Drop` uses hand-written `unsafe` where the safe std/trait path exists; lifecycle behaviour untested

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The exclusive initialized-byte wipe is sound; tests omit Drop and into_inner handoff. Zeroize 1.8.2 published src/lib.rs implements safe full-capacity String wiping, so replacement availability is now established.

Evidence: [crates/shamir-types/src/secret.rs:40](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/secret.rs#L40); [crates/shamir-types/src/secret.rs:72](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/secret.rs#L72); [Cargo.lock:5473](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L5473).

Grouping/duplicate: [security-crypto.md#6](security-crypto.md#review-6). This is not an additional independent defect.

<a id="review-9"></a>

### Claim 9 — `trace_access` -- a `Result` that is always `Ok`, with an error type the crate itself never constructs

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

The result shape is intentional and cross-linked to actual enforcement; the facade constructs AccessError on denial. Optional unit-return simplification is not a missing error path.

Evidence: [crates/shamir-types/src/access.rs:632](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/access.rs#L632); [crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:863](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/access_control.rs#L863).

<a id="review-10"></a>

### Claim 10 — `pos + len` unchecked additions in the tree decoder's `read_str`/`read_bin`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

A tiny Str32/Bin32 maximum-length input makes cursor+u32::MAX overflow before checking on 32-bit targets. Ordinary 64-bit inputs do not have that arithmetic witness.

Evidence: [crates/shamir-types/src/codecs/interned/messagepack.rs:133](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/interned/messagepack.rs#L133); [crates/shamir-types/src/codecs/interned/messagepack.rs:147](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/interned/messagepack.rs#L147).

## Evidence and recipe corrections

- with_state reverse resolution is first-wins, not later-name-wins. Identical recaptured pairs are legitimate and must not be rejected.
- Hydration also requires validating ID-based persistence progress; accepting identical tuples while counting them as unique progress can skip later mappings.
- Exact zeroize 1.8.2 replacement semantics are available: String wiping clears length and covers capacity, whereas the current slice wipe covers initialized bytes only.
- Never inspect deallocated String storage to test zeroization. into_inner transfer and controlled live-buffer wiping are separate obligations.
- The lens's universal error-path assurance is disproved by header allocation and 32-bit Ext32 skip arithmetic; silence from fields() is not complete validation.
- Do not convert documented constructor/API behavior into a required incompatible change without a reachable caller violation.

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
