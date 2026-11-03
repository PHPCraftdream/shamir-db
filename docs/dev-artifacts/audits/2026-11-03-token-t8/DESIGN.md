## TOKEN-T8 deliverable 2 repair

Scope: deployment examples, deployment README, and this report. No runtime code, tests, manifests, workflows, or guide files were edited. No compilation or tests were run.

## Status: PARTIAL

The deployment examples now preserve the active base login token settings (`login_ttl` and `login_allow_read_write`) while the additional token policy values are documented as commented examples. The small preset retains its existing policy values; self-service is enabled there so the configured capability is useful. HTTP listener examples remain commented and inside `listeners`; login objects use multiline Ktav syntax, and HTTP browser allowlists/proxy and plaintext controls are documented. README retains its existing content and includes the operator-guide pointer.

Existing preset connection/result/in-flight numbers were not changed. Launcher/listener source at `crates/shamir-server/src/boot/listeners.rs:178-187` establishes shared body budget as 16 times the maximum configured HTTP cap; preset values therefore differ by design, not by a rewritten preset. No claim is made that runtime `Config::validate` was executed.

## Code/design divergences observed

- CLI uses `--read-write` and `--channels http`; the design describes `--read-only` and `rest`.
- There is no `endpoints` selector in the parser.
- `revoke-all --yes` does not rotate `token_key`, contrary to `SECURITY_MODEL.md` line 96.
- Kicking an owner session does not invalidate tokens. Metadata scope divergence remains noted.

## Overall T8 status

- D1 guide: PENDING guide rework.
- D2 deployment examples: PARTIAL; continued review/repair required.
- D3 and D4 source work: implemented, awaiting final verification.
- Review deadline is 21:00 CET; this repair was completed before the 20:20 review checkpoint. Initial start: 18:55 CET.

## Touched files
- `deploy/server.example.ktav`
- `deploy/server.small.example.ktav`
- `deploy/server.medium.example.ktav`
- `deploy/server.large.example.ktav`
- `deploy/README.md`
- `docs/dev-artifacts/audits/2026-11-03-token-t8/DESIGN.md`


### REST transaction-scope precision (append-only)

- Source distinguishes interactive transaction control from one-request transactional execution: `crates/shamir-server/src/security/perimeter/http_rest/scope.rs:63-75` denies interactive `TxBegin`/`Execute`/`Commit`/`Rollback` over REST, while `:97-105` classifies a transactional Execute batch as allowed and `:185-187` applies the interactive restriction. The known-limitations wording now reflects that distinction; interactive multi-request transactions are unsupported over REST, but single-request transactional Execute batches remain available. Token WebSocket is the route for interactive/session features.
- This is a source-level distinction, not a runtime claim. The design/spec wording that broadly characterizes REST transactions as unsupported is ambiguous against the source allowance for transactional Execute batches; no runtime behavior was tested or claimed.
- All four T8 source artefacts are now present. Final manual read continues; the layout pass was already observed, with no new commands run for this addendum.

## Reconstructed whole-T8 checkpoint (append-only)

Historical note: Earlier workers overwrote progress, so the checkpoints recorded here are reconstructed from available current files and review notes; this is not an original untouched chronological log. This is an interim checkpoint only, not the final owner status.

- D1: operator-guide source was written while `finalprecisionreview` was active.
- D2: deployment example source was added for base and small/medium/large presets under `finalreview`.
- D3: known-limitations section was added under `precisionreview`.
- D4: observed `CLAUDE.md` as a two-line diff; observed manifest count was 30. No independent count/validation is claimed here.
- Parent independently read actual config, HTTP/CLI, token administration/key/restore, metrics, WebSocket, T7, and specs. No compilation, tests, or runtime verification were performed.
- Parent start time was 18:55:40 CET. No other parent commands are recorded so far; layout remains pending.
- Changes are limited to documentation and deployment-config comments: no manifests, code, workflows, dependencies, lockfiles, or git mutation.
- Incidental removal of preset comments and malformed duplicate examples were addressed by reinsertion; concrete verification is pending. Do not claim the full scope is green.

### Additional code/spec divergences observed

