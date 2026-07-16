# Crate-extraction survey — shamir-engine / shamir-db / shamir-wasm-host

*Research artefact, 2026-07-16. Read-only survey; no code was changed.*

Question under investigation: within the three biggest crates of the
workspace, is there code that should be extracted further into standalone
crates — (a) for testability/isolation, or (b) as generically useful
infrastructure worth publishing to crates.io (precedent: `bench-scale-tool`,
extracted and published 2026-07-07)?

**Headline verdict.** One strong publish-grade candidate found
(`shamir-wasm-host::net_gateway` — the SSRF/egress guard), one borderline
micro-candidate (`shamir-engine::repo::group_commit`), and a set of honest
"no" verdicts. The bulk of all three crates is correctly non-extractable:
it is the product, tightly coupled to `shamir-types`' `Value`/`Interner`
and `shamir-storage`'s `Store`, and extraction would just drag half the
workspace along.

---

## 1. shamir-wasm-host (~4.5k LOC src, ~6.2k with tests)

**What it does.** Hosts untrusted user functions as WASM: compiles guest
Rust source to `wasm32-unknown-unknown` (`compile.rs`), structurally
sanitizes compiled artifacts (`wasm/wasm_sanitizer.rs`), executes them
under Wasmtime with fuel + epoch deadlines + pooling allocator
(`wasm/wasm_engine.rs`, `wasm/wasm_function.rs`), and exposes a narrow
`shamir_host` import ABI (batch/globals/db/call/http host imports). Egress
is mediated by a `NetGateway` trait with a default-deny allowlist + SSRF
guard (`net_gateway.rs`); env-var exposure by `EnvPolicy`
(`env_policy.rs`).

Workspace deps: `shamir-types`, `shamir-collections`, `shamir-funclib` —
but crucially the two security modules below use **none** of them.

### 1.1 ✅ STRONG candidate: `net_gateway.rs` SSRF/egress guard → crate `egress-guard` (or `ssrf-guard`)

- **Module path:** `crates/shamir-wasm-host/src/net_gateway.rs` (+ optional
  companion `crates/shamir-db/src/shamir_db/curl_gateway.rs`).
- **Scope:** ~515 LOC core + ~460 LOC tests (`tests/net_gateway_tests.rs`)
  + 244 LOC `CurlNetGateway` reference implementation in shamir-db.
- **Dependency footprint:** effectively zero — `std::net`, `async-trait`,
  `tokio::net::lookup_host`. No `shamir-*` types anywhere in the module;
  errors are plain `String`. The curl impl adds only `tempfile` +
  `tokio::process`. It would compile standalone today with a 5-line
  Cargo.toml.
- **What makes it valuable (not a toy):**
  - default-deny host allowlist with `*`-glob matching, where
    private/loopback targets require an **exact** (non-wildcard) entry;
  - canonicalization of **non-canonical IP literals** attackers use to
    bypass naive `IpAddr::from_str` checks: bare decimal (`2130706433`),
    hex (`0x7f000001`), classic BSD `inet_aton` octal/hex/shorthand dotted
    forms (`0177.0.0.1`, `0x7f.0.0.1`, `127.1`, `192.168.1`), and
    IPv4-mapped IPv6 (`::ffff:169.254.169.254`);
  - full private/link-local/unique-local range coverage for v4 and v6;
  - **DNS-rebind TOCTOU closure**: `check_url_allowed_resolved` resolves
    once, validates every address, and returns a `ResolvedPin`
    (host/port/IPs) so the caller pins the actual connection (curl
    `--resolve`) to exactly what was validated — no second lookup window.
