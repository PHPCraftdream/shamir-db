# 01 — Initial Auth (full SCRAM)

Full SCRAM-Argon2id handshake с channel binding и Ed25519 server identity. См. AUTH_PROTOCOL §2-§5.

```mermaid
sequenceDiagram
    autonumber
    participant C as Client
    participant T as TLS 1.3
    participant S as Server
    participant DB as SystemStore

    Note over C,T: Pre-condition: client имеет pinned_hash<br/>(URI param, known_hosts, или embedded в bundle)

    C->>T: TCP/TCP+TLS connect
    T->>T: TLS 1.3 handshake (cert не верифицируется по CA)
    Note over C,S: TLS exporter извлечён обеими сторонами<br/>(label="EXPORTER-ShamirDB-AUTH-v1", L=32)

    C->>S: auth_init { user, client_nonce(32), binding_mode=0x01, version=1 }

    rect rgb(255, 230, 230)
    Note over S: ⚠ Pre-Argon2id binding_mode policy check<br/>(защита от DoS amplification)
    S->>S: Reject silently если binding_mode не входит в listener policy
    end

    S->>DB: lookup user by username_nfc
    alt user exists
        DB-->>S: { stored_key, server_key, salt, kdf_params }
    else user not found
        Note over S: HKDF fake_blob (constant-time):<br/>HKDF(server_secret, "SHAMIR-FAKE-SALT-v1", username, L=80)
    end

    S->>C: challenge { salt, kdf_params, server_nonce(32) }

    rect rgb(230, 240, 255)
    Note over C: Argon2id (~2s, 128 MB):<br/>salted_pw = Argon2id(password, salt, kdf_params)<br/>client_key = HMAC(salted_pw, "Client Key")<br/>server_key = HMAC(salted_pw, "Server Key")<br/>stored_key = SHA256(client_key)<br/>auth_message = §4.1 (149 bytes для default params)<br/>client_signature = HMAC(stored_key, auth_message)<br/>client_proof = client_key XOR client_signature<br/>zeroize: password, salted_pw, client_key
    end

    C->>S: client_proof { bytes(32) }

    rect rgb(255, 240, 220)
    Note over S: Verify (constant-time, real OR fake path):<br/>client_signature = HMAC(stored_key OR fake_stored_key, auth_message)<br/>recovered = client_proof XOR client_signature<br/>ok = ConstantTimeEq(SHA256(recovered), stored_key)<br/><br/>ALWAYS compute (anti-timing):<br/>server_signature = HMAC(server_key OR fake_server_key, auth_message)<br/>session_id = random(32)<br/>identity_input = "SHAMIR-IDENTITY-v1" || SHA256(server_pub) ||<br/>                 transport_kind || binding_mode || tls_exporter ||<br/>                 auth_message || session_id || u64_be(expires_at_ns)<br/>identity_sig = Ed25519::sign(server_priv, identity_input)
    end

    alt ok == true
        S->>DB: persist session, reset auth_failures[(subnet, user_hash)]
        S->>C: auth_ok { server_signature, server_pub_key, identity_sig,<br/>session_id, expires_at_ns,<br/>resumption_ticket?, rotation_in_progress? }
    else ok == false
        S->>DB: increment auth_failures[(subnet, user_hash)]<br/>backoff: 100ms × 2^N
        Note over S: Latency padding до target_constant_time<br/>(50ms floor + uniform[0,25] jitter)
        S--xC: error { authentication_failed }
    end

    rect rgb(220, 255, 220)
    Note over C: Verify (mutual auth, любой fail → disconnect):<br/>1. ConstantTimeEq(HMAC(server_key, auth_message), server_signature)<br/>2. SHA256(server_pub_key) == pinned_hash (или TOFU save)<br/>3. Ed25519::verify_strict(server_pub_key, identity_input, identity_sig)<br/>4. Если rotation_in_progress present → §6.5 handling
    end

    Note over C,S: Active session — все запросы { sid, req } ↔ { rid, res }
```

## Ключевые свойства

- **Pre-Argon2id binding_mode check** (шаг 4) — защита от DoS amplification
- **Constant-time fake path** (шаг 6) — anti-enumeration через HKDF
- **All three crypto operations always computed** (шаг 9) — устраняет timing oracle
- **Latency padding** на negative path — устраняет real/fake distinguisher на microsecond уровне
- **Reset on success** — auth_failures очищается для (subnet, user_hash)
- **Mutual auth** — обе стороны верифицируют (SCRAM proof + Ed25519 identity + pin)

## Edge cases

- **Pre-state GC:** state без proof через `HANDSHAKE_TIMEOUT` (≥15s) → drop
- **Argon2id semaphore exhaustion:** `MAX_CONCURRENT_ARGON2=64` → `server_busy`
- **Rate limit per subnet:** `RATE_LIMIT_AUTH_INIT_PER_SUBNET=10/sec` → `rate_limited`
- **Lockout** (50 fails/час per (subnet, user)): silent (генерик `authentication_failed`)
- **`rotation_in_progress` в auth_ok** (during overlap window) → клиент handles via §6.5
