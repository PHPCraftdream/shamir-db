# Network Protocol & Security v27

## 1. Протокол

1.1. **SCRAM-SHA-256** — proof пароля (сервер: наносекунды)
1.2. **Ed25519** — proof владения ключом (сервер: 70μs, только после SCRAM)
1.3. **Curve25519 DH** — общий секрет
1.4. **ChaCha20-Poly1305** — AEAD шифрование потока

## 2. Режимы

2.1. `full` — SCRAM + Ed25519, DH + binding, ChaCha20-Poly1305
2.2. `auth_only` — SCRAM + Ed25519, DH + binding, ChaCha20-Poly1305 tag only
2.3. `none` — без auth и шифрования (localhost/dev, отключён по умолчанию)

2.4. Режим жёстко задан в конфиге клиента. Включён в HKDF info.

2.5. **auth_only семантика:**
  2.5.1. Цель: читаемость трафика (tcpdump, debug), не экономия CPU
  2.5.2. MAC: `tag = HMAC-SHA256(mac_key, nonce(12) || length(4 BE) || plaintext)`
  2.5.3. `mac_key` — отдельный ключ: `HKDF(IKM=direction_key, salt=session_id, info="ShamirDB-AuthOnly-MAC", L=32)`
  2.5.4. Фрейм: `[length: 4 BE][plaintext: length][tag: 32]` (tag 32 байта, не 16)
  2.5.5. Nonce = `epoch(4 BE) || counter(8 BE)` — тот же что в full mode
  2.5.6. ChaCha20-Poly1305 **не используется** в auth_only. Только HMAC-SHA256
  2.5.7. Обоснование: ChaCha20-Poly1305 encrypt+discard тратит CPU на keystream для 16MB данных, которые выбрасываются. HMAC-SHA256 — стандартный MAC, нет AEAD misuse
  2.5.8. Trade-off: отдельный кодпас (vs один с full mode). Принят для CPU efficiency и криптографической чистоты
  2.5.9. В auth_only direction_key не используется для шифрования — только как IKM для HKDF → mac_key
  2.5.10. **Verify before parse:** получатель читает весь фрейм → вычисляет HMAC → constant-time compare tag → только после success передаёт plaintext парсеру. Никогда не парсить до проверки tag
  2.5.11. Только для debug/trusted network. Warning в логах

## 3. Лимиты

3.1. `MAX_PRE_AUTH_MSG = 1024 байт` — до аутентификации
3.2. `MAX_FRAME_SIZE = 16 MB` — проверка ДО аллокации буфера
3.3. `MAX_KDF_TIME = 4` — Argon2id time/passes (не секунды)
3.4. `MAX_KDF_MEMORY = 128 MB` — Argon2id memory
3.5. `MAX_KDF_PARALLEL = 8` — Argon2id parallelism
3.6. `USERNAME_MAX = 255 байт UTF-8` — проверяется в шаге 4.1
3.7. `SESSION_MAX_AGE = 24 часа` — абсолютный таймаут
3.8. `SESSION_IDLE_TTL = 30 минут` — по неактивности
3.9. `REKEY_INTERVAL = 2^32 фреймов` — per direction (~4 млрд фреймов)
3.10. `PER_SESSION_MEM = 64 MB` — буферы фреймов (ручной accounting)
3.11. `MAX_SESSIONS_USER = 16` — параллельные сессии per user
3.12. `BACKOFF_CAP = 30 секунд` — верхняя граница backoff
3.13. `BACKOFF_RESET = 5 минут` — без попыток → сброс
3.14. `RATE_LIMIT = 10 auth_init/IP/сек` — sliding window, IPv6: per /64
3.15. `HANDSHAKE_SECRET_TTL = 15 минут` — current + previous = 30 мин
3.16. `NONCE_CACHE_CAP = 1M entries` — global LRU, partitioned by username hash (lock contention reduction). Nonce добавляется только при SCRAM success (4.5.7)
3.17. `NONCE_CACHE_TTL = 30 минут` — совпадает с handshake_secret window
3.18. `CLIENT_MIN_VERSION = 1` — минимальная версия протокола