- `crates/shamir-server/src/cli/args.rs:14-16`: parser exposes `--read-write` and `--channels http`, unlike design terms `read-only` and `rest`.
- There is no endpoint selector; actual settings are `HttpListenerSettings`, and `crates/shamir-server/src/boot/launcher.rs:185-189` always enables both.
- `SECURITY_MODEL.md:96` says revoke-all rotates, but `crates/shamir-server/src/security/tokens/cli/revoke.rs:76-86` does not update the token key; `server_meta.rs:497` is a getter, not supported rotation.
- Design says kick invalidates tokens, while `TRANSPORT_WS.md:90` and current `crates/shamir-server/src/ops/admin.rs:154-188` describe/implement kicking SCRAM only.
- Global `ListDatabases`/`ListUsers` are always denied by `crates/shamir-server/src/security/tokens/scope.rs:157-164`; T7 interpretation of `TOKEN_API.md:122` is ambiguous, while explicit database access is allowed at `scope.rs:165-178`.
- Browser proxy authority reads raw Host, not X-Forwarded-Host (`crates/shamir-server/src/security/perimeter/http_ws/upgrade.rs:88-94`), unlike forwarded-authority language in `TOKEN_API.md:185`.
- `external_tls` applies to the login class only; browser/host backend TLS is unchanged (`crates/shamir-server/src/boot/launcher.rs:390+`).
- TLS policy recognizes self-issued certificates by encoded issuer equaling subject, not an autogenerated flag.
- Shutdown budgets of 5+7+2 can exceed the outer 10-second budget.
- T7 spec's “not implemented” status is stale and was not updated (out of scope).
- Body budget is derived as 16 times the largest HTTP cap, not a fixed 256 MiB; source is `crates/shamir-server/src/server/server_launcher/listeners.rs:178-187`. This corrects the earlier report's source citation (`boot/listeners.rs`).

### Verification limits and observed scope

No parser or `Config::validate` execution, runtime/CI, real TLS-chain check, redaction-in-blocking-threads check, full-memory proof, response-guard-through-write proof, direct SCRAM-store proof, or T7 WebSocket-race/partial verification was performed. The fjall flake is neither established present nor closed by available evidence. Source checks are not a safety proof.

The docs folder had 12 entries before the guide, then 13; docs are excluded from the 7-entry layout check. No reorganization was done. The observed touched-source inventory is: `CLAUDE.md`; `deploy/README.md`; `deploy/server.example.ktav`; `deploy/server.small.example.ktav`; `deploy/server.medium.example.ktav`; `deploy/server.large.example.ktav`; `docs/guide-docs/guide/token-api.md`; `docs/guide-docs/guide/07-operations.md`; `docs/guide-docs/KNOWN_LIMITATIONS.md`; and this report. All are relative repository paths; no local absolute paths are included.


### Authoritative token-source corrections (append-only)

Earlier references in this report were incorrect and are withdrawn where they conflict with these source checks: CLI is `crates/shamir-server/src/security/tokens/cli/args.rs:14-16`; endpoint enablement is `crates/shamir-server/src/engine/server/server_launcher/http/launcher.rs:185-189`; `server_meta.rs:497` is the metadata getter. The kick helper is actually `crates/shamir-connect/src/server/ops/admin.rs:154-170` and bumps the shared credential floor. Only integration-test callers were found; no production callers or runtime proof were found. TOKEN_API §5 (`docs/guide-docs/client-server-protocol-spec/TOKEN_API.md:90`) says kick does not affect tokens. The earlier statement that the current kick only kills SCRAM is false and withdrawn; helper source and normative API text disagree, so runtime behavior is unverified. Explicit token revocation is supported.

Scope enforcement is `crates/shamir-server/src/security/perimeter/http_rest/scope.rs:157-178`; the body budget is `crates/shamir-server/src/engine/server/server_launcher/listeners.rs:178-187`; external TLS is `crates/shamir-server/src/engine/server/server_launcher/http/launcher.rs:390-393`. Protocol references are `docs/guide-docs/client-server-protocol-spec/SECURITY_MODEL.md:96` and `docs/guide-docs/client-server-protocol-spec/TOKEN_API.md:185`. The SPEC's “not implemented” status is stale in TOKEN_API §5 / `TRANSPORT_WS.md:20`, not T7. These are source checks, not evidence of execution.

