# S5c-X PR6b-3 / R2-W4 — documentation consistency fixes (review round 2)

Worktree: current directory. Hard rules: ⛔ no git mutation commands, no compile/test commands
of any kind, no `#[allow]`, no absolute machine paths; surgical edits only.

Fix the three inconsistencies the review found in
`docs/guide-docs/follower-journal-durability.md`, then sweep both it and
`docs/guide-docs/KNOWN_LIMITATIONS.md` for the same false claims:

1. Line ~7 (crash-image paragraph): it still claims the fixtures use "a dropping-unflushed
   fake Store" and "the production verification/flush helper". Reality (read
   `crates/shamir-engine/src/tx/follower_durability/tests/crash_image_tests.rs` first): the
   image is a mutex-backed in-memory byte map; the tests assert repeated TYPED REFUSAL of
   hand-built legacy images (u64/v1/v2 + floor-only) with nothing rewritten, no v3 minted,
   and a v3 image classifying Native; the old barrier verification/flush helper was deleted
   with the legacy executor. Rewrite the paragraph accordingly and KEEP the honesty
   statements: these are not power-loss simulations; graceful subprocess termination tests
   (`crash_subprocess_tests.rs`) are not power-cut models either.
2. Line ~11: "fence-only/no-horizon saves are stored as v3 records with bookmark zero and no
   fence" is wrong — `save_fence_record_v3_aware` on a recordless store writes a v3 baseline
   that CARRIES the fence at bookmark zero (`meta/repl/bookmark.rs:83-92`). Fix the sentence
   (adoption writes the fence INTO the v3 baseline at bookmark zero).
3. Line ~64 (Activation rule): "Fresh self-adopts by writing the v3 baseline — upstream =
   source version, local J = 1, no receipt" conflates the SEED baseline with fresh ADOPTION.
   Reality: fresh adoption writes bookmark 0, `clean_through: None`, no receipt, no fence
   (`receipt/apply.rs:159-165`); seed COMPLETION writes upstream = source version, J = Some(1)
   (`seed_install/writer.rs:314-323`). Fix the bullet and re-read the whole section for
   consistency (the PR6b-3 status bullet at ~76 states the seed baseline — keep it accurate).
4. Sweep BOTH docs for any other statement contradicted by the round-1/round-2 code (strict
   refusals at open AND admission; provenance = valid v3 record; gate-init derivation;
   capability gate for Native AND Fresh admissions) and fix minimally. Do NOT touch
   `docs/dev-artifacts/roadmap/REPLICATION.md` (reported only in round 1).

Report: each fix with the before/after sentence, any additional inconsistency you found and
fixed, and the list of code references you verified the corrected text against.
