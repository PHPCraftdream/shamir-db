# Crate-extraction research — synthesis

**Inputs:** `01-core-utils.md`, `02-storage-persistence.md`, `03-query-layer.md`, `04-engine-db-wasm.md`, `05-network-server-client.md` — five parallel read-only surveys covering all ~23 workspace crates plus `shamir-client-ts`.

## 1. Ranked extraction candidates ("strong" / "worth pursuing")

Ordered by compellingness = community value vs. effort/coupling-to-unwind.

**1. `KeyBytes` — SSO byte-key → `keybytes`** (source: 02-storage-persistence §1a). `crates/shamir-storage/src/key_bytes.rs` (316 LOC) + tests (~660 LOC), ~1k total. Deps: `bytes`, `serde`, `serde_bytes` — zero shamir-* imports. **Extract now** — "a file move + re-export," the best effort/value ratio in the whole survey. Bytes-interop SSO key with serde-parity guarantees; niche is crowded (`minibytes`, `smallbytes`) but essentially free to ship.

**2. `shamir-numa` (whole crate) → `numa-replicated`** (01-core-utils §4). ~1.2k LOC incl. tests. Deps: `arc-swap`, `thiserror`, `libc`; one internal dep (`shamir-collections`) trivially swappable. **Extract with prerequisite work** — same profile as `bench-scale-tool`. `NodeReplicated<T>` fills a real gap vs. `hwloc2`/`libnuma` (topology-only) and academic `node-replication` (heavier). Prerequisites: one-line dep swap, plus waiting on Фаза-2 multi-socket hardware validation (no real perf numbers yet; Windows/macOS topology detection is a stub).

**3. Segmented two-tier WAL + group commit → `tierwal`/`segwal`** (02-storage-persistence §2a). `wal_segment.rs`+`segment_set.rs`+`wal_group_commit.rs`+`wal_sink.rs`, ~1.8k LOC impl + ~2k tests/benches. Deps: tokio, bytes, crc32fast; only real coupling is `WalEntryV2` (genericizable to `(payload, commit_version)`) and `DbError`. **Extract with prerequisite work** — called "the strongest community-value candidate" in report 02: explicit two-tier durability contract, liveness-proved rotating-leader group commit, zero-coordination watermark truncation, dir-fsync-on-create, async-native (vs. sync `okaywal`). Medium effort: genericizing `WalEntryV2` touches call sites in shamir-tx/shamir-engine.

**4. SSRF/egress guard → `egress-guard`** (04-engine-db-wasm §1.1). `shamir-wasm-host/src/net_gateway.rs` (+ `shamir-db/.../curl_gateway.rs` as reference impl), ~1.2k LOC total. Deps: effectively zero (`std::net`, `async-trait`, `tokio::net::lookup_host`). **Extract now** (as unpublished workspace crate first, publish later if the maintainer wants the CVE/upkeep obligation). Default-deny allowlist + non-canonical-IP-literal canonicalization + a DNS-rebind-closing resolve-and-pin contract (`ResolvedPin`) — most hand-rolled SSRF guards only document this race rather than close it.

**5. HMAC-chained audit log → `hmac-audit-chain`** (05-network-server-client §1a). `shamir-connect/src/server/audit_chain.rs`, 444 LOC + tests. Deps: `hmac`, `sha2`, `parking_lot::Mutex`, `serde` — zero shamir-* deps. **Extract now** if a second published crate is wanted; not urgent otherwise. No dominant crates.io incumbent for "HMAC-chained audit log with checkpointed truncation defence." Caveat: canonical byte layout is normative to shamir's spec — publish as fixed layout or make it pluggable (loses byte-identity guarantee). Persistence/checkpoint glue (`audit_appender.rs`) stays behind.

**6. SQ8 quantizer + SIMD distance kernels → `sq8-dist`** (02-storage-persistence §4a). `shamir-index/src/vector/{simd,sq8,quantized_dist}.rs`, ~1.4–1.8k LOC + benches + cross-path invariant tests. `simd.rs`/`sq8.rs` have zero imports; `quantized_dist.rs` needs a trivial internal enum + optional `hnsw_rs` feature. **Extract now**, low cost (mostly a `pub(crate)` visibility flip). Pitch as "SQ8 quantizer with SIMD scoring" (differentiated from `simsimd`), not a generic SIMD crate.

