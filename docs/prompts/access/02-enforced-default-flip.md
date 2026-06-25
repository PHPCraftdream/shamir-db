בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# G.4c (A2) — переход дефолта open→enforced (Strategy A)

## Цель
Сменить дефолтный mode ВНОВЬ создаваемых mode-bearing объектов с `0o777`
(world-rwx) на enforced `0o700` (owner-rwx, group/other — none). **Strategy A**:
enforced ТОЛЬКО для новых объектов; legacy-записи без поля `mode` продолжают
грузиться как OPEN (это уже обеспечено `ResourceMeta::from_record`, не трогать).

## Контекст безопасности (подтверждено чтением — определяет churn)
- `session_actor` (shamir-server/src/db_handler/handler.rs:117-120): **superuser
  → `Actor::System`**; не-superuser → `Actor::User(principal_id)`.
- `authorize_access` (access_control.rs:318): `Actor::System` БАЙПАСИТ гейт.
- Следствие: e2e под admin/superuser (e2e-ddl весь) → System → enforced-дефолт
  на них НЕ влияет, остаются зелёными. Churn — только в тестах с реальными
  не-superuser юзерами (e2e-permissions, e2e-principal), где не-владелец читает
  чужой ресурс БЕЗ явного гранта/chmod.
- Unit-тесты на `Actor::System` (большинство shamir-db) — байпас, не сломаются.

## Срез — 2 части

### Часть 1: enforced-конструктор + переключение create-сайтов

**1a.** В `crates/shamir-types/src/access.rs`, рядом с `owned_by` (:189), добавить:
```rust
/// Enforced default: owner = actor, group = None, mode = owner-rwx (`0o700`).
///
/// New mode-bearing objects are private to their creator (Strategy A).
/// Legacy catalogue records without a `mode` field still load as
/// [`open`](Self::open) via [`from_record`], so only NEW objects are
/// enforced — existing data is unaffected.
pub fn owned_enforced(actor: Actor) -> Self {
    Self {
        owner: actor,
        group: None,
        mode: Mode::from_rwx(true, false, false),
    }
}
```
(`Mode::from_rwx(true,false,false)` = `0o700`, см. access.rs:116.)

**1b.** Переключить ВСЕ create-сайты с `ResourceMeta::owned_by(actor)` на
`ResourceMeta::owned_enforced(actor)` (грепни `owned_by` по crates/shamir-db/src
и crates/shamir-engine/src — должно быть ~7 сайтов):
- `db_management.rs:39` (save_database) и `:118` (repo/store).
- `table_management.rs:63` (save_table).
- `function_management.rs:214` (save_function) и `:406` (save_function_folder).
- `validator_management.rs:140` и `:296` (save_validator).
- Грепни на случай новых/пропущенных сайтов — переключи КАЖДЫЙ create-сайт.
- `owned_by` оставить в access.rs (pub API; не дед-код т.к. публичный).
  НЕ трогать `ResourceMeta::open()`/`from_record` (legacy-путь Strategy A).

**ВНИМАНИЕ:** create-сайты под `Actor::System` (superuser) дадут owner=System,
mode 0o700. System байпасит гейт, поэтому admin читает свои — ок; но НЕ-superuser
к System-owned ресурсу под 0o700 получит deny (это и есть желаемый enforced).

### Часть 2: починка фикстур (вся churn ЗДЕСЬ)
Прогнать полный gate (ниже), собрать КАЖДЫЙ упавший тест и починить по принципу:
- Тест полагался на world-rwx, где не-владелец читал чужой ресурс → дать явный
  grant/chmod (как уже делают capability-тесты в e2e-permissions: `admin.chmod`,
  `admin.permission`, `grantRole`), ЛИБО создавать ресурс ТЕМ актором, что его
  потом читает (он станет владельцем → owner-rwx).
- Rust-тесты с `ResourceMeta` ассертами на дефолтный mode (напр.
  `crates/shamir-db/src/shamir_db/tests/access_meta_tests.rs` — там ассерты
  `== ResourceMeta::open()`): если тест проверяет дефолт create-пути — обновить
  ожидание на enforced; если проверяет именно `open()`-конструктор как таковой —
  оставить (open() не менялся). Разобраться по каждому ассерту, НЕ скопом.
- НЕ ослабляй проверки бездумно (не «убрать ассерт»): каждая правка фикстуры
  должна отражать новую корректную enforced-семантику. Если тест ловит реальную
  смену поведения — обнови ожидание осознанно с комментарием.
- Логируй (в финальном тексте) КАЖДЫЙ изменённый тест и ПОЧЕМУ.

## Гейт (ПОЛНЫЙ — это смена поведения, широта максимальна)
1. Rust:
   - `cargo fmt -p shamir-types -p shamir-db -p shamir-engine -- --check`
   - `cargo clippy --workspace --all-targets -- -D warnings`
   - `./scripts/test.sh --full`  (ВЕСЬ workspace, lib+integration — НЕ один крейт!)
     Тесты ТОЛЬКО так; вывод в файл, грепай файл (не inline-pipe).
2. Пересобрать debug-сервер (серверный код зависит от изменённого):
   `cargo build -p shamir-server` (бинарь D:/dev/rust/.cargo-target/debug/).
3. e2e (КРИТИЧНО — здесь основной churn):
   ```
   cd crates/shamir-client-ts && \
   SHAMIR_SERVER_BIN=D:/dev/rust/.cargo-target/debug/shamir-server.exe \
   npx vitest run 2>&1 | tail -80
   ```
   ⚠️ ВЕСЬ e2e-набор (не только ddl/permissions) — enforced может задеть
   e2e-principal, e2e-interner, e2e-keyset и др., если там не-superuser юзеры.
   Все должны стать зелёными.

## Дисциплина (ОБЯЗАТЕЛЬНО)
- ⛔ НЕ используй agent/sub-agent — падает context-canceled. Читай файлы напрямую.
- ⛔ NEVER git reset/checkout/clean/stash/restore/rm или любую мутирующую git-команду.
  Только редактируй файлы. НЕ коммить — коммитит оркестратор.
- НЕ поднимай версии. use в шапке. Queries — только билдеры. Surgical: меняешь
  дефолт + чинишь фикстуры, ничего лишнего. НЕ трогай open()/from_record.
- Заверши финальным текстом: (1) список переключённых create-сайтов;
  (2) список ПОЧИНЕННЫХ тестов с причиной каждого; (3) вывод ВСЕГО гейта
  (Rust full + e2e полный). Будь честен: если что-то красное — скажи, не скрывай.

## Коммит (оркестратор, после zero-trust verify; возможно разбить)
`feat(access): G.4c — enforced (owner-rwx) default for new objects (Strategy A)`