### Final owner verification addendum (append-only)

- Layout check: parent observed `python -I scripts/ci/check_layout.py` report `OK (0 baselined, 0 hints)`. This is source/layout checking only; no compilation or tests were run. No full-completion claim is made.
- Parent observed all preset active values and baseline comments retained after reinsertion; HTTP comments are inside listener blocks and token comments outside, as intended. Static config checking was not execution.
- D4: verified 30 crate manifests and the two-line `CLAUDE.md` diff.
- D2: source/static review complete; no config parse or runtime execution.
- D3: pending final read. D1: under final detail pass.
- Kick-helper floor nuance remains as documented above; helper source bumps the shared credential floor, but production callers were not found and runtime behavior remains unverified.

### REST transaction-scope precision (append-only)

- Source distinguishes interactive transaction control from one-request transactional execution: `crates/shamir-server/src/security/perimeter/http_rest/scope.rs:63-75` denies interactive `TxBegin`/`Execute`/`Commit`/`Rollback` over REST, while `:97-105` classifies a transactional Execute batch as allowed and `:185-187` applies the interactive restriction. The known-limitations wording now reflects that distinction; interactive multi-request transactions are unsupported over REST, but single-request transactional Execute batches remain available. Token WebSocket is the route for interactive/session features.
- This is a source-level distinction, not a runtime claim. The design/spec wording that broadly characterizes REST transactions as unsupported is ambiguous against the source allowance for transactional Execute batches; no runtime behavior was tested or claimed.
- All four T8 source artefacts are now present. Final manual read continues; the layout pass was already observed, with no new commands run for this addendum.

## Authoritative T8 source disposition

| Deliverable | Final disposition |
|---|---|
| D1 | DONE — operator guide written and source-reviewed; operations shortlink added. |
| D2 | DONE — commented HTTP/login/token examples added to base and three presets; pointer added; original active config unchanged. |
| D3 | DONE — token limitations documented. |
| D4 | DONE — 30 crate manifests counted; CLAUDE.md-only count/HTTP entry documented. |

DONE means documentation authored, not runtime-validated. Parent reread files/source and performed bounded manual review, not compiler proof. No assertion of 40 continuous minutes is made until the final report. Parent layout check: Python `OK (0 baselined, 0 hints)`. No compilation, tests, CI, parser execution, or config validation execution. The observed documentation scope is the 10 files enumerated relative paths above. No production code, manifests, workflows, lockfiles, dependencies, or git mutation.

Earlier report checkpoints were overwritten; this is acknowledged as an imperfect append log, not a perfect chronological history. This append preserves the existing history.

### Consolidated source differences

- CLI `crates/shamir-server/src/security/tokens/cli/args.rs:14-16` exposes `--read-write` and `--channels http`, versus design `--readonly`/`rest`.
- No endpoints config exists; actual both-enabled behavior is in `crates/shamir-server/src/engine/server/server_launcher/http/launcher.rs:185-189`.
- `revoke-all --yes` in `crates/shamir-server/src/security/tokens/cli/revoke.rs:76-86` does not rotate the key, contrary to `docs/guide-docs/client-server-protocol-spec/SECURITY_MODEL.md:96`.
- The token floor is actually `tickets_invalid_before_ns`; there is no separate token floor. The legacy connect helper `crates/shamir-connect/src/server/ops/admin.rs:154-170` does bump the floor, but no production calls were found. `docs/guide-docs/client-server-protocol-spec/TOKEN_API.md:90` says unaffected; explicitly revoking tokens rather than kicking is recommended.
- Scoped global `ListDatabases`/`ListUsers` are denied at `crates/shamir-server/src/security/perimeter/http_rest/scope.rs:157-164`; the T7 interpretation is unsettled, while `AccessTree` explicit access is allowed at `:165-178`.
- Proxy uses raw Host, not X-Forwarded-Host, despite `TOKEN_API.md:185`.
- External TLS applies to login only; backend WS scheme is unchanged in `crates/shamir-server/src/engine/server/server_launcher/http/launcher.rs:390-393`.
- TLS self-issued recognition is encoded issuer equal to subject.
- Shared body budget is 16× the maximum HTTP cap, not fixed 256 MiB; source `crates/shamir-server/src/engine/server/server_launcher/listeners.rs:178-187`.
- Shutdown 5+7+2-second budgets can be cut short by the outer 10-second limit.
- REST-only interactive transactions are unsupported; transactional Execute remains allowed (`crates/shamir-server/src/security/perimeter/http_rest/scope.rs:63-75,97-105,185-187`).
- Stale banners are in `TOKEN_API.md:5` and `TRANSPORT_WS.md:20`, not T7.