- **Case FOR extraction.** This is precisely the "security infrastructure
  the wider Rust community needs" category. SSRF guards get re-implemented
  badly in every webhook/agent/plugin-host codebase; the crates.io
  landscape has URL parsers and IP-range helpers but (to our knowledge) no
  maintained crate combining allowlist policy + non-canonical IP
  canonicalization + a resolve-and-pin contract that actually closes the
  rebind race instead of just documenting it. The module is already
  API-shaped (`check_url_allowed`, `check_url_allowed_resolved`,
  `ResolvedPin`, `NetGateway` trait), has a real test suite, and shamir-db
  would consume it unchanged. Extraction also removes the current oddity
  that shamir-db reaches through `shamir_engine::function::*` re-exports to
  get at these types.
- **Case AGAINST.** Publishing a security crate is a liability, not a
  vanity line: it invites CVE reports, obligates responsiveness, and puts
  the hand-rolled minimal URL parser (`parse_url` — deliberately avoiding
  the `url` crate "to keep the binary lean") under adversarial scrutiny;
  a public crate would probably be pushed toward depending on `url`/
  WHATWG parsing, diverging from the in-tree goal. ~500 LOC is small, and
  the workspace cost of keeping it in-tree is zero. If the maintainer does
  not want the upkeep of a *security* package, extract it as an
  **unpublished workspace crate** first (testability/isolation win, no
  crates.io obligation) and publish later if desired.

### 1.2 ❌ NOT recommended: `wasm/wasm_sanitizer.rs` as a standalone crate

- **Scope:** 180 LOC (mostly documentation) + tests. Deps: `wasmparser`
  only, plus the crate-local `FunctionError`.
- The genuinely reusable core — "parse only section headers, reject
  component encoding, verify every import against an allowlist" — is
  ~50 lines of `wasmparser` driving. The value here is the *domain*
  allowlist (`SANCTIONED_HOST_IMPORTS`) and the sync-test that keeps it
  matched to the Linker registrations; both are shamir-specific. A
  parameterized `wasm-import-allowlist` crate would be a thin veneer over
  `wasmparser` that any host can write in an afternoon. Against: no
  extraction. (If the egress-guard crate happens, this stays home.)

### 1.3 ⚠️ Marginal, lean against: `compile.rs` untrusted-Rust-to-WASM pipeline

- **Scope:** 728 LOC + `tests/compile_tests.rs`. Deps: std,
  `wait-timeout`, `tempfile`; crate-local error type.
- Contains three separable hardening pieces: a lexer-aware forbidden-macro
  scanner (rejects `include!`/`env!`/… while correctly skipping string
  literals and comments), a child-process env allowlist, and a wall-clock
  `cargo build` timeout. The *pattern* ("compile untrusted guest Rust on
  the host, minimally hardened") is generic and other WASM-plugin hosts
  face it; but the scaffolding is hard-coded to `shamir-sdk` (generated
  Cargo.toml template, `use shamir_sdk as shamir;` prelude), and the
  honest security posture is explicitly "not a sandbox" (layer-0 gate
  lives in shamir-db). Publishing a "safe-ish rustc invoker" would
  oversell its guarantees. The forbidden-macro scanner alone is the only
  clean nugget and is too small to carry a crate. Verdict: keep in-tree;
  revisit only if the guest dependency model is generalized.

### 1.4 ❌ Not extractable: the Wasmtime harness itself

`wasm_engine.rs` + `wasm_function.rs` (~830 LOC) — pooling-allocator
config, fuel/epoch budgets, `ResourceLimiter`, guest ABI (packed-i64
ptr/len, `shamir_alloc`/`shamir_call`). The ABI, msgpack `QueryValue`
payloads, and host-import surface are all product-specific; the generic
parts are Wasmtime's own documented recipes. No.

Small utilities (`env_policy.rs` 106 LOC, its `*`-glob matcher duplicated
in `net_gateway.rs`) are below crate threshold — at most a shared private
module if the egress crate is created.

---

## 2. shamir-engine (~88.6k LOC)

**What it does.** The database engine proper: `DbInstance`/`RepoInstance`
lifecycle (`db_instance/`, `repo/`), table layer with buffers, indexes,
change-feeds, replication hooks (`table/`, 12k LOC), the query layer —
filters, batch DAG execution, reads/aggregates, auth (`query/`, 11k LOC),
the durable commit pipeline over WAL + MVCC (`tx/`, 5.7k), record
validators (`validator/`, 3.5k), online migration (`migration/`).

Important context: the two most "obviously generic" pieces of engine-space
infrastructure **were already extracted**: MVCC/transaction machinery lives
in the separate `shamir-tx` crate, and the index/FTS/vector machinery
(BM25, tokenizer, posting layouts, vector backends) in `shamir-index`.
The interner named in the brief also does **not** live in the engine — the
`u64`-id string interner is `shamir-types::core::interner::Interner`; the
engine only has `table/interner_manager.rs` (~500 LOC), which is chunked
*persistence* glue over `Store` and is not separable. (If interner
extraction is ever wanted, it is a shamir-types question, and the honest
answer there is that crates.io already has `string-interner`/`lasso`; the
in-house one earns its keep via workspace-specific persistence deltas and
`InternerKey` wire semantics.)

### 2.1 ⚠️ Borderline micro-candidate: `repo/group_commit` → crate `flush-coalesce` / `group-commit`

- **Module path:** `crates/shamir-engine/src/repo/group_commit/mod.rs`.
- **Scope:** 128 LOC impl + 208 LOC tests. Deps: `tokio` (`oneshot`,
  `Mutex`, `spawn`) + `shamir-storage`'s `DbError` (trivially
  genericizable to `E: Clone` or `String`).
