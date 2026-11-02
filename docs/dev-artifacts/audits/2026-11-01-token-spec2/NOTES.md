# TOKEN-SPEC2 implementation notes

Observed task date/time: 2026-11-01 20:18:53 CET. The entries below preserve the original pre-edit history; later final-state findings are in the authoritative final table and report.

## Initial amendment table (pre-edit; historical)

| Source / # | Target | Quoted anchor (short) | Action | Initial status |
|---|---|---|---|---|
| JS0 #1 | TOKEN_API §1 | WebSocket row ending `/shamir/v1/ws` | Replace row | pending |
| JS0 #2 | TOKEN_API §1 | Listener security-boundary paragraph | Replace paragraph | pending |
| JS0 #3 | TOKEN_API §4 | `## 4. Представление токена` | Replace 3 items | pending |
| JS0 #4 | TOKEN_API §5 | Header verification lead-in | Replace lead-in, retain steps 1–8 | pending |
| JS0 #5 | TOKEN_API §10 | WebSocket heading | Replace body to §11 | pending |
| JS0 #6 | TOKEN_API §13 | Audit heading | Append browser redaction | pending |
| JS0 #7 | TOKEN_API §14 | Old browser-client exclusion paragraph | Replace paragraph | pending |
| JS0 #8 | TOKEN_API §15 | Test Checklist heading | Append real-socket checklist | pending |
| JS0 #9 | TRANSPORT_WS opening | `Только wss://...` | Replace scope paragraph | pending |
| JS0 #10 | TRANSPORT_WS §1 | Path distinction paragraph | Append bearer/SCRAM distinction | pending |
| JS0 #11 | TRANSPORT_WS §6 | 4010–4012 close-code paragraph | Append bearer close codes | pending |
| JS0 #12 | TRANSPORT_WS §8 | Pre-auth/data-frame limit sentence | Replace/extend | pending |
| JS0 #13 | TRANSPORT_WS §9 | Origin policy | Append bearer distinction | pending |
| JS0 #14 | SECURITY_MODEL §4 | End of Non-Guarantees | Append browser bearer threat | pending |
| JS0 #15 | SECURITY_MODEL §5 | RFC 6750 table row | Replace row | pending |
| JS0 #16 | CLIENT_BROWSER §6 | Connection Code heading | Append bearer API contract | pending |
| JS0 #17 | CLIENT_BROWSER §9 | Test Checklist heading | Append browser tests | pending |
| AUTH0 AUTH_PROTOCOL §5.1.1 bounds | AUTH_PROTOCOL §5.1.1 | Client KDF validation block | Replace with lower/upper bounds | pending |
| AUTH0 AUTH_PROTOCOL §5.1.1 rejection | AUTH_PROTOCOL §5.1.1 | Rejection sentence | Replace | pending |
| AUTH0 AUTH_PROTOCOL §5.2.5 | AUTH_PROTOCOL §5.2.5 | Pair backoff paragraph | Append bounded pair-only store | pending |
| AUTH0 AUTH_PROTOCOL §8.4 | AUTH_PROTOCOL §8.4 | Lockout-silent paragraph | Append HTTP-only exception | pending |
| AUTH0 AUTH_PROTOCOL §8 limits | AUTH_PROTOCOL §8 | LOCKOUT_THRESHOLD / AUTO_RESET rows | Replace both rows | pending |
| AUTH0 AUTH_PROTOCOL §14.4 | AUTH_PROTOCOL §14.4 | Non-disclosure statement | Append HTTP-only exception | pending |
| AUTH0 IMPLEMENTATION_GUIDE §1.3 cap | IMPLEMENTATION_GUIDE §1.3 | 1M entry cap | Replace cap bullet | pending |
| AUTH0 IMPLEMENTATION_GUIDE §1.3 snapshot | IMPLEMENTATION_GUIDE §1.3 | SystemStore persistence lead-in | Insert bounded snapshot note | pending |
| AUTH0 TOKEN_API §1 | TOKEN_API §1 | Listener-boundary paragraph | Replace; reconcile JS0 #2 | pending |
| AUTH0 TOKEN_API §3 | TOKEN_API §3 label row | Label uniqueness | Replace active-only definition | pending |
| AUTH0 TOKEN_API §4 | TOKEN_API §4 item 3 | Malformed/header-only item | Append login exception | pending |
| AUTH0 TOKEN_API §5 | TOKEN_API §5 lead-in | Header verification | Replace; preserve JS0 browser sequence | pending |
| AUTH0 TOKEN_API §9 | TOKEN_API §9 Expect sentence | `Expect: 100-continue` | Replace with bearer/login order | pending |
| AUTH0 TOKEN_API §12 opening | TOKEN_API §12 | SCRAM DbRequest paragraph | Add separate non-DbRequest exception | pending |
| AUTH0 TOKEN_API §12 rule 1 browser | TOKEN_API §12 rule 1 | `binding_mode = 0x02` restriction | Append login distinction | pending |
| AUTH0 TOKEN_API §12 rule 1 limits | TOKEN_API §12 rule 1 | `tokens.max_per_user` sentence | Replace shared active controls | pending |
| AUTH0 TOKEN_API §12 rule 3 | TOKEN_API §12 rule 3 | Secret-delivery sentence | Replace with protected HTTP option | pending |
| AUTH0 TOKEN_API §11 | TOKEN_API §11 plaintext bullet | Bearer plaintext paragraph | Append login-only remote restriction | pending |
| AUTH0 SECURITY_MODEL §2 | SECURITY_MODEL §2 | Final threat row | Append two login threats | pending |
| AUTH0 SECURITY_MODEL §4.15 | SECURITY_MODEL §4.15 | Browser-session final sentence | Append login risks | pending |
| AUTH0 ADMIN_UI_HOSTING §4.2 | ADMIN_UI_HOSTING §4.2 | `[admin_ui]` config block | Append reverse-proxy guidance | pending |
| AUTH0 TRANSPORT_WS §1 | TRANSPORT_WS §1 | Path distinction paragraph | Append HTTP-login proposal scope | pending |
| AUTH0 README | README core index | TOKEN_API bullet | Insert TOKEN_LOGIN_HTTP immediately after | pending |

