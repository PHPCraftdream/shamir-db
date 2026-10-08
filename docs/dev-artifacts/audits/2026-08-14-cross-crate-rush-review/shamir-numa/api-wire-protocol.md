<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-numa — api-wire-protocol revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The replication contract, stale shipped-scope documentation, explicit-node clamping, raw-node mapping loss, and Guard exposure remain. The phantom-workflow allegation and shared-constructor-invariant argument are refuted; doctest nonexecution is intentional.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 10 | 9 | 0 | 0 | 1 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Documented consistency contract of `NodeReplicated` is wrong under concurrent writers -- a replica can diverge permanently

Status: `confirmed-open`. Current risk: `high`.

Docs still promise eventual convergence over a nanosecond window, while unversioned store/rcu mirror passes can finish with stale nonzero replicas and no future repair.

Evidence: [crates/shamir-numa/src/node_replicated.rs:22](../../../../../crates/shamir-numa/src/node_replicated.rs#L22); [crates/shamir-numa/src/node_replicated.rs:82](../../../../../crates/shamir-numa/src/node_replicated.rs#L82); [crates/shamir-numa/src/node_replicated.rs:103](../../../../../crates/shamir-numa/src/node_replicated.rs#L103).

Grouping/duplicate: `concurrency-lockfree.md#1`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — Crate-level docs and README describe the shipped API as future work

Status: `confirmed-open`. Current risk: `medium`.

lib.rs, README, and test-manifest prose still describe LinuxTopology as forthcoming, despite its export and Linux detect dispatch. Both index consumers are already migrated; the wider Phase 2 roadmap is only partly implemented.

Evidence: [crates/shamir-numa/src/lib.rs:34](../../../../../crates/shamir-numa/src/lib.rs#L34); [crates/shamir-numa/src/lib.rs:59](../../../../../crates/shamir-numa/src/lib.rs#L59); [crates/shamir-numa/README.md:22](../../../../../crates/shamir-numa/README.md#L22); [crates/shamir-numa/README.md:79](../../../../../crates/shamir-numa/README.md#L79); [crates/shamir-numa/src/tests/mod.rs:5](../../../../../crates/shamir-numa/src/tests/mod.rs#L5); [crates/shamir-index/src/base_index/index_info.rs:109](../../../../../crates/shamir-index/src/base_index/index_info.rs#L109).

<a id="review-3"></a>

### Claim 3 — `NodeReplicated` silently clamps out-of-range `NodeId` to node 0, masking caller bugs

Status: `confirmed-open`. Current risk: `medium`.

Invalid explicit-node reads and writes still resolve to node 0 without Result or diagnostic. The clamp is documented on those methods, while topology pinning uses NodeOutOfRange; no production store_node caller was found.

Evidence: [crates/shamir-numa/src/node_replicated.rs:75](../../../../../crates/shamir-numa/src/node_replicated.rs#L75); [crates/shamir-numa/src/node_replicated.rs:114](../../../../../crates/shamir-numa/src/node_replicated.rs#L114); [crates/shamir-numa/src/node_replicated.rs:121](../../../../../crates/shamir-numa/src/node_replicated.rs#L121); [crates/shamir-numa/src/topology.rs:37](../../../../../crates/shamir-numa/src/topology.rs#L37).

<a id="review-4"></a>

### Claim 4 — Dense-slot `NodeId` discards the kernel's raw NUMA node id with no recovery API

Status: `confirmed-open`. Current risk: `low`.

probe() reads raw node IDs but retains only dense core vectors and CPU-to-slot mappings. No raw-ID accessor exists. Dense semantics are already documented, so wrong-node pinning requires a caller to misuse that documented type.

Evidence: [crates/shamir-numa/src/node.rs:3](../../../../../crates/shamir-numa/src/node.rs#L3); [crates/shamir-numa/src/linux.rs:69](../../../../../crates/shamir-numa/src/linux.rs#L69); [crates/shamir-numa/src/linux.rs:84](../../../../../crates/shamir-numa/src/linux.rs#L84); [crates/shamir-numa/src/linux.rs:92](../../../../../crates/shamir-numa/src/linux.rs#L92); [crates/shamir-numa/src/topology.rs:21](../../../../../crates/shamir-numa/src/topology.rs#L21).

<a id="review-5"></a>

### Claim 5 — Public API returns `arc_swap::Guard`, welding the crate's semver to a foreign dependency type

Status: `confirmed-open`. Current risk: `low`.

Both load methods expose Guard<Arc<T>>, so dependency-type coupling remains. This is an API tradeoff in a publish=false workspace crate, not a demonstrated compatibility break or runtime defect.

Evidence: [crates/shamir-numa/src/node_replicated.rs:71](../../../../../crates/shamir-numa/src/node_replicated.rs#L71); [crates/shamir-numa/src/node_replicated.rs:77](../../../../../crates/shamir-numa/src/node_replicated.rs#L77); [crates/shamir-numa/Cargo.toml:4](../../../../../crates/shamir-numa/Cargo.toml#L4); [Cargo.lock:209](../../../../../Cargo.lock#L209).

<a id="review-6"></a>

### Claim 6 — Inline `#[cfg(all(test, ...))] mod tests` in `linux.rs` violates the documented test-organisation rule

Status: `confirmed-open`. Current risk: `low`.

The Linux implementation still contains its two host tests inline, rather than registering separate test files through the crate manifest.

Evidence: [crates/shamir-numa/src/linux.rs:179](../../../../../crates/shamir-numa/src/linux.rs#L179); [crates/shamir-numa/src/tests/mod.rs:8](../../../../../crates/shamir-numa/src/tests/mod.rs#L8); [CLAUDE.md:594](../../../../../CLAUDE.md#L594).

Grouping/duplicate: `style-claude-md.md#1`. This row is not another independent defect.

<a id="review-7"></a>

### Claim 7 — Test-coverage claims vs reality: `detect()` untested off-Linux, `LinuxTopology` error paths untested, README references a non-existent CI workflow

Status: `confirmed-open`. Current risk: `low`.

Off-Linux detect dispatch, injected Linux errors, and explicit-node OOB operations remain uncovered. The workflow facet is refuted: numa.yml exists, runs the wrapper on three OSes, and wires push commit-message QEMU opt-in.

Evidence: [crates/shamir-numa/src/detect.rs:40](../../../../../crates/shamir-numa/src/detect.rs#L40); [crates/shamir-numa/src/tests/mod.rs:8](../../../../../crates/shamir-numa/src/tests/mod.rs#L8); [crates/shamir-numa/src/tests/node_replicated_tests.rs:82](../../../../../crates/shamir-numa/src/tests/node_replicated_tests.rs#L82); [.github/workflows/numa.yml:29](../../../../../.github/workflows/numa.yml#L29); [.github/workflows/numa.yml:39](../../../../../.github/workflows/numa.yml#L39); [.github/workflows/numa.yml:50](../../../../../.github/workflows/numa.yml#L50).

<a id="review-8"></a>

### Claim 8 — `parse_cpulist` allocates unboundedly on adversarial range tokens

Status: `confirmed-open`. Current risk: `low`.

The public parser still materializes every valid ascending usize range with no bound. Existing call sites use trusted discovery input; hostile tooling input is an embedding threat model, not a proven service endpoint.

Evidence: [crates/shamir-numa/src/cpulist.rs:29](../../../../../crates/shamir-numa/src/cpulist.rs#L29); [crates/shamir-numa/src/cpulist.rs:40](../../../../../crates/shamir-numa/src/cpulist.rs#L40); [crates/shamir-numa/src/linux.rs:57](../../../../../crates/shamir-numa/src/linux.rs#L57).

Grouping/duplicate: `error-handling-lifecycle.md#1`. This row is not another independent defect.

<a id="review-9"></a>

### Claim 9 — Inconsistent out-of-range constructor conventions between the two topology constructors

Status: `refuted`. Current risk: —.

The arguments represent different dimensions: MockTopology rejects zero nodes, while fallback always has one node and clamps zero CPU count. MockTopology permits zero CPUs per node. Both documented behaviors have registered tests; they do not enforce the same invariant differently.

Evidence: [crates/shamir-numa/src/mock.rs:41](../../../../../crates/shamir-numa/src/mock.rs#L41); [crates/shamir-numa/src/mock.rs:47](../../../../../crates/shamir-numa/src/mock.rs#L47); [crates/shamir-numa/src/fallback.rs:29](../../../../../crates/shamir-numa/src/fallback.rs#L29); [crates/shamir-numa/src/fallback.rs:38](../../../../../crates/shamir-numa/src/fallback.rs#L38); [crates/shamir-numa/src/tests/mock_tests.rs:76](../../../../../crates/shamir-numa/src/tests/mock_tests.rs#L76); [crates/shamir-numa/src/tests/fallback_tests.rs:20](../../../../../crates/shamir-numa/src/tests/fallback_tests.rs#L20).

<a id="review-10"></a>

### Claim 10 — README's "run tests" instruction bypasses the mandated central test entry point

Status: `confirmed-open`. Current risk: `nit`.

README still teaches raw cargo test. Current authoritative guidance and runner configuration block it outright; the report's supposed single-crate exception is stale.

Evidence: [crates/shamir-numa/README.md:42](../../../../../crates/shamir-numa/README.md#L42); [CLAUDE.md:199](../../../../../CLAUDE.md#L199); [.cargo/config.toml:105](../../../../../.cargo/config.toml#L105); [.github/workflows/numa.yml:39](../../../../../.github/workflows/numa.yml#L39).

## Corrections and qualified non-findings

- Serialization/versioning and query-builder requirements are not applicable here: no serde, wire messages, or database-query construction exist in this crate.
- AffinityError remains exhaustive and publish=false; adding variants could break workspace matches, but no published-library semver violation is established.
- The workflow was introduced with the original skeleton, before the reports. Its existence is refutation, not evidence of a subsequent fix.
- QEMU currently checks guest kernel NUMA announcements, not Rust topology tests. Source wiring does not prove a successful boot, and the head_commit condition does not establish PR-message opt-in.
- Do not mark all Phase 2 consumers complete: only the cited IndexInfo/SortedIndexManager migrations were established.
- Fallback-on-single-socket prose is inaccurate: Linux detect returns any successfully probed topology with at least one node, including a one-node LinuxTopology.
- Raw-node-ID loss is real, but the proposed configured WAL-writer misuse is hypothetical; production thread-pinning integration is not present.
- Explicit-node clamps are documented, not concealed. Returning Result would be a deliberate API change rather than restoration of a previously promised error contract.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-numa -- API & wire-protocol design

## Summary

The public surface (`Topology` trait, `NodeReplicated<T>`, `detect()`, two topologies, `parse_cpulist`, `CachePadded`) is small, thoroughly rustdoc'd, and trivially compliant with the builder-only query rule -- the crate contains zero `serde`/`serde_json` usage and constructs no queries or wire ops; its only external-format parser is the sysfs cpulist parser (no versioned wire format exists, so serialization/versioning rules are N/A). The dominant issues are public-contract accuracy: the documented consistency model of `NodeReplicated` understates a real mirror race that can strand a replica on a superseded value *permanently*, and the crate-level docs/README still describe the shipped `LinuxTopology`/multi-node `detect()` API as future "Фаза 1b" work. Secondary: inconsistent out-of-range `NodeId` handling (silent clamping vs `Result`), lossy dense-slot vs raw-kernel-node-id semantics with no recovery API, and stale test-infrastructure references in the README.

## Findings

### 1. Documented consistency contract of `NodeReplicated` is wrong under concurrent writers -- a replica can diverge permanently
- **File:line:** `crates/shamir-numa/src/node_replicated.rs:20-30` (doc), `82-108` (`store`/`rcu` impl)
- **Severity:** high
- **Issue:** The struct doc claims the cross-node inconsistency window is "a few nanoseconds" and calls the model "eventual consistency" (implying convergence). It is neither. `rcu`'s mirror phase reads node 0's value once *after its own CAS* (`load_full`, line 103) and then blindly stores that snapshot to the remaining nodes (lines 104-106). Interleaving: thread A's CAS commits `A'`; A's `load_full` returns `A'`; thread B's CAS commits `B'` (computed from `A'`) and mirrors `B'` to all replicas; A's mirror loop then overwrites every non-zero replica with the superseded `A'`. Node *k* now serves `A'` **indefinitely** -- nothing converges until an unrelated future write touches it. The same interleaving exists between two concurrent `store()` calls (which have no CAS at all; their per-replica stores interleave arbitrarily) and between `store` and `rcu`. `store`'s doc (line 81) doesn't discuss concurrency whatsoever.
- **Failure scenario:** `shamir-index` already runs this on the read hot path for index-definition registries (`crates/shamir-index/src/base_index/index_info.rs:142,152,167,204,340`; `sorted_index_manager.rs:299-300`). Two DDL writes racing through `rcu` can leave a node's readers serving an index list that is permanently missing the other write's committed index -- well beyond the transient "eventual consistency" the contract promises, and invisible (no error, no log).
- **Suggested fix:** Either close the race or make the contract honest -- ideally both. Tag each replica value with a monotonically increasing `u64` sequence number and have the mirror loop CAS the seq so a stale mirror can never overwrite a newer value (seqlock-style re-check after each `store`); or serialize writers with an atomic writer flag and document single-writer as an API requirement. Update the doc to distinguish per-write visibility from convergence, and document `store`'s (weaker) guarantee explicitly.

### 2. Crate-level docs and README describe the shipped API as future work
- **File:line:** `crates/shamir-numa/src/lib.rs:34-40`; `crates/shamir-numa/README.md:12-22, 39-46, 77-79`; `crates/shamir-numa/src/tests/mod.rs:1-6`
- **Severity:** medium
- **Issue:** `lib.rs` "Scope of this version (Фаза 1)" says "The real `LinuxTopology` (`/sys` probe + `sched_setaffinity`) ... land in Фаза 1b" -- yet `linux.rs` is implemented, `LinuxTopology` is publicly exported (`lib.rs:59-60`), and `detect()` probes `/sys` on Linux (`detect.rs:23-31`). The README's item table omits `LinuxTopology`, calls `detect()` "single-node fallback for now" (line 22), and lists as Roadmap both "Фаза 1b -- `LinuxTopology`" and "Фаза 2 -- migrate the hot `ArcSwap` registries ... to `NodeReplicated`" -- but the Фаза 2 migration has already happened (`shamir-index` consumes `NodeReplicated::new(detect(), ...)` today). `src/tests/mod.rs` likewise claims Tier-2 Linux tests "land in Фаза 1b" while `tests/linux_topology.rs` exists.
- **Failure scenario:** A consumer reading the README assumes `detect()` never returns a multi-node topology on Linux (e.g. bakes in single-replica assumptions or skips NUMA handling); a reviewer of `lib.rs` wrongly concludes affinity is unimplemented and duplicates it.
- **Suggested fix:** Rewrite the lib.rs scope section, README table, and Roadmap to match the shipped surface (list `LinuxTopology`, describe `detect()`'s real Linux behavior, mark Фазы 1b/2 as done); refresh the tier claims in `src/tests/mod.rs`.

### 3. `NodeReplicated` silently clamps out-of-range `NodeId` to node 0, masking caller bugs
- **File:line:** `crates/shamir-numa/src/node_replicated.rs:77-79` (`load_node`), `110-118` (`store_node`), `120-128` (`replica`)
- **Severity:** medium
- **Issue:** `load_node(NodeId(9))` on a 2-node topology silently returns node 0's replica; `store_node` likewise writes node 0 -- including clobbering node 0 with data intended for another node. The crate applies three different conventions to the same out-of-range input: `cores_on_node` -> empty slice (`topology.rs:25-27`), `pin_current_thread_to_node` -> `Err(AffinityError::NodeOutOfRange)` (`topology.rs:35-42`), `NodeReplicated` -> silent clamp. Only the first two are documented at the trait level; the clamps are defense-in-depth for a case the type system could largely prevent.
- **Failure scenario:** A caller passes a raw kernel node id or a config-supplied node number `>= num_nodes` (see finding 4); every store lands on node 0's replica, silently corrupting the registry for node-0 readers while other nodes never see the update -- no error, no log signal.
- **Suggested fix:** Return `Result<_, AffinityError::NodeOutOfRange>` from `load_node`/`store_node` (they are explicit, non-hot-path inspection/staging APIs), or at minimum `debug_assert!` plus a logged warning on the release-path clamp. Keep the clamp only for the `load_local` <- `current_node()` handoff, documented as such.

### 4. Dense-slot `NodeId` discards the kernel's raw NUMA node id with no recovery API
- **File:line:** `crates/shamir-numa/src/node.rs:3-9`; `crates/shamir-numa/src/linux.rs:66-69, 84-90`
- **Severity:** low
- **Issue:** `LinuxTopology::probe` correctly maps raw sysfs node ids from `/sys/devices/system/node/online` (which can be sparse, e.g. `0,2` after hotplug) onto dense 0-based replica slots -- and then drops the raw ids entirely. `NodeId` is documented as a dense slot (good), but there is no `raw_node_id(slot)`/`slot_for(raw)` accessor on `Topology`, and the trait doc's "The logical CPUs that belong to `node`" invites passing a raw sysfs/libnuma node number -- which silently returns the *wrong node's* CPUs (compounding finding 3). Фаза 3's planned `sched_setaffinity`-style pinning of named threads and any tooling correlating with `numactl`/`mbind`/`set_mempolicy` (which take raw node ids) has no way back.
- **Failure scenario:** A config says "pin WAL writer to NUMA node 2" (operator reads `2` from `numactl --hardware`); on a sparse-node host, dense slot 2 is kernel node 3 -- the thread pins to the wrong socket, with no API to detect or correct it.
- **Suggested fix:** Add `fn raw_node_id(&self, node: NodeId) -> usize` (default: identity) to `Topology`, or at minimum rename/loudly document in `NodeId`'s doc and the trait docs that `NodeId` is a replica slot and never a kernel node number, and how to translate.

### 5. Public API returns `arc_swap::Guard`, welding the crate's semver to a foreign dependency type
- **File:line:** `crates/shamir-numa/src/node_replicated.rs:69-79`
- **Severity:** low
- **Issue:** `load_local`/`load_node` return `arc_swap::Guard<Arc<T>>`, making `arc-swap` a public API dependency: any arc-swap release that changes `Guard` is a breaking change for `shamir-numa` consumers, and every caller must understand `Guard`'s debt-slot semantics (including its interaction with concurrent writers).
- **Suggested fix:** Return plain `Arc<T>` via `load_full()` at the public boundary (one refcount clone; negligible for read-mostly registries), or wrap in a newtype. Keep `Guard` internal if a benchmark ever justifies it.

### 6. Inline `#[cfg(all(test, ...))] mod tests` in `linux.rs` violates the documented test-organisation rule
- **File:line:** `crates/shamir-numa/src/linux.rs:179-200` (vs CLAUDE.md "Test organisation" rule 5)
- **Severity:** low
- **Issue:** CLAUDE.md is explicit: "Never embed `#[cfg(test)] mod tests { ... }` inline inside implementation files. Move them to the `tests/` directory." The two Linux-host tests (`probe_on_real_linux_host_succeeds`, `current_node_is_in_range`) live inline in the implementation file, while the rest of the crate correctly uses `src/tests/` + the `tests/` manifest. (Overlap note: this may be the sibling test-organisation reviewer's territory; recorded here because it is a documented-standards violation.)
- **Suggested fix:** Move to `src/tests/linux_topology_tests.rs` behind `#[cfg(target_os = "linux")]`, wired through `src/tests/mod.rs`.

### 7. Test-coverage claims vs reality: `detect()` untested off-Linux, `LinuxTopology` error paths untested, README references a non-existent CI workflow
- **File:line:** `crates/shamir-numa/src/detect.rs:23-43`; `crates/shamir-numa/src/linux.rs:52-93, 119-157`; `crates/shamir-numa/README.md:44-46, 63`; `crates/shamir-numa/src/tests/`
- **Severity:** low
- **Issue:** The non-Linux `detect()` -- the path every Windows/macOS dev box and CI runner actually executes -- has zero tests anywhere (`fallback_tests.rs` covers `FallbackSingleNodeTopology` directly, never the `detect` dispatch; the Linux-side coverage in `tests/linux_topology.rs` is `#![cfg(target_os = "linux")]`). `LinuxTopology::probe`'s error mapping (`fs_read_trim` NotFound->`Unsupported` vs other->`Syscall`, empty `online` -> `Unsupported`) and `pin_current_thread_to_node`'s syscall-failure branch are untested (only happy-path real-host tests exist; the fs reads would need a seam). `NodeReplicated::load_node`/`store_node` out-of-range clamping (finding 3's behavior) is also untested. README's Tier-3 section points at `.github/workflows/numa.yml` ("Opt-in via `[numa-qemu]` flag", "change `runs-on` in `numa.yml` tier3-qemu") -- `scripts/ci-qemu-numa-test.sh` exists, but no `numa.yml` workflow exists anywhere in the repo, so the documented opt-in mechanism doesn't exist.
- **Suggested fix:** Add a per-platform `detect()` smoke test; refactor `LinuxTopology::probe` to take an injectable fs-read closure (or a `from_parts(online, cpulists)` constructor) so error mapping is unit-testable; add a clamp test for `load_node`/`store_node`; either add the `numa.yml` workflow or delete the README references to it.

### 8. `parse_cpulist` allocates unboundedly on adversarial range tokens
- **File:line:** `crates/shamir-numa/src/cpulist.rs:36-43`
- **Severity:** low
- **Issue:** A single token like `0-4294967295` expands into a 2^32-entry `Vec<usize>` (~32 GiB) before sort/dedup -- an OOM abort. Input is trusted sysfs today, but the function is `pub` and explicitly advertised as "useful standalone for tooling that inspects `/proc` / `/sys` cpu masks" (`cpulist.rs:11-12`), where strings may be untrusted.
- **Suggested fix:** Cap range length (e.g. `hi - lo <= MAX_CPUS`, skip beyond) or return a compact representation; keep best-effort semantics.

### 9. Inconsistent out-of-range constructor conventions between the two topology constructors
- **File:line:** `crates/shamir-numa/src/mock.rs:46-47` vs `crates/shamir-numa/src/fallback.rs:29-34`
- **Severity:** nit
- **Issue:** `MockTopology::with_nodes(0, _)` panics ("a topology must have at least one node") while `FallbackSingleNodeTopology::with_cpus(0)` silently clamps to 1 -- both encode the same invariant ("a topology has >= 1 node") with opposite conventions. CLAUDE.md's error-handling stance prefers panics only for genuine programmer-bug invariants, which argues for one consistent choice.
- **Suggested fix:** Pick one convention (panic-on-invalid reads better for a construction-time invariant) and document both constructors identically.

### 10. README's "run tests" instruction bypasses the mandated central test entry point
- **File:line:** `crates/shamir-numa/README.md:41-42`
- **Severity:** nit
- **Issue:** README teaches `cargo test -p shamir-numa --lib`. CLAUDE.md makes `./scripts/test.sh` the contract; a single-crate `--lib` raw run sits inside the documented letter of the exception, but READMEs are where the convention gets copied from, and `./scripts/test.sh -p shamir-numa` is the sanctioned form (nextest, timeouts, guard).
- **Suggested fix:** Point at `./scripts/test.sh -p shamir-numa` (and `--full` for the Linux integration test).

## Theme compliance notes

- **Builder-only query construction:** compliant -- no `serde`/`serde_json`/`json!` anywhere under `crates/shamir-numa/` (verified by grep); no query/batch/filter/wire op is constructed.
- **Serialization/versioning:** no wire format or `serde` derives exist in this crate; the only external-format parser (`parse_cpulist`, sysfs cpulist text) has no versioning surface. `AffinityError` is non-`#[non_exhaustive]` and matchable (mock_tests matches on it), so adding variants later is a breaking change -- acceptable at `0.1.0-alpha.1` / `publish = false`, worth revisiting if the crate is ever published.

</details>
