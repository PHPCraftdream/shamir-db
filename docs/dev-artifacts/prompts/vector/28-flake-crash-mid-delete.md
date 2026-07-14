בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# flake-hunt — crash_at_mid_delete_recovers_all (#419)

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Задача #419:
> редкий флейк `crash_at_mid_delete_recovers_all` в
> `crates/shamir-engine/tests/crash_recovery.rs` (подозрение: WAL
> crash-recovery child sidecar race). Принцип проекта: флейк = БАГ со
> знанием о дефекте; НЕ маскировать (никаких retry/tolerance/повышений
> таймаута) — найти корень и починить.

## Шаги

1. **Воспроизведи.** Тест редкий — гоняй под параллелизмом nextest в лупе:
   `./scripts/test.sh -p shamir-engine --full -- crash_at_mid_delete` в
   цикле (50–200 итераций, лог в файл), при необходимости весь
   crash_recovery-файл параллельно для нагрузки. Если не воспроизводится
   изолированно — гоняй в составе `@e2e`/`--full` (флейк всплывал под
   общей нагрузкой).
2. **Найди корень.** Известный контекст: crash-тесты используют
   child-процесс (спавн себя / отдельный бинарь), убиваемый на середине
   delete; recovery читает WAL + sidecar. Подозрения: гонка записи
   sidecar-файла child'ом vs kill (partial write без fsync?); гонка
   file-lock между child и recovery; недетерминированный порядок
   kill/flush; Windows-специфика (file handle не отпущен на момент
   повторного открытия). Читай harness crash-тестов целиком.
3. **Почини корень** (в коде ИЛИ в харнессе теста — где реально дефект).
   Если дефект в проде (recovery неверно обрабатывает partial state) —
   это ценная находка, чини прод.
4. **Регресс-тест**: если корень — отдельный класс (напр. partial sidecar
   write), добавь именованный детерминированный регресс (инъекция
   обрезанного sidecar и т.п.), не полагайся на вероятностный луп.

## Гейт

- Луп воспроизведения ПОСЛЕ фикса: тот же цикл зелёный (те же итерации).
- `./scripts/test.sh -p shamir-engine --full` + `-p shamir-wal --full`.
- `cargo clippy -p shamir-engine -p shamir-wal --all-targets -- -D warnings`;
  fmt тронутых.

## Дисциплина

- Тесты ТОЛЬКО через ./scripts/test.sh; вывод в файл → grep файла.
- НЕ поднимать таймауты nextest. НЕ добавлять retry. Хирургичность.
- stray-логи отметь, не удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

Корень назван и доказан (лог/трейс падения до фикса), фикс хирургичен,
регресс-тест зелёный, луп после фикса чист, гейт зелёный. Финал: корень,
изменённые файлы, доказательство красный→зелёный, вывод гейта. Если за
разумное время (30–40м) корень НЕ воспроизведён — честно отчитайся что
пробовал (итерации/окружение), НЕ выдумывай фикс.