## Historical progress and checkpoints (pre-final)

- Initial read covered the brief, both amendment sections, and the eight principal targets.
- TOKEN_API was recorded complete after JS0 #1–8 and AUTH0 changes, with JS0 #2/AUTH0 §1 and JS0 #4/AUTH0 §5 reconciled; §9 bearer/login `100-continue` order recorded.
- TRANSPORT_WS was recorded complete for JS0 #9–13 and AUTH0 §1; an earlier checkpoint noted the opening scope needed attention. Later final checkpoint restored JS0 #10 paragraph; final evidence appears below.
- SECURITY_MODEL was recorded complete for JS0 #14–15 and AUTH0 threat rows/§4.15; an earlier misplaced duplicate was removed. Later final row distinction is recorded below.
- CLIENT_BROWSER #16–17; AUTH_PROTOCOL lower KDF bounds, pair-only LockoutStore, §8/§14 exceptions and limits; IMPLEMENTATION_GUIDE cap/snapshot; ADMIN_UI_HOSTING proxy guidance; README index entry were recorded complete by earlier workers.
- TOKEN_LOGIN_HTTP source cleanup was recorded complete: remains specification of an unimplemented feature, proposal payload removed, binding decisions retained.
- TOKEN-JS0 source cleanup was recorded complete: proposal payload replaced by applied-status paragraph naming four normative targets; JS1–JS5 design retained.
- Earlier AUTH_PROTOCOL §5.2.5 review found source OLD anchor included an exponent definition absent from target; later final reconciliation specifies count-1 from failure 1. A provisional note about an added §15.2 scope heading was erroneous; final read shows only original §15.2 auth_message clause, with no extra numbered heading.
- Earlier provisional items marked pending or “must complete” are historical, not current status. The final tables below are authoritative.

## Final amendment table

Status labels APPLIED / RECONCILED indicate that the accepted payload’s intent is present in the normative text; the final wording may be reconciled to overlapping amendments and is not necessarily a character-for-character payload copy.

### JS0 (source: `docs/dev-artifacts/design/token-client-js.md`, proposed amendments 1–17)

