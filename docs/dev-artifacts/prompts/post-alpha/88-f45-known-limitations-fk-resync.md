# Brief for F-45 (#853, P2) — re-sync KNOWN_LIMITATIONS.md's FK closure claims after F-35/F-36/F-40

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

A readonly review (`docs/dev-artifacts/research/2026-07-27-new-wave-readonly-review.md`,
finding P2-1) correctly flagged that `KNOWN_LIMITATIONS.md`'s FK
reverse-check closure entry (~line 92-148, under the `foreign_key`
bullet in §2) claimed a fuller closure than the code actually delivered
at review time — specifically because the cache only tracked `on_delete`
(F-35's bug) and had an invalidate-vs-build race (F-36's bug), neither of
which existed as KNOWN issues when this entry was originally written
(F-28 Step 6). This task runs the audit the review asked for, NOW that
the underlying bugs are actually fixed:

- **F-35 (#843, commit `44c4317e`)** — `ReverseFkEntry` now tracks
  `on_delete` and `on_update` independently; the implicit UPDATE arm's
  Serializable upgrade decision is no longer silently wrong for
  `on_delete = NoAction, on_update != NoAction` FKs.
- **F-36 (#844, commit `d3d06c82`)** — the reverse-FK cache is now
  generation-safe (single-flight + compare-and-publish), closing the
  invalidate-vs-build race that could publish a stale snapshot.
- **F-40 (#848, commit `5679edfa`)** — `require_footprint_if_fk_child`/
  `implicit_tx_isolation_for_fk_parent` now fail CLOSED on a discovery
  error (widen the footprint / upgrade to Serializable) instead of
  silently falling back to the permissive behavior; the SEPARATE
  explicit-Snapshot gap was investigated and scoped into a memo
  (`docs/dev-artifacts/research/f40-explicit-snapshot-ri-gap-memo.md`)
  and a follow-up task (F-40b, #854) rather than closed in F-40 itself.

**This is an AUDIT-and-fix pass, mirroring the F-32b doc sweep's own
methodology** (verify every claim against real code/git history, cite
commit SHAs, don't invent unverified residuals) — read
`docs/guide-docs/KNOWN_LIMITATIONS.md`'s FK entry (~line 92-148) in full
first, and read the three landed commits' actual diffs/doc comments
(`git show 44c4317e`, `git show d3d06c82`, `git show 5679edfa` — or read
the current source they left behind, e.g.
`crates/shamir-engine/src/repo/fk_reverse_cache.rs`,
`crates/shamir-engine/src/query/batch/query_runner.rs`) before writing
anything.

## What to verify and update

1. **Confirm (don't assume) that F-35/F-36's fixes are accurately
   reflected.** The entry's "Cross-transaction race — CLOSED" sub-bullet
   currently doesn't mention that the cache's role-flag data used to be
   `on_delete`-only, or that the cache had an invalidate-vs-build race —
   because those weren't known issues when it was written. Add a note
   (or amend the existing prose) recording that F-35/F-36 closed two
   ADDITIONAL bugs found by the 2026-07-27 review in the SAME mechanism
   this entry describes: the cache previously silently mis-served the
   UPDATE arm's isolation decision for `on_update`-only FKs (F-35), and
   could publish a stale snapshot across a concurrent DDL invalidate
   (F-36). Cite the actual commits.
2. **Update the "Residual scope" bullet** (~line 139-145) to reflect
   F-40's fail-closed improvement: discovery failures (a `resolve_repo`
   or cache-build error) now widen the footprint / upgrade to
   Serializable rather than silently falling back to the permissive
   behavior — this WASN'T true when the entry was written (the
   pre-F-40 code fell back to Snapshot/skipped the footprint on error,
   silently). The explicit-Snapshot gap itself (a caller-opened `Snapshot`
   transaction gets no automatic upgrade) remains OPEN — do not claim it's
   closed — but note it now has a scoped, cited follow-up plan (F-40b,
   #854, and the decision memo at
   `docs/dev-artifacts/research/f40-explicit-snapshot-ri-gap-memo.md`)
   rather than being an unscoped, indefinitely-open gap.
3. **Cross-check every file:line citation in the entry still resolves.**
   The entry cites `fk_restrict.rs`'s module doc, `fk_race_closure_tests.rs`,
   and the S3 mechanism decision memo — spot-check these against the
   CURRENT source (line numbers/content may have shifted since F-35/F-36
   touched these same files) and fix any that have drifted.
4. **Do NOT claim more than what's actually true.** In particular: do not
   say the FK closure is "fully closed" — it correctly remains
   "closed for the implicit path, open (but now scoped) for explicit
   Snapshot." Match the precision level of the existing entry's prose,
   which already correctly distinguished these cases before F-35/F-36/F-40
   — you are updating it to reflect NEW ground truth, not loosening its
   honesty.

## Constraints

- Docs-only — only `docs/guide-docs/KNOWN_LIMITATIONS.md`. No `.rs` files.
- English, matching the file's existing style (bold lead sentence,
  specific file:line/commit citations) — do not switch its language.
- This is an audit-and-fix pass, not a rewrite: if you find the existing
  entry is ALREADY accurate on some sub-point once you check it against
  the current code, leave that sub-point untouched and say so in your
  summary rather than rewriting prose that doesn't need it.
- Do NOT touch any other section of `KNOWN_LIMITATIONS.md` unrelated to
  this FK entry.

## Verification the orchestrator will run

Docs-only — no build/test gate. The orchestrator will re-verify every
updated claim against the actual landed commits/current source before
committing.
