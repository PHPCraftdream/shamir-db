# P34-11 - shamir-sdk (+macros) + shamir-wasm-host: ABI limit, error categories, oracles, docs

Source: P3/P4 revalidation of the active findings against master `07bc804c` plus the P2 carry-overs (`docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-SPRINT-REPORT-2026-10-30.md`). Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P3.md` and `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P4.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), the verified tables in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P3-REVALIDATION-2026-10-30.md` / `P4-REVALIDATION-2026-10-30.md`, and the board `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P34-FIX-TASKS.md`.

## Goal

Classify unexpected DB response shapes and codec causes consistently in the SDK, make HTTP response decoding fail closed on duplicate fields, deprecate the lossy catalogue decoder in favour of the checked one, cover host-import trap paths, and fix the remaining documentation. The signed-i32 ABI limit stays an accepted, documented limitation.

This is one change set for shamir-sdk, shamir-sdk-macros, shamir-wasm-host: 9 active findings, effort units 18 (S=1, M=2, L=4). They are adjacent (same crates, overlapping files), so they are solved together and share tests and review. Decisions marked IMPLEMENT were taken by the design decider (@oh) on 2026-10-30 and are binding; SKIP means the card is accepted as it is; ESCALATE waits for the owner.

## Scope

Crates: `shamir-sdk`, `shamir-sdk-macros`, `shamir-wasm-host`. Dependents are type-checked with `cargo check` when a public signature changes.

Files outside these crates that the evidence names (edit only these, and only as the cards require):

- `crates/shamir-db/src/shamir_db/shamir_db/catalog/artifact_kind.rs` (4669FDA6, FF7C2D6E)
- `crates/shamir-db/tests/functions/lifecycle/http_egress.rs` (L3-RV-SDK-B5ACD67D-FACET)

## Acceptance

- Unexpected DB response shapes classify as Codec and codec sources are preserved (6D2497CA-FACET).
- A duplicate top-level status/headers/body field in an HTTP response map is a codec error, not first-match (B5ACD67D-FACET).
- FunctionMeta::from_record is #[deprecated] with a doc that states the lossy fallbacks; consumers use try_from_record (4669FDA6).
- Host-import trap/error paths, including malformed or non-map nested-call Params, have in-crate tests (91385BE0); remaining cards change comments only.

## Coordination with other tasks

Wave 3; starts after P34-06 are merged. 4669FDA6 migrates its consumer in shamir-db catalog/artifact_kind.rs and B5ACD67D edits shamir-db function tests; both are P34-06 territory.
- 4669FDA6 also migrates the consumer in shamir-db catalog/artifact_kind.rs; B5ACD67D edits shamir-db/tests/functions/lifecycle/http_egress.rs.

## Decisions

Recorded by the orchestrator from the design decider (@oh) on 2026-10-30. Cards: RV-SDK-723F5FD2-SEMANTIC-FACET, RV-SDK-B5ACD67D-FACET, RV-WASM-HOST-4669FDA6. The full decision paragraph sits in the finding below.

- L3-RV-SDK-723F5FD2-SEMANTIC-FACET: **SKIP** (no change).
- L3-RV-SDK-B5ACD67D-FACET: **IMPLEMENT** (change as decided).
- L3-RV-WASM-HOST-4669FDA6: **IMPLEMENT** (change as decided).

## Findings (ordered by risk, then size)

### L3-RV-SDK-723F5FD2-SEMANTIC-FACET - Guest ABI passes pointers as signed `i32`; host rejects addresses ≥ 2 GiB

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `L/decision`; **OWNER DECISION**.
- **Decision: SKIP** (@oh design decider, 2026-10-30; compat `n/a`, risk `low`). Accepted, documented limitation, continuing the P2-17 decision, which has landed. Pointer and length fields are nonnegative signed i32, exchange buffers are range-checked against 2 GiB (crates/shamir-sdk/src/runtime/__rt.rs:37-44), and the host rejects negative fields (wasm_function.rs:364-371). The default pool ceiling is 64 MiB, far below 2 GiB. An unsigned ABI would break the guest ABI for all compiled modules with no current benefit. If guest memory above 2 GiB is ever authorized, the ABI migration becomes an owner decision then.
- Evidence at `07bc804c`: crates/shamir-sdk/src/runtime/__rt.rs:40; crates/shamir-wasm-host/src/wasm/wasm_function.rs:369
- Recipe: Accept the documented lower-2-GiB limitation explicitly or authorize a coordinated unsigned-address ABI migration with boundary fixtures.
- Verifier note: Signed exchange fields still exclude high-bit addresses; owner acceptance or coordinated ABI migration remains required.

### L3-RV-SDK-6D2497CA-FACET - SDK protocol-error diagnostic category is flattened

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/code`.
- Evidence at `07bc804c`: crates/shamir-sdk/src/api/db.rs:100-122; crates/shamir-sdk/src/runtime/__rt.rs:233-234; crates/shamir-sdk/src/error.rs:16-19
- Recipe: Classify unexpected DB response shapes as Codec and preserve structured codec sources consistently.
- Verifier note: Categories exist, but unexpected DB shapes remain User errors and codec causes are flattened into strings.

