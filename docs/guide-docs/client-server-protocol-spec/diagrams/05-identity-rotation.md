# 05 — Server Identity Rotation

Ed25519 server identity rotation с overlap window и orphan client recovery. См. AUTH §6.4-§6.5, §12.2.

## Часть A: Active session (broadcast)

```mermaid
sequenceDiagram
    autonumber
    participant Adm as Admin (active session)
    participant S as Server
    participant DB as SystemStore
    participant C1 as Client 1<br/>(active, pinned old_pub)
    participant C2 as Client 2<br/>(active, pinned old_pub)

    Adm->>S: rotateServerIdentity {} (admin command)

    rect rgb(255, 230, 230)
    Note over S: Pre-condition (NORMATIVE):<br/>now_ns < server_ed25519_rotation_until_ns<br/>→ reject с rotation_in_progress_already<br/>(защита от двойной rotation lockout)
    end

    rect rgb(255, 240, 220)
    Note over S,DB: Atomic store:<br/>previous_pub = current_pub<br/>previous_priv = current_priv<br/>current_pub, current_priv = Ed25519::generate()<br/>rotation_until_ns = now_ns + 7 days
    S->>DB: persist atomic
    DB-->>S: ok
    end

    S->>Adm: ok { new_pub, transition_until_ns }

    par Per-recipient broadcast (NO sig caching)
        rect rgb(230, 240, 255)
        Note over S: signed_by_old_C1 = Ed25519::sign(previous_priv,<br/>  "SHAMIR-ROTATE-v1" ||<br/>  SHA256(old_pub) ||<br/>  new_pub ||<br/>  u64_be(transition_until_ns) ||<br/>  recipient_session_id_C1)
        end
        S->>C1: identity_rotation { old_pub, new_pub,<br/>transition_until_ns,<br/>recipient_session_id = C1.sid,<br/>signed_by_old_C1 }
    and
        S->>C2: identity_rotation { ..., recipient_session_id = C2.sid,<br/>signed_by_old_C2 (different sig!) }
    end

    rect rgb(220, 255, 220)
    Note over C1: Verify в указанном порядке (any fail → disconnect):<br/>(a) ConstantTimeEq(SHA256(old_pub), pinned_hash)<br/>(b) ConstantTimeEq(recipient_session_id, my_session_id)<br/>(c) Ed25519::verify_strict(old_pub, payload, signed_by_old)<br/>(d) transition_until_ns > now_ns + 60s — overlap valid<br/>(e) transition_until_ns ≤ now_ns + 7 days + 1h skew<br/>     (HIGH-2: upper bound vs leaked-key forge)
    end

    alt Interactive client
        Note over C1: Show prompt:<br/>"Server identity rotated.<br/>OLD: <fingerprint old_pub><br/>NEW: <fingerprint new_pub><br/>Until: <transition_until><br/>Confirm? [y/N]"
        Note over C1: User confirms → update pin to SHA256(new_pub)
    else Non-interactive (CI/script)
        Note over C1: fail-closed без --accept-rotation flag<br/>(operator must set flag explicitly)
    end

    Note over S: Через transition_until_ns<br/>background task: zeroize previous_priv,<br/>previous_pub = NULL,<br/>rotation_until_ns = NULL<br/>→ rotateServerIdentity снова доступен
```

## Часть B: Orphan client recovery (offline во время rotation)

