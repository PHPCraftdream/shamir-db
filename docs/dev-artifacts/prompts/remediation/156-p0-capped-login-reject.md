# P0 stage D4 — p0-capped-login-reject

## Goal

Cards `RV-CONNECT-0A42F42C` (capped login evicts active sessions instead of the specified all-active rejection), `RV-CONNECT-8AD8B1C8` (resume bypasses MAX_SESSIONS_PER_USER), `RV-CONNECT-D8C2825A` (SessionStore::cap_lock: parking_lot Mutex held across an O(all-sessions) scan) in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` (read whole with the 2026-10-24 paragraphs) and the spec `docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md` ~:455-458. Code: `crates/shamir-connect/src/server/state/{session.rs,resume.rs}`, `crates/shamir-connect/src/common/model/error.rs`, `crates/shamir-server/src/sync/connection/handshake.rs`.

Ruling (D4): keep the spec: at the cap, REJECT a new login when every existing session of the user was active within the last 60 s (age < 60 s is active; >= 60 s is idle), otherwise evict the least-recently-active idle session; ONE admission function for login and resume that enforces the count after insert; explicit admission timestamp (no hidden clock); generic external `authentication_failed`, internal audit reason `auth_failed/max_sessions`, no `auth_success`/ticket on rejection. A previous executor attempt failed review three times on atomicity (replacement cleanup and insertion not atomic; a lock held across the all-sessions scan; session-id replacement corrupting the index and cap accounting; tests in an undiscovered module). Do NOT repeat those: implement exactly the design below.

## BINDING DESIGN (two rush reviewers + @oh approve per-user sharded admission; follow @oh's lock order)

Replace `cap_lock` and the full-map scan with a per-user index: `by_user: DashMap<[u8;16], SmallVec<[(Sid, Arc<Session>); 16]>, SessionIdHasher>` next to `by_sid`. `SessionStore::admit(sid, session, now_ns, cap) -> Admission` becomes the ONLY way a session is admitted. Under ONE per-user shard guard (work O(cap), never O(all sessions), sync, never across .await):
1. prune entries whose `by_sid` binding was removed or replaced (ptr_eq);
2. decide with NO side effects: if the sid is already in this user's slot it is a replacement (needs no extra capacity), else `need = (len+1).saturating_sub(cap)` (cap 0 disables the cap); order slot entries by (last_activity, sid); the victims are the first `need`; if any victim has `now_ns.saturating_sub(last) < ACTIVE_WINDOW_NS (60 s)` return `Rejected(AllActive)` mutating nothing;
3. claim the sid atomically in `by_sid` (vacant -> insert; occupied by the same user's replacement -> replace; occupied by a FOREIGN user's sid -> `Rejected(SidConflict)`); drop that entry guard before step 4 (same-shard reentrancy);
4. evict the victims (`by_sid.remove_if(k, |v| Arc::ptr_eq(v, a))`), cannot fail;
5. update the per-user slot (remove victims, replace or push) and return `Admitted{ session, evicted, replaced }`.
Types in a new `state/admission.rs` re-exported from `state/mod.rs`: `enum Admission { Admitted{ session: Arc<Session>, evicted: SmallVec<[Sid;1]>, replaced: bool }, Rejected(AdmitReject) }`, `enum AdmitReject { AllActive, SidConflict }`, `const ACTIVE_WINDOW_NS: u64 = 60 * 1_000_000_000`.
`remove(sid)` takes `by_sid.remove` first and drops that guard, then `by_user.get_mut(user)` and `retain(!(k==sid && ptr_eq))`, `remove_if(empty)` — a stale removal cannot delete a session that replaced it. `count_for_user`/`snapshot_by_user` read `by_user` (O(cap)). The old `insert_with_per_user_cap` is removed and its tests rewritten against `admit`; plain `insert` becomes `admit(.., cap 0)` for test fixtures only (not used in production code paths).
Lock order: a `by_user` guard may be held while taking a `by_sid` guard, never the reverse; never two `by_sid` guards at once; no guard across .await. Inline comments name the contention model of each map.
Resume (resume.rs ~:448-464): call `admit` AFTER `try_advance` and after encryption — admission is the LAST step that can fail so nothing needs rollback; map `Rejected` to a new `Error::AdmissionRejected(AdmitReject)` whose wire form collapses to `AuthFailed`; `ResumeOk` carries `evicted`.
Server (handshake.rs): login takes `admit_ns` once and passes it to `Session::new` and `admit`; `Rejected` => audit `auth_failed` with reason `max_sessions` (or `sid_conflict`) BEFORE any `auth_success`/ticket, return a new `HandshakeError::Admission`, no lockout registration; `Admitted` => audit `session_evicted/max_sessions_lru` per evicted sid; resume rejection maps to the same error; the outer handler pads to the generic failure floor and closes (externally indistinguishable from other auth failures).
Invariants: I2 exact index (set(by_user[U]) == sessions of U in by_sid by ptr_eq when nothing is in flight); I3 an admit never takes the user's count above max(cap, previous length); I4 a Rejected result changes nothing apart from idempotent pruning; I5 active iff now - last < 60 s (saturating), only idle sessions evicted in (last, sid) order; I6 admission is last.

Tests (deterministic: injected `now_ns`, Barriers; registered through the CURRENT wired manifests — store tests go in `src/server/tests/session_flow/` (check its mod.rs wiring), resume tests in a module you verify is compiled; confirm each new file is reachable from a mod.rs):
1 all active: 16 sessions with last = T - 59_999_999_999 => Rejected(AllActive), count 16, new sid absent, all 16 unchanged. 2 exactly 60 s: oldest last = T - 60 s => Admitted, evicted == [oldest], count 16. 3 two threads (std Barrier(2)) racing at cap-1, 1000 iterations on fresh stores: with 15 active => outcomes always {Admitted, Rejected}, count 16; with one idle => both Admitted, exactly one eviction in total, count 16. 4 64 threads with a Barrier, mixed idle/active: count_for_user <= 16 after every join and snapshot_by_user equals a scan of by_sid. 5 replacement: same sid + same user twice => replaced, count unchanged, lookup returns the new Arc, then remove => count 0; a stale remove racing behind a Barrier does not drop the new Arc; same sid from another user => SidConflict with both counts unchanged. 6 resume cannot bypass the cap: 16 active sessions then process_resume at T+1 s => Err(AdmissionRejected(AllActive)), wire AuthFailed, count 16; with an idle session => Ok, evicted.len()==1, count 16. 7 server e2e: seed 16 active sessions via admit then one real login => connection closed, no auth_ok, audit `auth_failed/max_sessions`, no `auth_success` for that login.
Acceptance: `git grep` finds no `cap_lock`, no `by_sid.iter()` in admission/count/snapshot; every new test file is reachable; each test fails when only its own fix is reverted (state the revert in its doc comment).

Reviewer A's text for reference:
## Verdict — approve one design: sharded per-user transactions, with SID ownership revalidation

**Keep the D4 ruling and the specification unchanged.** Implement one admission operation for login and resume, backed by bounded per-user membership and an explicit lock-order protocol covering SID replacement. Reject designs that update the secondary index after insertion, serialize admission around a global scan, or leave an uncapped insertion API reachable.

This is a **design approval, not implementation acceptance**. Findings below are statically observed; the proposed concurrency guarantees remain to be demonstrated in GitHub CI.

### 1. Current evidence

| Status | Evidence |
|---|---|
| **OBSERVED — unconditional eviction** | `crates/shamir-connect/src/server/state/session.rs:470–496` takes `cap_lock`, scans every SID, sorts matching sessions, and removes the oldest without an activity threshold. The method has no admission timestamp. |
| **OBSERVED — resume bypass** | `crates/shamir-connect/src/server/state/resume.rs:448–457` constructs a session and calls uncapped `insert`. |
| **OBSERVED — replacement is currently implicit** | `crates/shamir-connect/src/server/state/session.rs:449` and `:496` insert by SID without an ownership/accounting transaction. Any secondary index must explicitly handle the previous value. |
| **OBSERVED — activity is independently mutable** | `crates/shamir-connect/src/server/state/session.rs:306–307` uses an unconditional atomic store; `lookup_at` touches after obtaining an `Arc` at `:519–520`. An admission lock alone would not serialize activity classification. |
| **OBSERVED — server cannot reject admission today** | `crates/shamir-server/src/sync/connection/handshake.rs:580–602` receives only `(session, evicted)` and proceeds to `auth_success`; ticket issuance follows at `:608`. |
| **OBSERVED — normative boundary** | `docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md:455–458` requires idle LRU eviction, rejection when all existing sessions were active **less than** one minute ago, and internal-only reasons. Thus **exactly 60 seconds is idle**. |

These mechanisms agree with the October 24 revalidation paragraphs in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md:627`, `:665`, and `:775`.