### L3-RV-SDK-B5ACD67D-FACET - Malformed HTTP-envelope and success-decoder oracle gaps

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/test`; **OWNER DECISION**.
- **Decision: IMPLEMENT** (@oh design decider, 2026-10-30; compat `compatible`, risk `low`). Duplicate top-level fields fail closed: HttpResponse::from_value (crates/shamir-sdk/src/api/http.rs:235-283) returns a codec error when status, headers or body appears more than once in the response map, instead of taking the first match. Duplicate header names inside the headers pair-list (Set-Cookie) stay allowed, and the legacy map-form headers are kept. Add local SDK unit fixtures (under crates/shamir-sdk/src/tests/, e.g. http_tests.rs): non-map response; missing status; out-of-range status; non-int status; malformed header pairs or values; a non-Bin body; each duplicated top-level field. Each must return Err; a valid envelope must decode the exact status, headers and body. In crates/shamir-db/tests/functions/lifecycle/http_egress.rs, wasm_function_http_fetch_allowed (:117-138) must require exactly "hello from mock server 8c", with no err: tolerance. Make start_mock_http_server (crates/shamir-db/tests/functions/lifecycle/support.rs:127-153) read until the end of the request headers (CRLFCRLF) before replying, so the strict test is deterministic. Keep the existing toolchain and curl skip gates. Compatible: the host never emits duplicate keys (bindings/http.rs:103 builds the map with insert). Out of scope: HttpRequest encoding, gateway policy, and the other egress tests' assertions.
- Evidence at `07bc804c`: crates/shamir-db/tests/functions/lifecycle/http_egress.rs:118-138; crates/shamir-sdk/src/api/http.rs:241-275
- Recipe: Require exact successful HTTP body decoding and add duplicate top-level response-field policy fixtures.
- Verifier note: Require exact successful body decoding and add duplicate top-level response-field fixtures; choosing duplicate semantics needs an owner decision.

### L3-RV-SDK-MACROS-853FC350-FACET - Lexical return-name diagnostics versus concrete SDK typing

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `07bc804c`: crates/shamir-sdk-macros/src/lib.rs:294; crates/shamir-sdk-macros/src/lib.rs:557; crates/shamir-sdk-macros/src/lib.rs:564
- Recipe: Use concrete wrapper typing for return compatibility and add alias/foreign-type consumer fixtures.
- Verifier note: Lexical return-name gating still rejects compatible differently named aliases; concrete SDK typing already prevents foreign-type confusion.

### L3-RV-WASM-HOST-4669FDA6 - Catalogue record decoding has silent fallbacks and no format versioning

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/code`; **OWNER DECISION**.
- **Decision: IMPLEMENT** (@oh design decider, 2026-10-30; compat `compatible`, risk `low`). Mark FunctionMeta::from_record (crates/shamir-wasm-host/src/builtin/meta.rs:114-149) #[deprecated(note = "lossy legacy decoder; use FunctionMeta::try_from_record")]. Its doc must say that malformed or non-string values silently fall back to defaults. Keep its behaviour and signature unchanged. It has no callers anywhere in the workspace (both production callers already use try_from_record: catalog/artifact_kind.rs:30, access/authorize.rs:359), so no #[allow(deprecated)] is needed. Fix the stale wording in crates/shamir-db/src/shamir_db/tests/access/enforcement_tests.rs:570 so it names try_from_record. Removing the function is deferred to the next approved API-break window. No new behavioural test: the existing tests in crates/shamir-wasm-host/src/tests/runtime/registry_tests.rs:25-61 already cover legacy defaults, versioned round trips and rejection. Out of scope: the persisted metadata_version format, ResourceMeta::from_record in shamir-db, and the setuid/effective-actor logic.
- Evidence at `07bc804c`: crates/shamir-wasm-host/src/builtin/meta.rs:115-143,156-208; crates/shamir-db/src/shamir_db/shamir_db/catalog/artifact_kind.rs:26-37
- Recipe: Deprecate the lossy public decoder, migrate consumers to try_from_record, then remove it through an approved API transition.
- Verifier note: Production decoding is checked and versioned, but the public lossy decoder remains; staged API deprecation is appropriate.

