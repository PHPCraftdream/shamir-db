# Brief: post-auth per-session rate limiter (taskId #608, P1, HIGH not-fixed)

## Контекст

`crates/shamir-server/src/connection/request_loop.rs:232`-ish (semaphore
acquire) — ЕСТЬ ограничение на количество ОДНОВРЕМЕННО исполняющихся
запросов (`max_in_flight`, semaphore permits), но НЕТ ограничения на
ЧАСТОТУ запросов — клиент, укладывающийся в `max_in_flight`, может слать
сколь угодно много коротких дорогих запросов подряд.

**Важное открытие**: rate-limit инфраструктура УЖЕ СУЩЕСТВУЕТ в кодовой
базе (`crates/shamir-connect/src/server/rate_limit.rs`,
`InMemoryRateLimiter`, `RateLimiter` trait, `RateDecision::{Allowed, RateLimited{retry_after_secs}}`,
token-bucket с fixed-point `micro_tokens`) — НО применяется **только
до аутентификации**, по IP-подсети, для троттлинга `auth_init`
(`crates/shamir-server/src/connection/handshake.rs:534`,
`ctx.rate_limit.check(subnet, now_ns)`). После входа в сессию (post-auth
request loop) этот лимитер больше не консультируется вообще.

Единственная общая точка, через которую проходит АБСОЛЮТНО КАЖДЫЙ
post-auth запрос (из request_loop.rs и из любого другого будущего
транспорта) — функция `dispatch_request_view`
(`crates/shamir-connect/src/server/dispatch.rs:122-161`):

```rust
pub async fn dispatch_request_view<H: RequestHandler + ?Sized, F: Fn(&[u8; 16]) -> u64>(
    view: &RequestEnvelopeView<'_>,
    store: &SessionStore,
    lookup_tickets_invalid_before_ns: F,
    handler: &H,
    conn: &ConnectionServices,
) -> Result<DispatchOutcome> {
    let sid = view.session_id_array()?;
    let session: Arc<Session> = match store.lookup(sid) { ... };
    let user_invalid_before = lookup_tickets_invalid_before_ns(&session.user_id);
    if !session.is_valid_for_user(user_invalid_before) { ... }
    // <-- НОВЫЙ ГЕЙТ СЮДА
    match handler.handle(&session, view.req, conn).await { ... }
}
```

Это — правильное место: гейт ставится ОДИН РАЗ, покрывает все
транспорты/вызовы, а `session: Arc<Session>` — уже resolved к этому
моменту (нужен для per-session bucket).

## Задача

### 1. Токен-бакет на самом `Session`

В `crates/shamir-connect/src/server/session.rs`, `struct Session`
(рядом с `last_activity_ns: AtomicU64`, строка ~144) — добавь состояние
бакета:

```rust
/// Post-auth request-rate token bucket (task #608). Bounded per-session
/// contention (≤ `max_in_flight` concurrent requests on one session) —
/// mirrors the pre-auth `InMemoryRateLimiter`'s per-subnet `DashMap`
/// shard-lock precedent (`rate_limit.rs`), just scoped to one session
/// instead of a sharded map. `std::sync::Mutex` is the sanctioned
/// exception here (CLAUDE.md): no `.await` is held across the lock, and
/// contention is bounded by the connection's own concurrency cap, not a
/// workspace-wide hot path.
post_auth_bucket: std::sync::Mutex<PostAuthBucket>,
```

```rust
/// Fixed-point token-bucket state — mirrors `rate_limit.rs`'s
/// `BucketState` shape (`micro_tokens` = tokens × 1e9, refill without
/// floats).
struct PostAuthBucket {
    micro_tokens: u64,
    last_refill_at_ns: u64,
}
```

(Точное имя/видимость подгони под существующие конвенции файла — `PostAuthBucket`
может быть приватным типом внутри `session.rs`, не обязан быть `pub`.)

Инициализация — там, где `Session::new`/конструктор уже ставит
`created_at_ns`/`last_activity_ns` (найди это место грепом
`created_at_ns:` в `session.rs`): бакет стартует ПОЛНЫМ (по аналогии с
`rate_limit.rs`'s `or_insert_with`'s "first request: bucket starts FULL"
— НО для новой сессии естественнее стартовать full, не full-minus-cost,
поскольку это не "первый запрос", а "создание сессии"):

```rust
post_auth_bucket: std::sync::Mutex::new(PostAuthBucket {
    micro_tokens: PostAuthBucket::capacity(),
    last_refill_at_ns: created_at_ns, // тот же now_ns, что уже используется для created_at_ns
}),
```

Метод на `Session` (публичный, вызывается из `dispatch.rs`):

```rust
/// Check + consume one post-auth request token (task #608). Returns
/// `None` if allowed (bucket debited), `Some(retry_after_secs)` if
/// rejected. Mirrors `rate_limit.rs::InMemoryRateLimiter::check`'s exact
/// refill math (fixed-point micro_tokens, no floats), scoped to this one
/// session instead of a per-subnet map — no warmup window (that's a
/// pre-auth/post-restart-abuse concept, not applicable to an
/// already-authenticated session).
pub fn check_post_auth_rate_limit(&self, now_ns: u64) -> Option<u32> {
    let mut b = self.post_auth_bucket.lock().unwrap();
    let capacity = PostAuthBucket::capacity();
    let rate = shamir_tunables::instance_defaults::POST_AUTH_RATE_LIMIT_PER_SEC as u64;

    let elapsed = now_ns.saturating_sub(b.last_refill_at_ns);
    let refill = elapsed.saturating_mul(rate);
    b.micro_tokens = b.micro_tokens.saturating_add(refill).min(capacity);
    b.last_refill_at_ns = now_ns;

    let cost = 1_000_000_000u64;
    if b.micro_tokens >= cost {
        b.micro_tokens -= cost;
        None
    } else {
        let deficit = cost - b.micro_tokens;
        let secs_to_wait = (deficit / rate) / 1_000_000_000;
        Some((secs_to_wait as u32).max(1))
    }
}
```

