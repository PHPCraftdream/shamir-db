# TOKEN-JS3 final audit report

## Acceptance status (source-level; tests unrun)

1. PARTIAL — token option forms, channel derivation/mismatch and preflight validation are implemented in `src/core/types/connection.ts` and `src/platform/connect/{node-connect,browser-connect}.ts`; fake-entry preflight tests are authored. No runtime or compiler verification.
2. PARTIAL — public connect dispatch and HTTP/WS operations are wired. Token connect results remain an HTTP/WS client union, not the single nominal ShamirClient requested; JS0 explicitly rejects universal nominal-client branding. HTTP DB cursor/tx/runLive typed denials are authored in `src/entry/token-db.ts`, with WS overrides sharing structural types. Broader model compatibility remains unresolved.
3. PARTIAL — entries export ApiToken, TokenTransportError and public connect option types. Public redaction/entry-surface assertions are present but unrun; exported signatures and internal type leakage need strict declaration review.
4. PARTIAL — SCRAM core implementation is unchanged and public overloads retain `Promise<ShamirClient>`; test assertions for both entrypoints' original options/platform forwarding are authored and unrun.
5. PARTIAL — public fake Node/browser transport tests cover HTTP/WS connect, execution, subscriptions/push/unsubscribe, capability denial, config validation and redaction. Duplicate WS subscription test was retained per file-preservation rule and renamed; both WS subscription tests now invoke unsubscribe before asserting the outbound frame count. Strict TypeScript/Vitest remains unrun.

## Final report

PARTIAL — Token connect wiring/options/exports, HTTP capability rejections, and public-contract tests are present. Acceptance 2 remains incomplete: token connections return an HTTP/WS client union, not the single nominal ShamirClient requested. Strict TypeScript/Vitest CI is still required; no local builds/tests were run. The permitted layout check passed earlier (untracked paths counted separately). Changes and per-item gaps are recorded in `docs/dev-artifacts/audits/2026-11-02-token-js3/DESIGN.md`. No files were moved/deleted and no git mutation was performed.

## Revision 1 progress

- Item 1 DONE source-level: `crates/shamir-client-ts/src/entry/token-db.ts` declares the async iterator protocol; regression `returns a stable async iterable iterator` in `src/entry/ws/__tests__/token-ws-client-regression.test.ts` is relevant. Tests unrun.
- Item 2 PARTIAL: SCRAM `ConnectOptions` was restored as an interface and platform token unions renamed, but imports/type references require correction and full strict review.
- Item 3 PARTIAL: browser overload work is incomplete and has unresolved type references.
- Item 4 PARTIAL: a public WS interface file was added, but the implementation currently refers to the wrong import/export symbols; not complete.
- Item 5 PARTIAL: per-channel overloads started; HTTP overload and ordering/type review remain.
- Item 6 PARTIAL: token type exports started, but browser exports and target declarations are incomplete.
- Item 7 PARTIAL: types barrel narrowed; imports/entry type* effects require broad review.
- Item 8 PARTIAL: Node plaintext rejection narrowed to remote ws; corresponding URL test not updated and browser WS validation still needs review.
- Item 9 PARTIAL: browser connector now accepts browser token options; compile-safety review pending.
- Item 10 PARTIAL: tests not fully updated (error codes, TLS/plaintext/timeout/channel cases, source graph isolation assertions remain); no test runs.
- Item 11 PARTIAL: platform re-export removals and direct entry imports started; spies and platform factory usages require adapting.

## Revision 1 report

Items 1–11 are PARTIAL overall except item 1, which is source-level DONE. Several partial edits are not yet type-safe; in particular public WS type layering and entry overload/imports need immediate repair before CI review. Added/retained relevant tests: `returns a stable async iterable iterator`, `preserves SCRAM overload result and passes original options and platform through`, public connect fake platform tests, and entry isolation/redaction assertions. All are UNRUN. No builds, compilers, or tests were run. CI must run strict TypeScript declaration/type checking and Vitest; verify browser-vs-Node graph exclusion, all overload call sites, platform spies, and layout limits. No compile/runtime claims are made.

