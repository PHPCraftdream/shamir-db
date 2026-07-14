בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V-lockfree — session pending_changepw_challenge: Mutex → ArcSwapOption (#417)

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Задача #417
> (единственная замена из mutex-аудита): убрать
> `parking_lot::Mutex<Option<PendingChangePwChallenge>>` из
> `crates/shamir-connect/src/server/session.rs:131` в пользу lock-free
> `arc_swap::ArcSwapOption<PendingChangePwChallenge>` (пиллар: никаких
> parking_lot/std Mutex вне санкционированных мест).

## Скоуп

- `crates/shamir-connect/src/server/session.rs` — поле
  `pending_changepw_challenge` (строки ~52–157: struct
  PendingChangePwChallenge, Debug impl, поле, init) и его использования.
- `crates/shamir-connect/src/server/changepw.rs` — потребитель (issue /
  verify / consume challenge). Найди ВСЕ обращения grep'ом.
- НЕ трогать `cap_lock: Mutex<()>` (строка 272) — он вне задачи
  (санкционирован аудитом).

## Требования

1. `pub pending_changepw_challenge: ArcSwapOption<PendingChangePwChallenge>`;
   init `ArcSwapOption::const_empty()` (или `::empty()`).
2. Семантика одноразового challenge ОБЯЗАНА сохраниться атомарной:
   consume = `swap(None)` — ровно один поток получает challenge
   (защита от double-submit гонки changePassword §12.5). Проверь текущую
   семантику в changepw.rs: если там read-then-take под одним lock'ом —
   воспроизведи эквивалент через swap/compare_and_swap, НЕ через
   load+store (это TOCTOU).
3. Debug impl: замени плейсхолдер `"<Mutex>"` на нейтральный
   (`"<ArcSwapOption>"` или наличие Some/None без содержимого — секреты
   не логировать).
4. Если PendingChangePwChallenge содержит секретный материал — проверь,
   есть ли Drop/zeroize-требования (не сломай).

## Тесты (принцип: каждый инвариант → тест)

- Существующие changepw-тесты (SCRAM, Argon2-bound, timeout 60s в
  nextest) должны остаться зелёными: `./scripts/test.sh -p shamir-connect`
  (+ `@server`, т.к. shamir-server потребитель).
- Добавь регресс-тест атомарности consume: два конкурентных consume
  одного challenge → ровно один Some (tokio::join! / spawn), по образцу
  соседних тестов крейта. Тестовая раскладка — в tests/ директории
  модуля (НЕ inline #[cfg(test)] в session.rs).

## Гейт

- `./scripts/test.sh -p shamir-connect -p shamir-server --full`
- `cargo clippy -p shamir-connect -p shamir-server --all-targets -- -D warnings`
- `cargo fmt -p shamir-connect -- --check`

## Дисциплина

Хирургично: только challenge-поле и его потребители. Импорты в шапке.
stray-логи отметь, не удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

Mutex удалён, ArcSwapOption с атомарным consume, регресс-тест
конкурентного consume зелёный, гейт зелёный. Финал: изменённые файлы,
как сохранена одноразовость challenge, вывод гейта.