## 4. Аутентификация

### 4.1. Client → Server (auth_init)

4.1.1. Формат: `{"auth_init": {"user": "alice", "client_nonce": "base64(32)", "protocol_version": 1, "min_version": 1}}`
4.1.2. ≤ 1024 байт. Nonce строго 32 байта CSPRNG
4.1.3. Сервер проверяет: `server_version >= client_min_version AND client_version >= server_min_version`
4.1.4. Сервер проверяет: `client_nonce != all-zeros`

### 4.2. Server → Client (challenge, constant-time, stateless)

4.2.1. Fake salt: `HMAC-SHA256(server_secret, username)`
4.2.2. Fake stored_key: `HMAC-SHA256(server_secret, "fake_sk" || username)`
4.2.3. Real user: `lookup(username)` — обе ветки одинаковое время
4.2.4. Stateless nonce: `server_nonce_part = HMAC-SHA256(handshake_secret, "ShamirDB-Nonce" || client_nonce || username || timestamp_slot)`
4.2.5. `timestamp_slot = unix_time / 900` (15-минутный слот)
4.2.6. `combined_nonce = client_nonce(32) || server_nonce_part(32)`
4.2.7. Сервер хранит per-connection: username, client_nonce, salt — TCP и так stateful. HMAC-nonce оправдан: архитектурная гибкость (edge-балансировщик может передать state в самом nonce без shared memory)
4.2.8. kdf и kdf_params — глобальные (конфиг), одинаковы для real и fake
4.2.9. Ответ: `{"salt": "base64(16)", "kdf": "argon2id", "kdf_params": {...}, "server_nonce": "base64(64)"}`

### 4.3. Client вычисляет (SCRAM + подготовка)

4.3.1. Проверка kdf_params: time ≤ 4, memory ≤ 128MB, parallelism ≤ 8. Иначе disconnect
4.3.2. `salted_password = Argon2id(password, salt, kdf_params)` — ~1с, 64MB RAM
4.3.3. `client_key = HMAC-SHA256(salted_password, "Client Key")`
4.3.4. `server_key = HMAC-SHA256(salted_password, "Server Key")`
4.3.5. `local_decrypt_key = HKDF-SHA256(IKM=salted_password, salt="local", info="ShamirDB-LocalKey", L=32)` — RFC 5869 порядок. HKDF salt фиксирован, но local_key зависит от серверного salt через salted_password (Argon2id input). Salt mismatch → hard failure (15.4)
4.3.6. `zeroize: salted_password, password`
4.3.7. Канонизация KDF: `"argon2id:memory_kb=65536,parallelism=4,time=2"` — алфавитный порядок, decimal ASCII, без leading zeros. Только параметры протокола. Новые параметры = новый protocol_version
4.3.8. `kdf_hash = SHA256(kdf_canonical_string)` — 32 байта
4.3.9. `auth_message = len(username)(2 BE) || username || client_nonce(32) || server_nonce(64) || salt(16) || kdf_hash(32) || protocol_version(1) || min_version(1)`
4.3.10. `client_signature = HMAC-SHA256(SHA256(client_key), auth_message)`
4.3.11. `client_proof = client_key XOR client_signature`
4.3.12. `zeroize: client_key`
4.3.13. Один Argon2id за handshake. local_decrypt_key через HKDF — мгновенно

### 4.4. Client → Server (SCRAM proof)

4.4.1. `{"client_proof": "base64(32)"}`

### 4.5. Server проверяет SCRAM (наносекунды)