## 2. Concrete data structure

**Recommendation — use fixed user shards, not one global mutex and not independently maintained counters.**

Conceptually:

```text
user_shards[N]:
    Mutex<HashMap<UserId, UserSessions, FxHasher>>

UserSessions:
    sessions: HashMap<Sid, Arc<Session>, FxHasher>

by_sid:
    concurrent Fx-hashed map<Sid, Arc<Session>>

sid_write_stripes[M]:
    Mutex<()>
```

- A user hashes to exactly one user shard. Admission examines **only that user's sessions**, normally at most 16.
- The per-user map contains the actual `Arc<Session>`; its last-activity value is the authoritative activity value, not a separately copied timestamp.
- `by_sid` provides routing, but a routing read alone does **not** establish current membership.
- Fixed user shards avoid the detached-empty-user-cell race: removing an empty user entry happens while its shard remains locked; there cannot be two live admission locks for the same user.
- SID write stripes serialize publication of the same SID even when two different users submit it simultaneously.
- These are explicitly **sharded locks, not a lock-free implementation**. Their permitted critical sections contain bounded in-memory membership work only—no I/O, audit calls, cryptography, callbacks, or `.await`.

The replacement target is `SessionStore` at `crates/shamir-connect/src/server/state/session.rs:414–417`. Existing dependencies already include DashMap, parking_lot, and rustc-hash (`crates/shamir-connect/Cargo.toml:53–62`).

