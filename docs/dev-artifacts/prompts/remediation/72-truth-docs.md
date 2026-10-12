# Brief — TRUTH-DOCS truth-docs

You are a sub-agent working inside an isolated git worktree (your cwd) of the S.H.A.M.I.R. Rust database repository. Read `CLAUDE.md` in the cwd first: code ideology (lock-free / async / scc maps, no guard across `.await`), TDD, test layout (`tests/` dir per module, `mod.rs` = manifest only), imports at the top of the file, one primary export per file, query builder only (never hand-assembled JSON).

## Hard rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Do NOT run `cargo`, `rustc`, `nextest`, or any build or test command: the owner's machine must stay idle and ALL compilation and testing happens later in GitHub CI. Therefore be extra careful that the code compiles: read the real definitions (Read/Grep) of every type, trait and function you call instead of guessing signatures; keep the change small and local; prefer the narrowest layer that owns the bug.
- Stay strictly inside the FILE SCOPE below. Other agents work concurrently on other files of this repository; do not touch any file outside your scope even if it looks related (if you believe an out-of-scope change is required, say so in your final message instead of doing it).
- TDD: add the regression tests described in the task in the same change. They must FAIL on the current code for the stated reason, be deterministic (no wall clock, no sleeps; use barriers, notifies, hooks or paused tokio time), assert observable behaviour (not just `is_ok()`), and live in the module's `tests/` directory wired through its `mod.rs` manifest.
- Write rustfmt-clean code (4 spaces, trailing commas, short lines); the orchestrator formats touched files.
- Do not add dependencies, do not bump versions, do not rename or reformat unrelated code, do not touch unrelated comments.
- Finish with a final message listing: files changed, what each test proves and why it fails today, any design deviation from the task text, and anything you could not verify.


## Scope discipline

If a file outside the FILE SCOPE is strictly required to make the change compile or to keep an existing test meaningful (a caller of a changed signature, a fixture, a manifest line), edit it minimally and LIST it in your final message; do not stop to ask. Ask (and stop) only for genuine design decisions or when the scope would grow into another task area.
## FILE SCOPE (only these paths may be edited)

- docs ONLY: `docs/guide-docs/KNOWN_LIMITATIONS.md`, `docs/guide-docs/client-server-protocol-spec/TRANSACTION_OUTCOMES.md`, `vendor/hnsw_rs/VENDORED.md`, `docs/checkpoints/2026-10-10-1847.md`, `docs/checkpoints/2026-08-12-1420.md`, `docs/checkpoints/2026-08-12-1730.md`
- private-path sanitization in comments ONLY (no code/semantic change): root `Cargo.toml` comments (~:24-29), `deny.toml` comment (~:129), `crates/shamir-bench-utils/src/lib.rs` doc comment, and the bench doc comments in `crates/shamir-engine/benches/{cond_expr_eval,f78_writer_latency,for_each_eval,when_skip_eval}.rs` and `crates/shamir-index/benches/reader_drain_gate.rs` (replace workstation paths such as `D:/dev/rust/...` with neutral wording like `<dedicated bench target dir>`; keep the measured numbers)
- NOT in scope (list them in your report as owner decisions): `CLAUDE.md` files, the `bench-tool` alias in `.cargo/config.toml`, everything under `docs/dev-artifacts/**` (historical records)
- NO code changes

## Task