4.5.1. Nonce cache lookup: client_nonce в LRU cache → reject (replay)
4.5.2. Шаг 5 recovery: **всегда** вычислить все 4 server_nonce: `(current_secret, current_slot)`, `(current_secret, slot-1)`, `(previous_secret, current_slot)`, `(previous_secret, slot-1)`. Выбрать совпавший через constant-time OR bitmask. Не short-circuit. 4× HMAC = наносекунды. Покрывает slot boundary
4.5.3. `client_signature = HMAC-SHA256(stored_key, auth_message)` — используется real stored_key ИЛИ fake_stored_key (4.2.2). Все операции выполняются в обоих случаях
4.5.4. `recovered_client_key = client_proof XOR client_signature`
4.5.5. `constant_time_eq(SHA256(recovered_client_key), stored_key)?`
4.5.6. Fail → reject, backoff per (IP, username): 100ms × 2^N, cap 30s, reset 5 мин
4.5.7. Success → добавить client_nonce в nonce cache (TTL 30 мин)
4.5.8. **Constant-time:** для unknown users криптография (4.5.3–4.5.5) выполняется с fake_stored_key. Нельзя пропускать — timing oracle

### 4.6. Server → Client (mutual auth + challenge)

4.6.1. `server_proof = HMAC-SHA256(server_key, auth_message)`
4.6.2. `{"server_proof": "base64(32)", "auth_challenge": "base64(32)"}`

### 4.7. Client: проверка сервера + Ed25519 + DH

4.7.1. `HMAC-SHA256(server_key, auth_message) == server_proof?` — constant-time
4.7.2. Fail → disconnect немедленно, "server authentication failed" (client-side)
4.7.3. `zeroize: server_key` — больше не нужен (DH binding теперь Ed25519, не HMAC)
4.7.4. `private_key = AES-256-GCM::decrypt(local_decrypt_key, encrypted_private_key)`
4.7.5. `zeroize: local_decrypt_key`
4.7.6. Self-check: `Ed25519::public(private_key) == stored_public_key`. Fail → zeroize, disconnect, "key file corrupted"
4.7.7. `client_dh_private = random(32)`
4.7.8. `client_dh_public = Curve25519(client_dh_private)`
4.7.9. `binding_context = client_proof || server_proof || auth_challenge`
4.7.10. `auth_sig = Ed25519::sign(private_key, "ShamirDB-Auth" || binding_context)`
4.7.11. `dh_binding = Ed25519::sign(private_key, "ShamirDB-DH" || binding_context || client_dh_public)`
4.7.12. `zeroize: private_key`
4.7.13. Ed25519 подписи детерминистичны (RFC 8032). Random nonce не рекомендуется

### 4.8. Server: проверка auth + DH

4.8.1. `Ed25519::verify(public_key, "ShamirDB-Auth" || binding_context, auth_sig)?`
4.8.2. `Ed25519::verify(public_key, "ShamirDB-DH" || binding_context || client_dh_public, dh_binding)?`
4.8.3. Reject low-order DH public keys
4.8.4. `server_dh_private = random(32)`
4.8.5. `server_dh_public = Curve25519(server_dh_private)` — reject low-order
4.8.6. `server_dh_binding = Ed25519::sign(server_signing_private, "ShamirDB-DH" || binding_context || server_dh_public)` — серверный Ed25519 ключ (6.5), не HMAC
4.8.7. Ответ: `{"authenticated": true, "server_signing_public": "base64(32)", "key_exchange": {"public": "base64(32)", "binding": "base64(64)"}}`
4.8.8. `server_signing_public` — для TOFU pinning при первом подключении

### 4.9. Client: проверка server DH binding

4.9.1. TOFU check: `server_signing_public == saved?` First connect → save. Mismatch → warning + disconnect
4.9.2. `Ed25519::verify(server_signing_public, "ShamirDB-DH" || binding_context || server_dh_public, binding)?`
4.9.3. Reject low-order DH public keys

### 4.10. Обе стороны: session_id + keys