### Mandatory lock order

1. Read SID routing hints and **drop all concurrent-map guards**.
2. Lock required user shards in ascending shard order, deduplicating identical shards.
3. Lock required SID write stripes in ascending stripe order, deduplicating identical stripes.
4. Re-read and validate ownership/generation.
5. Commit or release everything and retry.

**Never acquire a user lock while holding a SID stripe or DashMap guard.** Never retain multiple DashMap guards while performing other map operations.

For replacement, the required users are the candidate's user and the current SID owner's user. If revalidation finds a different owner, release and retry with the correct lock set.

## 3. Admission algorithm and return type

Replace the split insertion policies at `crates/shamir-connect/src/server/state/session.rs:434–498` with:

```text
Admission =
    Admitted {
        session: Arc<Session>,
        evicted: Vec<Arc<Session>>
    }
  | Rejected
```

`evicted` is a vector so an explicitly supported pre-existing over-cap state can be repaired without returning incomplete cleanup information. Ordinary admission evicts zero or one session. A replaced SID generation is **not** a `max_sessions_lru` eviction.

Fix the cap in the store's policy at construction—production uses 16. **Do not accept a per-call cap or `0 = unlimited` bypass.**

### Pseudocode

```text
admit(sid, candidate, now):
    prepare candidate SID/HMAC state outside locks

    retry:
        old_hint = read by_sid[sid]; drop routing guard
        lock candidate-user and old-owner user shards, ordered

        old = current ownership visible from routing
        if old owner is outside the locked set:
            unlock; retry

        target = candidate user's membership
        replacing_target = target contains this SID's current generation

        projected_count =
            target.count - replacing_target + 1

        required_evictions = max(projected_count - cap, 0)

        idle = target entries excluding the replaced SID generation
               whose now.saturating_sub(last_activity) >= 60 seconds

        victims = first required_evictions ordered by
                  (last_activity, SID)

        if insufficient idle victims:
            return Rejected without changing either index

        lock SID stripes for candidate SID and victims, ordered
        revalidate old SID generation and every victim generation
        if ownership changed:
            unlock; retry

        remove old generation from its owner's membership, if present
        remove victims from membership and by_sid
        install candidate in target membership and by_sid
        assert target.count <= cap
        remove empty old-user membership entry if necessary
        unlock

        return Admitted(candidate, victims)
```

