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
