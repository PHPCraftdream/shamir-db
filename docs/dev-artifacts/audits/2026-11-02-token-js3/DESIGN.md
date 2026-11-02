# TOKEN-JS3 final audit report

## Acceptance status (source-level; tests unrun)

1. PARTIAL — token option forms, channel derivation/mismatch and preflight validation are implemented in `src/core/types/connection.ts` and `src/platform/connect/{node-connect,browser-connect}.ts`; fake-entry preflight tests are authored. No runtime or compiler verification.
2. PARTIAL — public connect dispatch and HTTP/WS operations are wired. Token connect results remain an HTTP/WS client union, not the single nominal ShamirClient requested; JS0 explicitly rejects universal nominal-client branding. HTTP DB cursor/tx/runLive typed denials are authored in `src/entry/token-db.ts`, with WS overrides sharing structural types. Broader model compatibility remains unresolved.
3. PARTIAL — entries export ApiToken, TokenTransportError and public connect option types. Public redaction/entry-surface assertions are present but unrun; exported signatures and internal type leakage need strict declaration review.
4. PARTIAL — SCRAM core implementation is unchanged and public overloads retain `Promise<ShamirClient>`; test assertions for both entrypoints' original options/platform forwarding are authored and unrun.
5. PARTIAL — public fake Node/browser transport tests cover HTTP/WS connect, execution, subscriptions/push/unsubscribe, capability denial, config validation and redaction. Duplicate WS subscription test was retained per file-preservation rule and renamed; both WS subscription tests now invoke unsubscribe before asserting the outbound frame count. Strict TypeScript/Vitest remains unrun.

## Final report

PARTIAL — Token connect wiring/options/exports, HTTP capability rejections, and public-contract tests are present. Acceptance 2 remains incomplete: token connections return an HTTP/WS client union, not the single nominal ShamirClient requested. Strict TypeScript/Vitest CI is still required; no local builds/tests were run. The permitted layout check passed earlier (untracked paths counted separately). Changes and per-item gaps are recorded in `docs/dev-artifacts/audits/2026-11-02-token-js3/DESIGN.md`. No files were moved/deleted and no git mutation was performed.
