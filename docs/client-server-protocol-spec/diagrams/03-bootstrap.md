# 03 — Bootstrap (first admin creation)

Создание первого superuser. **Out-of-band pin mandatory**, browser bootstrap запрещён в v1. См. AUTH §11.

```mermaid
sequenceDiagram
    autonumber
    participant Op as Operator (out-of-band)
    participant S as Server
    participant TTY as stdout/file/cmd
    participant C as Client (native CLI)
    participant DB as SystemStore

    Note over S: First start, __system__/users пуст,<br/>bootstrap_token_hash IS NULL,<br/>superuser_ever_existed == false

    rect rgb(255, 240, 220)
    Note over S: Generate:<br/>bootstrap_token = random(32) с prefix "shbst1_"<br/>SHA256(token) → bootstrap_token_hash<br/>expires_at_ns = now_ns + bootstrap_token_ttl<br/>(default 1h, configurable 5min..24h)<br/>+ atomic store в server_meta
    end

    S->>TTY: Output token + SERVER_PUB_FINGERPRINT
    Note over TTY: Mode: tty | file:<path> | command:<cmd><br/>tty: только если isatty(stdout) AND не systemd-managed<br/>file: chmod 600, recommend tmpfs/ramdisk<br/>command: pipe в pass/age/gpg

    TTY->>Op: BOOTSTRAP TOKEN: shbst1_xxxxx<br/>SERVER_PUB_FINGERPRINT: base64url(SHA256(server_pub))

    Op->>C: Передаёт token + pin (manual transport)
    Note over Op,C: ⚠ Out-of-band channel:<br/>- physical transport<br/>- encrypted email<br/>- KMS-mediated<br/>- secure messenger<br/>НЕ через тот же сетевой канал что bootstrap!

    Note over C: URI: shamir+tcp://host:port?bootstrap=1&pin=base64url(SHA256(pub))<br/>Без pin — refuse слать token (защита от MITM)

    C->>S: TCP+TLS connect (binding_mode = 0x01 enforced)

    C->>S: bootstrap_hello { client_nonce(32) }
    Note over C,S: ≤ 256 bytes

    rect rgb(255, 240, 220)
    Note over S: Sign:<br/>identity_sig_bootstrap = Ed25519::sign(server_priv,<br/>  "SHAMIR-BOOTSTRAP-v1" ||<br/>  SHA256(server_pub_key) ||<br/>  u8(transport_kind) ||<br/>  tls_exporter(32) ||<br/>  client_nonce(32) ||<br/>  u64_be(server_time))
    end

    S->>C: bootstrap_challenge { server_pub_key,<br/>server_time,<br/>identity_sig_bootstrap }

    rect rgb(220, 255, 220)
    Note over C: Verify (любой fail → disconnect, plain pw НЕ уходит):<br/>(a) ConstantTimeEq(SHA256(server_pub_key), pinned_hash) — pin check<br/>(b) Ed25519::verify_strict(...) — подпись valid<br/>(c) ConstantTimeEq(client_nonce_in_payload, sent_client_nonce)<br/>     — anti-replay challenge другому клиенту<br/>(d) abs(now - server_time) ≤ 60 sec — clock anomaly
    end

    rect rgb(230, 240, 255)
    Note over C: Local derivation (как §3.3, ~2s Argon2id):<br/>salt = random(16)<br/>salted_pw = Argon2id(password, salt, server_kdf_params)<br/>client_key = HMAC(salted_pw, "Client Key")<br/>stored_key = SHA256(client_key)<br/>server_key = HMAC(salted_pw, "Server Key")<br/>zeroize: password, salted_pw, client_key
    end

    C->>S: bootstrap { token, user, salt,<br/>stored_key, server_key,<br/>memory_kb, time, parallelism, argon2_version }

    rect rgb(255, 230, 230)
    Note over S,DB: Atomic (mutex + CAS):<br/>1. expires_at_ns > now_ns<br/>2. ConstantTimeEq(SHA256(token), bootstrap_token_hash)<br/>3. kdf_params == server_defaults<br/>4. username unique (PRECIS + NFC normalized)<br/>5. Create user с роль "superuser"<br/>6. Set bootstrap_token_hash = NULL,<br/>   bootstrap_token_expires_at_ns = NULL,<br/>   superuser_ever_existed = true<br/>Invariant: bootstrap_token_hash IS NULL ⇔ superuser EXISTS
    S->>DB: persist user + meta atomically
    DB-->>S: ok
    end

    alt success
        S->>C: ok { user_id }
        Note over S: Audit event: bootstrap_used
    else fail (token expired, replay, kdf mismatch, username collision)
        S--xC: error { bootstrap_failed }
        Note over S: Generic — не раскрывает причину
    end
```

## Когда bootstrap **не** работает

| Условие | Reason |
|---|---|
| `superuser_ever_existed == true` | Защита от silent re-bootstrap при corrupted backup |
| `binding_mode != 0x01` (нет TLS exporter) | Browser bootstrap запрещён в v1 |
| `addr` не loopback AND profile=plain | Plain TCP loopback не поддерживает bootstrap |
| Username collision | Operator должен сначала удалить collision |
| `kdf_params != server_defaults` | Защита от malicious client |
| Token expired | TTL configurable 5min..24h |
| Token already used (CAS fail) | Single-use enforced atomically |
| Pin mismatch | Plain password НЕ покидает client |

## Recovery procedures

- **Lost admin password:** `shamir-server --regen-bootstrap --confirm` (требует stop сервера + физический доступ)
- **Token leak via logs:** TTL 1h спасает; alert на bootstrap_used event если source unexpected
- **Token file orphan:** server cleanup при startup (audit `bootstrap_token_file_orphan_cleaned`)
