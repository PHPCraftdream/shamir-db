# 04 — Change Password

Self-service смена пароля внутри активной сессии. Two-step с fresh challenge, БЕЗ серверного Argon2id (anti-DoS amplification). См. AUTH §12.5.

```mermaid
sequenceDiagram
    autonumber
    participant C as Client
    participant S as Server
    participant Sess as Session<br/>(in-memory)
    participant DB as SystemStore

    Note over C,S: Active authenticated session (session_id, channel_binding_at_auth)

    C->>S: changePasswordChallenge { client_nonce_cp(32) }

    rect rgb(255, 240, 220)
    Note over S,Sess: Generate fresh challenge:<br/>server_nonce_cp = random(32)<br/>Store в session.pending_changepw_challenge:<br/>  { server_nonce_cp, client_nonce_cp,<br/>    issued_at_ns = now_ns }<br/>SHOULD expire после CHANGEPW_CHALLENGE_TTL = 5 min<br/>(защита от stale state в multi-tab)<br/><br/>⚠ Single in-flight per session_id —<br/>повторный changePasswordChallenge invalidates previous
    S->>Sess: store pending_changepw_challenge
    end

    S->>C: challenge_cp { server_nonce_cp, salt(current),<br/>memory_kb, time, parallelism, argon2_version (current user kdf) }

    rect rgb(230, 240, 255)
    Note over C: Compute auth_message_cp (header "SHAMIR-CHGPW-v1"):<br/>auth_message_cp =<br/>  "SHAMIR-CHGPW-v1" ||<br/>  u16_be(byte_len(username)) || username_nfc ||<br/>  session_id(32) ||<br/>  client_nonce_cp(32) || server_nonce_cp(32) ||<br/>  salt(16) || u32_be(memory_kb) || u32_be(time) ||<br/>  u32_be(parallelism) || u8(argon2_version) ||<br/>  u8(transport_kind) || u8(binding_mode) ||<br/>  channel_binding_at_auth(32)<br/><br/>Derive proof_old (Argon2id ~2s):<br/>salted_old = Argon2id(old_password, salt, kdf_params)<br/>client_key_old = HMAC(salted_old, "Client Key")<br/>stored_key_old = SHA256(client_key_old)<br/>client_proof_old = client_key_old XOR HMAC(stored_key_old, am_cp)<br/><br/>Derive new material (Argon2id ~2s second time):<br/>new_salt = random(16)<br/>new_salted = Argon2id(new_password, new_salt, server_defaults)<br/>new_stored_key = SHA256(HMAC(new_salted, "Client Key"))<br/>new_server_key = HMAC(new_salted, "Server Key")<br/>zeroize: old_password, salted_old, client_key_old,<br/>         new_password, new_salted, new_client_key
    end

    C->>S: changePassword { client_proof_old(32),<br/>new_salt(16), new_stored_key(32), new_server_key(32) }
    Note over C: kdf_params от клиента игнорируются —<br/>server применяет current defaults

    rect rgb(255, 230, 230)
    Note over S,Sess: Verify:<br/>1. Lookup session.pending_changepw_challenge — must be present<br/>2. (now_ns - pending.issued_at_ns) ≤ CHANGEPW_CHALLENGE_TTL<br/>3. Reconstruct auth_message_cp (server-side)<br/>4. client_signature = HMAC(user.stored_key, auth_message_cp)<br/>5. recovered = client_proof_old XOR client_signature<br/>6. ok = ConstantTimeEq(SHA256(recovered), user.stored_key)<br/><br/>⚠ NO server-side Argon2id (anti-DoS)
    end

    alt ok == true
        rect rgb(220, 255, 220)
        Note over S,DB: Atomic (mutex per user):<br/>1. Update user record:<br/>   salt = new_salt, stored_key = new_stored_key,<br/>   server_key = new_server_key,<br/>   kdf_params = server_defaults,<br/>   updated_at_ns = now_ns<br/>2. Set tickets_invalid_before_ns = now_ns<br/>3. Clear session.pending_changepw_challenge (single-use)<br/>4. Snapshot all sessions of user, kill them (включая текущую)<br/>5. Audit event: password_changed
        S->>DB: persist atomic
        S->>Sess: kill all sessions of user
        end
        S->>C: ok {}
        S-xC: connection close
    else fail (proof invalid, challenge expired, no pending state)
        S->>Sess: clear pending_changepw_challenge<br/>(защита от brute-force через retry)
        S--xC: error { authentication_failed }
        Note over S: Generic error
    end

    Note over C: Client must full re-auth с new password
```

## Multi-tab semantics

```mermaid
sequenceDiagram
    participant TabA as Tab A
    participant TabB as Tab B
    participant S as Server

    Note over TabA,TabB: Same session_id (shared)

    TabA->>S: changePasswordChallenge { client_nonce_cp_A }
    Note over S: pending = { nonce_A, ... }
    S->>TabA: challenge_cp { server_nonce_cp_A }

    TabB->>S: changePasswordChallenge { client_nonce_cp_B }
    Note over S: ⚠ INVALIDATES pending_A<br/>pending = { nonce_B, ... }
    S->>TabB: challenge_cp { server_nonce_cp_B }

    TabA->>S: changePassword { proof using nonce_A }
    Note over S: server reconstructs auth_message_cp с nonce_B<br/>SCRAM proof не сходится → fail
    S--xTabA: error { authentication_failed }

    TabB->>S: changePassword { proof using nonce_B }
    Note over S: matches → success
    S->>TabB: ok {}
    S-xTabA: kill all sessions
    S-xTabB: kill all sessions
```

## Свойства

- **Plain password не покидает client** ни на одном шаге
- **No server-side Argon2id** в verify path (DoS-amp защита)
- **Session-bound** через `session_id` + `channel_binding_at_auth` в auth_message_cp
- **Single in-flight challenge** per session — multi-tab race well-defined
- **TTL 5 min** на pending challenge (защита от stale state)
- **All sessions killed** включая текущую (security boundary при rotation password)
- **Per-user mutex** — concurrent changePassword serialized