| Source / # | Target § | Quoted anchor | Action | Final status / evidence |
|---|---|---|---|---|
| JS0 #1 | TOKEN_API §1 | WebSocket row with `/shamir/v1/ws` | Replace row; WSS inherits | APPLIED — TOKEN_API.md:16–18 |
| JS0 #2 | TOKEN_API §1 | listener boundary paragraph | Replace boundary; reconcile login exception | RECONCILED with AUTH0 §1 — TOKEN_API.md:22 |
| JS0 #3 | TOKEN_API §4 | `## 4. Представление токена` | Replace numbered representation items | RECONCILED with AUTH0 §4 — TOKEN_API.md:64–67 |
| JS0 #4 | TOKEN_API §5 | verification lead-in before steps 1–8 | Replace lead-in; preserve steps | RECONCILED with AUTH0 §5 — TOKEN_API.md:71–86 |
| JS0 #5 | TOKEN_API §10 | `## 10. WebSocket (каналы 2 и 4)` | Replace section body through §11 | APPLIED — TOKEN_API.md:179–210 |
| JS0 #6 | TOKEN_API §13 | audit section | Append browser frame redaction text | APPLIED — TOKEN_API.md:246–252 |
| JS0 #7 | TOKEN_API §14 | old browser WS exclusion paragraph | Replace paragraph | APPLIED — TOKEN_API.md:254–256 |
| JS0 #8 | TOKEN_API §15 | `## 15. Test Checklist` | Append browser real-socket checklist | APPLIED — TOKEN_API.md:258–278 |
| JS0 #9 | TRANSPORT_WS opening | opening `Только wss://...` scope | Replace opening scope | APPLIED — TRANSPORT_WS.md:3–5 |
| JS0 #10 | TRANSPORT_WS §1 | `## 1. Профили`; path distinction paragraph | Append bearer/SCRAM distinction | APPLIED — TRANSPORT_WS.md:16–20 |
| JS0 #11 | TRANSPORT_WS §6 | `Коды 4010-4012 выдаёт только...` | Append bearer-only close codes | APPLIED — TRANSPORT_WS.md:85–87 |
| JS0 #12 | TRANSPORT_WS §8 | SCRAM frame-limit sentence | Replace/extend limits | APPLIED — TRANSPORT_WS.md:97–99 |
| JS0 #13 | TRANSPORT_WS §9 | Origin rules | Append bearer Origin distinction | APPLIED — TRANSPORT_WS.md:117–119 |
| JS0 #14 | SECURITY_MODEL §4 | end of Non-Guarantees section | Append browser bearer threat analysis | APPLIED — SECURITY_MODEL.md:159 |
| JS0 #15 | SECURITY_MODEL §5 | RFC 6750 row | Replace row | APPLIED — SECURITY_MODEL.md:175 |
| JS0 #16 | CLIENT_BROWSER §6 | `## 6. Connection Code (Pseudo-API)` | Append browser bearer API contract | APPLIED — CLIENT_BROWSER.md:251 |
| JS0 #17 | CLIENT_BROWSER §9 | `## 9. Test Checklist (release blockers)` | Append real-browser checklist | APPLIED — CLIENT_BROWSER.md:282–297 (full section readback observed) |

### AUTH0 (source: `TOKEN_LOGIN_HTTP.md`, §10 proposed amendments; 23 distinct amendment headings/actions)