- **What it is:** a cancellation-safe group-commit primitive — concurrent
  `flush+fsync` callers are coalesced so the flush runs once per batch;
  every caller only returns after a flush that *began after it
  registered* (the structural durability invariant); the leader loop is a
  detached task so a cancelled caller cannot strand `leader_busy` (a real
  audited DoS fixed here).
- **FOR:** the cancellation-safety and "flush began after registration"
  reasoning is subtle, repeatedly needed (WAL fsync, any write-behind
  buffer), and the existing `singleflight`-style crates on crates.io
  mostly implement *result-sharing dedup*, not the durability-ordered
  variant — a caller must NOT be served by a flush that started before it
  registered, which is exactly what generic singleflight gets wrong for
  fsync.
- **AGAINST:** it is ~100 lines once genericized; the documentation is
  worth more than the code; publishing means committing to an API for a
  primitive the engine may still evolve (e.g. per-round error routing).
  Reasonable outcomes: leave it, or extract to a tiny unpublished
  workspace crate only if a second in-workspace consumer appears (the
  engine's `tx/group_commit.rs`, 543 LOC, is a *different*, WAL-coupled
  mechanism — not a second consumer of this one).

### 2.2 ❌ Filter compilation layer (`query/filter/`, ~2.8k LOC)

`compile_filter` folds a `Filter` AST into a `FilterNode` tree with
interned field paths; `eval_bytes.rs` evaluates directly over msgpack
bytes. Superficially "a filter VM", but every leaf is welded to
`shamir_types::Value`/`QueryValue`, the `Interner`, and
`shamir-query-types` DTOs. Extracting it means shipping the whole type
system with it — that boundary already exists and is called
`shamir-query-types` + `shamir-types`. No extraction.

### 2.3 ❌ Commit pipeline (`tx/`, 5.7k LOC)

`commit.rs`/`commit_phases.rs`/`drainer.rs`/`recovery.rs` are the glue
binding `shamir-tx` (MVCC), `shamir-wal`, `shamir-storage`, and the table
layer into one durability story (overlay-ack + background drain +
watermark gates). This is the least extractable code in the workspace by
design — its whole job is cross-crate orchestration. The extractable part
(pure MVCC) already left as `shamir-tx`. No.

### 2.4 ❌ Declarative schema validator (`validator/schema/`, ~1.5k LOC)

