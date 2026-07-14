# 06 — Update User (admin) + Race Protection

Atomic role update с двухуровневой защитой от race с in-flight resumption. См. AUTH §12.5 + §7.5.

```mermaid
sequenceDiagram
    autonumber
    participant Adm as Admin (active session, superuser)
    participant S as Server
    participant DB as SystemStore<br/>(durable)
    participant Sess as sessions<br/>DashMap

    Adm->>S: updateUser { user: "alice", roles: ["read_write"] }

    rect rgb(255, 230, 230)
    Note over S,DB: No-op semantic check:<br/>if roles == None AND nothing changes →<br/>  return { ok: { changes_applied: false } }<br/>(защита от silent DoS via repeated noop calls)
    end

    rect rgb(255, 240, 220)
    Note over S,DB: Atomic procedure (single transaction):<br/><br/>Step 1: Update user.roles → persist (durable)<br/>Step 2: Set user.tickets_invalid_before_ns = now_ns → persist (durable)<br/>Step 3: PERSIST BARRIER<br/>        (subsequent reads видят new value — критично!)<br/>Step 4: Snapshot active sessions matching user_id<br/>Step 5: Kill snapshotted sessions (close TCP)<br/>Step 6: Audit event roles_changed
    S->>DB: Step 1+2: persist atomically
    DB-->>S: barrier complete
    S->>Sess: Step 4-5: snapshot+kill
    end

    S->>Adm: ok { changes_applied: true }
```

## Race scenario: in-flight resume escapes snapshot

```mermaid
sequenceDiagram
    autonumber
    participant C as Alice's mobile
    participant S as Server
    participant DB as SystemStore
    participant Sess as sessions<br/>DashMap
    participant Adm as Admin (concurrent)

    Note over C,Adm: T=0: Alice имеет stale ticket с original_auth_at_ns = 0

    C->>S: resume { ticket, ... }
    Note over S: Step 9: ticket.original_auth_at_ns(0) ><br/>  user.tickets_invalid_before_ns(0)? NO<br/>(strict >)
    Note over S: Stale ticket from old auth — actually rejected here

    Note over C,Adm: New scenario: Alice has fresh ticket с original_auth_at_ns = T=10

    C->>S: resume { ticket family_id=A, counter=5, original_auth_at_ns=10, ... }
    Note over S: Step 9: 10 > 0 (current tickets_invalid_before_ns) → pass

    par Concurrent
        Adm->>S: updateUser { user: "alice", roles: ["readonly"] }
        Note over S,DB: Step 1: update roles<br/>Step 2: tickets_invalid_before_ns = 11<br/>Step 3: persist barrier
    and
        Note over S: Resume continues:<br/>Step 10: downgrade check pass<br/>Step 11: atomic CAS family_counter<br/>Step 12: create new Session (created_at_ns = 11)<br/>(Note: created_at_ns может быть BEFORE adm's set or AFTER — race!)
    end

    alt Resume created session BEFORE updateUser snapshot
        Note over Sess: Session(created_at_ns=11) escape'нула snapshot
    else Resume created session AFTER snapshot
        Note over Sess: Session killed by updateUser
    end

    Note over C: Если escaped — отправляет first request

    C->>S: { sid: new_session_id, req: ... }

    rect rgb(220, 255, 220)
    Note over S,Sess: §7.5 Per-request session validity check (NORMATIVE):<br/>if session.created_at_ns(11) <= user.tickets_invalid_before_ns(11):<br/>  ⚠ kicked!<br/>(strict ≤ catches simultaneous timestamps)
    S->>Sess: evict session (reason="invalidated")
    end

    S--xC: error { session_invalidated }
    Note over C: Client must full re-auth с new permissions

    Note over Sess: Audit event: session_evicted{reason="invalidated"}
```

## Defence layers summary

| Layer | Защита |
|---|---|
| **In-flight resume** (§5.4 step 9) | `original_auth_at_ns > tickets_invalid_before_ns` (strict >). Resume started **before** updateUser → passes. Resume started **after** persist barrier → fails immediately. |
| **Per-request validity check** (§7.5) | `session.created_at_ns <= tickets_invalid_before_ns` → kick. Catches sessions created **between** updateUser persist and snapshot/kill (escape window). |
| **Persist barrier** (Step 3) | Memory-store consistency: subsequent reads MUST see new tickets_invalid_before_ns. Without it, Step 4 snapshot might see stale value. |
| **Eager snapshot/kill** (Step 4-5) | Best-effort immediate eviction. Required для immediate TCP close. |
| **No-op semantic** | `roles=None` без changes → no invalidation. Защита от silent DoS via repeated noop. |

## Why both Steps 4-5 AND §7.5?

**Без §7.5** — Step 4-5 alone insufficient: race window между Step 3 (persist) и Step 4 (snapshot) позволяет new sessions slip through.

**Без Step 4-5** — §7.5 alone: session lives до next request. Idle session с stale permissions могла бы сидеть до 30 минут. Step 4-5 = best-effort immediate close.

**Both вместе:**
- Step 4-5: immediate kill для observable sessions
- §7.5: catch-all для escape sessions на их first subsequent request

Cost для §7.5 — один `u64 ≤` compare per request. Тривиально.

## Аналогичные операции

Те же двухуровневые защиты применяются для:
- `kickSession` (§12.4) — устанавливает `tickets_invalid_before_ns = now_ns`
- `revokeUserTickets` admin (SESSION_RESUMPTION §7.2) — same