4.10.1. `session_id = SHA256("ShamirDB-Session" || len(username)(2 BE) || username || client_nonce(32) || server_nonce(64) || client_proof(32) || server_proof(32) || auth_challenge(32) || auth_sig(64) || client_dh_public(32) || dh_binding(64) || server_signing_public(32) || server_dh_public(32) || server_dh_binding(64))`
4.10.2. Полный transcript hash. Все binding включены
4.10.3. `shared_secret = Curve25519(my_dh_private, their_dh_public)`
4.10.4. `zeroize: dh_private`
4.10.5. `session_keys = HKDF-SHA256(IKM=shared_secret, salt=session_id, info="ShamirDB v1 " || SECURITY_MODE, L=96)`
4.10.6. `zeroize: shared_secret`
4.10.7. `client_to_server_key = [0..32]`
4.10.8. `server_to_client_key = [32..64]`
4.10.9. `rekey_material = [64..96]`

## 5. Шифрование (ChaCha20-Poly1305)

### 5.1. Фрейм

5.1.1. `[length: 4 BE][ciphertext: length][tag: 16]`
5.1.2. `length` = байты между length и tag. `len(ciphertext) == len(plaintext)`
5.1.3. Nonce не передаётся — детерминистичен

### 5.2. Nonce

5.2.1. `nonce = epoch(4 BE) || counter(8 BE)`
5.2.2. Counter: per direction, монотонный, **не сбрасывается** при rekeying
5.2.3. Epoch: начинается с 0, инкрементируется при rekey
5.2.4. Counter инициализируется 0. Первый фрейм counter=1
5.2.5. Epoch overflow (2^32): disconnect. 2^32 × 2^32 = 2^64 total → невозможно при любых скоростях

### 5.3. Encrypt / Decrypt

5.3.1. `counter++`
5.3.2. `nonce = epoch(4 BE) || counter(8 BE)`
5.3.3. `ciphertext, tag = ChaCha20Poly1305::encrypt(direction_key, nonce, plaintext, aad=length(4 BE))`
5.3.4. send: `[length][ciphertext][tag]`
5.3.5. recv: `length → MAX_FRAME_SIZE check → counter++ → nonce → decrypt+verify → plaintext or reject`

### 5.4. Rekeying

5.4.1. Каждые 2^32 фреймов per direction
5.4.2. `epoch++`
5.4.3. `new_key = HKDF-SHA256(IKM=current_key || rekey_material, salt=session_id || epoch(4 BE), info="ShamirDB rekey", L=32)`
5.4.4. `zeroize: old_key`
5.4.5. auth_only: `new_mac_key = HKDF(IKM=new_direction_key, salt=session_id, info="ShamirDB-AuthOnly-MAC", L=32)`. `zeroize: old_mac_key`
5.4.6. Counter **не сбрасывается** — nonce уникален между epoch
5.4.6. TCP ordered delivery → синхронность гарантирована. Десинхронизация = баг → disconnect
5.4.7. Backward secrecy: компрометация текущего ключа не раскрывает прошлые
5.4.8. Full memory dump → будущие ключи вычислимы через rekey_material (не forward secrecy). Для post-compromise security: periodic DH re-handshake (Signal ratchet, не реализован)

### 5.5. Graceful close

5.5.1. Пустой фрейм (length=0) = close notification
5.5.2. Close = отправитель прекращает отправку
5.5.3. Получатель обрабатывает буфер, затем TCP close
5.5.4. TCP close — authoritative

### 5.6. Fragmentation

5.6.1. Результаты >16 MB: несколько фреймов на application layer
5.6.2. Application-level message framing определяется отдельно
5.6.3. Per-frame read timeout: 60 секунд (настраиваемо)

## 6. Хранение

### 6.1. Сервер

6.1.1. `stored_key` — SHA256(client_key), 32 байта
6.1.2. `server_key` — HMAC(salted_password, "Server Key"), 32 байта. Используется **только** для SCRAM server_proof (4.6.1). **Не** для DH binding (теперь Ed25519, 6.5)
6.1.3. `salt` — crypto_random(16), 16 байт
6.1.4. `public_key` — Ed25519 public, 32 байта
6.1.5. KDF параметры **глобальные** (конфиг), не per-user → anti-enumeration
6.1.6. Нет на сервере: пароля, private_key клиента, salted_password, client_key
6.1.7. При регистрации: проверить что public_key — валидная Ed25519 точка