```mermaid
sequenceDiagram
    autonumber
    participant C as Orphan Client<br/>(pinned old_pub, was offline)
    participant S as Server<br/>(during overlap window)
    participant U as User (interactive)

    Note over C: Имеет pinned_hash = SHA256(old_pub)<br/>Не получал identity_rotation event (был offline)

    C->>S: TCP+TLS connect, full SCRAM handshake (auth_init → ... → client_proof)

    rect rgb(255, 240, 220)
    Note over S: Server detect overlap window:<br/>server_ed25519_rotation_until_ns > now_ns AND previous_pub != NULL<br/><br/>Compute identity_sig (signed by current_priv)<br/>Compute identity_sig_previous (signed by previous_priv,<br/>  ON SAME identity_input — byte-exact с identity_sig)<br/><br/>Compute rotation_proof = Ed25519::sign(previous_priv,<br/>  "SHAMIR-ROTATE-PROOF-v1" ||<br/>  SHA256(previous_pub) ||<br/>  current_pub ||<br/>  u64_be(transition_until_ns))
    end

    S->>C: auth_ok {<br/>  server_signature, server_pub_key=current_pub,<br/>  identity_sig (signed by current),<br/>  session_id, expires_at_ns,<br/>  rotation_in_progress: {<br/>    previous_pub,<br/>    identity_sig_previous (signed by previous),<br/>    transition_until_ns,<br/>    rotation_proof<br/>  }<br/>}

    rect rgb(220, 255, 220)
    Note over C: Step 1: Ed25519::verify_strict(current_pub, identity_input, identity_sig) ✓
    Note over C: Step 2: SHA256(current_pub) == pinned_hash? NO (pinned = old)
    Note over C: Step 3: SHA256(previous_pub) == pinned_hash? YES (pinned = SHA256(old_pub) = previous_pub)<br/>  AND rotation_in_progress present →<br/>  Verify identity_sig_previous против previous_pub ✓<br/>  Verify rotation_proof против previous_pub ✓<br/>  Verify transition_until_ns > now_ns ✓<br/>  Verify transition_until_ns ≤ now_ns + 7d + 1h ✓
    end

    rect rgb(255, 230, 230)
    Note over C: ⚠ NEVER auto-update pin
    end

    alt Interactive
        C->>U: Show prompt:<br/>"Server identity rotated.<br/>Old fingerprint: <SHA256(previous_pub)><br/>New fingerprint: <SHA256(current_pub)><br/>Transition until: <date><br/>Verify out-of-band! Trust new identity? [y/N]"
        U->>C: confirm
        C->>C: update pin = SHA256(current_pub)
        Note over C: Continue session
    else Non-interactive (CI/script)
        Note over C: fail-closed → disconnect, exit code != 0<br/>operator must set --accept-rotation flag<br/>с awareness of security implications
    end
```

## ⚠ Security caveat: leaked previous_priv

`rotation_proof` valid против previous_pub доказывает только: **подписан кем-то с previous_priv**. НЕ доказывает legitimate server, если previous_priv был скомпрометирован.

**Атака:**

```mermaid
sequenceDiagram
    participant Att as Attacker<br/>(имеет leaked previous_priv +<br/>network MITM position)
    participant C as Orphan client<br/>(pinned old_pub)

    Att->>Att: Generate (att_pub, att_priv)
    Att->>Att: Forge identity_sig = sign(att_priv, identity_input_with_att_pub)
    Att->>Att: Forge rotation_proof = sign(LEAKED_previous_priv,<br/>  "SHAMIR-ROTATE-PROOF-v1" ||<br/>  SHA256(previous_pub) ||<br/>  att_pub ||<br/>  far_future_ts)
    Att->>Att: Forge identity_sig_previous = sign(LEAKED_previous_priv,<br/>  identity_input_with_att_pub)
    
    C->>Att: TCP+TLS connect (atтакующий MITMит)
    Att->>C: auth_ok с fake rotation_in_progress (att_pub as new)
    Note over C: Verify identity_sig vs att_pub ✓<br/>Verify rotation_proof vs previous_pub ✓<br/>(но previous_priv leaked!)
    Note over C: User prompt: "Trust new identity? [y/N]"
    
    alt User clicks Yes (без out-of-band verify)
        Note over C: Pin updated to SHA256(att_pub)<br/>Permanent MITM!
    else User verifies out-of-band
        Note over C: Operator detects fingerprint mismatch<br/>в bulletin → declines → disconnect
    end
```

## Mitigations

1. **Operators MUST verify через second channel** (signed announcement по email/GPG/etc) перед confirming
2. При подозрении на compromised previous_priv — использовать **emergency rotation** (`--identity-revoked` flag, IMPL §5.2), НЕ planned rotateServerIdentity. Emergency rotation НЕ выпускает rotation_in_progress payload — orphan клиенты получают `server_identity_changed` и выполняют manual re-pin
3. **Browser admin UI:** prompt должен показывать оба fingerprints visually и требовать typed confirmation, не click
4. **Server SHOULD** включать `transition_until_ns ≤ now + 7 days` (default). Не давать slack window > 7 дней
5. **Двойная rotation запрещена** в течение overlap (см. pre-condition в Part A)