**7. Auth brute-force-defence kit → `auth-hardening`** (05-network-server-client §1b). `shamir-connect/src/server/{lockout,rate_limit,argon2_semaphore}.rs` + `common/latency.rs`, ~1,100 LOC. Deps: `dashmap`, `rustc-hash`, `serde`, `hmac`/`sha2`, `rand`. **Extract with prerequisite work**, only with real ownership commitment. Coherent bundle (KDF concurrency cap + subnet rate limit + lockout/backoff + timing-equalized responses) with no coherent crates.io answer (`governor` only covers generic rate limiting). Prerequisite: normative constants → config, plus real docs for the security-reasoned snapshot rehydration policy.

**8. `shamir-funclib` (whole crate) → `valuefn`/`scalar-funclib`** (03-query-layer §3). ~5.5k LOC non-test + 2.8k tests. Deps: only `shamir-types` (`QueryValue`) + `shamir-collections`; rest is crates.io. **Extract with prerequisite work** — the prerequisite is large: split `Value<K>` out of `shamir-types` into a leaf `shamir-value` crate first, or rewrite against a generic `trait ScalarValue`. Called "the closest analogue to `bench-scale-tool`" on design merit (~150 curated pure scalar functions, purity metadata, lock-free user-override layer), but explicitly a two-crate operation.

## 2. Borderline / revisit-later candidates