**All decisions precede mutation. All mutation completes before releasing the locks.** The operation has no fallible application work between removal and publication.

### Required invariants

1. Every published SID has exactly one matching per-user membership entry, referring to the **same generation/Arc**.
2. Each completed admission leaves the target count at or below the cap.
3. Rejection preserves membership, SID routing, and existing session generations.
4. Same-user SID replacement has count delta zero; it does not evict an unrelated session, even at an all-active cap.
5. Cross-user replacement has old-user delta −1 and target-user delta +1, subject to target admission. Rejection preserves the old owner.
6. Cleanup of an old generation cannot remove its replacement.

**Concurrency consequence, inferred from the proposed locking:** two admissions at cap−1 cannot both use that same vacancy. The first commits count=cap. The second evaluates the updated membership and either rejects or evicts an eligible idle session; it cannot publish count=cap+1.

### Activity, lookup, removal, and GC belong to this protocol

Updating only admissions is insufficient.

- `lookup_at` must route, acquire the user shard, revalidate membership/generation, and touch **while holding that shard**. Update activity monotonically with `max(previous, supplied_now)`.
- Direct writes to `last_activity_ns` must not bypass this coordination. The current public atomic at `session.rs:152` and direct touch path at `:306` need controlled mutation semantics.
- `remove`, kicks, expiration cleanup, and replacement use the same user→SID-stripe order.
- GC may collect candidates globally **without holding admission locks**, but must remove conditionally by captured generation and recheck expiry. Its current SID-only collection/removal at `session.rs:541–551` must not delete a replacement.
- `count_for_user` and `snapshot_by_user` use the user membership index, replacing the global scans at `session.rs:557–570`.
- Monitoring clones entries before invoking callbacks; no callback executes under these locks.

An already-returned `Arc` may finish an in-flight request, consistent with the existing ownership contract at `session.rs:404–406`. That does not count as continued store membership.

## 4. Timestamp and resume integration

**Recommendation — admission time is an explicit input, never inferred from `created_at_ns`.**

Full login must sample a fresh admission timestamp near the admission call, not reuse the pre-challenge timestamp captured at `crates/shamir-server/src/sync/connection/handshake.rs:210`. Tests supply exact timestamps. Future-valued activity is conservatively active through saturating subtraction.

For resume:

1. Preserve ticket validation and durable replay-counter advancement at `crates/shamir-connect/src/server/state/resume.rs:275–393`.
2. Build the candidate and call **the same `admit` function** instead of `insert` at `:457`.
3. On rejection, return a typed internal resume failure carrying `MaxSessions` and the validated audit identity.
4. Do not construct a refresh ticket or return `ResumeOk` on rejection. Move refresh issuance currently at `:420–441` behind admission.
5. Return admitted eviction metadata to `run_resume`, so resume eviction has the same audit behavior as login.

**Decide explicitly:** a valid resume attempt rejected by the cap **consumes its ticket counter**. Keep the durable advancement before admission; do not perform persistence under admission locks and do not roll back a durable anti-replay advance. The client must use full authentication afterward.

## 5. Server mapping

At `crates/shamir-server/src/sync/connection/handshake.rs:580`:

- **Admitted:** emit eviction audit events from returned victims; then continue with success/ticket handling.
- **Rejected:** emit exactly `event="auth_failed", reason="max_sessions"`; return an internal `MaxSessions` handshake outcome.
- Externally expose only generic **`authentication_failed`**, never occupancy, victim identifiers, or the internal reason.
- Do not emit `auth_success`, issue a ticket, send `auth_ok`/`resume_ok`, or enter the request loop.
- Capacity rejection is not a wrong-password event: do not increment credential-failure backoff, and do not reset lockout as though admission succeeded.

**Important observed integration gap:** the current outer error branches merely shut down at `handshake.rs:760–763` and `:820–821`; they do not send a generic failure frame. D4 must implement the required generic failure response, not assume that returning `BadProof` produces it. Preserve full-login negative-path latency padding.

Move success-only effects currently starting at `handshake.rs:483` behind admission so a rejected login does not reset lockout or consume bootstrap credentials.

## 6. Deterministic CI acceptance oracles

