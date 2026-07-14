בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# CRIT-6 — WASM-хост: небезопасная компиляция + Security::Definer/Invoker не применяется (#440)

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Задача #440 —
> связка из ДВУХ CRITICAL находок панельного ревью
> (`docs/dev-artifacts/audits/2026-07-06-security-network-surface.md`, Топ-5 #2 и #3).
> Область: `crates/shamir-wasm-host/src/compile.rs`,
> `crates/shamir-db/src/shamir_db/shamir_db/access_control.rs`.

Это ДВЕ независимые точечные задачи в одном тикете — делай их
последовательно, не смешивай в одном коммите логики (но можешь
закоммитить одним PR/ответом, орchestrator сам решит про коммиты).

---

## Часть A — `compile_rust_source` компилирует недоверенный код без изоляции

**Файл:** `crates/shamir-wasm-host/src/compile.rs:21-100` (`compile_rust_source`).

### Дефект (подтверждён)

`Command::new("cargo").args([...]).env_remove("CARGO_TARGET_DIR").output()`
— это ЕДИНСТВЕННАЯ модификация окружения. Дочерний процесс `cargo build`
наследует ПОЛНОЕ окружение хоста (включая любые секреты в env — API keys,
DB creds и т.п., если процесс сервера их так получает) БЕЗ таймаута.
Гостевой Rust-исходник — это код, который реально КОМПИЛИРУЕТСЯ (не
просто интерпретируется в песочнице), т.е. может содержать
`build.rs`/proc-macro/`include_str!("/etc/passwd")`/`env!("SECRET_KEY")`
— полноценное произвольное выполнение кода НА ХОСТЕ во время сборки, с
доступом к файловой системе и окружению хоста, без rlimit/seccomp/timeout.

Полный `seccomp`/`rlimit`/sandboxing процесса — вне разумного скоупа
точечного фикса (требует платформенно-специфичного кода, вне
Windows-совместимости этого репо, см. `CLAUDE.md`: "Platform: win32").
Реализуй ТОЛЬКО тот минимум, который аудит явно называет достижимым без
полного передела:

### Фикс — три точечных шага

1. **Scrub env**: замени `.env_remove("CARGO_TARGET_DIR")` на явную
   allowlist через `.env_clear()` + `.envs([...])` — сохрани ТОЛЬКО то,
   что реально нужно cargo для сборки: `PATH` (нужен для поиска
   `rustc`/линкера), `HOME`/`USERPROFILE` (нужен для cargo registry
   cache — иначе каждая компиляция будет скачивать registry заново;
   ПРОВЕРЬ, действительно ли `shamir-sdk` зависимость требует
   registry-доступа — она `path`-зависимость, так что скорее всего
   офлайн; если сборка проходит без registry-переменных, не добавляй
   их), `TEMP`/`TMP`/`TMPDIR` (Windows/Unix temp-dir), возможно
   `CARGO_HOME`/`RUSTUP_HOME`/`RUSTUP_TOOLCHAIN` если установлены у
   хоста (проверь `check_toolchain()` рядом — грепни, как она находит
   cargo, чтобы не сломать локальный dev-toolchain resolution). НЕ
   прокидывай ничего похожего на `*_KEY`, `*_SECRET`, `*_TOKEN`,
   `*_PASSWORD`, `DATABASE_URL` и т.п. — если сомневаешься, не включай.
2. **Timeout**: замени `.output()` (блокирующий без таймаута) на
   `.spawn()` + `wait-timeout` крейт (`0.2.1`, уже в `Cargo.lock` как
   транзитивная зависимость — добавь явно в `crates/shamir-wasm-host/Cargo.toml`
   `[dependencies]`) с разумным лимитом (предложи константу, например
   `WASM_COMPILE_TIMEOUT: Duration = Duration::from_secs(60)` — компиляция
   маленькой cdylib-крейты с LTO/opt-level=z обычно укладывается в
   секунды-десятки секунд; дай запас). На таймауте: `child.kill()` +
   верни `FunctionError::Compute("compilation timed out after Ns")`
   (проверь точное имя/сигнатуру `FunctionError` варианта — возможно
   нужен новый вариант типа `FunctionError::Timeout` — посмотри
   `crates/shamir-wasm-host/src/error.rs`, реши что уместнее, обоснуй в
   докладе).
3. **Запрет опасных макросов в исходнике** — ПЕРЕД записью `lib.rs` и
   запуском cargo, просканируй `source` (гостевой Rust-текст) на
   вхождение `include!`, `include_str!`, `include_bytes!`, `env!`,
   `option_env!` как ИДЕНТИФИКАТОРОВ МАКРОСА (т.е. ищи паттерн
   `<name>!` — используй простую границу слова, не грубый substring —
   `env!` не должен ложно сработать на переменную `environment`).
   Если найден — верни `FunctionError::Compute` с понятным сообщением
   ("forbidden macro `env!` in function source — file/env access at
   compile time is not permitted"). Это НЕ надёжная защита (можно
   обойти через доп. трюки типа макросов из зависимостей), но закрывает
   самый очевидный путь эксфильтрации/чтения секретов из окружения —
   документируй это ограничение честно в комментарии (не выдавай за
   полную защиту).

### НЕ трогай

- Общую архитектуру "компилируем недоверенный Rust на хосте" — полный
  переход на "принимать только провалидированный `.wasm`" (первый вариант
  фикса в аудите) — это отдельная, гораздо более крупная архитектурная
  задача, вне скоупа. Здесь только hardening текущего пути.
- `wasm-opt`-постпроцессинг ниже в файле — не трогай.

### Тесты (часть A)

1. Env scrub: скомпилируй лёгкую функцию, установи заведомо чувствительную
   env-переменную (`std::env::set_var` в тесте) типа `SHAMIR_TEST_SECRET`
   с распознаваемым значением, скомпилируй функцию с исходником, который
   пытается прочитать её через `env!("SHAMIR_TEST_SECRET")` — этот
   вызов ДОЛЖЕН быть отклонён твоим фильтром макросов ДО компиляции (см.
   пункт 3) — так что тест этой части фактически сливается с тестом
   пункта 3 (запрет `env!`). Добавь отдельно: скомпилируй ЛЕГИТИМНУЮ
   функцию (без запрещённых макросов) и убедись, что она по-прежнему
   успешно компилируется (env scrub не сломал обычный путь) — используй
   существующий `DOUBLE_SOURCE`-подобный фикстур из
   `crates/shamir-wasm-host/src/tests/compile_tests.rs`.
2. Timeout: не пытайся имитировать реальный 60-секундный hang в тесте
   (слишком долго) — вместо этого либо (а) вынеси таймаут-длительность
   в параметр/константу, доступную тесту, и временно используй очень
   короткий таймаут (например 1ms) с намеренно тяжёлым исходником, чтобы
   гарантированно поймать `Timeout`, либо (б) если конструкция не
   позволяет параметризовать таймаут без более широкого рефакторинга —
   просто протестируй, что нормальная (быстрая) компиляция ПО-ПРЕЖНЕМУ
   укладывается в реальный дефолтный таймаут и не регрессирует. Выбери
   практичный вариант, обоснуй в докладе.
3. Запрет макросов: `include_str!`, `include_bytes!`, `include!`,
   `env!`, `option_env!` — каждый отдельным тестом, assert
   `FunctionError::Compute` с сообщением про запрещённый макрос.
   Позитивный тест: функция с легитимным использованием слова
   "environment" в строковом литерале / комментарии НЕ должна быть
   отклонена (проверка на ложные срабатывания границы слова).

---

## Часть B — `Security::Definer`/`Invoker` не применяется

**Файлы:** `crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:412-425`
(`effective_fn_actor`), `crates/shamir-wasm-host/src/meta.rs:43-45`
(`Security` — комментарий сам признаёт: "Stored in the catalogue;
enforcement is deferred to slice 10").

### Дефект (подтверждён)

`effective_fn_actor(fn_name, caller)` эскалирует `caller` до
`meta.owner` ТОЛЬКО если `Mode::is_setuid(meta.mode)` (POSIX-style
setuid-бит на `ResourceMeta` функции) — это СОВЕРШЕННО ОТДЕЛЬНЫЙ
механизм от WASM-хостового `FunctionMeta.security` (`Security::Definer`/
`Invoker`, хранится в catalogue через `FunctionMeta::from_record`,
дефолт `Invoker`). `security`-поле НИГДЕ не читается для решения "как
исполнять" — оно чисто декларативное, мёртвый груз.

Итог: функция, объявленная с `security = "definer"` (задумана как
"исполняется от имени владельца", аналог Postgres `SECURITY DEFINER`),
НЕ получает никакой эскалации, если на неё отдельно НЕ выставлен POSIX
setuid-бит — то есть декларация `Security::Definer` создаёт ЛОЖНОЕ
ощущение защиты/поведения, которое не применяется. И наоборот: если
setuid-бит выставлен, а `security = "invoker"` — функция всё равно
эскалируется (setuid бит не проверяет security-декларацию вообще).

### Фикс — точечная интеграция `security` в `effective_fn_actor`

`effective_fn_actor` сейчас грузит `ResourceMeta` через
`self.system_store.load_function(fn_name)` (сырую запись) — тебе нужно
ТАКЖЕ прочитать `security` из той же записи (используй
`FunctionMeta::from_record(&rec)` — уже существующий метод, парсит
и `security`, и `visibility`) и объединить с существующей
`Mode::is_setuid` проверкой по следующей логике (предложение, обоснуй
если видишь лучший вариант — ЭТО решение о семантике, не чисто
механическое, подумай):

```rust
pub async fn effective_fn_actor(&self, fn_name: &str, caller: &Actor) -> Actor {
    let Ok(Some(rec)) = self.system_store.load_function(fn_name).await else {
        return caller.clone();
    };
    let res_meta = ResourceMeta::from_record(&rec);
    let fn_meta = FunctionMeta::from_record(&rec);
    match fn_meta.security {
        // Definer: ALWAYS execute as the function's owner, regardless of
        // the POSIX setuid mode bit — declaring `security=definer` is an
        // explicit, unconditional request for owner-privilege execution
        // (mirrors Postgres SECURITY DEFINER semantics: the bit is about
        // legacy setuid-style escalation, `security` is the modern,
        // explicit declaration and takes precedence).
        Security::Definer => res_meta.owner,
        // Invoker: ALWAYS execute as the caller, even if the legacy
        // setuid mode bit happens to be set — an explicit `security=invoker`
        // declaration is a hard guarantee against privilege escalation.
        Security::Invoker => {
            if Mode::is_setuid(res_meta.mode) {
                // Legacy setuid bit without an explicit definer declaration:
                // preserve existing pre-slice-10 behavior for callers that
                // rely on the mode bit alone (backward compat).
                res_meta.owner
            } else {
                caller.clone()
            }
        }
    }
}
```

**ВАЖНО — обдумай эту семантику критически, это решение с последствиями
для безопасности:** если считаешь, что `Invoker` должен ВСЕГДА
игнорировать даже setuid-бит (т.е. явная декларация `invoker` должна
быть train-нельзя-обойти гарантией, а не "если бит не стоит"), опиши
это как альтернативу в докладе и реализуй ТУ версию, которую сочтёшь
более безопасной и консистентной, с явным обоснованием. Не оставляй
это решение мне — прими его и защити в докладе.

Импортируй `Security`/`FunctionMeta` из `shamir_wasm_host` (или
реэкспорт, если `shamir-db` уже его имеет — грепни существующие импорты
`FunctionMeta`/`Security` в `shamir-db`).

### НЕ трогай

- `Mode::is_setuid` механизм сам по себе (POSIX permission mode) —
  оставь как legacy-путь для обратной совместимости (см. предложенную
  логику выше).
- Остальной `access_control.rs` — не расширяй `authorize_access`/
  traversal-логику, фикс живёт целиком в `effective_fn_actor`.

### Тесты (часть B)

1. **Definer всегда эскалирует**: функция с `security=definer`, mode БЕЗ
   setuid-бита, caller ≠ owner → `effective_fn_actor` возвращает
   `owner` (не `caller`).
2. **Invoker с setuid-битом** (сценарий который ты выбрал выше) —
   задокументируй и протестируй ТУ семантику, которую реализовал.
3. **Invoker без setuid-бита** (базовый случай) → возвращает `caller`
   (не регрессия существующего поведения).
4. **Функция не найдена** (`load_function` → `Ok(None)` или `Err`) →
   по-прежнему возвращает `caller.clone()` (existing fallback, не
   регрессируй).
5. Существующие setuid-related тесты (грепни
   `crates/shamir-db/src/shamir_db/tests/` для `effective_fn_actor`/
   `setuid`/`is_setuid`) остаются зелёными.

## Гейт

- `./scripts/test.sh -p shamir-wasm-host --full` (часть A) +
  `./scripts/test.sh -p shamir-db --full` (часть B) 1× каждый, целевые
  новые тесты 5-10× повторно;
- `cargo clippy --workspace --all-targets -- -D warnings`;
- `cargo fmt -p shamir-wasm-host -p shamir-db -- --check`.

## Дисциплина

Тесты ТОЛЬКО через ./scripts/test.sh. Хирургично: `compile.rs`
(+ `Cargo.toml` для `wait-timeout` dep) для части A; `access_control.rs`
для части B. НЕ трогай остальные CRITICAL/HIGH находки того же аудита
(TLS/WS timeout — HIGH-security кластер #444, HMAC-гейт асимметрия —
не твоя задача).

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

Часть A: `compile_rust_source` больше не наследует полное окружение
хоста (allowlist), имеет таймаут на компиляцию, отклоняет исходник с
`include!`/`include_str!`/`include_bytes!`/`env!`/`option_env!`. Часть B:
`Security::Definer`/`Invoker` реально влияет на то, чьим актором
исполняется функция — задекларированная семантика применяется, не
мёртвый груз. Обе части покрыты regression-тестами, существующие тесты
зелёные, гейт зелёный. Финал доклада: точный diff обеих частей, вывод
тестов, вывод гейта, явное обоснование выбранной семантики
Invoker+setuid (часть B) и выбранного подхода к timeout-тестированию
(часть A).