`PostAuthBucket::capacity()` — ассоциированная константа/функция:
`rate_per_sec × 1_000_000_000` (та же формула, что
`BucketState::capacity_at_rate` в `rate_limit.rs` — 1 секунда burst).

### 2. Новый tunable

В `crates/shamir-tunables/src/lib.rs`, модуль `instance_defaults` (рядом
с `CONN_MAX_IN_FLIGHT`, строка ~48) — добавь:

```rust
/// Post-auth per-session request-rate limit (task #608). Token-bucket,
/// burst = 1 second's worth (mirrors the pre-auth `auth_init`
/// rate-limiter's burst convention in `shamir-connect::server::rate_limit`).
/// Bounds the frequency of cheap-but-frequent requests a single
/// authenticated session can issue — separate from `CONN_MAX_IN_FLIGHT`,
/// which bounds CONCURRENCY, not frequency.
pub const POST_AUTH_RATE_LIMIT_PER_SEC: u32 = 500;
```

(Значение — разумный дефолт, не блокирующий легитимный высокочастотный
batch-трафик; подстрахуйся, посмотрев есть ли в существующих
бенчах/тестах эталонный "нормальный" RPS на одну сессию, чтобы не выбрать
число, которое сломает существующий perf-тест — если такого ориентира
нет, оставь 500/сек как стартовое значение.)

### 3. Гейт в `dispatch_request_view`

`crates/shamir-connect/src/server/dispatch.rs:122-161` — добавь ПОСЛЕ
`is_valid_for_user` проверки (строка ~148), ДО `handler.handle(...)`:

```rust
if let Some(retry_after_secs) = session.check_post_auth_rate_limit(
    shamir_connect::common::time::UnixNanos::now().as_u64(),
) {
    // (подгони точный путь импорта UnixNanos под то, что уже используется
    // в этом файле — grep `UnixNanos` в dispatch.rs/session.rs)
    return Ok(DispatchOutcome::Error(ErrorEnvelope::new(
        view.request_id,
        "rate_limited",
    )));
    // retry_after_secs пока не встраиваем в ErrorEnvelope — проверь, есть
    // ли у ErrorEnvelope поле для доп. данных (как retry_after у
    // RateDecision) — если да, прокинь; если формат ErrorEnvelope плоский
    // "error: String" без extra payload, оставь как есть (код "rate_limited"
    // достаточен, клиент может ретраить с backoff).
}
```

### 4. Тесты

- `crates/shamir-connect/src/common/tests/` или где удобнее по
  конвенции этого крейта (посмотри где лежат существующие session-related
  тесты) — unit-тест на `Session::check_post_auth_rate_limit`:
  1. Свежая сессия: burst из `POST_AUTH_RATE_LIMIT_PER_SEC` запросов подряд
     (одинаковый `now_ns`) — все `None` (allowed), запрос номер
     `rate+1` — `Some(_)` (rejected).
  2. После рефилла (сдвиг `now_ns` на 1 секунду вперёд) — снова разрешено.
- `crates/shamir-server/tests/` (найди подходящий существующий e2e-файл,
  например рядом с `hmac_gate.rs`/`permission_e2e.rs`, или создай
  `rate_limit_post_auth_e2e.rs`) — end-to-end: реальная SCRAM-сессия,
  быстрый спам запросов (например `Ping`) сверх лимита получает
  `code == "rate_limited"` в ответе, легитимный редкий трафик — нет.

## Прогон проверок

- `cargo fmt -p shamir-tunables -p shamir-connect -p shamir-server -- --check`
- `cargo clippy -p shamir-tunables -p shamir-connect -p shamir-server --all-targets -- -D warnings`
- `./scripts/test.sh -p shamir-tunables -p shamir-connect -p shamir-server --full`

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

- НЕ трогай существующий pre-auth `InMemoryRateLimiter`/`rate_limit.rs` —
  это отдельный, уже рабочий механизм для `auth_init`, не в scope.
- НЕ добавляй warmup-window логику к post-auth лимитеру — она специфична
  для pre-auth restart-abuse сценария (§8.6), здесь не нужна.
- НЕ меняй `max_in_flight`/semaphore логику в `request_loop.rs` — это
  ортогональный механизм (concurrency, не frequency), не трогай.

## Проверка (сделает оркестратор)

- Диф ограничен `session.rs`, `dispatch.rs` (оба shamir-connect),
  `lib.rs` (shamir-tunables), плюс новые тесты.
- fmt/clippy по `shamir-tunables`/`shamir-connect`/`shamir-server` чисты.
- `./scripts/test.sh` по тем же крейтам зелёный, включая новые тесты.
- Новый тест реально ловит регресс (burst+1-й запрос до фикса не
  существовал бы как проверка — убедись что assertion осмысленный, не
  тавтологичный).
