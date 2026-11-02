# TOKEN-SPEC2 - apply the accepted spec amendments of JS0 and AUTH0 to the normative documents

Two accepted documents carry "proposed amendments" with literal anchors against the CURRENT text of the normative spec files in `docs/guide-docs/client-server-protocol-spec/`:

- `docs/dev-artifacts/design/token-client-js.md`, section "Proposed spec amendments" (17 amendments: TOKEN_API.md, TRANSPORT_WS.md, SECURITY_MODEL.md, CLIENT_BROWSER.md). Browser WebSocket endpoint `/shamir/v1/ws/browser`, token as the first frame.
- `docs/guide-docs/client-server-protocol-spec/TOKEN_LOGIN_HTTP.md`, section 10 "Proposed amendments" (about 23 amendments: AUTH_PROTOCOL.md, IMPLEMENTATION_GUIDE.md, TOKEN_API.md, SECURITY_MODEL.md, ADMIN_UI_HOSTING.md, TRANSPORT_WS.md, README.md). Login over HTTP, bounded LockoutStore, label uniqueness over active tokens, and so on.

Both were verified by independent reviewers against the current target text. Your job is mechanical but careful: make them real.

## Steps

1. Read both amendment sections completely and the target files (TOKEN_API.md, TRANSPORT_WS.md, SECURITY_MODEL.md, CLIENT_BROWSER.md, AUTH_PROTOCOL.md, IMPLEMENTATION_GUIDE.md, ADMIN_UI_HOSTING.md, README.md) before editing. Build a working table (in your note, see below): one row per amendment, source document and number, target file and section, anchor quoted, action, status.
2. Apply every amendment literally to its target file: find the quoted anchor in the current file, perform the stated action (replace / insert after / append), insert the stated new text. The target files are Russian: the new text is already in Russian where the amendment says so; keep identifiers and wire fields in English. If an anchor no longer matches (because an earlier amendment of the other set changed the same sentence), do not guess: reconcile the two amendments into ONE consistent final text that satisfies both intents, and record the reconciliation in your note (which two amendments, what the final text is, why).
3. Overlaps are expected (both sets touch TOKEN_API.md sections 1, 3, 4, 5, 9, 10, 11, 12 and SECURITY_MODEL.md, TRANSPORT_WS.md). Apply in this order: all JS0 amendments first, then the AUTH0 ones; after each target file is done, re-read the whole changed sections once for contradictions (numbers of sockets and limits, endpoint names `/shamir/v1/ws` vs `/shamir/v1/ws/browser`, close codes 4000/4010-4014/1013, error codes, the order of the verification steps, the meaning of "active tokens", the security classes of the login ceremony).
4. In the two source documents, replace the whole "Proposed amendments" section by one short paragraph stating that the amendments were applied to the named files in this change (list the files); change status lines that say proposal/"design proposal only" so they say what is now true (TOKEN_LOGIN_HTTP.md stays a specification of a not yet implemented feature: say so; the JS design note stays the design of stages JS1-JS5). Remove any text in them that duplicates what the normative files now say if it would drift, keeping only cross-references.
5. README.md of the spec folder: add TOKEN_LOGIN_HTTP.md to the index next to TOKEN_API.md in the same style as the neighbouring entries (the AUTH0 amendment for README already says how; if it conflicts with the actual README format, follow the README format).
6. Verification by reading (no builds exist for docs): grep the whole `docs/guide-docs/client-server-protocol-spec/` folder for the leftovers: old endpoint statements that contradict the browser endpoint, `allow_plaintext_login`, aggregate lockout wording, `Connection: close` statements for pipelining that changed, "fixed overflow" bucket wording, label uniqueness "among all tokens", dangling section numbers (every `§N` and "section N" cross reference you touched must point to an existing heading). List what you checked and found in the note.

## Rules

Documentation only. Edit only the files named above and your note `docs/dev-artifacts/audits/2026-11-01-token-spec2/NOTES.md` (create it: the amendment table, reconciliations, the grep checks, anything you could not apply and why). No cargo/npm/builds/tests. NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index; the orchestrator commits. Forward slashes. You cannot move or delete files. Do not create files outside the note.

## Final report (mandatory, also in the note under `## Final report`)

Per amendment (JS0 1-17, AUTH0 by its heading) APPLIED / RECONCILED / NOT APPLIED with the target file and line evidence; every reconciliation; the grep results; anything left for the orchestrator.