### 6.5. Серверный ключ (Ed25519)

6.5.1. Генерируется при первом запуске сервера, хранится в SystemStore
6.5.2. `server_signing_private` — secret, zeroize при graceful shutdown. SIGKILL: остаётся в памяти до overwrite ОС. Mitigations: mlock, disable core dumps
6.5.3. `server_signing_public` — отдаётся клиенту при первом auth (шаг 4.8)
6.5.4. Используется для DH binding (шаг 4.8) вместо HMAC(server_key)
6.5.5. TOFU: клиент сохраняет server_signing_public при первом подключении. Изменение → warning + disconnect. **Out-of-band pinning** (приоритетнее TOFU): `shamir://alice@db.local?server_key=base64...` — ключ в строке подключения, TOFU отключён, жёсткий pinning. Для БД рекомендуется out-of-band
6.5.6. Независим от пароля → компрометация пароля не позволяет MITM

### 6.2. Клиент

6.2.1. `~/.shamir/keys/alice.key` (chmod 600, проверять owner+permissions)
6.2.2. Формат: `salt(16) || nonce(12) || encrypted_private_key(32) || tag(16) || public_key(32)` = 108 байт
6.2.3. `local_key = HKDF-SHA256(IKM=salted_password, salt="local", info="ShamirDB-LocalKey", L=32)` — RFC 5869, как 4.3.5
6.2.4. `encrypted = AES-256-GCM(local_key, nonce, private_key)`
6.2.5. AES-GCM nonce **обязательно** генерируется CSPRNG при каждом обновлении файла
6.2.6. Self-check: `Ed25519::public(private_key) == public_key` после расшифровки

### 6.3. KDF

6.3.1. Default: **Argon2id** (time=2, memory=64MB, parallelism=4)
6.3.2. Legacy: PBKDF2 (`"kdf": "pbkdf2"`, `"iterations": N`)
6.3.3. Brute-force Argon2id 64MB: GPU ~150 h/s. 8-char (a-zA-Z0-9) ~46 000 лет
6.3.4. Brute-force PBKDF2 100k: GPU ~10k h/s. 8-char ~700 лет
6.3.5. Без private_key (на клиенте) — аутентификация невозможна в любом случае

### 6.4. Bootstrap

6.4.1. Первый запуск: сервер генерирует одноразовый token (32 байта random, base64)
6.4.2. Выводит в stdout
6.4.3. Token хешируется в конфиге после использования
6.4.4. Open registration отключена. Только admin session или bootstrap

## 7. Регистрация

7.1. `salt = random(16)`
7.2. `salted_password = Argon2id(password, salt, kdf_params)`
7.3. `client_key = HMAC-SHA256(salted_password, "Client Key")`
7.4. `stored_key = SHA256(client_key)`
7.5. `server_key = HMAC-SHA256(salted_password, "Server Key")`
7.6. `keypair = Ed25519::generate()`
7.7. `zeroize: salted_password, client_key`
7.8. Клиент → сервер: `stored_key, server_key, salt, public_key`

## 8. Key Rotation

> **Self-service password rotation удалён в v1.** Admin пересоздаёт SCRAM-юзера
> через `CreateScramUser` (с предварительным удалением старой записи через
> `RedbUserDirectory`). См. `docs/roadmap/PRODUCTION_HARDENING_ROADMAP.md` P0 #9.

### 8.1. Смена Ed25519 ключа

8.1.1. Server challenge + SCRAM proof + подпись старым ключом
8.1.2. `old_signature = Ed25519::sign(old_private_key, "ShamirDB-KeyChange" || session_id || challenge_nonce || new_public_key)`
8.1.3. `{"change_key": {"scram_proof", "old_signature", "new_public_key", "invalidate_sessions": true}}`
8.1.4. Crash recovery: запись новой версии .key.new → `fsync(file)` → `fsync(directory)`, после server OK атомарный rename.