Use injected timestamps and barriers/channels at actual store transitions—not sleeps, probabilistic stress alone, or a copied admission implementation.

| Oracle | Required assertion |
|---|---|
| **All-active cap** | Fill 16 sessions with ages below 60 s; next login returns `Rejected`; exact SID/generation set unchanged. |
| **Exact boundary** | At age `60s−1ns`, reject; at exactly `60s`, evict that session; at `60s+1ns`, evict. |
| **Idle LRU** | Mixed active/idle sessions; evict the minimum `(last_activity, SID)` eligible entry, preserving all others. |
| **Two admissions at cap−1** | Barrier-start two candidates against 15 active sessions at the same `now`; exactly one admitted, one rejected, final count 16. Check invariant snapshots at commit boundaries to detect transient over-cap publication. |
| **Competing idle eviction** | At cap with exactly one idle victim, two admissions race; exactly one evicts it, the second rejects. |
| **Activity race** | Force touch-before-admission and admission-before-touch. First ordering protects the touched session; second cannot resurrect an evicted generation. Stale timestamps never regress activity. |
| **Same-user replacement** | Replace one SID at a full all-active cap; count remains 16, no unrelated eviction, both indexes reference the new Arc. |
| **Cross-user replacement** | Verify exact old/target counts for both accepted and rejected transfers; rejected transfer leaves old ownership intact. |
| **Stale cleanup** | Pause GC after capturing an old generation, replace its SID, resume GC; replacement survives and accounting is unchanged. |
| **Resume cannot bypass** | Use a valid unconsumed ticket against 16 active sessions; get typed internal cap rejection, unchanged membership, no success frame/ticket. Assert the counter-consumption decision separately. |
| **Login versus resume** | Race both at cap−1 through their real entry points; combined membership never exceeds 16. |
| **User isolation** | Pause one admission inside its user transaction; an unrelated user on another shard completes without waiting for that transaction. |
| **Wire/audit** | Both rejection paths produce generic `authentication_failed`, one internal max-sessions audit, zero success audits, zero ticket issuance, and no request-loop entry. |

**Discovery is a merge criterion.** Library tests must be reachable through `crates/shamir-connect/src/server/mod.rs:6–7` → `tests/mod.rs:3` → `tests/session_flow/mod.rs`. Existing integration-session tests are explicitly registered at `crates/shamir-connect/Cargo.toml:114–116`; their unconditional-LRU and unlimited-cap expectations at `tests/session/integration_session.rs:381–451` must be replaced, not retained as contradictory policy.

**Acceptance:** approve implementation only when the complete mutation-path audit satisfies these invariants and GitHub CI both discovers and passes the discriminating tests.

**Files changed:** none. **Builds/tests run:** none; read-only review only.

## Requirements

1. Before editing, confirm each mechanism in current code (read callers and guards). If a card is already fixed or wrong at HEAD, do not change code for it: say so with file:line evidence in the final message.
2. Fix at the narrowest layer that owns the invariant. No unrelated refactors, renames or comment edits.
3. TDD: for every card add a regression test whose name contains the card's hex suffix in lower case (for example `rv_storage_98f6650d_...`). The test must fail on the unfixed code and pass on the fix — reason about that from the code, you cannot run it. Tests live in the module's `tests/` folder (`tests/mod.rs` is a manifest only; no inline `#[cfg(test)] mod tests {}`). Prefer deterministic constructions (injected failures, barriers, paused clocks) over sleeps and timing.
4. House rules: imports at the top of the file; `mod.rs` re-exports only; one primary export per file; at most 7 entries per folder and 1000 lines per source file for new/changed code; Fx hashers; no `std::sync::Mutex`/`RwLock` on hot paths; `Result` instead of panics; no private machine paths in committed files; terse comments.
5. Do not bump crate or dependency versions, do not add dependencies, do not touch CI workflows.
6. Run only the allowed local gates from the policy (clippy, fmt, layout) on the touched crates until clean.

## Final message (mandatory)

1. Per card: fixed / already-fixed / refuted, with the changed file:line and the regression test name.
2. Why each test fails on the old code (one sentence each).
3. Real exit codes of clippy, fmt --check and check_layout. Anything you could not verify.