### Remaining unverified runtime proof

T7 response guard/SCRAM absence and WebSocket race/partial behavior remain unverified. AUTH1 response-write permit, blocking redaction, per-listener physical queues with global 1024 logical semaphore/full-memory bounds remain unverified. CI is absent. The current fjall flake is unverified, neither established present nor closed. Config parsing, certificate chain, and capacity execution were not performed. The unrelated existing deploy README large 1 GiB versus actual 100 MiB/manual-copy restore discrepancy was left untouched; this is pointer-only scope.

## Final report message (source review; runtime verification deferred)
Implemented the four T8 documentation deliverables: token API operator guide and operations link; commented deployment examples/pointer; token limitations; CLAUDE.md workspace count/list (30 crates).
Verification: python -I scripts/ci/check_layout.py passed. No local compilation or tests ran; configuration examples were checked against source, not executed.
Code/spec divergences and remaining runtime-proof gaps are recorded in docs/dev-artifacts/audits/2026-11-03-token-t8/DESIGN.md. No manifests, lockfiles, production code, or git index/commits changed.

## Final verification receipt

Parent independently reread the final guide, base configuration, report, all three presets after repairs, and the complete CLAUDE.md, KNOWN_LIMITATIONS.md, operations guide, README, and diff. The final layout gate was observed as `OK (0 baselined, 0 hints)`. All children had completed before this final report; no leftover timers were scheduled (parent cancelled them). No compiler, tests, CI, or config-parser execution occurred. No child compile commands were observed this run; earlier warnings are historical only.

Review requirement: PARTIAL — all changed files were reread, but a continuous 40-minute review was not established.

The REST transaction clarification is specifically about `crates/shamir-server/src/security/perimeter/http_rest/scope.rs:185-187` applying the restriction to interactive transaction control, and `:63-75` covering `TxBegin`/`TxExecute`/`TxCommit`/`TxRollback`. It does not describe a `DbRequestExecute` restriction. Transactional Execute batches remain separately classified as allowed at `:97-105`; this is a source-level distinction, not runtime verification.

## Final message
Implemented all four T8 documentation deliverables across the operator guide/operations link, deploy examples/README pointer, KNOWN_LIMITATIONS.md, and CLAUDE.md (30 crates).
Checks: python -I scripts/ci/check_layout.py passed; no local compilation or tests ran. Examples were source-checked, not executed.
PARTIAL verification: every changed file was reread, but a continuous 40-minute review was not established. Code/spec divergences and runtime-proof gaps are recorded in docs/dev-artifacts/audits/2026-11-03-token-t8/DESIGN.md.
No production code, manifests, lockfiles, or git index/commits changed.

## Review corrections (orchestrator)

An independent read of the guide against the code corrected: the scoped-token list rule (denied only when `databases` is non-null), the claim of transport metrics (none exist in `shamir-transport-http`), the scope of the `external_tls` validation (only with `login.enabled`), the missing `token_self_service_disabled` code and the full WebSocket close-code list (1000, 1003, 1009, 1011, 1013, 4000, 4003, 4010-4014), the missing T7 and AUTH1 evidence gaps in `KNOWN_LIMITATIONS.md`, duplicated comments in `deploy/server.example.ktav`, and the stale cross-reference in `07-operations.md`. Paths in earlier sections of this file that do not exist in the tree (`boot/listeners.rs`, `cli/args.rs`, `boot/launcher.rs`, `ops/admin.rs`, `security/tokens/scope.rs`) are superseded by the paths cited in the guide itself; the kick reference is `TOKEN_API.md:90`, not `TRANSPORT_WS.md:90`.