| Source / heading | Target section | Quoted anchor | Action | Final status / evidence |
|---|---|---|---|---|
| AUTH0 AUTH_PROTOCOL §5.1.1 bounds | AUTH_PROTOCOL §5.1.1 | client KDF validation block | Replace with lower+upper bounds | APPLIED — AUTH_PROTOCOL.md:242–249 |
| AUTH0 AUTH_PROTOCOL §5.1.1 rejection | AUTH_PROTOCOL §5.1.1 | rejection sentence after block | Replace wording | APPLIED — AUTH_PROTOCOL.md:249 |
| AUTH0 AUTH_PROTOCOL §5.2.5 | AUTH_PROTOCOL §5.2.5 | existing pair backoff paragraph | Append bounded pair-only store semantics | RECONCILED — original omitted exponent definition; clarified accepted count-1 curve; AUTH_PROTOCOL.md:311–319 |
| AUTH0 AUTH_PROTOCOL §8.4 | AUTH_PROTOCOL §8.4 | lockout-silent paragraph | Append explicit HTTP-only exception | APPLIED — AUTH_PROTOCOL.md:519 |
| AUTH0 AUTH_PROTOCOL §8 limits | AUTH_PROTOCOL §8 table | `LOCKOUT_THRESHOLD` / `LOCKOUT_AUTO_RESET` rows | Replace both rows | APPLIED — AUTH_PROTOCOL.md:502–503 |
| AUTH0 AUTH_PROTOCOL §14.4 | AUTH_PROTOCOL §14.4 | non-disclosure statement | Append same HTTP-only exception | APPLIED — AUTH_PROTOCOL.md:904 |
| AUTH0 IMPLEMENTATION_GUIDE §1.3 cap | IMPLEMENTATION_GUIDE §1.3 | `Hard cap MAX_AUTH_FAILURES_ENTRIES = 1M...` | Replace cap bullet | APPLIED — IMPLEMENTATION_GUIDE.md:77 |
| AUTH0 IMPLEMENTATION_GUIDE §1.3 snapshot | IMPLEMENTATION_GUIDE §1.3 | persistence lead-in | Insert snapshot/reload semantics | APPLIED — IMPLEMENTATION_GUIDE.md:87–89 |
| AUTH0 TOKEN_API §1 | TOKEN_API §1 | listener-boundary paragraph | Replace; reconcile JS0 #2 | RECONCILED — TOKEN_API.md:22 |
| AUTH0 TOKEN_API §3 label | TOKEN_API §3 | `label` row | Replace with active-token uniqueness | APPLIED — TOKEN_API.md:45 |
| AUTH0 TOKEN_API §4 login exception | TOKEN_API §4 | malformed/auth-header item 3 | Append login-route exception | RECONCILED with JS0 #3; separately non-bearer login item clarifies scope — TOKEN_API.md:66–67 |
| AUTH0 TOKEN_API §5 bearer verification | TOKEN_API §5 | header-verification lead-in | Replace lead-in and preserve separate browser sequence | RECONCILED with JS0 #4 — TOKEN_API.md:71–86 |
| AUTH0 TOKEN_API §9 Expect | TOKEN_API §9 | `Expect: 100-continue` sentence | Replace with bearer/login order | APPLIED — TOKEN_API.md:175 |
| AUTH0 TOKEN_API §12 opening | TOKEN_API §12 | opening SCRAM `DbRequest` paragraph | Append non-DbRequest login exception | APPLIED — TOKEN_API.md:227 |
| AUTH0 TOKEN_API §12 rule 1 browser restriction | TOKEN_API §12, rule 1 | `binding_mode = 0x02` restriction | Append separate HTTP login limitation | APPLIED — TOKEN_API.md:238 |
| AUTH0 TOKEN_API §12 rule 1 limits | TOKEN_API §12, rule 1 | `tokens.max_per_user` limits sentence | Replace with shared active-only controls | APPLIED — TOKEN_API.md:238 |
| AUTH0 TOKEN_API §12 rule 3 delivery | TOKEN_API §12, rule 3 | SCRAM secret delivery sentence | Replace with protected HTTP login option | APPLIED — TOKEN_API.md:240 |
| AUTH0 TOKEN_API §11 plaintext | TOKEN_API §11 | plaintext bearer final bullet | Append login-only remote plaintext rule | APPLIED — TOKEN_API.md:223 |
| AUTH0 SECURITY_MODEL §2 | SECURITY_MODEL §2 | final existing threat row | Append two HTTP login threat rows | APPLIED — SECURITY_MODEL.md:72–73 |
| AUTH0 SECURITY_MODEL §4.15 | SECURITY_MODEL §4.15 | final browser-session sentence | Append login-specific risk | APPLIED — SECURITY_MODEL.md:155 |
| AUTH0 ADMIN_UI_HOSTING §4.2 | ADMIN_UI_HOSTING §4.2 | `[admin_ui]` TOML block | Append reverse-proxy guidance | APPLIED — ADMIN_UI_HOSTING.md:86 |
| AUTH0 TRANSPORT_WS §1 | TRANSPORT_WS §1 | SCRAM path distinction paragraph | Append proposal scope note | APPLIED — TRANSPORT_WS.md:20 |
| AUTH0 README core index | README core | TOKEN_API bullet | Insert TOKEN_LOGIN_HTTP entry after it | APPLIED — README.md:38 |

## Reconciliations and repair record