Type tags, min/max/format (email/url/uuid/date), cross-field compare,
FK/unique constraints. Looks like a candidate "declarative record
validation" crate, but it validates via `RecordFields`/`ScalarRef` over
interned msgpack records and reuses `FilterValue` — i.e. it is an engine
feature, not a general validator (and the general niche is occupied:
`validator`, `garde`, JSON-Schema crates). No.

### 2.5 ❌ Everything else surveyed

`query/auth/session.rs` (pre-computed permission cache — domain resources),
`query/common/parser.rs` (SDBQL wire parsing — belongs with query-types),
`migration/` shadow log (coupled to `Store`/`RecordId`), `repo/changelog_store.rs`,
`table/` (the product core): all correctly in-place. Nothing to extract.

---

## 3. shamir-db (~21.5k LOC)

**What it does.** The facade crate: `ShamirDb` (open/close, db/table/
function/validator/schema management), `SystemStore` (users, roles,
grants, persisted catalogues), the `execute/` admin-op dispatch (~5.5k LOC
of `admin_*.rs` arms), POSIX-style access-control enforcement
(`access_control.rs`, 1.2k LOC — note the *model* (`Actor`, `Mode`,
`permits`) lives in `shamir-types::access`; this file is enforcement glue),
and `curl_gateway.rs` (the `NetGateway` impl).

**Verdict: nothing independently extractable — by construction.** The
facade is definitionally the least separable crate: every file exists to
wire other shamir crates together behind one API. The single mobile piece
is `curl_gateway.rs` (244 LOC, deps: tokio + tempfile + the gateway
types), which should move **with** the egress-guard extraction of §1.1 as
that crate's reference implementation (secrets via `-K` config file so
they never hit argv, `--resolve` pinning, no `--location`) — it has no
shamir dependency other than the `NetGateway` types themselves.

Housekeeping observation (not extraction): `Cargo.toml` still declares
`rustls`/`tokio-rustls`/`rcgen` for a "legacy network / TLS module
(`db/net/*`)… kept compiling before its planned deletion", but no
`src/**/*.rs` references rustls/rcgen and no `net/` module exists — the
deletion apparently happened and the three dependencies (and comment) are
now dead weight in the build graph. Worth a small chore task.

---

## Summary table

| Candidate | Source | LOC (impl) | shamir-deps | Verdict |
|---|---|---|---|---|
| `egress-guard` (SSRF allowlist + IP canonicalization + DNS-rebind pin) | `shamir-wasm-host/src/net_gateway.rs` + `shamir-db/.../curl_gateway.rs` | ~760 | none | **Extract.** Publish-grade if maintainer accepts security-crate upkeep; otherwise unpublished workspace crate |
| `group-commit` (cancel-safe fsync coalescer) | `shamir-engine/src/repo/group_commit/` | ~130 | `DbError` only (genericizable) | Borderline — too small today; revisit on 2nd consumer |
| `wasm-import-allowlist` sanitizer | `shamir-wasm-host/src/wasm/wasm_sanitizer.rs` | 180 | error type | No — thin `wasmparser` veneer; value is the domain allowlist |
| Untrusted-rust-to-wasm compile pipeline | `shamir-wasm-host/src/compile.rs` | 728 | error type | No — SDK-templated, guarantees too weak to publish |
| Filter compile/eval | `shamir-engine/src/query/filter/` | ~2.8k | types, query-types, interner | No — drags the type system |
| Commit pipeline / drainer | `shamir-engine/src/tx/` | ~5.7k | tx, wal, storage | No — cross-crate glue by design (MVCC already extracted as `shamir-tx`) |
| Schema validator | `shamir-engine/src/validator/schema/` | ~1.5k | types, query-types | No — record-format-coupled; niche occupied |
| Facade / admin dispatch / access glue | `shamir-db/src/**` | ~21k | everything | No — facades don't extract |