### L3-RV-WASM-HOST-91385BE0 - No in-crate test coverage for any host-import trap/error path (db/http/batch/global imports, depth limit, OOB result pointers, missing exports)

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/-`.
- Evidence at `07bc804c`: crates/shamir-wasm-host/src/wasm/bindings/call.rs:23-27,135-136; crates/shamir-wasm-host/src/tests/wasm/wasm_tests.rs:436-447,479-598
- Verifier note: First pass missed the explicitly scoped Params rejection obligation: added tests cover other categories but not malformed/non-map nested-call Params.

### L4-RV-WASM-HOST-C43E9092 - Internal audit-tracking references leaked into public API docs

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `M/doc`.
- Evidence at `07bc804c`: crates/shamir-wasm-host/src/gateway/net_gateway.rs:23-25; crates/shamir-wasm-host/src/gateway/db_gateway.rs:37
- Recipe: Remove internal audit identifiers from both public gateway-error descriptions while retaining substantive explanations.
- Verifier note: Old finding-2c references are removed, but internal audit IDs remain in two public error descriptions.

### L4-RV-SDK-MACROS-FEC0E5E4-FACET - Guest driver comments retain obsolete pure-only/future-import wording

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `07bc804c`: crates/shamir-sdk/src/runtime/__rt.rs:248-252; crates/shamir-sdk-macros/src/lib.rs:244; crates/shamir-sdk-macros/src/lib.rs:381
- Recipe: Replace the obsolete pure-only claim with the shared one-poll, no-suspension contract for all wrapper kinds.
- Verifier note: The shared driver still claims pure-only support despite all wrapper kinds using its one-poll implementation.

### L4-RV-WASM-HOST-FF7C2D6E - WASM maintenance bundle: duplicate prose/glob code and fallback descriptions

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `07bc804c`: crates/shamir-wasm-host/src/builtin/meta.rs:112-143; crates/shamir-db/src/shamir_db/shamir_db/catalog/artifact_kind.rs:26-36
- Recipe: Document the retained lossy metadata adapter and direct persisted-data consumers to checked try_from_record decoding.
- Verifier note: Document the retained public adapter as lossy compatibility decoding; production recovery already uses checked decoding.

## Rules (verbatim policy for every delegated run)

- Compilation, builds and tests run on GitHub CI, not on this machine. BANNED: `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, any npm/tsc/vitest build or test run, WASM builds, e2e/integration harnesses, loom, any release or full-workspace build.
- ALLOWED locally, scoped to the touched crates (`-p <crate>`, plus their dependents when a public signature changed): `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `cargo fmt -p <crate> -- --check`, `python -I scripts/ci/check_layout.py`. One cargo process at a time, `CARGO_BUILD_JOBS=2`. Report real exit codes.
- A child that believes it needs a build or test run says so in its final message; the orchestrator decides.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Touch only the listed crates and the files this brief names. No new dependency (if one is unavoidable, stop and report). No `#[allow]`, no visibility widening beyond what the card needs. Layout limits: at most 7 direct entries per folder, at most 1000 lines per source file; new logic goes into new files. `mod.rs` files are re-exports only; imports at the top of the file; tests live in `tests/` dirs (one topic file per group); JSON literals in tests are multi-line.
- Every behavioral fix ships with a test that FAILS on the old code (red first, observed in CI). Documentation-only cards need no test. A card that turns out to be already fixed or wrong is reported, not forced.
- A card whose Decision is SKIP is not changed; a card whose Decision is ESCALATE or that carries OWNER DECISION without an answer is skipped and said so in the final report. A card whose Decision is IMPLEMENT is implemented exactly as the decision paragraph says; deviations need a written reason.
- Cards whose recipe or evidence you cannot reproduce from the code at the stated location are reported with your own evidence (file:line), not silently changed.

## Local gates (only these)

`cargo fmt -p shamir-sdk -p shamir-sdk-macros -p shamir-wasm-host -- --check`; `cargo clippy -p shamir-sdk -p shamir-sdk-macros -p shamir-wasm-host --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(decision) / SKIPPED(owner) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