- JS0 #2 × AUTH0 TOKEN_API §1: one listener-boundary statement now distinguishes bearer data endpoints, separately enabled login routes, and SCRAM endpoints. Login does not authenticate `/shamir/v1/request`; browser first-frame bearer exception remains narrow. Rationale: both amendments reserve an HTTP-listener exception without turning it into a DB bearer/SCRAM session.
- JS0 #3 × AUTH0 TOKEN_API §4: kept the browser first-frame exception to header authentication while explicitly defining login as separate, non-bearer routes with fresh proof and signature. §4 item 3 preserves the no-URL/query/cookie/body/subprotocol rule for token routes.
- JS0 #4 × AUTH0 TOKEN_API §5: retained all numbered steps 1–8 as bearer-data-route logic. REST/native remain header-preauth; browser does pre-upgrade Host/Origin and pre-auth reservation, then first-frame credential verification; login ceremony is outside these steps. §5 step 8 scopes HTTP 401/429 to REST/native and cross-references browser 4000/no HTTP 429.
- JS0 browser failure semantics vs native bearer errors: browser unverified failures close 4000 after upgrade; verified channel/plaintext denials close 4013/4014; native bearer preserves pre-upgrade HTTP 401/403/503. Explicit definitions are in TOKEN_API §10 and TRANSPORT_WS §6; SECURITY_MODEL API guessing row distinguishes these at SECURITY_MODEL.md:66.
- AUTH_PROTOCOL §5.2.5 accepted amendment OLD anchor included `N = count-1, begins at count 1`; committed target lacked that definition. Applied curve is count-1 from failure 1, avoiding an ambiguous exponent. AUTH_PROTOCOL §15.2 remains its original auth_message clause; no extra numbered method-scope clause is retained.
- Lockout is pair-only, not aggregate per account. HTTP source credits separately constrain untracked-pair attempts. The initial AUTH0 document contains both 256 pending-entry quota and distinct 64 concurrent input-slot cap; implementation and final notes preserve this distinction (TOKEN_LOGIN_HTTP.md:111–117; AUTH_PROTOCOL.md:311–319; IMPLEMENTATION_GUIDE.md:77,87). Browser preauth max 8/subnet is separate from active token WS socket limits.
- Source cleanup: `token-client-js.md:218–220` replaces the JS proposal payload with applied-status paragraph and retains JS1–JS5 design. `TOKEN_LOGIN_HTTP.md:205–217` labels the document a proposed, unimplemented feature and retains binding decisions; its section 10 amendment payload is absent. Cross-reference-only/status edits preserve the proposed-feature limitation.
- Transient repair errors during work: TOKEN_API had duplicated §10 tail, §13 audit content, and §14/§15 headings; corrected to one each, with all headings 1–16 once and `### 10.3` once (TOKEN_API.md lines 11–280). SECURITY_MODEL initially had duplicate `## 2` heading at the appended threat rows; corrected to continue the existing table (rows 66–73). Do not treat transient corruption as final status.

## Read-only verification results

Commands used (read-only only):
- `rg -n -i 'GET /shamir/v1/ws[^/` ]*|/shamir/v1/ws|allow_plaintext_login|aggregate(d)? lockout|aggregate lockout|fixed overflow|overflow bucket|among all tokens|уникальн[а-яё ]+среди всех токенов|Connection: close|pipelining|per-account cap' docs/guide-docs/client-server-protocol-spec`
- `rg -n -i 'allow_plaintext_login|aggregate.{0,30}lockout|lockout.{0,30}aggregate|fixed overflow|overflow bucket|unique among all tokens|among all tokens|among all.*token|уникальн.{0,60}всех токен|Connection: close.{0,80}pipeline|pipelining.{0,80}Connection: close' docs/guide-docs/client-server-protocol-spec`
- Additional narrow searches for fixed bucket/overflow, active-only label language, Connection: close, and heading counts.

