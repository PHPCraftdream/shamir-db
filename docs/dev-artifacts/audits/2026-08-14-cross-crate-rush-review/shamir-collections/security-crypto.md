<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-collections — security-crypto revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Deterministic non-keyed hashing of authenticated client-controlled names is source-proven and remains unmitigated at decoding. The report's specific trivial-collision explanation describes the wrong algorithm, so practical HashDoS amplification is unverified rather than established.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 2 | 1 | 0 | 0 | 0 | 1 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Unseeded FxHasher exported as THE workspace hasher is fed client-controlled string keys downstream — precomputable HashDoS amplifier

Status: `unverified`. Current risk: `medium` (provisional; not a confirmed defect).

THasher still creates zero-seeded Fx hashers; derived request maps, nested bind maps, return_only sets and transaction interner overlays retain client-controlled strings. However rustc-hash 2.1.2 uses polynomial accumulation, rotated finishing and wyhash-inspired byte hashing, not the alleged multiply-xor string fold. No practical collision family or amplification proof was supplied for that pinned implementation.

Evidence: [crates/shamir-collections/src/lib.rs:17](../../../../../crates/shamir-collections/src/lib.rs#L17); [Cargo.lock:3007](../../../../../Cargo.lock#L3007); [Cargo.lock:3383](../../../../../Cargo.lock#L3383); [crates/shamir-query-types/src/batch/batch_request.rs:87](../../../../../crates/shamir-query-types/src/batch/batch_request.rs#L87); [crates/shamir-query-types/src/batch/sub_batch_op.rs:15](../../../../../crates/shamir-query-types/src/batch/sub_batch_op.rs#L15); [crates/shamir-server/src/db_handler/handler.rs:343](../../../../../crates/shamir-server/src/db_handler/handler.rs#L343); [crates/shamir-engine/src/query/batch/batch_execute.rs:869](../../../../../crates/shamir-engine/src/query/batch/batch_execute.rs#L869); [crates/shamir-tx/src/layered_interner.rs:95](../../../../../crates/shamir-tx/src/layered_interner.rs#L95).

<a id="review-2"></a>

### Claim 2 — Crate-wide `#![allow(clippy::disallowed_types)]` permanently disables a workspace-`deny` lint for all future code in this leaf

Status: `confirmed-open`. Current risk: `nit`.

The broad allow remains and covers future additions by default. It is explicitly sanctioned; no current default-hasher violation is present. This is lint-containment hardening, not a demonstrated security vulnerability.

Evidence: [crates/shamir-collections/src/lib.rs:9](../../../../../crates/shamir-collections/src/lib.rs#L9); [Cargo.toml:34](../../../../../Cargo.toml#L34); [clippy.toml:39](../../../../../clippy.toml#L39); [clippy.toml:44](../../../../../clippy.toml#L44).

## Corrections and qualified non-findings

- Qualify application-map reachability as authenticated-client exposure: session validity and rate limiting precede application decoding; application permissions and query-count caps occur afterward.
- The 16 MiB frame ceiling and post-auth request-rate gate constrain attacks, but do not establish keyed hashing or prevent collision work during deserialization.
- PerIpLimiter keys are observed transport peer IP addresses, not arbitrary payload strings. Attacker control and cardinality there require a separate network threat model.
- Precomputation is not universally portable across every deployment: pointer width and hashing implementation affect outputs.
- Interning names does not itself close the ingress boundary: the interner/transaction overlay still hashes raw strings while assigning IDs.
- The manifest and migration commit claim an exact same algorithm, but pinned rustc-hash 2.1.2 does not implement the old fxhash algorithm. Do not treat historical byte-identity assertions as proof.
- The dependency pins and absence of local crypto, parsing, I/O and unsafe blocks remain confirmed. No current advisory-database audit was performed.
- The assertion that pure aliases have nothing independently testable is refuted by their accessible hasher, capacity, iteration and serde contracts.
- An allow for disallowed_types does not suppress the separate disallowed_methods lint.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-collections -- Security & crypto boundary

## Summary

The crate is a 64-line leaf (`Cargo.toml` + `src/lib.rs`, nothing else): insertion-ordered collection
aliases (`TMap`/`TSet`/`TFxMap`/`TFxSet`) plus constructor fns. It contains **no** auth, HMAC/SCRAM/TLS,
crypto primitives, `unsafe` blocks (grep-verified: zero hits), parsing, path assembly, or I/O — so the
classic timing-side-channel and injection surfaces do not exist locally. Dependency footprint is minimal
and hygiene-conscious: indexmap 2.14.0 and rustc-hash 2.1.2 resolved, with an in-manifest comment citing
RUSTSEC-2025-0057 as the reason for moving off `fxhash`. The only theme-relevant exposure is *exported*
rather than coded: this crate mints the workspace's single unseeded, non-keyed hasher (`THasher =
BuildHasherDefault<FxHasher>`), and downstream crates demonstrably feed client-controlled strings into
maps built from it — the "we don't accept untrusted hash inputs" premise of ideology pillar 4 does not
hold at every consumption site (Finding 1). No `tests/` directory exists under `src/`; for pure type
aliases that is acceptable — nothing here is independently behaviorally testable, and consumer crates'
suites (engine batch/filter tests, tx MVCC tests) exercise `THasher`-based maps transitively and heavily.

## Findings

### 1. Unseeded FxHasher exported as THE workspace hasher is fed client-controlled string keys downstream — precomputable HashDoS amplifier

**File:** `crates/shamir-collections/src/lib.rs:17` (`pub type THasher = BuildHasherDefault<FxHasher>`);
consumer sites that violate the trusted-input premise: `crates/shamir-engine/src/query/batch/batch_execute.rs:130,357`
(`params: &TMap<String, QueryValue>`), `:350,435,463` (`queries: &TMap<String, QueryEntry>`),
`:712` (`TFxSet<String>` used against result names); `crates/shamir-query-builder/src/batch/batch.rs:33`
(`queries: TMap<String, QueryEntry>`); `crates/shamir-tx/src/tx_context.rs:207`
(`interner_overlay: scc::HashMap<String, u64, THasher>` keyed by raw field-name strings);
`crates/shamir-server/src/conn_limiter.rs:140` (`DashMap<IpAddr, AtomicUsize, THasher>`).
**Severity:** medium

**Issue:** `BuildHasherDefault<FxHasher>` always seeds state at zero and FxHash is a non-keyed
multiply-xor construction with no final avalanche — its 64-bit outputs are trivially collidable offline
(classic HashDoS family; craft-once, reuse-forever, because unlike `RandomState` the seed never changes
across processes or restarts). CLAUDE.md pillar 4 explicitly trades away that protection on the premise
*"we don't accept untrusted hash inputs here"*. That premise is factually broken at the sites above:
query **params** and **alias/result names** in batch requests are deserialized from client payloads
(server/engine/DTO layers) and become `String` keys of `TMap`/`TFxSet` built on `THasher`; user-supplied
field names likewise flow into `TxContext::interner_overlay`. This crate cannot enforce the boundary —
it is where the primitive originates, which is why it is reported here.

**Failure scenario:** An attacker submits a batch whose parameter names form an FxHash collision set
(precomputed once, valid against every deployment and restart). All keys collapse into one hash bucket;
insertion and lookup degrade from O(1) toward O(N²) per request. Repeating with modestly growing N turns
each connection into disproportionate CPU load on the engine's batch-execute hot path — a cheap,
persistent resource-exhaustion vector against a multi-connection server, amplified further if `IndexMap`
keeps probing/degenerate chains on top.

**Suggested fix:** Do not roll back pillar 4 globally. Close the boundary instead: (a) map client-supplied
strings to interned `u64` ids (an interner already exists server-side) *before* they become hash-map keys,
or (b) use a seeded/keyed builder (e.g. `RandomState`, random-seeded ahash) at exactly the few
client-string-keyed sites, each carrying the sanctioned inline contention/justification comment; and
(c) amend CLAUDE.md pillar 4 and this crate's docs to name *which* upstream inputs are considered trusted,
so the assumption stops being silently inherited by new call sites. A request-shape cap on param-count is
a complementary mitigation, not a substitute.

### 2. Crate-wide `#![allow(clippy::disallowed_types)]` permanently disables a workspace-`deny` lint for all future code in this leaf

**File:** `crates/shamir-collections/src/lib.rs:9`
**Severity:** low

**Issue:** The attribute is the sanctioned exception site (`clippy.toml:37-45` designates it as "the ONE
sanctioned allow-site"), so this is per-convention, not a violation. But scoping is crate-wide while the
need is item-local (only lines 11–63 touch banned `std::collections::{HashMap, HashSet}`): any *future*
code added to this crate also silently escapes the deny-level ban — e.g. a helper reaching for
`std::collections::hash_map::RandomState` would produce zero diagnostics in exactly the crate least
supervised by consumer review.

**Suggested fix:** Narrow the allow to the items that require it (per-item `#[allow(clippy::disallowed_types)]`
on the two alias groups and their constructors) so the rest of the crate keeps the deny-by-default posture;
update the `clippy.toml` comment's "ONE sanctioned allow-site" wording to match. Cosmetic effort, durable
lint assurance.

---

No other findings for this theme: the crate defines no secret-bearing comparisons (no constant-time
questions arise), performs no wire-format or SQL/template assembly, contains no `unsafe`, and adds no
new third-party attack surface beyond the two small, reputable dependencies pinned above.

</details>