### 8.2. Admin reset

8.2.1. Удаление auth записи → re-registration через admin

## 9. Session

9.1. `session_id: [u8; 32]` — transcript hash
9.2. `c2s_key: Zeroizing<[u8; 32]>` — client → server encryption
9.3. `s2c_key: Zeroizing<[u8; 32]>` — server → client encryption
9.4. `rekey_material: Zeroizing<[u8; 32]>`
9.5. `c2s_counter: u64`, `s2c_counter: u64`
9.6. `c2s_epoch: u32`, `s2c_epoch: u32`
9.7. Инвалидация: max_age, idle_ttl, admin kill (TCP close), epoch overflow
9.8. Max 16 per user. Zeroize при Drop

## 10. Zeroize

10.1. `password` — шаг 4.3 (после Argon2id + HKDF)
10.2. `salted_password` — шаг 4.3 (после key derivations)
10.3. `client_key` — шаг 4.3 (после client_proof)
10.4. `local_decrypt_key` — шаг 4.7 (после расшифровки private_key)
10.5. `server_key (клиент)` — шаг 4.7 (после проверки server_proof)
10.6. `private_key` — шаг 4.7 (после обеих подписей)
10.7. `dh_private` — после shared_secret
10.8. `shared_secret` — после HKDF
10.9. `old_key (rekey)` — сразу
10.10. `session keys` — при Drop

## 11. DoS защита

11.1. Pre-auth: ≤ 1024 байт
11.2. Frame: ≤ 16 MB, ДО аллокации
11.3. Memory: 64 MB per session
11.4. Rate: 10/IP/сек, sliding window, IPv6 per /64
11.5. Backoff: per (IP, username), 100ms × 2^N, cap 30s, reset 5 мин
11.6. Timeout: `max(10s, kdf_time × 5)`, настраиваемо
11.7. Constant-time: fake salt/stored_key, одинаковые kdf_params
11.8. SCRAM first: Ed25519 только после proof
11.9. Nonce cache: LRU, cap 1M, TTL 30 мин, обязательный
11.10. Sessions: max 16 per user, max_age 24h, idle 30min
11.11. DH: reject low-order points
11.12. Bootstrap: одноразовый token, stdout
11.13. Client nonce: CSPRNG обязателен, reject all-zeros
11.14. Rate limit rejection **не увеличивает** backoff

## 12. Компрометация

12.1. **БД** — stored_key: Argon2id 64MB, GPU ~150 h/s, 8-char ~46k лет. Без private_key → ничего
12.2. **Клиент** — salt + public_key открыты. Encrypted key: Argon2id offline. 8-char ~46k лет
12.3. **БД + клиент** — brute-force пароля → компрометация
12.4. **Пароль** — без private_key → ничего
12.5. **Session key** — SCRAM re-proof для rotation → без пароля не сменить
12.6. **Трафик (full)** — зашифрован. Rekeying: backward secrecy
12.7. **1 direction key** — другое направление не затронуто
12.8. **Server identity** — Ed25519 server key (6.5), TOFU pinning. Независим от пароля. Компрометация пароля **не** позволяет MITM. Компрометация server_signing_private → MITM
12.9. **server_secret** — не ротируется. Утечка = offline enumeration, но это уже server compromise

## 13. Domain Separation Tags

