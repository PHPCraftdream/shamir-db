# 11 — Component Overview (architecture)

High-level архитектура: clients, transports, server components, persistence.

## Clients ↔ Server transports

```mermaid
flowchart LR
    subgraph "Native Clients"
        CLI["CLI Tool<br/>(Rust SDK)"]
        Embed["Embedded<br/>(Rust SDK)"]
    end
    
    subgraph "Browser"
        SPA["Admin UI SPA<br/>+ argon2-browser WASM<br/>+ noble-ed25519"]
    end
    
    subgraph "Server: ShamirDB"
        L1[":7331 TCP+TLS<br/>profile=tls<br/>binding_mode=0x01"]
        L2[":7332 WSS native<br/>profile=tls<br/>endpoint=/shamir/v1<br/>binding_mode=0x01"]
        L3[":7333 WSS browser<br/>profile=tls_browser<br/>endpoint=/shamir/v1/browser<br/>binding_mode=0x02"]
        L4[":7334 TCP plain<br/>profile=plain<br/>binding_mode=0x00<br/>LOOPBACK ONLY"]
        AdminUI[":7335 HTTPS admin UI<br/>static + Bearer REST"]
        
        AuthEngine["Auth Engine<br/>SCRAM + Ed25519<br/>+ HKDF anti-enum<br/>+ constant-time discipline"]
        
        L1 --> AuthEngine
        L2 --> AuthEngine
        L3 --> AuthEngine
        L4 --> AuthEngine
        
        AdminUI -->|serves bundle| SPA
    end
    
    CLI -->|out-of-band pin| L1
    CLI -->|out-of-band pin| L2
    Embed -->|loopback only| L4
    SPA -->|embedded pin in bundle| L3
```

## Server internal components

```mermaid
flowchart TB
    subgraph "Network Layer"
        Listeners["Listeners<br/>(per-binding_mode policy)"]
    end
    
    subgraph "Auth Layer"
        SCRAM["SCRAM Verifier<br/>(real/fake constant-time)"]
        Identity["Ed25519 Signer<br/>(identity_sig +<br/>rotation orphan recovery)"]
        Bootstrap["Bootstrap Handler<br/>(CAS + invariants)"]
        ChangePw["changePassword<br/>(no server Argon2id)"]
    end
    
    subgraph "Session Layer"
        Sessions["sessions: DashMap<sid, Session><br/>per-request validity check (§7.5)"]
        Resume["Resume Handler<br/>(per-family counter +<br/>anti-downgrade)"]
        AdminCmds["Admin Commands<br/>updateUser / kickSession /<br/>rotateServerIdentity / etc"]
    end
    
    subgraph "Defence Layer"
        RateLimit["Rate Limit<br/>per-subnet sliding window<br/>(warmup /4 first 60s)"]
        Backoff["Backoff<br/>per (subnet, user_hash)<br/>100ms × 2^N, cap 30s"]
        Lockout["Lockout<br/>persisted, batched 5s<br/>50 fails/hour threshold"]
        Semaphore["Argon2id Semaphore<br/>derived from RAM"]
        Latency["Latency Padding<br/>50ms floor + jitter"]
    end
    
    subgraph "Audit Layer"
        AuditLog["Append-only Log<br/>HMAC-chained +<br/>truncation defence<br/>(checkpoint 60s/1000)"]
        Metrics["Prometheus Metrics<br/>+ alerts"]
    end
    
    subgraph "SystemStore (persistent)"
        Users["__system__/users/{id}"]
        Meta["__system__/server_meta<br/>(secrets + ed25519 keys +<br/>ticket_key + audit_chain_key)"]
        AuditDB["__system__/audit_log"]
    end
    
    Listeners --> SCRAM
    SCRAM --> Identity
    SCRAM --> Sessions
    Bootstrap --> Users
    Bootstrap --> Meta
    ChangePw --> Sessions
    Resume --> Sessions
    AdminCmds --> Sessions
    AdminCmds --> Users
    AdminCmds --> Meta
    
    SCRAM -.constant-time.- Latency
    SCRAM --> RateLimit
    SCRAM --> Backoff
    SCRAM --> Lockout
    SCRAM --> Semaphore
    
    Sessions --> AuditLog
    Bootstrap --> AuditLog
    ChangePw --> AuditLog
    AdminCmds --> AuditLog
    
    AuditLog --> AuditDB
    Sessions --> Users
    Resume --> Meta
```

## Trust boundaries

```mermaid
flowchart LR
    subgraph "Untrusted: Public Internet"
        InetClients["Random clients<br/>(authentic + malicious)"]
    end
    
    subgraph "Trusted: Auth Network Path"
        TLS["TLS 1.3 (rustls)<br/>+ Ed25519 server identity<br/>+ channel binding"]
    end
    
    subgraph "Trusted: Server Process"
        SecretsRAM["RAM secrets<br/>server_secret, lockout_secret,<br/>ed25519_priv, ticket_key,<br/>audit_chain_key"]
        AuthLogic["Auth + Session logic"]
    end
    
    subgraph "Trusted: Persistent Storage"
        DB["SystemStore<br/>(durable, fsync, chmod 600,<br/>backup encrypted-at-rest)"]
    end
    
    subgraph "External (out-of-band)"
        OOB["Out-of-band pin distribution<br/>(operator → users)"]
        Backup["Backup secure storage"]
    end
    
    InetClients -- "passwords / tickets / proofs" --> TLS
    TLS -- "verified channel" --> AuthLogic
    AuthLogic -- "stored_keys / server_keys / etc" --> DB
    AuthLogic -.lives in.- SecretsRAM
    DB -.encrypted.- Backup
    OOB -- "Ed25519 server fingerprint" --> InetClients
    
    note1["⚠ A11: Single-process trusted server<br/>RCE → all secrets compromised<br/>(documented limitation §4.13)"]
    SecretsRAM -.- note1
```

## Adversary mapping (см. SECURITY_MODEL §1)

| Adversary | Защищён | Документировано как limitation |
|---|---|---|
| A1 Passive observer | TLS 1.3 | — |
| A2 Active MITM | TLS + Ed25519 pin + channel binding | — |
| A3 Offline DB snapshot | Argon2id memory-hard | — |
| A4 Live RAM read | mlock / disable_core_dumps best-effort | Partial |
| A5 Malicious admin | Audit log HMAC chain (forensics, not prevention) | Out of scope |
| A6 Compromised client | known_hosts MAC | Partial |
| A7 Supply chain | — | Out of scope |
| A8 Spectre/cache | — | Out of scope (acknowledged) |
| A9 Hardware tamper | — | Out of scope |
| A10 DoS | Multi-layer (rate-limit, backoff, lockout, semaphore, padding) | — |
| A11 Single-process RCE | mitigations only (mlock, etc) | Documented (§4.13) |
| A12 Compromised origin (browser) | Limited (embedded pin защищает narrow case) | Documented (§4.9) |
