# RI-6: Full auth test-vector suite + honest fuzz/power-fail status

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

`docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md` §16 ("Test
Vectors") declares, in NORMATIVE language: *"Release blocker для v1. Файл
`docs/guide-docs/client-server-protocol-spec/test-vectors/auth_v1.msgpack`
обязан содержать полный набор"* — listing 6 required vector categories:
`kdf_canonical_string`, `Argon2id(fixed password/salt/params) → 32-byte
output`, `client_proof`/`server_signature`/`identity_sig` for the full flow,
`fake_blob` via HKDF for a fixed username, resumption-ticket encrypt/decrypt
round-trip with a fixed key/nonce, and identity-rotation `signed_by_old`.

No such `.msgpack` file exists anywhere in the tree (verified: `find . -iname
"*.msgpack" -not -path "./target/*"` — empty). **What DOES exist** (read
these first, they are your foundation):

- `crates/shamir-connect/test-vectors/auth_message_default.json` +
  `.toml` — ONE fixed vector, for `auth_message` construction only (the
  §16 hex-dump example, already covering the first "Example" block in §16).
  Read `crates/shamir-connect/src/common/tests/auth_message_tests.rs` to see
  how it's consumed.
- `crates/shamir-connect/src/common/tests/scram_tests.rs`,
  `fake_blob_tests.rs`, `identity_tests.rs` — real, substantial test
  coverage (9 + 7 + 9 tests) for SCRAM/fake-blob/identity — but read
  `scram_tests.rs`'s own header comment: **these are ROUND-TRIP tests**
  ("derive on 'client', verify on 'server' using public values only... pins
  the entire SCRAM arithmetic") with RANDOM/arbitrary inputs, NOT fixed
  hardcoded vectors with byte-for-byte expected hex output. They prove
  internal (Rust-to-Rust) consistency; they do NOT give a second
  implementation (the browser/TS SDK) anything concrete to check its own
  output against, and they do NOT pin the exact bytes against regression —
  a subtle change to domain-tag ordering or HKDF info-string construction
  could silently drift and every one of these tests would still pass (both
  sides drift together).
- `crates/shamir-connect/src/common/crypto_tests.rs` (24 tests) — DOES use
  fixed vectors, but only for the underlying primitives (RFC 6234/4231/5869
  SHA-256/HMAC/HKDF test vectors) — proving the crypto *library wiring*,
  not this protocol's own composite constructions (client_proof =
  domain-separated-HMAC-of-derived-keys, fake_blob = HKDF with THIS
  protocol's specific salt/info strings, etc).

**The real gap, precisely stated**: no FIXED, hardcoded, cross-language-
consumable vectors exist for `client_proof`, `server_signature`,
`identity_sig`, `fake_blob`, or the resumption-ticket AES-GCM round-trip.
This is what §16 actually demands and what a future browser/TS
implementation (or a Rust regression) needs to check against.

Read these modules for the real functions to derive vectors FROM (do not
invent expected outputs — compute them by actually running the real code
with fixed inputs and capturing the real output):
- `crates/shamir-connect/src/common/crypto.rs` — `argon2id`, `hkdf_sha256`,
  `aes256gcm_encrypt`/`_decrypt`, `Ed25519Keypair::from_seed`/`sign`,
  `ed25519_verify_strict`.
- `crates/shamir-connect/src/common/scram.rs` — `DerivedKeys::derive`,
  `build_client_proof`, `build_server_signature`, `recover_client_key`,
  `verify_client_proof`.
- `crates/shamir-connect/src/common/fake_blob.rs` — `FakeBlob::derive`.
- `crates/shamir-connect/src/common/identity.rs` — `build_identity_input`,
  `sign_identity`, `verify_identity`.
- `crates/shamir-connect/src/common/domain_tags.rs` — the exact domain-
  separation tag strings (§17 of the spec) each construction uses; every
  vector's inputs must use the REAL tags from this file, not re-typed
  copies that could silently diverge.

## The task — Part 1: full test-vector suite (DO this)

1. **Decide the file format honestly.** The spec literally names
   `auth_v1.msgpack` (one binary blob) but the ALREADY-ESTABLISHED,
   ALREADY-WORKING convention in this repo is per-vector JSON+TOML pairs
   under `crates/shamir-connect/test-vectors/` (git-diffable, human-
   readable, easy to extend one vector at a time). Recommend: KEEP that
   convention (add more files in the same shape as
   `auth_message_default.{json,toml}`), and REWRITE §16 to describe the
   REAL location/format instead of forcing a msgpack blob into existence
   that would just duplicate the same data less legibly. This mirrors this
   campaign's established discipline (fix docs to match a good real
   decision, don't force reality to match stale prose) — cite the specific
   files you create in your rewritten §16.

2. **Generate one new fixed vector file per category** (JSON, matching
   `auth_message_default.json`'s shape: `name`, `spec_section`, `inputs`,
   `expected`), computed by ACTUALLY RUNNING the real functions above with
   fixed inputs (reuse the SAME fixed inputs as `auth_message_default.json`
   where the categories chain together — e.g. derive `client_proof` from
   the SAME fixed `auth_message` bytes that file already pins, so the
   vectors compose into one coherent fixed scenario rather than being
   disconnected):
   - `kdf_canonical_string` (check `KdfParams`'s canonical-string
     serialization — grep for "canonical" in `kdf_params.rs`).
   - `argon2id_default.json` — `Argon2id(password="hello world!1", salt=
     <fixed 16 bytes>, params=KdfParams::DEFAULT)` → 32-byte output hex.
   - `scram_flow_default.json` — chained: `DerivedKeys::derive` from the
     same password/salt/params → `client_proof` (needs the auth_message
     bytes as context, reuse `auth_message_default.json`'s) →
     `server_signature`. All as hex, all derived from ONE coherent fixed
     scenario.
   - `identity_sig_default.json` — `Ed25519Keypair::from_seed(<fixed 32-byte
     seed>)`, `build_identity_input(...)` with fixed fields, `sign_identity`
     → 64-byte signature hex, plus the verifying public key.
   - `fake_blob_default.json` — `FakeBlob::derive(<fixed 32-byte
     server_secret>, <fixed username>)` → 80-byte hex (per §16's own
     description "через HKDF для fixed username → 80 байт hex" — verify
     the real length by running it, don't assume the spec prose is exact).
   - `resumption_ticket_roundtrip.json` — `aes256gcm_encrypt` with a fixed
     32-byte key + 12-byte nonce over a representative fixed ticket
     plaintext, capturing ciphertext hex, then `aes256gcm_decrypt` proving
     round-trip — check `SESSION_RESUMPTION.md` for the real ticket
     plaintext shape to use a realistic (not made-up) structure.
   - `identity_rotation_signed_by_old.json` — the rotation event signature
     construction (`crates/shamir-connect/src/client/rotation.rs`) with
     fixed old/new keypairs.

3. **Write a Rust test per vector file** (new test file e.g.
   `crates/shamir-connect/src/common/tests/test_vectors_tests.rs`, wired
   into `tests/mod.rs` per this repo's test-organisation convention — one
   `tests/` dir per module, `mod.rs` manifest-only, see CLAUDE.md "Test
   organisation") that loads the JSON, feeds `inputs` into the real
   function, and asserts BYTE-FOR-BYTE equality against `expected`. This is
   what makes the vector suite load-bearing (an implementation drift
   fails loudly), not decorative.

4. **Rewrite AUTH_PROTOCOL.md §16** to describe the real, current state:
   the file location/format decided in step 1, the actual list of vector
   files now present, and drop the `.msgpack` filename reference (or keep
   it as a documented alias if you decide to ALSO emit a merged msgpack
   file from the JSON sources for convenience — your call, but don't do
   BOTH formats as separately-maintained sources of truth; one JSON/TOML
   set is the source, an optional msgpack export is derived if you choose
   to add it).

## The task — Part 2: fuzz / power-fail — decide, don't half-ship

`IMPLEMENTATION_GUIDE.md:605` requires (NORMATIVE): comprehensive property
tests, pre-auth fuzzing, power-fail testing, Unicode normalization vectors.
Investigate what's actually feasible:

1. **Unicode normalization vectors**: check
   `crates/shamir-connect/src/common/username_tests.rs` (12 existing
   tests) — does it already cover NFC/casefold/forbidden-character edge
   cases from real Unicode test suites (e.g. UTS46/PRECIS test vectors), or
   only ad-hoc cases? If real Unicode conformance vectors are missing, add
   a SMALL set (5-10 canonical NFC composition/decomposition edge cases,
   e.g. from Unicode's own `NormalizationTest.txt` categories) as a
   pragmatic middle ground — do not attempt full UTS46 conformance in this
   task.

2. **Pre-auth fuzzing**: no `cargo-fuzz`/AFL harness exists (verified: `find
   . -iname "fuzz_targets" -o -iname "*.fuzz.rs"` — empty, no `fuzz/`
   directory). Investigate whether a MINIMAL harness for the single
   highest-value target — parsing `AuthMessage`/wire-frame bytes from an
   untrusted pre-auth client — is cheap to stand up (`cargo fuzz init`,
   one target function, no corpus curation beyond the seed). If it's a
   contained, bounded addition (a few hours of scope, not a new CI
   subsystem), add it AND wire a `workflow_dispatch`-only or scheduled
   fuzz job (following `stress-nightly.yml`'s conventions) that runs it for
   a BOUNDED time budget (e.g. 60s) as a smoke check, not a real fuzzing
   campaign. If it turns out to need real scope (corpus seeding,
   sanitizer setup, longer runs to be meaningful) — STOP and downgrade
   instead (see step 4).

3. **Power-fail testing**: `crates/shamir-engine/src/tx/tests/
   recovery_tests.rs:364`'s own comment already says a subprocess-kill
   harness is a TODO (Stage 7.rest) and out of reach for the existing
   test. Do NOT attempt to build a subprocess-kill/power-fail harness in
   this task — it's a substantial testing-infrastructure investment
   (spawn real process, SIGKILL at random points, verify recovery), not a
   documentation or vector-generation task. This is a clear "downgrade"
   candidate (step 4).

4. **Downgrade what you don't implement.** For whichever of
   {fuzzing, power-fail, full Unicode conformance} you decide NOT to fully
   implement (power-fail almost certainly; fuzzing maybe partially; Unicode
   maybe a small addition suffices), rewrite `IMPLEMENTATION_GUIDE.md:605`'s
   language from NORMATIVE/release-blocker to an explicit "Roadmap /
   Not Yet Implemented" framing — state plainly what exists today (the
   real round-trip/RFC-vector tests already in the tree) and what's
   deferred, with a one-line reason each. Do not leave the doc promising
   more than the repo delivers — that mismatch is the exact thing this
   whole campaign exists to close.

## Out of scope

- Do NOT attempt full UTS46/PRECIS conformance test suite — a small,
  representative set of Unicode edge cases is sufficient per step 1 of
  Part 2.
- Do NOT build a subprocess-kill power-fail harness — downgrade the doc
  instead (Part 2, step 3-4).
- Do NOT touch `crates/shamir-connect/test-vectors/auth_message_default.
  {json,toml}` — reuse it as a shared fixed scenario, don't modify it.
- Do NOT change any real crypto/auth production code — this task adds
  tests and vectors that verify EXISTING behavior; if a vector-generation
  step reveals an actual bug in the real implementation (unlikely but
  possible), STOP, do not silently "fix" security-critical crypto code —
  report it in your summary as a finding for a dedicated follow-up.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-connect --full` green, including the new
  vector-driven tests.
- Every new vector's `expected` field must be traceable to an ACTUAL run
  of the real function (report in your summary how you generated each —
  e.g. "ran `argon2id(...)` via a scratch `#[test]`, captured its hex
  output, then wrote it into the vector file" — not hand-computed or
  guessed).
- If you added a fuzz target: report how you ran it locally (even briefly)
  and confirmed it builds/executes; if you added a scheduled workflow for
  it, validate the YAML (js-yaml / actionlint, both available in this
  environment — actionlint via `go install
  github.com/rhysd/actionlint/cmd/actionlint@latest`).
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets
  -- -D warnings` clean.
- Report literal command output for all of the above, plus your Part 1
  step-1 format decision and your Part 2 downgrade decisions with
  reasoning.