13.1. `"Client Key"` — HMAC(salted_password) → client_key
13.2. `"Server Key"` — HMAC(salted_password) → server_key
13.3. `"fake_sk"` — HMAC(server_secret) → fake stored_key
13.4. `"ShamirDB-Nonce"` — stateless server_nonce
13.5. `"ShamirDB-Auth"` — Ed25519 auth signature
13.6. `"ShamirDB-DH"` — DH binding (Ed25519: клиент + сервер)
13.7. `"ShamirDB-LocalKey"` — HKDF → local file encryption key
13.8. `"ShamirDB-KeyChange"` — Ed25519 при смене ключа
13.9. `"ShamirDB-Session"` — SHA256 transcript → session_id
13.10. `"ShamirDB v1 "` — HKDF info (+mode). Trailing space осознан
13.11. `"ShamirDB rekey"` — HKDF → rekey
13.12. `"local"` — HKDF salt для local_decrypt_key (фиксированный)
13.13. `"ShamirDB-AuthOnly-MAC"` — HKDF info для auth_only MAC key

## 14. Error Messages

14.1. Все ошибки аутентификации: `{"error": "authentication_failed"}`
14.2. Не раскрывается: неверный пароль, user не существует, неверная подпись, неверный binding
14.3. Server proof fail (клиент): "server authentication failed" (client-side)
14.4. Self-check fail: "key file corrupted" (client-side)

## 15. Примечания

15.1. **Constant-time:** tag, stored_key (4.5), server_proof (4.7), DH binding (4.9)
15.2. **Ed25519 deterministic:** RFC 8032. Random nonce → private_key leak при двух подписях
15.3. **AES-GCM nonce:** CSPRNG при каждом обновлении файла. Reuse = catastrophic
15.4. **Salt mismatch:** server salt ≠ file salt → hard failure. Recovery: re-registration
15.5. **server_secret:** персистентный, SystemStore, **не ротируется**. Fake salt = `HMAC(server_secret, username)` — стабилен навсегда. Ротация ломает anti-enumeration (salt до/после ротации различим). Утечка server_secret = server compromise, ротация не нужна
15.6. **Handshake timeout:** `max(10s, kdf_time × 5)`. Embedded/satellite: автоадаптация
15.7. **Session resumption:** отсутствует. Disconnect = полный handshake
15.8. **HKDF local_key:** фиксированный HKDF salt "local". Однако local_key зависит от серверного salt через salted_password (Argon2id). Salt mismatch → hard failure (15.4)
15.9. **Server identity:** Ed25519 server key (6.5) + TOFU pinning. Server binding через Ed25519::sign (не HMAC(server_key)). Независим от пароля. Компрометация пароля **не** = MITM
15.10. **Post-compromise:** backward secrecy only. Rekey interval = 2^32. DH re-handshake (Signal ratchet) — future
15.11. **Audit logging:** auth success/fail, change_key, admin kill
15.12. **Rekeying sync:** TCP ordered delivery. Десинхронизация = баг → disconnect
15.13. **Backoff entries:** TTL = BACKOFF_RESET. Eviction по TTL
15.14. **Cluster:** nonce cache per-node. Multi-node: sticky sessions или shared cache
15.15. **auth_init unsigned:** mitigated через auth_message inclusion + constant-time

## 16. Flow

```
TCP connect
  ▼
4.1  auth_init(user, nonce, version, min_version)   ≤1024b
  ▼
4.2  salt, kdf, kdf_params, server_nonce            constant-time, stateless
  ▼
4.3  SCRAM client_proof                             клиент: Argon2id ~1с
4.5  server: nonce_cache + HMAC = нс                сервер: нс
  │  ✗ reject (backoff per IP+user)
  ▼  ✓
4.6  server_proof + auth_challenge                  mutual auth
  ▼
4.7  auth_sig + DH(Ed25519 binding)                 одно расшифрование private_key
4.8  server: 2× verify = 140μs                      reject low-order
  │  ✗ reject
  ▼  ✓
4.8  server DH(Ed25519 binding) + server_signing_public + authenticated
  ▼
4.9  client: verify server DH binding
  ▼
4.10 session_id = SHA256(transcript)                обе стороны
     session_keys = HKDF(DH, session_id, mode)      3×32
  ▼
5.3  ChaCha20-Poly1305(epoch+counter)               rekey per 2^32
```
