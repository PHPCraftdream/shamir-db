בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Security-ревью: комплаенс, supply-chain, логирование, лицензии (защитный аудит)

_Агент: @fm, 2026-07-06. Часть панели ревью проекта S.H.A.M.I.R. Это защитный аудит собственной кодовой базы (авторизованный, для укрепления)._

**Область.** Этот отчёт НЕ дублирует находки параллельного ревью сетевой поверхности
(`2026-07-06-security-network-surface.md` — pre-auth authz, DoS-окна хендшейка, timing,
resume-binding, WASM-fuel). Здесь — **то, что тот отчёт не покрыл**: комплаенс с
общепринятыми стандартами (OWASP / крипто-best-practices / GDPR-совместимое хранение PII),
supply-chain (версии зависимостей, известные уязвимости, отсутствие CI-гейта), утечка
секретов/PII в логи, лицензионная чистота, гигиена репозитория (`.gitignore`, захардкоженные
секреты).

**Метод.** Инвентаризация `Cargo.lock` (крипто/сеть/парсинг-крейты) с сопоставлением
версий против известных RUSTSEC; греп логирующих макросов по секрето-/PII-несущим полям;
проверка `git ls-files` на трекнутые секреты; аудит `license`-полей всех 22 крейтов;
поиск захардкоженных секретов и compile-time env-эксфильтрации; проверка наличия
security-политики и CI-гейта аудита.

---

## Резюме (топ находок)

