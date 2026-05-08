# 10 — Session Lifecycle (state diagram)

State machine от TCP connect до session eviction. См. AUTH §7.

```mermaid
stateDiagram-v2
    [*] --> TLS_Handshake: TCP/WSS connect
    
    TLS_Handshake --> PreAuth: TLS 1.3 OK
    TLS_Handshake --> [*]: TLS fail
    
    state PreAuth {
        [*] --> AwaitInit
        AwaitInit --> AwaitProof: auth_init received,<br/>handshake_state created,<br/>(pre-Argon2id binding_mode check)
        AwaitProof --> AwaitProof: challenge sent
        AwaitInit --> AwaitInit: rate_limited / server_busy<br/>(stays in PreAuth)
    }
    
    PreAuth --> [*]: HANDSHAKE_TIMEOUT (≥15s)<br/>handshake_state GC
    PreAuth --> [*]: authentication_failed<br/>(backoff incremented)
    PreAuth --> [*]: TLS layer disconnect
    
    PreAuth --> ActiveSession: client_proof verified,<br/>auth_ok sent,<br/>session created<br/>(reset auth_failures)
    
    state ActiveSession {
        [*] --> Idle
        Idle --> Processing: request {sid, req}
        Processing --> Idle: response sent
        
        Idle --> ChangePwPending: changePasswordChallenge<br/>(stores pending_changepw_challenge)
        ChangePwPending --> Idle: challenge expired (5min)<br/>OR proof verification fail<br/>(success path terminates session — see outer)
        
        note right of Processing
            §7.5 Per-request validity check:
            if created_at_ns <= 
               user.tickets_invalid_before_ns
            → kicked with session_invalidated
        end note
    }
    
    ActiveSession --> [*]: idle_ttl_expired (30 min)
    ActiveSession --> [*]: max_age_expired (24h)
    ActiveSession --> [*]: logout
    ActiveSession --> [*]: kickSession admin
    ActiveSession --> [*]: changePassword (kills self)
    ActiveSession --> [*]: updateUser (если this user) → §7.5 kicks
    ActiveSession --> [*]: identity_rotation (если client refuses pin update)
    ActiveSession --> [*]: server shutdown
    
    ActiveSession --> Disconnected: TCP/WSS layer close
    Disconnected --> ActiveSession: resume (within 5s grace)
    Disconnected --> [*]: 5s grace expired<br/>session evicted
```

## State transitions: terminal events

| Event | Reason in audit | Recovery |
|---|---|---|
| `HANDSHAKE_TIMEOUT` | `auth_aborted{reason="timeout"}` | Client retry |
| `authentication_failed` | `auth_failed` (rate-limited) | Client checks password |
| `idle_ttl_expired` | `session_evicted{reason="idle"}` | Resume via ticket OR full auth |
| `max_age_expired` | `session_evicted{reason="max_age"}` | Full auth (24h limit absolute) |
| `logout` | `session_evicted{reason="logout"}` | Explicit user action |
| `kickSession admin` | `kick_session` | Admin action, full re-auth required |
| `changePassword` | `password_changed` + `session_evicted{reason="kicked"}` | Full re-auth с new password |
| `updateUser` (per §7.5) | `session_evicted{reason="invalidated"}` | Full re-auth с updated permissions |
| `disconnect (no resume)` | `session_evicted{reason="disconnect"}` | Resume или full auth |
| `max_sessions overflow` | `session_evicted{reason="max_sessions_lru"}` | Older session killed |

## Concurrent state per (user_id, family_id)

```mermaid
stateDiagram-v2
    [*] --> Active: full SCRAM auth<br/>family_id = random,<br/>counter = 1
    
    Active --> Active: refreshTicket<br/>same family_id,<br/>counter++
    
    Active --> Consumed: resume successful<br/>consumed_counters[(user, family)] = N
    
    Consumed --> Active: NEW issued ticket с counter = N+1
    
    Active --> [*]: tickets_invalid_before_ns updated<br/>(kickSession / changePassword / updateUser /<br/>revokeUserTickets / revokeAllTickets)
    
    Active --> [*]: original_auth_at_ns + 24h expired<br/>RESUMPTION_MAX_CHAIN_AGE
    
    Active --> [*]: ticket_key rotated без overlap<br/>(emergency revokeAllTickets)
    
    Active --> [*]: GC (background, 60s):<br/>last_observed_at + 24h < now
```

## Per-listener state machine (server-side)

```mermaid
stateDiagram-v2
    [*] --> Listening: server start
    
    state Listening {
        [*] --> AcceptingConnections
        AcceptingConnections --> AcceptingConnections: handshake state in DashMap
    }
    
    Listening --> Listening: warmup_window (60s after restart)<br/>rate_limit /4
    
    Listening --> Identity_Rotation_Active: rotateServerIdentity called<br/>(rotation_until_ns set,<br/>previous_priv kept for 7 days)
    
    Identity_Rotation_Active --> Identity_Rotation_Active: rotateServerIdentity rejected<br/>(rotation_in_progress_already error)
    
    Identity_Rotation_Active --> Listening: now > rotation_until_ns<br/>(zeroize previous_priv)
    
    Listening --> ShutdownInProgress: SIGTERM/SIGINT
    
    state ShutdownInProgress {
        [*] --> SyncFlush: synchronous flush<br/>lockout_state +<br/>consumed_counters<br/>(MUST per §1.3 IMPL)
        SyncFlush --> [*]
    }
    
    ShutdownInProgress --> [*]: clean exit
    
    Listening --> [*]: SIGKILL / crash<br/>(up to 5s lockout state lost,<br/>backup restore = mandatory revokeAllTickets)
```
