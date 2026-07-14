# 02 — Session Resumption

Fast reconnect (~10ms vs ~2s Argon2id) через encrypted ticket. См. SESSION_RESUMPTION.md.

```mermaid
sequenceDiagram
    autonumber
    participant C as Client
    participant T as TLS 1.3
    participant S as Server
    participant CC as consumed_counters<br/>DashMap
    participant DB as SystemStore<br/>(durable)

    Note over C: Имеет ticket из предыдущей auth_ok<br/>(memory-only для browser, в файле для CLI)

    C->>T: TCP/WSS connect (any supported transport)
    T->>T: TLS 1.3 handshake

    C->>S: resume { ticket: ticket_wire,<br/>client_nonce(32),<br/>binding_mode_now,<br/>channel_binding_now }

    rect rgb(255, 240, 220)
    Note over S: Step 1: Parse envelope (version, nonce, ciphertext_len, ciphertext, tag)
    Note over S: Step 2: Validate envelope.version supported
    Note over S: Step 3: aad = "SHAMIR-TICKET-v1" || u8(envelope.version)
    Note over S: Step 4: AES-256-GCM decrypt with current ticket_key<br/>fail → try previous ticket_key<br/>both fail → resumption_failed
    Note over S: Step 5: Parse ticket_plain (canonical msgpack)
    Note over S: Step 6: ticket_plain.version == envelope.version (defense-in-depth)
    Note over S: Step 7: ticket_plain.expires_at_ns > now_ns
    end

    S->>DB: lookup user by ticket_plain.user_id

    alt user не существует
        S--xC: error { resumption_failed }
    else user exists
        Note over S: Step 9: ticket_plain.original_auth_at_ns ><br/>  user.tickets_invalid_before_ns<br/>(СТРОГОЕ >, не >= — защита от 1ns race)
    end

    rect rgb(255, 230, 230)
    Note over S: Step 10: Anti-downgrade check<br/>binding_strength(now) >= binding_strength(at_auth)<br/>{ plain=0, browser=1, tls_exporter=2 }<br/>+ allow_browser_ticket_upgrade config check
    end

    rect rgb(230, 240, 255)
    Note over S,CC: Step 11: ATOMIC compare-and-swap<br/>key = (user_id, ticket_family_id)<br/>if ticket.family_counter > consumed_counters[key]:<br/>    consumed_counters[key] = ticket.family_counter
    S->>CC: atomic update consumed_counters[(user_id, family_id)]
    CC-->>S: ok / replay_detected
    end

    alt replay (counter <= consumed)
        S--xC: error { resumption_failed }
    else fresh counter
        S->>DB: SYNCHRONOUS DURABLE persist counter<br/>(fsync + storage engine durability —<br/>SQLite PRAGMA synchronous=FULL, ext4 barrier=1)
        DB-->>S: persisted
        Note over S: Step 12: Создать новую Session<br/>(created_at_ns = now_ns,<br/>binding_mode = binding_mode_now,<br/>channel_binding_at_auth = channel_binding_now,<br/>permissions = re-fetch CURRENT roles<br/>from directory by user_id —<br/>ticket carries NO auth snapshot<br/>per task #558)
        S->>C: resume_ok { session_id(new),<br/>expires_at_ns,<br/>resumption_ticket?(family_id same, counter+1),<br/>resumption_expires_at_ns? }
    end

    Note over C,S: Active session
```

## Ticket lineage (multi-device)

```mermaid
graph TD
    Initial[Full SCRAM<br/>family_id = random_A<br/>counter = 1] --> R1[refreshTicket<br/>family_id = random_A<br/>counter = 2]
    R1 --> R2[refreshTicket<br/>family_id = random_A<br/>counter = 3]
    R2 --> R3[refreshTicket<br/>counter = 4]
    
    Initial2[Full SCRAM от другого device<br/>family_id = random_B<br/>counter = 1] --> R1B[refreshTicket<br/>family_id = random_B<br/>counter = 2]
    
    Note1[Laptop refresh advances family_A counter →<br/>не invalidates Mobile family_B ticket]
    
    R3 -.no impact.- R1B
```

## Anti-replay invariants

| Сценарий | Защита |
|---|---|
| Same ticket дважды | `family_counter > consumed_counters[(user, family)]` → second fail |
| Stolen ticket replay в другой family | Different `family_id` → independent counter |
| Counter rollback при server crash | Synchronous fsync перед `resume_ok` (НЕ batched) |
| Backup restore с старым counter | `revokeAllTickets` mandatory (IMPL §5.7) |
| AAD tampering | GCM tag покрывает ciphertext+plaintext+aad |
| Ticket key compromise | Rotation 24h + emergency `revokeAllTickets` |
| Stale ticket после `kickSession` / `updateUser` | `original_auth_at_ns > tickets_invalid_before_ns` (strict >) |

## Identity rotation interaction

Если ticket issued под previous Ed25519 keypair AND `transition_until_ns > now_ns`:
- **v1:** server **rejects** resume → forces full re-auth → client получает `rotation_in_progress` в auth_ok → handles per AUTH §6.5
- См. SESSION_RESUMPTION §5.7