Observed: no `allow_plaintext_login`, aggregate-lockout wording, fixed-overflow-bucket wording, or label-uniqueness-among-all-tokens match. Active-only uniqueness appears in TOKEN_API.md:45 and TOKEN_LOGIN_HTTP.md:172. `Connection: close` remains in TOKEN_API.md:173,175 for pre-body REST response/body framing behavior; it is not a pipelining mandate. TOKEN_LOGIN_HTTP.md:7 requires close per its own single-request/no-pipelining login contract. No old bearer endpoint statement contradicted the browser endpoint; search hits consistently distinguish SCRAM `/shamir/v1` and `/shamir/v1/browser` from bearer `/shamir/v1/ws` and `/shamir/v1/ws/browser`. Expected `/shamir/v1/ws` references include TOKEN_API, TRANSPORT_WS and CLIENT_BROWSER.

Read-only heading/reference spot-check: TOKEN_API `## 1`–`## 16` each once; `### 10.3` once. AUTH_PROTOCOL §5.1.1 is a numbered clause under `### 5.1`, §5.2.5 under `### 5.2`, §8.4 under `## 8`, §14.4 under `## 14`, §15.2 under `## 15`; these are clauses, not Markdown headings. Cross-references touched point to those clauses or existing section headings, including TOKEN_LOGIN_HTTP §§3–6, AUTH_PROTOCOL §§5.1.1/5.2.5/8.4/14.4, SECURITY_MODEL §4.15, TRANSPORT_WS §§1/6/8/9, TOKEN_API §§1/3/4/5/9/10/11/12/13/15, IMPLEMENTATION_GUIDE §§1.3/3/5.8, and README index targets. The older `Connection: close` pipeline wording is retained only where response closure is needed to avoid unread body bytes; login uses its separate no-pipelining rule.

## Completion checkpoints and remaining work

- `AUTH_PROTOCOL.md`: final readback observed KDF bounds/rejection at lines 242–249, pair-only amendment and count-1 backoff at 311–319, HTTP-only lockout exception at 519, threshold/reset rows at 502–503, and §14.4 HTTP-only exception at line 904. Existing §15.2 auth_message clause at line 912 is unmodified.
- `IMPLEMENTATION_GUIDE.md`: cap bullet line 77; bounded snapshot text line 87 followed by original persistence lead-in line 89.
- `token-client-js.md`: readback lines 218–220 confirms proposal heading followed by applied paragraph; retained JS1–JS5 planning content immediately above.
- `TOKEN_API.md`: complete final readback and prior section readbacks; targeted browser/native error and channel distinctions at §§4–5, §10, §15; one heading each 1–16.
- `TRANSPORT_WS.md`: opening scope lines 3–5; JS0 #10 line 18 and AUTH0 proposal scope line 20; close codes 85–87; frame limits 97–99; Origin distinction 119.
- `SECURITY_MODEL.md`: threat table token row line 66 and HTTP login threat rows 72–73; §4.15 line 155; RFC 6750 row 175; browser bearer threat 159.
- `CLIENT_BROWSER.md`: §6 bearer contract line 251, §9 checklist lines 282–297. Orchestrator final read observed the entire changed §9 and appended browser checklist at line 297.
- `ADMIN_UI_HOSTING.md`: §4.2 proxy deployment paragraph line 86.
- `README.md`: TOKEN_LOGIN_HTTP index entry line 38.
- `TOKEN_LOGIN_HTTP.md`: complete readback, 217 lines; §10 applied-status and owner-decision section lines 205–217; stays a specification for a not-yet-implemented feature.
- No other files were edited in this final NOTE-only chunk. No builds, tests, cargo, npm, node, Python validation programs, or git-mutating commands were run. No builds/tests were run anywhere in this documentation task. Remaining for orchestrator: commit only; no additional normative change is identified by this audit.

## Final report

All 17 JS0 amendments and 23 AUTH0 amendment items are recorded above as APPLIED or RECONCILED, with target paths/sections, anchors, actions and line evidence. Final evidence includes the full changed CLIENT_BROWSER §9 checklist (lines 282–297) as observed by the orchestrator. No cross-reference is invented to a nonexistent TOKEN_API §12.1/§12.3 heading; rule references are labeled as rule 1/rule 3 within §12. Completed TOKEN-SPEC2: all 17 JS0 and 23 AUTH0 amendments applied or reconciled across eight target specifications; both sources cleaned. Evidence, reconciliations, and read-only searches are recorded in `docs/dev-artifacts/audits/2026-11-01-token-spec2/NOTES.md`. No builds/tests or git mutation. Feature implementations remain out of scope; changes are ready for orchestrator review/commit.