TRUTH-DOCS (task #1319). The whole remediation series (SNAP-FIX, S5b, N1-live, S5c, W/B tasks) is MERGED on master, your base. Four independent read-only design reviewers have just reviewed the repository; their answers are appended as the binding rationale for this task. Verify every claim you act on against the code before changing anything (a reviewer's OBSERVED line numbers may have drifted by a few lines). Sibling agents run concurrently on other follow-ups; stay strictly inside your scope. Tests that park one commit and then await another INLINE hang (commit and non-tx writes now wait for the completion watermark W): spawn the second operation, never raise a timeout. Do NOT delegate to child agents and do not wait on schedules: finish the work yourself in this worktree. No git mutation, no builds/tests (CI only), imports at the top of files, one primary export per file, tests under `tests/` dirs, builder-only queries in code and docs (except the documented wire-format docs).

DO (each item: read the code, then edit the text so it is true for master, citing file:line that you re-verify):
1. `KNOWN_LIMITATIONS.md` section 5, the two STALE replication bullets (both found independently by two reviewers): (a) ~:542-545 says replicated map records carry no field bindings and may decode under wrong names - master translates ids through per-event bindings and persists interner bindings (`crates/shamir-engine/src/tx/replicated_event.rs`, `apply_replicated.rs` ~:348-372; test `replication_interner_tests.rs`); (b) ~:560-562 says an event for a missing table is skipped and the bookmark advances - master rejects it (`apply_replicated.rs` ~:220-227) and the follower loop stops before advancing (`follower_loop.rs` ~:389-393). Rewrite both to the true behaviour (keep whatever limitation genuinely remains, e.g. DDL/meta not replicated, no secondary indexes - verify).
2. `KNOWN_LIMITATIONS.md`: add concise, accurate statements (new bullets in the transactions section, near the existing snapshot/ack text) for: contiguous settlement (a transaction commit/non-tx write acknowledgement waits until every lower version is settled; one stuck lower version delays every higher acknowledgement of the same repository; no give-up timeout exists today; a post-WAL projection failure delays the acknowledgement until the drainer replays the orphan; `ReopenRequired` yields outcome `Deferred` for transactions but a NORMAL return for non-transactional writes (`crates/shamir-tx/src/repo_tx_gate.rs` ~:1113, `mvcc_store/mod.rs`); `AsOf(v)` waits indefinitely for an allocated unsettled version (callers cancel); `result_too_large` after execution is NOT proof of rollback: wire-size enforcement runs after execution (`crates/shamir-server/src/db_handler/handler.rs` ~:595,631), so a refused response may follow a committed write and must not be retried blindly; cursor pages use `cursor_page_too_large` (`cursor_handlers.rs`), not `result_too_large`.
3. `TRANSACTION_OUTCOMES.md` (~:65 and neighbours): the sentence that `Deferred` means recovery only at the next open is stale - online orphan replay now participates, and `Deferred` is not itself a visibility field; correct it, and add the `requires_reopen` / `AsOf` error rows if missing.
4. `vendor/hnsw_rs/VENDORED.md`: add the manifest deviation the reviewer found (empty `simdeez_f` / `stdsimd` features instead of forwarding upstream's, `vendor/hnsw_rs/Cargo.toml` ~:28-34) to the vendor-delta list; state that the shipped dependency is '0.3.5 + Shamir patch'.
5. Private-path sanitization as scoped above. In `docs/checkpoints/*.md` change only the private paths (keep the historical content). Grep the scoped files afterwards for `D:/`, `D:\`, `C:\Users`, `/d/dev`, `system_artefact` and report what remains and where (only out-of-scope files may remain).
Report: every statement you changed with the code reference that makes it true, and the out-of-scope private-path list.

REVIEWER ANSWERS (context; items 5, 7, 10 of the release review and the 'production interactions' section of the behaviour review are the relevant parts):

# Verdict: **NO-GO for the first alpha tag today**

The checkout is close to a defensible **single-node, non-production alpha**, but the release path has a confirmed hard blocker: **the required versioned changelog heading does not exist**. Before tagging, also close the signing rehearsal/identity-verification gap, ship third-party notices, correct materially stale replication documentation, and obtain fresh GitHub CI evidence for the exact release candidate.

**Do not block the alpha on upstream accepting the HNSW fix, checkpoint-branch publication, comparative benchmarks, or a replication redesign.**

Evidence terminology below:
- **OBSERVED:** local files, branch history, or source inspected here.
- **INFERRED:** expected execution consequences or design recommendations; no builds, tests, network calls, or live GitHub checks were performed.

## 1. `hnsw_rs`: keep the vendor patch for alpha; contribute upstream

### What is actually consumed

**OBSERVED:** the dependency declaration is `hnsw_rs = "=0.3.5"`, but the workspace replaces the crates.io package with **tracked local source**, not a Git dependency or remote fork:

- Exact dependency and explanation: `crates/shamir-index/Cargo.toml:46–54`.
- Workspace membership: `Cargo.toml:2–4`.
- Path override: `Cargo.toml:129–137`.
- Upstream origin and original crates.io checksum: `vendor/hnsw_rs/VENDORED.md:3–9`.
- Vendor package is explicitly non-publishable: `vendor/hnsw_rs/Cargo.toml:17`.

There are **two separate fixes**, not one:

1. **Reverse-edge layer placement:** already fixed upstream in 0.3.5. The project regression checks upper-level points receive layer-zero backlinks: `crates/shamir-index/src/vector/tests/hnsw_rs_contract_tests.rs:564–637`.
2. **Concurrent-insert lock ordering:** locally patched. The new-point neighbor list is cloned while holding its read guard; that temporary guard ends before acquiring another point’s write guard: `vendor/hnsw_rs/src/hnsw.rs:1245–1255`.

The lock-order regression deliberately parks two reverse updates on private neighbors, releases the pins, and requires both updates to finish within bounded channel deadlines: `vendor/hnsw_rs/src/hnsw.rs:1916–1963`.

The vendor commit `c31ec14d` records a CI stack probe and red-then-green development. **That is historical evidence recorded in Git, not a CI result independently verified during this review.**

The adapter genuinely uses parallel graph insertion, including batch ingest: `crates/shamir-index/src/vector/hnsw_adapter.rs:2479–2495,2613–2627`. Replacing the patch with blanket insertion serialization is therefore not the smallest equivalent solution.

### What else is local

The documented vendor delta includes trimmed targets/dependencies, warning suppression, formatting suppression, the marker, and lock fix: `vendor/hnsw_rs/VENDORED.md:21–30`.

**An additional manifest deviation exists:** `simdeez_f` and `stdsimd` are empty rather than forwarded upstream features: `vendor/hnsw_rs/Cargo.toml:28–34`. Add this to the vendor-delta inventory; do not represent the copy as “only a lock-order patch.”

A compile-time marker prevents silently reverting to unpatched crates.io source: `crates/shamir-index/src/vector/snapshot.rs:118–122`.

### Recommendation and risks

**Contribute the minimal lock-order fix plus its regression upstream.** Offer the reverse-edge regression separately as coverage for an already-fixed bug. Do not submit the Shamir manifest changes, marker, or blanket lint suppressions upstream.

**Keeping the vendor copy is acceptable for alpha**, subject to:

- **Licence:** MIT/Apache licensing permits modification and redistribution. Preserve upstream notices. The MIT text explicitly requires its copyright/permission notice in distributed copies or substantial portions: `vendor/hnsw_rs/LICENSE-MIT:1–15`. Source-tree preservation exists; binary-package notice coverage needs correction, discussed below.
- **Maintenance:** the project owns this delta until an equivalent upstream release is adopted. Removal also needs to replace/remove the compile-time marker check—not merely delete the directory and patch entry.
- **Supply chain:** vendoring makes source available and reviewable at the release commit, but the original crates.io checksum is an origin record, **not authentication of the modified tree**. Identify the shipped dependency as “0.3.5 + Shamir patch,” tied to the release commit.
- **`cargo deny`:** allowed licences cover MIT and Apache; unknown registries/Git sources are denied: `deny.toml:65–69,127–135`. A local path package is not an unknown Git source. **A passing deny check does not audit the patch’s correctness or prove equivalence to upstream.**
- **Lint blind spot:** third-party warnings/clippy are suppressed, expressly documented at `vendor/hnsw_rs/VENDORED.md:26–27`. Review the small delta separately.

**Smallest correct plan:** document the complete vendor delta, preserve notices in distributables, retain existing CI regression coverage, submit upstream, and keep the vendor patch until an equivalent release passes the documented cooldown and snapshot/adapter compatibility checks (`Cargo.toml:132–135`). Upstream acceptance is not a tag prerequisite.

## 2. Cosign: add a fail-closed, artifact-only dispatch rehearsal

### Current release behavior

**OBSERVED:**

- Release only triggers on pushed `v*` tags; there is **no dispatch entry**: `.github/workflows/release.yml:57–60`.
- Signing has `id-token: write`, downloads archives, and invokes keyless `cosign sign-blob`: `.github/workflows/release.yml:710–737`.
- It uploads bundles, signatures, and certificates, but performs **no verification step**: `.github/workflows/release.yml:743–751`.
- The final release job has `contents: write` and creates a prerelease: `.github/workflows/release.yml:763–835`.
- Docker is built/smoke-tested, **not pushed**: `.github/workflows/release.yml:561–568`.
- SBOM generation exists. **Build-provenance attestation does not exist in the inspected workflows.** A blob signature is not an attestation explaining how the binary was built.
- The distributed verification instructions omit issuer and signer identity: `deploy/VERIFY.md:27–34`.

### Safest rehearsal design—no tag, no release publication

Recommend extending the existing release workflow rather than maintaining a permanently divergent signing implementation:

1. Add `workflow_dispatch` with:
   - `dry_run`: boolean, default `true`;
   - `release_version`: candidate version, currently `0.1.0-alpha.1`.
2. **Every dispatch must be non-publishing, even if someone supplies `dry_run: false`.** Gate the publishing job on all of:
   - event is `push`;
   - ref is a tag;
   - tag meets the production release naming policy;
   - repository is the canonical repository.
3. Dispatch from trusted `master`, using an explicitly recorded candidate commit. Pass the version input through an environment variable and validate it; do not interpolate arbitrary input into shell code.
4. Run the real gate, archive builds, Docker smoke, SBOM generation, signing, and **identity-constrained verification**. Upload results only as workflow artifacts.
5. Verify every archive with its bundle and:
   - issuer: `https://token.actions.githubusercontent.com`;
   - exact dispatch signer identity:  
     `https://github.com/PHPCraftdream/shamir-db/.github/workflows/release.yml@refs/heads/master`.
6. Include negative verification: another identity must fail, and modified archive bytes must fail.

For the eventual production tag, require the **exact version-specific identity**, for example:

`https://github.com/PHPCraftdream/shamir-db/.github/workflows/release.yml@refs/tags/v0.1.0-alpha.1`

Do not use an unrestricted identity regexp or accept any GitHub-issued certificate.

**Important:** artifact-only means no GitHub Release or registry publication. Normal public keyless signing still contacts Sigstore and creates public transparency-log evidence. Obtain the owner’s consent for that disclosure; use non-sensitive artifacts.

### Test-tag/fork alternatives

- **`refs/tags/test-*` does not trigger this workflow today.** It proves nothing unless a dedicated trigger/path is added (`release.yml:57–60`).
- If a test-tag path is added, explicitly exclude it from publication. Its certificate identity contains `refs/tags/test-*`; it is not the production identity.
- A fork rehearsal is safer organizationally but proves the fork’s OIDC identity/policies—not canonical-repository release authorization.
- Do not push a “test” `v*` tag in the canonical repository. That enters the real publishing DAG.

A real production tag is still needed to validate actual tag-event routing, exact tag certificate identity, tag protections, release-creation permission, and final asset attachment. The dispatch rehearsal validates the machinery without claiming to validate those settings.

### Required permissions, credentials and external configuration

| Requirement | Repo evidence / status |
|---|---|
| Checkout/source access | Default `contents: read`, `release.yml:69–70` |
| Keyless signing | Signing-job `id-token: write`, `release.yml:714–715` |
| Release creation | Publishing-job `contents: write`, `release.yml:785–786` |
| GitHub authentication | Automatically supplied `GITHUB_TOKEN`; no custom PAT referenced |
| Private signing key/password/KMS secret | **None required or referenced** |
| Registry credentials / `packages: write` | Not needed; no image/package push exists |
| Artifact upload/download | Configured actions; no separate user secret referenced |
| OIDC audience | Cosign/Sigstore default audience is expected; not explicitly configured here |
| Trusted signer/issuer checks | **Missing** from workflow verification and release instructions |
| Actions/OIDC availability, organization policy, tag rules, protected environments, external-service access | **Unverifiable offline**; no protected release environment is declared |
| Provenance permissions | Not currently needed; adding GitHub attestations would require its own permissions and policy |

Also pin the actual cosign executable version for the rehearsal: the installer action is SHA-pinned, but no cosign-version input is specified at `release.yml:722–723`.

## 3. Local checkpoints: preserve privately; publish one sanitized summary, not the branches

**Recommendation:** do not push the three checkpoint branches as-is. Preserve their raw history in owner-controlled private backup, then publish **one curated docs commit based on current master**, containing durable decisions and evidence links—not live task lists or workstation instructions. Do not merge their old bases.

**OBSERVED from branch logs:**

- Wave1: `784c51a4`, adds `2026-10-11-1245.md`.
- Wave2: `02599c14`, adds `2026-10-11-1613.md`.
- Wave3: `4d477bf0`, `b7cec8ae`, `6dea7e6c`, adding 20:28, 20:43 and 23:55 notes.

These files describe historical intermediate states, sometimes explicitly red probes or unresolved questions now overtaken by master.

### Publication findings

- **Private machine/scratch paths block publication under the stated rule:**
  - `docs/checkpoints/2026-10-11-2028.md:4` on wave3 references private brief-generation and CI-monitor helper paths.
  - `docs/checkpoints/2026-10-11-2043.md:4` references the same monitoring/temp-script tooling.
  - `docs/checkpoints/2026-10-11-2355.md:4–6` references private watcher, reviewer helper and scratch design-brief paths.
- **Scratch/session identifiers:** wave1 `2026-10-11-1245.md:4` and wave2 `2026-10-11-1613.md:4` contain session cron identifiers, temporary worktree/process descriptions, and internal task IDs. These are not secrets by themselves, but should not become public operational instructions.
- **No credential material observed in the inspected branch-only notes.** This was not a comprehensive secret scan.
- **`docs/checkpoints/` is already tracked on master.** Git history and the current files establish that; the directory is not inherently prohibited.
- The existing master file `docs/checkpoints/2026-10-10-1847.md:6` already discloses private cache/temp paths and unrelated local project names. Publishing no new branches does **not** resolve the current-tree hygiene issue.
- Private development paths also appear as historical comments in `Cargo.toml:24–29`; those are no longer active dependency paths, but still violate a literal “no private machine paths” publication rule.

**Do not discard the only durable notes before preserving their useful information.** Conversely, their existence is not a reason to expose raw session state publicly.

## 4. Ranked release-hygiene findings

The list distinguishes blockers from accepted alpha limitations; it is not a claim that ten new product bugs were reproduced.

| Rank | Severity / decision | Finding and smallest action |
|---|---|---|
| **1** | **Hard release blocker** | **Missing release changelog heading.** `CHANGELOG.md:17–37` contains only `[Unreleased]`, while `release.yml:400–407` requires exactly `[0.1.0-alpha.1]`. **INFERRED:** the version-consistency job fails and blocks downstream release jobs. Add finalized, versioned notes before tagging. All inspected first-party Cargo package versions are `0.1.0-alpha.1`; README agrees at `README.md:47`. |
| **2** | **Distribution blocker** | **Third-party notices omitted.** Release staging includes only project licence texts (`release.yml:496–502`) and explicitly defers third-party notices (`release.yml:492–494`). Preserve required notices for shipped dependencies, starting with vendor MIT attribution (`vendor/hnsw_rs/LICENSE-MIT:1–15`); audit the shipped dependency graph rather than assuming project licences suffice. |
| **3** | **Security/release prerequisite** | **Signing is not verified against an authorized identity.** Add the dispatch rehearsal and fail-closed verification; correct `deploy/VERIFY.md:33–34`. Signing success alone is insufficient evidence of the intended release identity. |
| **4** | **Required external evidence—not a proven red workflow** | **Current CI status cannot be established from source.** Obtain exact-candidate master CI, supply-chain, TS e2e and release-rehearsal results in GitHub. Badges and historical checkpoint “green” statements are not evidence for this commit. Main lib CI is configured across three OSes (`ci.yml:59–84`); no run results were available here. |
| **5** | **Public-doc blocker** | **Replication docs contradict current source.** `KNOWN_LIMITATIONS.md:542–545` says maps lack field bindings; `replicated_event.rs:7–12` supplies them and `apply_replicated.rs:356–372` rewrites IDs. The existing test asserts cross-interner decoding (`replication_interner_tests.rs:101–147`), but was not executed here. `KNOWN_LIMITATIONS.md:560–562` says missing tables are skipped/bookmarks advanced; source rejects them (`apply_replicated.rs:220–227`) and the follower exits before bookmark advancement on rejection (`follower_loop.rs:389–393`). Correct both bullets. |
| **6** | **Publication-policy blocker** | **Private workstation paths already on master.** Sanitize the current-tree examples noted in section 3, and review raw checkpoint additions before publication. Whether old history must also be scrubbed is an owner policy decision, not permission to rewrite history during this review. |
| **7** | **Security decision before public deployment** | **Host Rust compilation is broadly authorized, not sandboxed.** Compiler default mode is `0o755` (`access_control.rs:162–174`); the compiler explicitly describes native host compilation and only defense-in-depth controls (`compile.rs:8–47,134–137`). **No exploit was established here.** Recommend shipped deployment guidance/defaults restrict compiler execution to trusted administrators; retain prebuilt WASM upload. Calling it “development only” does not enforce that boundary. |
| **8** | **Supply-chain evidence gap** | **Release does not require a fresh deny/audit job; audit policy differs.** Supply-chain push/PR coverage is branch-based (`supply-chain.yml:46–57`), not part of the release DAG. `deny.toml:47–59` accepts bincode’s unmaintained advisory, while scheduled/manual `cargo audit` uses no ignore (`supply-chain.yml:108–115`) and `.cargo/audit.toml` is absent. **INFERRED:** audit can report/fail on the intentionally accepted advisory. Reconcile the narrowly justified exception with a tracking issue/review trigger; require fresh supply-chain evidence before tag. Do not erase the warning by blanket suppression. |
| **9** | **Artifact completeness / claims** | **SBOM and provenance need precise scope.** Release stages only `find crates ...`, omitting the vendor member’s own SBOM (`release.yml:678–688`); on-demand upload similarly uses `crates/*/*.cdx.json` (`supply-chain.yml:137–144`). First-party dependency SBOMs may still contain HNSW, so total omission is **not established**. Inspect generated CI artifacts and identify the patched source accurately. No build-provenance attestation was observed; either add one or expressly state “signed artifacts + SBOM,” not provenance. |
| **10** | **Accepted alpha constraints; targeted docs follow-up** | **Do not confuse skipped/manual coverage with red CI.** NUMA QEMU is opt-in (`numa.yml:45–50`); performance is dispatch-only and documents an unregistered runner (`perf-gate.yml:53–71`); Node binding e2e is deliberately outside release gating (`release.yml:42–44`). These need explicit scope acceptance, not invented green status. Existing docs already acknowledge exact snapshot vector scans, migration freeze/in-memory destination and conservative crash gaps (`KNOWN_LIMITATIONS.md:28–35,333–365,563–610`). Add a concise contiguous-settlement/commit-wait explanation; avoid suggesting crash certification reconstructs journal events. |

### Prompt history and README

**Keep committed remediation briefs.** They are legitimate public engineering history, not a release blocker merely because they contain internal task IDs. Label them as historical intentions, not current guarantees.

That distinction matters: `docs/dev-artifacts/prompts/remediation/71-recovery-gap-consumer.md:31` requests convergence after recreating a follower from zero, whereas current operator guidance correctly says zero replay cannot traverse the certified source gap (`KNOWN_LIMITATIONS.md:599–610`). Do not silently rewrite the original brief; make the current outcome authoritative.

README positioning is appropriately conservative: non-production alpha, no compatibility promise, offline backups and experimental replication (`README.md:14–18,64–74`). Do not replace it with “production-grade” or “decentralized/HA” claims for this release.

**Final release decision:** authorize the alpha after the changelog, notices, signer verification/rehearsal, stale docs and publication-policy issues are closed, with fresh exact-candidate CI evidence. Retain the vendor patch and explicitly accept the documented experimental limitations.

**Files changed: none. Builds/tests run: none. Network services contacted: none.**

----

## Verdict: **conditional GO for first alpha; NO-GO for the current operational contract unchanged**

Keep the correctness changes. Do **not** restore eager snapshots, snapshot ANN, false settlement after projection failure, or silent subscription loss.

Before the first tag, require:
1. A distinct **committed-but-visibility-pending acknowledgement**, with a configurable bounded **post-commit** wait.
2. Operator-visible repository progress/degradation state, including stalled active tails and orphan replay.
3. Truthful shutdown reporting and corrected commit/retry documentation.

**One stuck lower version can indefinitely block acknowledgements for every higher version in the same repository.** This is not necessarily database-wide, and it does not block read-only/empty commits that allocated no version. `Unknown` tickets are different: they trigger reopen rather than indefinite visibility waiting. Persistent replay failures and live, wedged post-WAL tails have no equivalent automatic escape.

**Evidence standard:** **OBSERVED** below means inspected source behavior, not a reproduced runtime incident. **INFERRED** identifies production consequences. No builds, tests, benchmarks, or CI results were inspected.

## Decision table

| Item | Severity and alpha decision | Default/configuration decision | Required before first tag |
|---|---|---|---|
| **1. Result-size enforcement** | **Resource-safety win; compatibility break. Accept.** OBSERVED: server default is `true`; all four deployment examples enable it. The estimator charges cumulative materialization, including intermediates, not wire bytes or RSS. Encoded batch responses have an additional unconditional cap. `crates/shamir-server/src/config.rs:361`, `deploy/server.example.ktav:87`, `crates/shamir-engine/src/query/batch/result_size.rs:12`, `crates/shamir-server/src/db_handler/handler.rs:631`. | **ON is right.** Existing enforcement and size knobs suffice; disabling materialization enforcement must not disable wire protection. | Document separate materialization/wire limits and **post-execution rejection semantics**. Count refusals separately by stage; measure estimated bytes, encoded bytes, and peak RSS in CI. **Correction to the premise:** cursor rejection is `cursor_page_too_large`, not `result_too_large`; it uses the server page cap. `crates/shamir-server/src/db_handler/cursor_handlers.rs:315`, `crates/shamir-server/src/db_handler/handler.rs:747`. |
| **2. Exact snapshot vector ranking** | **Data-correctness win; substantial latency risk. Accept.** OBSERVED: Snapshot/Serializable dispatch bypasses live ANN for supported vector shapes; scoring scans snapshot-plus-overlay batches and sorts/truncates winners. `crates/shamir-engine/src/table/read_exec.rs:344`, `crates/shamir-engine/src/table/read_exec.rs:1965`. | **Exact is right. No “unsafe ANN in snapshot” knob.** Permit operators to restrict expensive queries or concurrency, not weaken isolation. | Existing limitations documentation is helpful. Require a CI scaling measurement over N, dimension, k, residual selectivity, and concurrency. Expose exact-plan frequency, scanned rows, and latency; query stats already record scanned rows. `crates/shamir-engine/src/table/read_exec.rs:2024`. A dedicated work/admission knob can follow unless measurements establish unacceptable resource exposure. |
| **3. W snapshots and acknowledgement waits** | **Data-correctness win; critical availability risk. Accept W, reject indefinite request residency as the only contract.** OBSERVED: allocated commits wait after the inner driver; read-only/empty outcomes skip the wait. `crates/shamir-engine/src/tx/commit.rs:587`. | Ordinary success must continue to mean visibility through the returned version. Add a configurable bounded acknowledgement wait returning a **distinct pending outcome**, not ordinary success or abort. Keep explicit strict/unbounded mode for callers that consciously require it. | Pending outcome, exported progress/age/waiter metrics, and a stalled-repository runbook. A gauge alone makes the incident diagnosable but does not release occupied request capacity. |
| **4. Post-WAL projection failure / follower partial apply** | **Data-correctness win; availability risk. Accept failure-closed settlement.** OBSERVED: failed data/index projection drops the guard and wakes orphan replay; successful replay records a changefeed gap before settling visibility. Follower partial apply has no local WAL replay owner. `crates/shamir-engine/src/tx/materialize.rs:258`, `crates/shamir-engine/src/tx/drainer.rs:1198`, `crates/shamir-engine/src/tx/apply_replicated.rs:456`. | **Do not configure false settlement.** Use the item-3 pending acknowledgement for known committed transactions. Expose explicit recovery-required/degraded state. | Distinguish active-tail stall, retrying orphan, and reopen-required states. Ensure follower supervisor/admission handles reopen as an operational stop, rather than generic endless backoff. |
| **5. Non-transactional write acknowledgement** | **Read-after-write correctness win; latency risk and compatibility break. Accept only with explicit degraded-outcome semantics.** OBSERVED: CRUD releases writer guards before waiting—essential to avoid replay cycles. `crates/shamir-engine/src/table/table_manager_crud.rs:258`. | Keep release-before-wait and migration no-wait exceptions. Apply acknowledgement policy consistently to embedded and server APIs. | **Current exception needs disclosure or a typed outcome:** on reopen, the helper logs and returns normally despite visibility not being established. `crates/shamir-tx/src/repo_tx_gate.rs:1113`, `crates/shamir-tx/src/mvcc_store/mod.rs:788`. Do not turn a completed write into a retryable failure. |
| **6. AsOf validation/waiting** | **Data-correctness win; compatibility break and latency risk. Accept.** OBSERVED: unallocated versions fail; allocated unsettled versions wait; reopen produces a typed error. `crates/shamir-engine/src/table/read_temporal.rs:85`. | **Right semantics.** Never silently clamp a requested version or substitute Latest. A cancellable read-wait deadline is safe and distinct from a commit deadline. | Document that AsOf can wait indefinitely and how callers cancel it; expose wait latency/degradation. An optional typed read-wait timeout is desirable, but not a blocker once the repository stall contract is explicit. |
| **7. Subscription termination and journal delivery** | **Data-correctness win; availability/compatibility tradeoff. Accept.** OBSERVED: 100 attempts mean **99 sleeps, 495 ms scheduled sleep**, not a hard 500-ms deadline. Gap/read failure terminates draining; snapshot failure returns before Ready. `crates/shamir-server/src/subscriptions/push.rs:76`, `crates/shamir-server/src/subscriptions/journal_drain.rs:78`, `crates/shamir-server/src/subscriptions/bridge.rs:501`. | Fail closed is right. Accept the retry window as an alpha default; do not require a tuning knob before measuring it. Terminal notifications are best-effort. | Export closure reasons, push retry/exhaustion counts, and commit-to-journal-to-delivery latency. Require clients to handle closure/absence of Ready, discard incomplete snapshots, and resynchronize. Current protocol already documents best-effort slow-consumer/closed notifications. `docs/guide-docs/client-server-protocol-spec/SUBSCRIPTIONS.md:304`. |
| **8. Cold-open certification and flush errors** | **Data-correctness win; major operational compatibility/availability break. Accept consciously as experimental replication.** OBSERVED: certification persists `(J,H]` conservatively; constructor recovery reaches certification before registry insertion; errors propagate. `crates/shamir-engine/src/repo/repo_instance.rs:1843`, `crates/shamir-engine/src/tx/recovery.rs:458`, `crates/shamir-engine/src/db_instance/db_instance.rs:55`. | **Fail closed is right. No bypass-certification knob.** Do not advertise automatic crash-transparent replication recovery. | Retain the existing reseed runbook and expose certification/shutdown failures. The documented workaround can require a **new source lineage**, not merely restarting a follower. `docs/guide-docs/guide/07-operations.md:89`. This is acceptable for alpha, not an HA claim. |

## Critical path: what actually escapes, and what does not

### 1. Reopen-required is an escape—but not repair

**OBSERVED:** `await_visible` checks W first, then reopen; pending waiters recheck at 50-ms intervals. On reopen, `commit_tx` changes the materialization outcome to Deferred. New transactional commits are refused at entry.

References: `crates/shamir-tx/src/repo_tx_gate.rs:1065`, `crates/shamir-tx/src/repo_tx_gate.rs:1089`, `crates/shamir-engine/src/tx/commit.rs:599`, `crates/shamir-engine/src/tx/commit.rs:814`.

An unresolved `Unknown` ticket sets reopen in the drainer; guard Drop also handles Unknown immediately. A `Pending` ticket merely stops the replay prefix.

References: `crates/shamir-engine/src/tx/drainer.rs:484`, `crates/shamir-tx/src/version_guard.rs:196`.

**Qualification:** the acknowledgement can subsequently encounter the separate, bounded five-second durability backpressure wait. Reopen is not necessarily an immediate wire response. `crates/shamir-engine/src/tx/commit.rs:615`, `crates/shamir-engine/src/tx/commit.rs:758`.

### 2. Persistent orphan replay failure can freeze W indefinitely

**OBSERVED:** finalization stops at a failed table. Orphan history failure leaves the version unsettled; changefeed-gap persistence must also succeed before completion. Background replay logs failure and retries on subsequent wake/interval passes. There is no failure-count escalation to reopen in these inspected paths.

References: `crates/shamir-engine/src/tx/drainer.rs:1094`, `crates/shamir-engine/src/tx/drainer.rs:1198`, `crates/shamir-engine/src/tx/drainer.rs:1210`, `crates/shamir-engine/src/tx/drainer.rs:1486`.

**INFERRED:** persistent storage errors, unavailable table registration, or failing gap persistence can hold W below the orphan forever. Higher-version acknowledgements accumulate even when their own projections succeeded.

### 3. A live wedged post-WAL tail is more dangerous than an orphan

**OBSERVED:** replay explicitly stops at active post-WAL ownership. It cannot safely replay concurrently with the live committer. Cancelling the caller transfers the suspended inner commit to a detached task rather than terminating ownership.

References: `crates/shamir-engine/src/tx/drainer.rs:546`, `crates/shamir-engine/src/tx/commit_driver.rs:91`.

**INFERRED:** an I/O operation that never completes can retain active ownership indefinitely. Caller disconnection does not necessarily convert it into a recoverable orphan.

There **is** a batch-operation diagnostic watchdog, but it only logs; it does not recover or fail-stop. Its registration covers executing aliases, whereas final transactional commit occurs after the plan finishes. Do not mistake it for a commit-tail watchdog.

References: `crates/shamir-engine/src/query/batch/op_watchdog.rs:11`, `crates/shamir-engine/src/query/batch/batch_execute.rs:562`, `crates/shamir-engine/src/query/batch/batch_execute.rs:864`.

### Smallest safe mitigation

Bound **only the acknowledgement wait after the inner commit has returned a known outcome**. Return transaction/version metadata plus a distinct `visibility_pending` state. Ordinary success still requires W ≥ version; pending callers must wait/reconcile, not replay writes.

This does **not** solve a stuck inner commit or establish the outcome of a Pending/Unknown WAL ticket. Those require a repository-level stall diagnostic/admission state and an explicit reopen/reconciliation procedure. Never report “committed” merely because a version was allocated.

## Production interactions the owner should expect

- **Repository head-of-line amplification.** OBSERVED: WAL windows share append/sync work, while visibility depends on contiguous version completion. INFERRED: batch-wide disk delay or one slow materialization tail amplifies acknowledgement p99 across otherwise unrelated tables. The WAL leader also drains successive windows before returning. `crates/shamir-wal/src/wal_group_commit.rs:208`, `crates/shamir-wal/src/wal_group_commit.rs:284`.
- **Cross-repository server capacity coupling.** OBSERVED: Execute reserves response bytes before execution and holds the reservation through commit/visibility waiting; dispatch tasks retain connection permits. INFERRED: a repository-local stall can occupy enough global byte budget to obstruct healthy repositories too. `crates/shamir-server/src/db_handler/handler.rs:562`, `crates/shamir-server/src/connection/request_loop.rs:323`.
- **The execution-time cap is not a commit deadline.** OBSERVED: the final deadline check precedes commit; no subsequent check bounds acknowledgement waiting. A configured 60-second execution cap does not imply a 60-second response bound. `crates/shamir-engine/src/query/batch/batch_execute.rs:814`.
- **Warning volume scales with waiters, not incidents.** OBSERVED: the five-second warning fires once **per wait**, not every five seconds. INFERRED: many affected requests still generate a warning burst; every parked waiter also has a 50-ms recheck timer. Aggregate/rate-limit by repository. `crates/shamir-tx/src/repo_tx_gate.rs:1095`.
- **Result rejection can conceal a completed write.** OBSERVED: wire-size enforcement happens after execution and subscription activation; non-transactional materialization charging can occur after an operation executes. INFERRED: `result_too_large` is not universally proof of rollback. Preserve committed metadata where possible and explicitly forbid blind retries. `crates/shamir-server/src/db_handler/handler.rs:595`, `crates/shamir-server/src/db_handler/handler.rs:631`, `crates/shamir-engine/src/query/batch/batch_execute.rs:577`.
- **Stalled W pins retention.** OBSERVED: the GC floor includes W even without explicit snapshots. INFERRED: continued writes above a stuck floor retain history/log state; unbounded waiting is not a memory bound. `crates/shamir-tx/src/repo_tx_gate.rs:774`.
- **A normal stop can become an unclean restart.** OBSERVED: shutdown requires W = allocation high-water after quiescence/drain. All repositories share one flush budget. The server logs flush failure, and `serve` still returns `Ok(())`; its outer 30-second deadline also returns success after warning. Consequently, a shutdown error is currently not reliably visible as process failure. `crates/shamir-engine/src/repo/repo_instance.rs:1872`, `crates/shamir-db/src/shamir_db/shamir_db/db_management.rs:629`, `crates/shamir-server/src/server/server_handle.rs:165`, `crates/shamir-server/src/runtime.rs:52`.

## Prioritized, PR-sized follow-ups

1. **P0 — Bounded visibility acknowledgement with truthful outcomes.**  
   Scope: `repo_tx_gate.rs`, engine `tx/commit.rs`, transaction outcome/wire types, DB transaction facade, server transaction handlers, clients. Add configurable post-inner-commit waiting and explicit pending metadata; preserve strict mode and WAL-durability distinctions. Handle non-tx outcomes without implying rollback.

2. **P0 — Repository progress/degradation diagnostics and admission policy.**  
   Scope: `completion_tracker.rs`, `repo_tx_gate.rs`, `version_guard.rs`, `commit_driver.rs`, `drainer.rs`, server observability/status. Export allocation H, W, history watermark, oldest blocking version/age/class, active tails, unresolved tickets, orphan retry/error state, waiters, and reopen-required. The slow counter exists, but waiter count is test-only and the inspected Prometheus bridge does not export these gate diagnostics. `crates/shamir-tx/src/completion_tracker.rs:133`, `crates/shamir-tx/src/completion_tracker.rs:147`, `crates/shamir-server/src/observability.rs:457`. Do not force-settle stalled versions.

3. **P0 — Follower recovery-required propagation.**  
   Scope: `tx/apply_replicated.rs`, replication `follower_loop.rs`, error classification and supervisor/status. Make partial-apply reopen state explicit and stop inappropriate retries/admission. Current apply errors take generic backoff, while the inspected apply allocation path has no reopen-entry check. `crates/shamir-server/src/replication/follower_loop.rs:395`, `crates/shamir-engine/src/tx/apply_replicated.rs:396`.

4. **P0 — Truthful shutdown and retry documentation.**  
   Scope: server `server_handle.rs`, `runtime.rs`, shutdown status/error plumbing; `TRANSACTION_OUTCOMES.md`, operations and limitations guides. Surface unsuccessful clean shutdown as a distinct result. Correct “Deferred means next-open-only recovery”: online replay now participates, and Deferred is not itself a visibility-status field. `docs/guide-docs/client-server-protocol-spec/TRANSACTION_OUTCOMES.md:65`. Include result-cap post-commit ambiguity and non-tx reopen behavior.

5. **P1 — CI-only performance/operability characterization before tag.**  
   Scope: existing engine/server benchmark and CI harnesses, observability instrumentation. Measure exact-vector scaling; group-commit acknowledgement p50/p99/max; prolonged lower-version stalls; global byte-budget occupancy; replay retry/log rate; estimated/encoded/RSS ratios; subscription delivery lag and slow-consumer closures. Use results to decide vector admission and subscription retry knobs—not to weaken consistency.

### Behaviors to consciously accept as-is

Accept conservative result estimates, exact snapshot vector search, contiguous snapshots, failure-closed projection settlement, typed AsOf rejection, terminal subscription gaps/backpressure, conservative cold-open certification, and propagated flush errors. Also accept **manual replication reseeding and potentially expensive snapshot reads as explicit alpha limitations**.

Do **not** accept indefinite opaque acknowledgement stalls, success-looking degraded non-tx acknowledgements, or success-looking unsuccessful clean shutdowns without the above operational contract.

**Files changed:** none. **Builds/tests run:** none, as requested. **Notable:** conclusions are a static design review; production latency and failure frequency remain unmeasured.