| # | Серьёзность | Область | Суть |
|---|---|---|---|
| C1 | **HIGH** | Supply-chain | Нет `cargo-deny`/`cargo-audit` гейта в CI — уязвимости зависимостей и лицензионный дрейф ловятся только вручную |
| C2 | **HIGH** | Комплаенс/крипто | `compile.rs` компилирует недоверенный Rust с `env!`/`include_str!` доступом (см. network-surface #2) — но здесь как **supply-chain/compliance** класс: нет policy-документа, нет запрета compile-time эксфильтрации |
| C3 | **MEDIUM** | Логирование | `auth_failed` логирует plaintext `username` (PII + user-enumeration signal в логах) |
| C4 | **MEDIUM** | Комплаенс | Нет `SECURITY.md` / vuln-disclosure policy — OWASP/индустриальный минимум для БД-продукта |
| C5 | **MEDIUM** | Supply-chain | Устаревший `wasmtime 45.0.0` — исполнитель недоверенного кода; нет процесса отслеживания wasmtime security-advisories |
| C6 | **LOW-MED** | Комплаенс/GDPR | Нет документированной политики retention/erasure PII и шифрования-at-rest (данные хранятся в MessagePack на диске в открытом виде) |
| C7 | **LOW** | Гигиена | `getrandom` в трёх мажорных версиях, `rand` 0.8+0.9 — не уязвимость, но раздувает audit-поверхность |

**Что уже ХОРОШО (подтверждено):**
- `server-cert.pem` в корне — **НЕ трекнут** git (корректно покрыт `.gitignore`: `/server-cert.pem`, `/web/server-cert.pem`).
- **Ни одного захардкоженного секрета** в `crates/*/src` (греп по `password|secret|api_key|token|private_key = "..."` — пусто).
- Все **22 крейта** декларируют `license = "MIT OR Apache-2.0"`; в корне есть `LICENSE-MIT` + `LICENSE-APACHE`. Лицензионная чистота workspace-кода — чистая.
- Криптостек современный и без известных дыр по версиям: `ring 0.17.14`, `rustls 0.23.37`, `argon2 0.5.3`, `ed25519-dalek 2.2.0`, `sha2 0.10.9`, `hmac 0.12.1`, `hkdf 0.12.4`, `aes-gcm 0.10.3`. Constant-time сравнения через `subtle` (подтверждено network-surface-ревью).
- **Нет `rsa`-крейта** → не затронуты RUSTSEC-2023-0071 (Marvin timing). Нет `atty`/`dotenv`/`native-tls`/`openssl` — типовые проблемные транзитивы отсутствуют.
- `.env`-файлы нигде не трекаются и не читаются; `dotenv` не в дереве.

---

## 1. Supply-chain

### C1 (HIGH) — Нет автоматического гейта аудита зависимостей

`cargo-audit` и `cargo-deny` **не установлены** и в репозитории нет ни `deny.toml`,
ни `audit.toml`, ни `.cargo/audit.toml`. Pre-commit/pre-push гейт (CLAUDE.md) покрывает
`fmt`/`clippy`/`test`, но **не** проверяет:
- новые RUSTSEC-advisories на транзитивных зависимостях;
- лицензионный дрейф транзитивов (workspace-код чист, но 200+ транзитивов не проверяются
  на copyleft/несовместимые лицензии — критично для продукта, распространяемого как единый бинарь);
- yanked-крейты и дубликаты версий.

Для БД-продукта, шиппящегося как единый бинарь с исполнителем недоверенного кода, отсутствие
supply-chain-гейта — главный комплаенс-пробел.

**Фикс.** Добавить `deny.toml` (секции `[advisories]`, `[licenses]` с allow-листом
`MIT`/`Apache-2.0`/`BSD-*`/`Unicode-*`/`ISC`/`Zlib`, `[bans]` на дубликаты), установить
`cargo-deny`, завести CI-шаг `cargo deny check` + периодический `cargo audit`. Приложить
SHA-пиннинг для `captrack` (сейчас `path = "D:/dev/rust/captrack"` — абсолютный локальный
путь, невоспроизводимый вне машины автора; для релиза заменить на git-rev или crates.io-версию).

### C5 (MEDIUM) — Устаревший wasmtime, исполнитель недоверенного кода

`wasmtime 45.0.0` (весь `wasmtime-internal-*` кластер). Wasmtime — прямая доверенная граница
для недоверенного гостевого кода; его security-advisories (CPU-fuel-обход, OOB в cranelift,
host-trap escape) исторически регулярны. По версии на момент аудита критичных открытых RUSTSEC
на 45.0.0 не выявлено, но **нет процесса** отслеживания wasmtime-advisories отдельно от общего
audit. Учитывая, что fuel/epoch-контроль уже отмечен как дырявый в network-surface #2f, версия
исполнителя — часть той же поверхности.

**Фикс.** Закрепить политику: обновлять `wasmtime` в приоритете, подписаться на Bytecode
Alliance security-announce, добавить wasmtime в отдельный audit-чеклист.

### C7 (LOW-MED) — Раздувание audit-поверхности дубликатами

`getrandom` в **трёх** мажорах (0.2.17 / 0.3.4 / 0.4.1), `rand` 0.8.5 + 0.9.2,
`rand_core` 0.6+0.9, `rand_chacha` 0.3+0.9, `webpki-roots` 0.26+1.0,
`toml_datetime`/`serde_spanned`/`rand_xoshiro` — по два мажора. Не уязвимость, но каждый
дубликат — отдельная строка, которую audit должен отслеживать, и лишний код в бинаре.

**Фикс.** `cargo deny check bans` с `multiple-versions = "warn"`; по возможности выровнять
`rand`/`getrandom` через обновление промежуточных зависимостей.

---

## 2. Логирование секретов и PII

Греп логирующих макросов (`tracing::*!`, `log::*!`) по секрето-/PII-полям
(`password|server_key|stored_key|secret|token|proof|nonce|channel_binding|salt|env`) по
`shamir-server` / `shamir-connect` / `shamir-wasm-host`.

**Хорошо:** секретов (пароли, ключи, proof, channel-binding, nonce) в логах **не найдено** —
это заметно лучше типового уровня. Конвенция redact-`Debug` (описанная в network-surface 1b)
удерживает секрето-несущие структуры от случайного логирования через `{:?}`.

### C3 (MEDIUM) — Plaintext username в auth-логах

`crates/shamir-server/src/connection/handshake.rs:377`:
```
tracing::info!(user = %username.as_str(), "auth_failed: bad proof");
```
Логирование **plaintext username при неудачной аутентификации** — это:
1. **PII в логах** (GDPR: логи с идентификаторами субъектов требуют того же retention/защиты,
   что и основные данные; `info`-уровень часто уходит в централизованный сбор).
2. **User-enumeration/credential-stuffing сигнал**: агрегированные `auth_failed` по username
   в логах дают атакующему (или инсайдеру с доступом к логам) карту валидных/атакуемых аккаунтов.

OWASP Logging Cheat Sheet: логировать факт события аутентификации — да; сырой идентификатор
на `info` — только с осознанной PII-политикой (хэш/усечение или отдельный защищённый audit-стрим).

**Фикс.** Логировать хэш/усечённый идентификатор или коррелляционный id вместо plaintext
username на общем `info`-канале; полный identity — только в защищённый HMAC-audit-лог (который
у проекта уже есть, см. README «Audit log with HMAC chain»).

### Замечание по WASM-логу (информационное)

`wasm/wasm_engine.rs:116` логирует имя env-переменной `SHAMIR_WASM_NO_POOL` (не значение) —
это безопасно, значение не выводится.

---

## 3. Комплаенс: крипто, at-rest, PII/GDPR

### C6 (LOW-MED) — Нет документированной политики at-rest шифрования и PII-retention

Records хранятся как MessagePack на диске (CLAUDE.md: «Records are MessagePack; field names
interned to u64»). Аудит **не нашёл** документа, описывающего:
- **Шифрование-at-rest.** Транспорт защищён TLS (rustls), но данные на диске, судя по storage-слою,
  лежат в открытом виде. Для БД с PII многие комплаенс-режимы (GDPR Art.32, SOC2) требуют
  at-rest-шифрования или явного документированного решения «шифрование делегируется ОС/тому».
- **Retention/erasure PII.** Есть `SetRetention`/`PurgeHistory` (упомянуты в network-surface),
  но нет документа, связывающего их с правом на удаление (GDPR Art.17) — как оператор
  реализует «забыть субъекта» при наличии WAL + history + interner (u64-имена полей переживают
  удаление записи).

**Фикс.** Завести `docs/security/data-protection.md`: явно зафиксировать модель at-rest
(шифруется/делегируется), процедуру PII-erasure через `PurgeHistory` + WAL-truncation +
последствия для interner, и retention-дефолты.

### C2 (HIGH, cross-ref) — Compile-time эксфильтрация как compliance-класс

`crates/shamir-wasm-host/src/compile.rs` компилирует недоверенный Rust-исходник на хосте БД
с полным env (`env!("SECRET")`, `include_str!("/etc/…")`) — детально разобрано в
network-surface #2. Здесь фиксирую это как **compliance-пробел**: нет policy-документа,
запрещающего приём Rust-исходника от недоверенной стороны, и нет allow-листа макросов. Для
комплаенса продукта это должно быть закрыто и **задокументировано** (принимать только
валидированный `.wasm`), а не только пофикшено в коде.

### Крипто — best practices (подтверждение)

Алгоритмы и версии соответствуют best practices: Argon2id для деривации, HKDF/HMAC-SHA256
для SCRAM, Ed25519 для identity, AES-GCM (AEAD), constant-time-сравнения (`subtle`),
channel-binding через TLS-exporter. Замечаний по выбору примитивов нет; проблемы — в
**применении** (resume-binding, Argon2-семафор на неверном пути), уже покрыты network-surface.

---

## 4. Комплаенс-процесс и гигиена репозитория

### C4 (MEDIUM) — Нет SECURITY.md / vuln-disclosure policy

Нет `SECURITY.md` (ни в корне, ни в `.github/`, ни в `docs/`). Для продукта уровня БД это
индустриальный минимум (OWASP, GitHub security best practices): канал ответственного
раскрытия, поддерживаемые версии, время реакции.

**Фикс.** Добавить `SECURITY.md` с контактом раскрытия и supported-versions.

### Гигиена репозитория (подтверждение — чисто)

- `.gitignore` корректно исключает `/server-cert.pem`, `/web/server-cert.pem`, `/.claude/`
  (session-local state), `/tmp/`, `**/test_data/`, `**/node_modules/`, `Cargo.lock` примеров.
  **Секреты/credentials в трекнутых файлах не найдены.**
- `server-cert.pem` в рабочем дереве присутствует, но `git ls-files --error-unmatch`
  подтверждает — **не трекнут**. Ключ (`key.pem`, `key_path` в `deploy/server.example.ktav`)
  — только в примере пути, не в репозитории.
- `deploy/Dockerfile` / `shamir-db.service` / `server.example.ktav` — секретов не содержат
  (только путь `key_path: /var/lib/shamir-db/key.pem`, ожидаемо).
- 9 файлов с `unsafe` (в пределах нормы для perf-БД; предметный аудит unsafe — вне области
  этого отчёта, отдельная задача).

---

## Рекомендации по приоритету

1. **HIGH C1** — `deny.toml` + `cargo-deny`/`cargo-audit` в CI (+ пиннинг `captrack` вне local path).
2. **HIGH C2** — задокументировать и закрыть compile-time-эксфильтрацию (координируется с network-surface #2).
3. **MEDIUM C3** — убрать plaintext username из общего `auth_failed`-лога.
4. **MEDIUM C4** — добавить `SECURITY.md`.
5. **MEDIUM C5** — политика обновления `wasmtime` + отслеживание advisories.
6. **LOW-MED C6** — `docs/security/data-protection.md` (at-rest, PII-erasure/retention).
7. **LOW C7** — выровнять дубликаты `getrandom`/`rand` через `cargo deny bans`.

_Крипто-примитивы, лицензионная чистота workspace-кода, отсутствие захардкоженных секретов и
корректность `.gitignore` — подтверждены как здоровые. Основной долг — в **процессе**
(supply-chain-гейт, security-policy, data-protection-документация), а не в самих алгоритмах._