- **`ann-bench-data`** (`shamir-bench-utils::vector_data.rs`, 01-core-utils §5a) — zero-dep, ~363 LOC, small audience ("dozens, not thousands"), natural `bench-scale-tool` companion. Maintainer's call.
- **Completion watermark + versioned overlay** (02-storage-persistence §3a) — ~400 LOC, "near the 20-line-utility floor scaled up." Only worth bundling with the WAL extraction (#3), not standalone.
- **SCRAM-Argon2id core → `scram-argon2id`** (05-network-server-client §1c) — ~1,500 LOC, low extraction cost, but not standard RFC 5802 SCRAM (msgpack envelopes, shamir domain tags, Ed25519 pinning baked in) — zero interop value for non-shamir users. Deferred until shamir-db ships externally and `shamir-connect` needs publishing anyway.
- **`tracing-livemask`** (`shamir-server/src/logging.rs`, 05-network-server-client) — real differentiator (atomic-load hot path vs. `tracing_subscriber::reload`'s RwLock) but entangled with config/namespace taxonomy; file-writer half duplicates `tracing-appender`.
- **`group-commit`/`flush-coalesce`** (`repo/group_commit`, 04-engine-db-wasm §2.1) — ~128 LOC, subtle cancellation-safety reasoning, but too small today; revisit only if a second in-workspace consumer appears (confirmed: `tx/group_commit.rs` is NOT a second consumer).
- **`lp-frame32`** (length-prefixed framing, 05-network-server-client §2) — clean, zero-dep, benchmarked, but `tokio_util::codec::LengthDelimitedCodec` already entrenches the niche; deltas are shamir-loop-specific optimizations.

## 3. Correctly NOT extractable (settled)

- **`BatchPlanner` DAG planner** (03-query-layer §1b) — generic kernel is ~200 LOC of petgraph-territory; 70% of the file is dependency extraction from shamir's own AST (`BatchOp`, `$query`/`$fn`/`$cond`/`$expr` marker decoding).
- **MVCC/commit pipeline** (`shamir-tx` core, `shamir-engine/src/tx/`, 02 & 04) — welded to `Store`, `RecordKey`, WAL format, interner, drain/watermark pipeline. "This is the database." Pure-MVCC piece already extracted as `shamir-tx` itself.
- **Engine facades / `shamir-db`** (04 §3) — ~21k LOC that exists solely to wire other crates together. Facades don't extract.
- **Filter compile/eval layer** (`shamir-engine/src/query/filter/`, 04 §2.2 + 03 §1a) — welded to `Value`/`QueryValue`, `Interner`, query-types DTOs; extracting means shipping the whole type system.
- **`shamir-query-builder`** (whole crate, 03 §2) — 1:1 coupling to wire DTOs is the entire product. Already the product of a prior good extraction.
- **Wasmtime harness** (`wasm_engine.rs`/`wasm_function.rs`, 04 §1.4) — product-specific ABI; generic parts are just Wasmtime's documented recipes.
- **Schema validator** (`shamir-engine/src/validator/schema/`, 04 §2.4) — record-format-coupled; niche occupied by `validator`/`garde`/JSON-Schema.
- **`shamir-connect` state machinery** minus the three flagged pieces (05 §1), **`shamir-server` integration modules** (05 §4), **`shamir-client` Rust** (05 §5) — protocol/integration glue by nature.
- **`shamir-collections`, `shamir-tunables`, `shamir-query-builder-macros`, `shamir-sdk-macros`** (01 §1/§3/§6/§7) — trivially small or coupled-by-design to proprietary ABIs.
- **HNSW snapshot codec** (02 §4b) — serializes adapter-private maps, pinned to `hnsw_rs =0.3.4`'s unstable dump format.
- **Sort-key codec + concurrent interner** (`shamir-types`, 01 §2a/§2b) — clean and decoupled but outclassed by `memcomparable`/`ordcode` and `lasso`/`string-interner`; our differentiators are precisely the database-shaped parts generic users don't want.

## 4. Cross-cutting patterns (visible only across all five reports)

1. **`Value<K>` inside heavy `shamir-types` is the single most-cited blocker.** Report 03 names it explicitly as the prerequisite for `shamir-funclib`; report 01 independently rejects extracting the interner/sort_codec from the same crate; report 04 independently rejects the filter layer for the same reason. No report proposes the `Value<K>` split as a standalone task, but three reports hit the same wall from different angles — this is the one genuine cross-report discovery.

2. **"Zero shamir-* deps today" correlates almost perfectly with "extract now."** Candidates #1–#6 all report near-zero or zero internal coupling as-is. The ones needing real prerequisite work (funclib, auth-hardening) are exactly the ones still importing `QueryValue` or normative constants. Coupling debt concentrates specifically at the `Value`/DTO/wire-format layer; storage/security/infra boundaries are already well-drawn.

3. **"The code is the product, not a utility inside the product" is the dominant rejection reason**, distinct from type coupling — `shamir-query-builder`, the `shamir-db` facade, `shamir-server` integration modules, `shamir-tx`'s commit pipeline all fail for this reason. No refactor fixes it because there's no generic version of "the thing that wires shamir together."

4. **Security/protocol infrastructure is a recurring theme across two independent reports (04, 05)** that converge, unprompted, on the same caveat: publishing a security crate is a maintenance/CVE-response obligation, not just a code drop — both suggest "extract as unpublished workspace crate first, publish only with commitment." This reads as a workspace-wide policy question neither report explicitly frames as one.

5. **A natural extraction order emerges from the effort gradient:** (a) zero-effort items first — `KeyBytes`, SQ8/SIMD, `shamir-numa`'s dep swap; (b) small security primitives next, gated on publish-commitment not code readiness — `egress-guard`, `hmac-audit-chain`, optionally `auth-hardening`; (c) the WAL — bounded but real engineering effort; (d) the `Value<K>` split — the one prerequisite big enough to unlock a second tier (funclib and possibly more of the query layer), currently unscheduled by any report.

6. **No report found an in-place testability problem anywhere** — stated explicitly in 02 and 05, implicit in 04. Every candidate across all five reports is justified purely on community/crates.io value, never on internal isolation pain — meaning none of this is urgent technical debt; it's all optional upside.

## 5. If we only do ONE

**Do `KeyBytes` → `keybytes`.** It is the only candidate with zero prerequisite work, zero shamir-* dependencies today, a test suite that IS the value proposition (size-gate, inline-vs-heap consistency, cross-format serde-identity proofs), and a real if modest niche. Extraction is literally "a file move + re-export" — a same-day, zero-risk publish.

## If we want to make a splash for community goodwill

**Do the WAL core → `tierwal`/`segwal`, or `egress-guard` if "security" beats "infra" for visibility.** The WAL got the strongest language in the entire survey ("the strongest community-value candidate in this group") — a liveness-proved rotating-leader group commit and async-native two-tier durability contract where the closest incumbent (`okaywal`) is sync-only. `egress-guard` is the faster alternative: SSRF guards are a live pain point for exactly the community (webhook/agent/plugin hosts) that would notice a crate closing the DNS-rebind TOCTOU window instead of just documenting it, and it ships today with a 5-line Cargo.toml.

---

**Top-line recommendation:** across all five reports, `KeyBytes` is the clear "ship it today" pick — zero coupling, zero prerequisites, and the test suite alone justifies publication. The WAL core (`tierwal`) is the highest-conviction *high-value* candidate but needs real (bounded) engineering work to genericize `WalEntryV2`, and `egress-guard`/`hmac-audit-chain` are the fastest "visible security contribution" plays if community goodwill matters more than raw effort/value ratio. The one structural finding worth acting on independent of any single extraction: a `shamir-value` leaf-crate split is an unscheduled prerequisite blocking `shamir-funclib` (and implicitly more of the query layer) — if that candidate is ever prioritized, scope the `Value<K>` split as its own preceding task rather than discovering it mid-extraction.
